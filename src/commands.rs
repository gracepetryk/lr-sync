//! The pull, push, and list-folders subcommands.

use anyhow::{Context, Result, bail, ensure};
use colored::Colorize;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Config;
use crate::folder::Folder;
use crate::remote::{
    checked_out_ancestor, remote_checked_out_subdirs, remote_file_list, remote_mv, remote_run,
    remote_script, remote_state, sh_quote,
};
use crate::rsync::{Echo, confirm_rsyncs, rsync_command, run_rsync};
use crate::ui::{Default, confirm};

pub fn pull(cfg: &Config, folders: &[Folder]) -> Result<()> {
    // resolve every folder before transferring anything, so a missing folder
    // or a checked-out conflict aborts the whole batch up front
    let mut photos = 0;
    let mut plan = Vec::new();
    for folder in folders {
        let (dir_exists, co_exists) = remote_state(cfg, folder)?;
        let (src, need_rename) = if co_exists {
            eprintln!(
                "{}",
                format!(
                    "note: {} is already checked out on {}; resuming pull",
                    folder.rel, cfg.remote_host
                )
                .dimmed()
            );
            (cfg.remote_checked_out(folder), false)
        } else if dir_exists {
            (cfg.remote_dir(folder), true)
        } else {
            bail!(
                "{} not found on {}",
                cfg.remote_dir(folder),
                cfg.remote_host
            );
        };
        photos += remote_file_list(cfg, &src)?
            .iter()
            .filter(|p| is_photo(p))
            .count();
        plan.push((folder, src, need_rename));
    }

    let mut cmds = Vec::new();
    for (folder, src, _) in &plan {
        let local_dir = cfg.local_dir(folder);
        fs::create_dir_all(local_dir.parent().expect("folder path has a parent"))?;
        // remote -> local only, no delete flags: the NAS copy is never touched
        cmds.push(rsync_command(
            cfg,
            &[],
            &format!("{}:{src}/", cfg.remote_host),
            &format!("{}/", cfg.local_dir(folder).display()),
        ));
    }
    confirm_rsyncs(
        cfg,
        &format!("Pull {} from {}", photos_label(photos), cfg.remote_host),
        &cmds,
    )?;

    for ((folder, _, need_rename), cmd) in plan.iter().zip(&mut cmds) {
        run_rsync(Echo::from(cfg.yes), cmd)?;
        if *need_rename {
            remote_mv(
                cfg,
                &cfg.remote_dir(folder),
                &cfg.remote_checked_out(folder),
            )?;
        }
        println!(
            "{}",
            format!(
                "pulled {} -> {} (remote copy kept as {}{})",
                folder.rel,
                cfg.local_dir(folder).display(),
                folder.rel,
                cfg.checked_out_suffix
            )
            .green()
        );
    }
    Ok(())
}

pub fn push(cfg: &Config, folders: &[Folder]) -> Result<()> {
    // resolve every folder (and settle its culled files) before transferring
    // anything, so a missing folder or a conflict aborts the batch up front
    let mut photos = 0;
    let mut plan = Vec::new();
    for folder in folders {
        let local_dir = cfg.local_dir(folder);
        ensure!(local_dir.is_dir(), "{} not found", local_dir.display());
        // pushing below a checked-out parent would create a plain tree next
        // to the renamed one, which Lightroom then sees twice
        if let Some(parent) = checked_out_ancestor(cfg, folder)? {
            bail!(
                "{parent} is checked out on {} (as {parent}{}); push {parent} as a whole instead",
                cfg.remote_host,
                cfg.checked_out_suffix
            );
        }
        let (dir_exists, co_exists) = remote_state(cfg, folder)?;

        let dest = if co_exists {
            cfg.remote_checked_out(folder)
        } else if dir_exists {
            cfg.remote_dir(folder)
        } else {
            eprintln!(
                "{}",
                format!(
                    "note: {} does not exist on {}; creating it",
                    folder.rel, cfg.remote_host
                )
                .dimmed()
            );
            let remote_dir = cfg.remote_dir(folder);
            let parent = remote_dir
                .rsplit_once('/')
                .expect("remote dir has a parent")
                .0;
            remote_run(cfg, &format!("mkdir -p {}", sh_quote(parent)))?;
            remote_dir
        };

        if dir_exists || co_exists {
            // subdirs checked out individually get their plain names back
            // first, so the rsync merges into them (and the cull comparison
            // below sees them under the same paths as the local copies)
            uncheckout_subdirs(cfg, &dest)?;
            // files deleted locally while checked out move to the culled tree
            // before the sync, so they neither reappear in Lightroom nor get lost
            cull_removed(cfg, folder, &dest, &local_dir)?;
        }

        photos += local_file_set(&local_dir)?
            .iter()
            .filter(|p| is_photo(p))
            .count();
        plan.push((folder, dest, co_exists));
    }

    let mut cmds: Vec<Command> = plan
        .iter()
        .map(|(folder, dest, _)| {
            rsync_command(
                cfg,
                &["--remove-source-files"],
                &format!("{}/", cfg.local_dir(folder).display()),
                &format!("{}:{dest}/", cfg.remote_host),
            )
        })
        .collect();
    confirm_rsyncs(
        cfg,
        &format!("Push {} to {}", photos_label(photos), cfg.remote_host),
        &cmds,
    )?;

    for ((folder, dest, was_checked_out), cmd) in plan.iter().zip(&mut cmds) {
        run_rsync(Echo::from(cfg.yes), cmd)?;
        let remote_dir = cfg.remote_dir(folder);
        if *was_checked_out {
            remote_mv(cfg, dest, &remote_dir)?;
        }

        if !cfg.dry_run {
            // rsync --remove-source-files leaves empty directories behind
            let local_dir = cfg.local_dir(folder);
            remove_empty_dirs(&local_dir)?;
            if local_dir.is_dir() {
                eprintln!(
                    "{}",
                    format!(
                        "warning: {} is not empty after push; left in place",
                        local_dir.display()
                    )
                    .yellow()
                );
            }
        }

        println!(
            "{}",
            format!(
                "pushed {} -> {}:{}",
                folder.rel, cfg.remote_host, remote_dir
            )
            .green()
        );
    }
    Ok(())
}

/// Count label for the confirmation prompt.
fn photos_label(n: usize) -> String {
    match n {
        1 => "1 photo".to_string(),
        _ => format!("{n} photos"),
    }
}

/// Whether a file counts as a photo in the confirmation prompt: leaves out
/// sidecars and other metadata (xmp/lua, dotfiles like .DS_Store, Thumbs.db).
fn is_photo(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    if name.starts_with('.') || name.eq_ignore_ascii_case("thumbs.db") {
        return false;
    }
    match name.rsplit_once('.') {
        Some((_, ext)) => !matches!(ext.to_ascii_lowercase().as_str(), "xmp" | "lua"),
        None => true,
    }
}

/// List remote folder candidates for pull completion: every directory up to
/// three layers under the remote root, as root-relative paths (one ssh round
/// trip; `_multi_parts` completes them one layer at a time). The checked-out
/// suffix is stripped so resumable checkouts complete like plain folders.
/// Push completion doesn't come through here: it uses zsh's native `_files`
/// rooted at the local tree.
/// List the directories exactly one layer below `prefix` (a clean, root-relative
/// path; empty means the top level), as clean root-relative paths. Completion
/// calls this once per tab to descend a single component at a time. Names are
/// compared after stripping the checked-out suffix from every component, so a
/// checked-out parent layer (`2026.checked-out`) still lists under its clean
/// name (`2026`) and stays navigable.
pub fn list_folders(cfg: &Config, prefix: &str) -> Result<()> {
    let prefix = prefix.trim_matches('/');
    let comps = if prefix.is_empty() {
        0
    } else {
        prefix.split('/').count()
    };
    let depth = comps + 1;
    let out = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=3"])
        .arg(&cfg.remote_host)
        .arg(format!(
            // hidden dirs (.claude, .lrdata, ...) are pruned, not offered
            "find {} -mindepth {depth} -maxdepth {depth} -name '.*' -prune -o -type d -print",
            sh_quote(&cfg.remote_root)
        ))
        .output()
        .context("failed to run ssh")?;
    let root_prefix = format!("{}/", cfg.remote_root);
    let want = if prefix.is_empty() {
        String::new()
    } else {
        format!("{prefix}/")
    };
    let mut folders = BTreeSet::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Some(rel) = line.strip_prefix(&root_prefix) else {
            continue;
        };
        let clean: Vec<&str> = rel
            .split('/')
            .map(|c| c.strip_suffix(&cfg.checked_out_suffix).unwrap_or(c))
            .collect();
        let clean = clean.join("/");
        if clean.starts_with(&want) {
            folders.insert(clean);
        }
    }
    for f in &folders {
        println!("{f}");
    }
    Ok(())
}

/// Rename checked-out subdirs under `dest` back to their plain names (deepest
/// first) before a parent push, refusing if a plain sibling already exists.
fn uncheckout_subdirs(cfg: &Config, dest: &str) -> Result<()> {
    let subdirs = remote_checked_out_subdirs(cfg, dest)?;
    if subdirs.is_empty() {
        return Ok(());
    }

    let mut script = String::from("set -eu\n");
    for co in &subdirs {
        let plain = co
            .strip_suffix(&cfg.checked_out_suffix)
            .expect("find matched the suffix");
        if cfg.dry_run {
            println!("+ would rename {co} -> {plain}");
            continue;
        }
        script.push_str(&format!(
            "test ! -e {plain_q} || {{ echo {msg} >&2; exit 1; }}\nmv {co_q} {plain_q}\n",
            plain_q = sh_quote(plain),
            co_q = sh_quote(co),
            msg = sh_quote(&format!("both {co} and {plain} exist; resolve manually")),
        ));
    }
    if cfg.dry_run {
        return Ok(());
    }

    eprintln!(
        "{}",
        format!(
            "note: {} checked-out folder(s) under {dest} on {}; renaming back before push",
            subdirs.len(),
            cfg.remote_host
        )
        .dimmed()
    );
    remote_script(cfg, &script, "checked-out folder rename")
}

/// Move files that exist in the NAS folder but not locally (culled in
/// Lightroom while checked out) into the culled tree, preserving relative
/// paths, so they don't reappear after push but are never lost.
fn cull_removed(cfg: &Config, folder: &Folder, dest: &str, local_dir: &Path) -> Result<()> {
    let local = local_file_set(local_dir)?;
    let mut culled: Vec<String> = remote_file_list(cfg, dest)?
        .into_iter()
        .filter(|p| !local.contains(p))
        .collect();
    culled.sort();
    if culled.is_empty() {
        return Ok(());
    }

    let culled_dir = format!("{}/{}", cfg.culled_root, folder.rel);
    if cfg.dry_run {
        for p in &culled {
            println!("+ would move {dest}/{p} -> {culled_dir}/{p} (after confirmation)");
        }
        return Ok(());
    }

    eprintln!(
        "{}",
        format!(
            "{} file(s) in {} on {} have been removed from the local copy:",
            culled.len(),
            folder.rel,
            cfg.remote_host
        )
        .bold()
    );
    const LISTED: usize = 10;
    for p in culled.iter().take(LISTED) {
        eprintln!("  {p}");
    }
    if culled.len() > LISTED {
        eprintln!("{}", format!("  [{} more]", culled.len() - LISTED).dimmed());
    }
    let move_them = cfg.yes
        || confirm(
            &format!("Move them to {culled_dir} on {}?", cfg.remote_host),
            Default::No,
        )?;
    if !move_them {
        eprintln!(
            "{}",
            "leaving them in place; they will reappear in Lightroom after push".dimmed()
        );
        return Ok(());
    }

    let mut script = String::from(concat!(
        "set -eu\n",
        // pick a free name: file, file.collision, file.collision-2, ...
        "culled_dst() {\n",
        "  d=$1\n",
        "  [ -e \"$d\" ] || { printf %s \"$d\"; return; }\n",
        "  [ -e \"$d.collision\" ] || { printf %s \"$d.collision\"; return; }\n",
        "  i=2\n",
        "  while [ -e \"$d.collision-$i\" ]; do i=$((i+1)); done\n",
        "  printf %s \"$d.collision-$i\"\n",
        "}\n",
    ));
    for p in &culled {
        let src = format!("{dest}/{p}");
        let dst = format!("{culled_dir}/{p}");
        let parent = dst.rsplit_once('/').expect("culled path has a parent").0;
        script.push_str(&format!(
            "mkdir -p {}\nmv {} \"$(culled_dst {})\"\n",
            sh_quote(parent),
            sh_quote(&src),
            sh_quote(&dst)
        ));
    }
    // prune dirs the moves emptied; rsync recreates any that still exist locally
    script.push_str(&format!(
        "find {} -mindepth 1 -type d -empty -delete\n",
        sh_quote(dest)
    ));
    remote_script(cfg, &script, "culled-file move")
}

/// Relative paths of all regular files under `dir`.
fn local_file_set(dir: &Path) -> Result<BTreeSet<String>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeSet<String>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                walk(root, &path, out)?;
            } else if entry.file_type()?.is_file() {
                let rel = path.strip_prefix(root).expect("path is under root");
                out.insert(rel.to_string_lossy().into_owned());
            }
        }
        Ok(())
    }
    let mut out = BTreeSet::new();
    walk(dir, dir, &mut out)?;
    Ok(out)
}

fn read_dirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(dirs),
        Err(err) => return Err(err).with_context(|| format!("reading {}", dir.display())),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            dirs.push(entry.path());
        }
    }
    Ok(dirs)
}

/// Remove empty directories bottom-up, including `dir` itself if it empties.
fn remove_empty_dirs(dir: &Path) -> Result<()> {
    for sub in read_dirs(dir)? {
        remove_empty_dirs(&sub)?;
    }
    if fs::read_dir(dir)?.next().is_none() {
        fs::remove_dir(dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_photo_skips_sidecars_and_metadata() {
        for p in ["IMG_0001.NEF", "IMG_0001.jpg", "sub/IMG_0002.dng", "raw"] {
            assert!(is_photo(p), "should count {p:?}");
        }
        for p in [
            "IMG_0001.xmp",
            "IMG_0001.XMP",
            "meta.lua",
            ".DS_Store",
            "sub/.hidden",
            "Thumbs.db",
        ] {
            assert!(!is_photo(p), "should not count {p:?}");
        }
    }
}
