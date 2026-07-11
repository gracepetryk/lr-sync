//! Configuration: the `~/.config/lr-sync/config` file, CLI-flag overrides,
//! and the interactive `configure` subcommand.

use anyhow::{Context, Result, ensure};
use colored::Colorize;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::Cli;
use crate::folder::Folder;
use crate::ui::prompt;

pub const CHECKED_OUT_SUFFIX: &str = ".checked-out";

pub struct Config {
    pub local_root: PathBuf,
    pub remote_host: String,
    pub remote_root: String,
    pub culled_root: String,
    pub checked_out_suffix: String,
    pub yes: bool,
    pub verbose: bool,
    pub dry_run: bool,
}

impl Config {
    /// Precedence for each value: CLI flag > config file. `culled` also has a
    /// derived default (sibling of the remote dir); the others are required.
    pub fn from_cli(cli: &Cli) -> Result<Config> {
        let file = load_config_file()?;
        let missing = |key: &str, flag: &str| {
            anyhow::anyhow!("no {key} tree configured; run `lr-sync configure` or pass {flag}")
        };

        let local = match &cli.local {
            Some(dir) => dir.clone(),
            None => expand_tilde(
                file.get("local")
                    .ok_or_else(|| missing("local", "--local"))?,
            )?,
        };
        let remote = cli
            .remote
            .as_deref()
            .or(file.get("remote").map(String::as_str))
            .ok_or_else(|| missing("remote", "--remote"))?;
        let (host, root) = split_remote(remote)?;
        let culled = cli
            .culled
            .as_deref()
            .or(file.get("culled").map(String::as_str))
            .map(|c| c.trim_end_matches('/').to_string())
            .unwrap_or_else(|| sibling_culled(&root));
        let suffix = cli
            .suffix
            .as_deref()
            .or(file.get("suffix").map(String::as_str))
            .unwrap_or(CHECKED_OUT_SUFFIX)
            .to_string();
        ensure!(
            !suffix.is_empty() && !suffix.contains('/'),
            "checked-out suffix must be non-empty and contain no '/' (got: {suffix})"
        );
        Ok(Config {
            local_root: local,
            remote_host: host,
            remote_root: root,
            culled_root: culled,
            checked_out_suffix: suffix,
            yes: cli.yes,
            verbose: cli.verbose || cli.dry_run,
            dry_run: cli.dry_run,
        })
    }

    pub fn local_dir(&self, folder: &Folder) -> PathBuf {
        self.local_root.join(&folder.rel)
    }

    pub fn remote_dir(&self, folder: &Folder) -> String {
        format!("{}/{}", self.remote_root, folder.rel)
    }

    pub fn remote_checked_out(&self, folder: &Folder) -> String {
        format!("{}{}", self.remote_dir(folder), self.checked_out_suffix)
    }

    /// With --verbose, echo an ssh command before it runs (stderr, so
    /// completion and piped output stay clean).
    pub fn trace_ssh(&self, command: &str) {
        if self.verbose {
            eprintln!(
                "{}",
                format!("+ ssh {} {command}", self.remote_host).dimmed()
            );
        }
    }
}

pub fn split_remote(remote: &str) -> Result<(String, String)> {
    let (host, root) = remote
        .split_once(':')
        .filter(|(h, d)| !h.is_empty() && !d.is_empty())
        .with_context(|| format!("remote must be HOST:DIR (got: {remote})"))?;
    Ok((host.to_string(), root.trim_end_matches('/').to_string()))
}

/// Default culled tree: sibling of the remote dir named culled/
/// (e.g. /photos/archive -> /photos/culled).
fn sibling_culled(remote_root: &str) -> String {
    match remote_root.rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => format!("{parent}/culled"),
        _ => "/culled".to_string(),
    }
}

fn expand_tilde(path: &str) -> Result<PathBuf> {
    if path == "~" || path.starts_with("~/") {
        let home = std::env::var_os("HOME").context("HOME is not set")?;
        Ok(Path::new(&home).join(path.trim_start_matches('~').trim_start_matches('/')))
    } else {
        Ok(PathBuf::from(path))
    }
}

fn config_path() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => {
            let home = std::env::var_os("HOME").context("HOME is not set")?;
            Path::new(&home).join(".config")
        }
    };
    Ok(base.join("lr-sync").join("config"))
}

/// Parse `key = value` lines; `#` comments and blank lines are ignored.
fn parse_config(text: &str) -> HashMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

fn load_config_file() -> Result<HashMap<String, String>> {
    let path = config_path()?;
    match fs::read_to_string(&path) {
        Ok(text) => Ok(parse_config(&text)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
    }
}

pub fn configure() -> Result<()> {
    let path = config_path()?;
    let existing = load_config_file()?;
    let get = |key: &str| existing.get(key).map(String::as_str);

    let local = prompt("Local Lightroom tree", get("local"))?;
    let (remote, remote_host, remote_root) = loop {
        let remote = prompt("Remote archive (HOST:DIR)", get("remote"))?;
        match split_remote(&remote) {
            Ok((host, root)) => break (remote, host, root),
            Err(err) => eprintln!("{err}"),
        }
    };
    let culled = prompt(
        &format!("Culled tree on {remote_host}"),
        Some(&sibling_culled(&remote_root)),
    )?;
    let suffix = loop {
        let suffix = prompt(
            "Checked-out suffix",
            Some(get("suffix").unwrap_or(CHECKED_OUT_SUFFIX)),
        )?;
        if !suffix.contains('/') {
            break suffix;
        }
        eprintln!("the suffix must not contain '/'");
    };

    let text = format!(
        "# lr-sync configuration (regenerate with `lr-sync configure`)\n\
         local = {local}\n\
         remote = {remote}\n\
         culled = {culled}\n\
         suffix = {suffix}\n"
    );
    fs::create_dir_all(path.parent().expect("config path has a parent"))?;
    fs::write(&path, text)?;
    println!("wrote {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_config_handles_comments_and_spacing() {
        let cfg = parse_config("# comment\n\nlocal = ~/Pics \nremote=nas:/photos/archive\n");
        assert_eq!(cfg.get("local").unwrap(), "~/Pics");
        assert_eq!(cfg.get("remote").unwrap(), "nas:/photos/archive");
        assert_eq!(cfg.len(), 2);
    }

    #[test]
    fn sibling_culled_derives_from_remote_root() {
        assert_eq!(sibling_culled("/photos/archive"), "/photos/culled");
        assert_eq!(sibling_culled("/archive"), "/culled");
        assert_eq!(sibling_culled("archive"), "/culled");
    }
}
