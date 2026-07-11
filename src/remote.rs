//! ssh helpers for inspecting and changing state on the NAS. Anything that
//! changes remote state goes through [`remote_run`] or [`remote_script`] so
//! `--dry-run` stays honest.

use anyhow::{Context, Result, bail, ensure};
use std::process::Command;

use crate::config::Config;
use crate::folder::Folder;

/// Relative paths of all regular files under the remote directory.
pub fn remote_file_list(cfg: &Config, dir: &str) -> Result<Vec<String>> {
    let command = format!("find {} -type f", sh_quote(dir));
    cfg.trace_ssh(&command);
    let out = Command::new("ssh")
        .arg(&cfg.remote_host)
        .arg(command)
        .output()
        .context("failed to run ssh")?;
    ensure!(
        out.status.success(),
        "listing files under {}:{dir} failed: {}",
        cfg.remote_host,
        String::from_utf8_lossy(&out.stderr).trim()
    );
    let prefix = format!("{dir}/");
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix(&prefix).map(str::to_string))
        .collect())
}

/// Run a multi-line shell script on the NAS via stdin (avoids arg limits);
/// `action` names the batch in the failure message.
pub fn remote_script(cfg: &Config, script: &str, action: &str) -> Result<()> {
    use std::io::Write;
    cfg.trace_ssh(&format!("sh -s <<'EOF'\n{script}EOF"));
    let mut child = Command::new("ssh")
        .arg(&cfg.remote_host)
        .arg("sh -s")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .context("failed to run ssh")?;
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(script.as_bytes())?;
    let status = child.wait()?;
    ensure!(
        status.success(),
        "{action} on {} failed: {status}",
        cfg.remote_host
    );
    Ok(())
}

/// The first ancestor layer of `folder` that is checked out on the NAS, if
/// any (one ssh round trip for all layers). Pushing below a checked-out
/// parent would create a plain tree next to the renamed one.
pub fn checked_out_ancestor(cfg: &Config, folder: &Folder) -> Result<Option<String>> {
    let probes: Vec<String> = folder
        .ancestors()
        .map(|a| {
            let co = format!("{}/{a}{}", cfg.remote_root, cfg.checked_out_suffix);
            format!("if test -d {}; then echo {}; fi", sh_quote(&co), sh_quote(a))
        })
        .collect();
    if probes.is_empty() {
        return Ok(None);
    }
    let command = probes.join("; ");
    cfg.trace_ssh(&command);
    let out = Command::new("ssh")
        .arg(&cfg.remote_host)
        .arg(command)
        .output()
        .context("failed to run ssh")?;
    ensure!(
        out.status.success(),
        "checking for checked-out parents on {} failed: {}",
        cfg.remote_host,
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .map(str::to_string))
}

/// Checked-out directories anywhere under `dir`, deepest first, so renaming
/// them in order never invalidates the path of a later entry.
pub fn remote_checked_out_subdirs(cfg: &Config, dir: &str) -> Result<Vec<String>> {
    let command = format!(
        "find {} -depth -mindepth 1 -type d -name {}",
        sh_quote(dir),
        sh_quote(&format!("*{}", cfg.checked_out_suffix))
    );
    cfg.trace_ssh(&command);
    let out = Command::new("ssh")
        .arg(&cfg.remote_host)
        .arg(command)
        .output()
        .context("failed to run ssh")?;
    ensure!(
        out.status.success(),
        "listing checked-out folders under {}:{dir} failed: {}",
        cfg.remote_host,
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect())
}

/// Whether the plain and checked-out folders exist on the NAS (one ssh probe
/// each). Fails if both exist, since it is ambiguous which is authoritative.
pub fn remote_state(cfg: &Config, folder: &Folder) -> Result<(bool, bool)> {
    let dir = cfg.remote_dir(folder);
    let co = cfg.remote_checked_out(folder);
    let dir_exists = remote_test_dir(cfg, &dir)?;
    let co_exists = remote_test_dir(cfg, &co)?;
    if dir_exists && co_exists {
        bail!(
            "both {dir} and {co} exist on {}; resolve manually",
            cfg.remote_host
        );
    }
    Ok((dir_exists, co_exists))
}

fn remote_test_dir(cfg: &Config, path: &str) -> Result<bool> {
    let command = format!("test -d {}", sh_quote(path));
    cfg.trace_ssh(&command);
    let status = Command::new("ssh")
        .arg(&cfg.remote_host)
        .arg(command)
        .status()
        .context("failed to run ssh")?;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!("ssh {} failed: {status}", cfg.remote_host),
    }
}

pub fn remote_mv(cfg: &Config, from: &str, to: &str) -> Result<()> {
    remote_run(cfg, &format!("mv {} {}", sh_quote(from), sh_quote(to)))
}

/// Run a state-changing command on the NAS (echoed instead when --dry-run).
pub fn remote_run(cfg: &Config, command: &str) -> Result<()> {
    use colored::Colorize;
    if cfg.dry_run {
        println!(
            "{}",
            format!("+ ssh {} {command}", cfg.remote_host).dimmed()
        );
        return Ok(());
    }
    cfg.trace_ssh(command);
    let status = Command::new("ssh")
        .arg(&cfg.remote_host)
        .arg(command)
        .status()
        .context("failed to run ssh")?;
    ensure!(
        status.success(),
        "ssh {} {command} failed: {status}",
        cfg.remote_host
    );
    Ok(())
}

/// Single-quote a string for the remote shell.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sh_quote_escapes_single_quotes() {
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
    }
}
