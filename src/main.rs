//! lr-sync -- move dated photo folders between the local Lightroom tree and
//! the NAS archive.
//!
//!   lr-sync pull 2026-07-01   NAS -> laptop; NAS copy renamed *.checked-out
//!   lr-sync push 2026-07-01   laptop -> NAS; NAS name restored, local copy removed
//!
//! `pull` never deletes anything on the NAS. The rename to *.checked-out makes
//! Lightroom "lose" the remote folder (so it can be re-pointed at the local
//! copy) and lets `push` return the folder with a cheap incremental rsync.

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use colored::Colorize;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const CHECKED_OUT_SUFFIX: &str = ".checked-out";

#[derive(Parser)]
#[command(
    name = "lr-sync",
    about = "Move dated photo folders between the local Lightroom tree and the remote archive"
)]
struct Cli {
    /// Local tree root (default: `local` from the config file)
    #[arg(short, long, global = true, value_name = "DIR")]
    local: Option<PathBuf>,

    /// Remote tree root (default: `remote` from the config file)
    #[arg(short, long, global = true, value_name = "HOST:DIR")]
    remote: Option<String>,

    /// Culled tree root on the remote, for files deleted locally
    /// (default: sibling of the remote dir named culled/)
    #[arg(long, global = true, value_name = "DIR")]
    culled: Option<String>,

    /// Suffix marking a folder as checked out on the remote
    /// (default: `suffix` from the config file, else ".checked-out")
    #[arg(long, global = true, value_name = "SUFFIX")]
    suffix: Option<String>,

    /// Print each rsync/ssh command before running it
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Show what would happen without changing anything (implies --verbose)
    #[arg(short = 'n', long, global = true)]
    dry_run: bool,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Copy a folder from the remote to the local tree; the remote copy is
    /// renamed with the checked-out suffix (never deleted)
    Pull { folder: String },

    /// Copy a folder back to the remote, restore its original name, and
    /// remove the local copy as files transfer
    Push { folder: String },

    /// Create the config file by prompting for each value
    Configure,

    /// List candidate folders (used by shell completion)
    #[command(hide = true)]
    ListFolders {
        #[arg(value_parser = ["pull", "push"])]
        cmd: String,
    },
}

struct Config {
    local_root: PathBuf,
    remote_host: String,
    remote_root: String,
    culled_root: String,
    checked_out_suffix: String,
    verbose: bool,
    dry_run: bool,
}

impl Config {
    /// Precedence for each value: CLI flag > config file. `culled` also has a
    /// derived default (sibling of the remote dir); the others are required.
    fn from_cli(cli: &Cli) -> Result<Config> {
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
            verbose: cli.verbose || cli.dry_run,
            dry_run: cli.dry_run,
        })
    }

    fn local_dir(&self, folder: &Folder) -> PathBuf {
        self.local_root.join(&folder.year).join(&folder.name)
    }

    fn remote_dir(&self, folder: &Folder) -> String {
        format!("{}/{}/{}", self.remote_root, folder.year, folder.name)
    }

    fn remote_checked_out(&self, folder: &Folder) -> String {
        format!("{}{}", self.remote_dir(folder), self.checked_out_suffix)
    }

    /// With --verbose, echo an ssh command before it runs (stderr, so
    /// completion and piped output stay clean).
    fn trace_ssh(&self, command: &str) {
        if self.verbose {
            eprintln!(
                "{}",
                format!("+ ssh {} {command}", self.remote_host).dimmed()
            );
        }
    }
}

struct Folder {
    name: String,
    year: String,
}

impl Folder {
    /// Accepts "2026-07-01", "2026-07-01/", or the checked-out form and
    /// normalizes to the plain folder name. The year directory comes from the
    /// first four characters.
    fn parse(raw: &str, checked_out_suffix: &str) -> Result<Folder> {
        let name = raw.trim_end_matches('/');
        let name = name.strip_suffix(checked_out_suffix).unwrap_or(name);
        let valid = name.len() > 5
            && name.as_bytes()[..4].iter().all(u8::is_ascii_digit)
            && name.as_bytes()[4] == b'-'
            && !name.contains('/');
        ensure!(
            valid,
            "folder must start with a year, e.g. 2026-07-01 (got: {raw})"
        );
        Ok(Folder {
            name: name.to_string(),
            year: name[..4].to_string(),
        })
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if matches!(cli.command, Cmd::Configure) {
        return configure();
    }
    let cfg = Config::from_cli(&cli)?;
    match &cli.command {
        Cmd::Pull { folder } => pull(&cfg, &Folder::parse(folder, &cfg.checked_out_suffix)?),
        Cmd::Push { folder } => push(&cfg, &Folder::parse(folder, &cfg.checked_out_suffix)?),
        Cmd::ListFolders { cmd } => list_folders(&cfg, cmd),
        Cmd::Configure => unreachable!(),
    }
}

fn split_remote(remote: &str) -> Result<(String, String)> {
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

/// Ask a yes/no question; enter (or EOF) takes the default "yes".
fn confirm(question: &str) -> Result<bool> {
    loop {
        print!("{} {} ", question.bold(), "[Y/n]:".dimmed());
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        match line.trim().to_lowercase().as_str() {
            "" | "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => eprintln!("please answer y or n"),
        }
    }
}

/// Like [`confirm`], but shows `hint` dimmed and indented on the line below
/// the prompt, with the cursor left at the answer position:
///
///   Pull 2026-07-01 from mediabox? [Y/n]: _
///       (rsync ...)
///
/// The hint is cleared once answered. Without a usable tty width the same
/// layout is printed without cursor movement (answered on the line below).
fn confirm_with_hint(question: &str, hint: &str) -> Result<bool> {
    let hint_line = format!("    ({hint})");
    let cols = terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .filter(|w| *w > 0);

    let Some(cols) = cols else {
        println!("{} {}", question.bold(), "[Y/n]:".dimmed());
        println!("{}", hint_line.dimmed());
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        return match line.trim().to_lowercase().as_str() {
            "" | "y" | "yes" => Ok(true),
            "n" | "no" => Ok(false),
            _ => {
                eprintln!("please answer y or n");
                confirm(question)
            }
        };
    };

    // print prompt + hint, then park the cursor back at the answer position
    // (one row per wrapped hint line up, then just past the prompt)
    let prompt_cols = question.chars().count() + " [Y/n]: ".len();
    let hint_rows = hint_line.chars().count().div_ceil(cols).max(1);
    print!(
        "{} {}\n{}\x1b[{hint_rows}A\x1b[{col}G",
        question.bold(),
        "[Y/n]:".dimmed(),
        hint_line.dimmed(),
        col = prompt_cols + 1
    );
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    // enter leaves the cursor at the start of the hint; clear it
    print!("\x1b[0J");
    io::stdout().flush()?;
    match line.trim().to_lowercase().as_str() {
        "" | "y" | "yes" => Ok(true),
        "n" | "no" => Ok(false),
        _ => {
            eprintln!("please answer y or n");
            confirm(question)
        }
    }
}

/// Prompt for a value; empty input takes the default, or re-asks if there is
/// none.
fn prompt(label: &str, default: Option<&str>) -> Result<String> {
    loop {
        match default {
            Some(d) => print!("{} {} ", label.bold(), format!("[{d}]:").dimmed()),
            None => print!("{}: ", label.bold()),
        }
        io::stdout().flush()?;
        let mut line = String::new();
        ensure!(io::stdin().read_line(&mut line)? > 0, "no input");
        let line = line.trim();
        match (line.is_empty(), default) {
            (false, _) => return Ok(line.to_string()),
            (true, Some(d)) => return Ok(d.to_string()),
            (true, None) => eprintln!("a value is required"),
        }
    }
}

fn configure() -> Result<()> {
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

fn pull(cfg: &Config, folder: &Folder) -> Result<()> {
    let remote_dir = cfg.remote_dir(folder);
    let remote_co = cfg.remote_checked_out(folder);
    let local_dir = cfg.local_dir(folder);

    let (dir_exists, co_exists) = remote_state(cfg, folder)?;
    let (src, need_rename) = if co_exists {
        eprintln!(
            "{}",
            format!(
                "note: {} is already checked out on {}; resuming pull",
                folder.name, cfg.remote_host
            )
            .dimmed()
        );
        (remote_co.clone(), false)
    } else if dir_exists {
        (remote_dir.clone(), true)
    } else {
        bail!("{remote_dir} not found on {}", cfg.remote_host);
    };

    fs::create_dir_all(cfg.local_root.join(&folder.year))?;
    // remote -> local only, no delete flags: the NAS copy is never touched
    rsync(
        cfg,
        &format!("Pull {} from {}", folder.name, cfg.remote_host),
        &[],
        &format!("{}:{}/", cfg.remote_host, src),
        &format!("{}/", local_dir.display()),
    )?;

    if need_rename {
        remote_mv(cfg, &remote_dir, &remote_co)?;
    }

    println!(
        "{}",
        format!(
            "pulled {} -> {} (remote copy kept as {}{})",
            folder.name,
            local_dir.display(),
            folder.name,
            cfg.checked_out_suffix
        )
        .green()
    );
    Ok(())
}

fn push(cfg: &Config, folder: &Folder) -> Result<()> {
    let remote_dir = cfg.remote_dir(folder);
    let remote_co = cfg.remote_checked_out(folder);
    let local_dir = cfg.local_dir(folder);

    ensure!(local_dir.is_dir(), "{} not found", local_dir.display());
    let (dir_exists, co_exists) = remote_state(cfg, folder)?;

    let dest_exists = dir_exists || co_exists;
    let dest = if co_exists {
        remote_co.clone()
    } else if dir_exists {
        remote_dir.clone()
    } else {
        eprintln!(
            "{}",
            format!(
                "note: {} does not exist on {}; creating it",
                folder.name, cfg.remote_host
            )
            .dimmed()
        );
        let year_dir = format!("{}/{}", cfg.remote_root, folder.year);
        remote_run(cfg, &format!("mkdir -p {}", sh_quote(&year_dir)))?;
        remote_dir.clone()
    };

    // files deleted locally while checked out move to the culled tree before
    // the sync, so they neither reappear in Lightroom nor get lost
    if dest_exists {
        cull_removed(cfg, folder, &dest, &local_dir)?;
    }

    rsync(
        cfg,
        &format!("Push {} to {}", folder.name, cfg.remote_host),
        &["--remove-source-files"],
        &format!("{}/", local_dir.display()),
        &format!("{}:{}/", cfg.remote_host, dest),
    )?;

    if dest == remote_co {
        remote_mv(cfg, &remote_co, &remote_dir)?;
    }

    if !cfg.dry_run {
        // rsync --remove-source-files leaves empty directories behind
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
            folder.name, cfg.remote_host, remote_dir
        )
        .green()
    );
    Ok(())
}

fn list_folders(cfg: &Config, cmd: &str) -> Result<()> {
    let mut folders = BTreeSet::new();
    match cmd {
        // anything sitting in the local tree can be pushed
        "push" => {
            for year in read_dirs(&cfg.local_root)? {
                for folder in read_dirs(&year)? {
                    if let Some(name) = folder.file_name().and_then(|n| n.to_str()) {
                        folders.insert(name.to_string());
                    }
                }
            }
        }
        // remote folders; checked-out ones are offered too since pull resumes them
        "pull" => {
            let out = Command::new("ssh")
                .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=3"])
                .arg(&cfg.remote_host)
                .arg(format!(
                    "find {} -mindepth 2 -maxdepth 2 -type d",
                    sh_quote(&cfg.remote_root)
                ))
                .output()
                .context("failed to run ssh")?;
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let name = line.rsplit('/').next().unwrap_or(line);
                let name = name.strip_suffix(&cfg.checked_out_suffix).unwrap_or(name);
                folders.insert(name.to_string());
            }
        }
        _ => unreachable!(),
    }
    // newest first, so completion cycling starts with recent folders
    for f in folders.into_iter().rev() {
        println!("{f}");
    }
    Ok(())
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

    let culled_dir = format!("{}/{}/{}", cfg.culled_root, folder.year, folder.name);
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
            folder.name,
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
    if !confirm(&format!(
        "Move them to {culled_dir} on {}?",
        cfg.remote_host
    ))? {
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
    remote_script(cfg, &script)
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

/// Relative paths of all regular files under the remote directory.
fn remote_file_list(cfg: &Config, dir: &str) -> Result<Vec<String>> {
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

/// Run a multi-line shell script on the NAS via stdin (avoids arg limits).
fn remote_script(cfg: &Config, script: &str) -> Result<()> {
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
        "culled-file move on {} failed: {status}",
        cfg.remote_host
    );
    Ok(())
}

/// Whether the plain and checked-out folders exist on the NAS (one ssh probe
/// each). Fails if both exist, since it is ambiguous which is authoritative.
fn remote_state(cfg: &Config, folder: &Folder) -> Result<(bool, bool)> {
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

/// Announce `action`, show the exact rsync command dimmed, confirm, run.
fn rsync(cfg: &Config, action: &str, extra_args: &[&str], src: &str, dest: &str) -> Result<()> {
    let mut cmd = Command::new("rsync");

    cmd.args(["-a", "-h", "--info=progress2"]);

    if cfg.dry_run {
        cmd.arg("--dry-run");
    }

    cmd.args(extra_args).arg(src).arg(dest);

    let shown: Vec<_> = std::iter::once(cmd.get_program())
        .chain(cmd.get_args())
        .map(|w| w.to_string_lossy())
        .collect();
    let command = shown.join(" ");
    let question = format!("{action}?");

    // verbose already shows the full command, so no need for the inline hint
    let proceed = if cfg.verbose {
        println!("{}", format!("+ {command}").dimmed());
        confirm(&question)?
    } else {
        confirm_with_hint(&question, &command)?
    };
    if !proceed {
        bail!("aborted");
    }
    let status = cmd.status().context("failed to run rsync")?;
    ensure!(status.success(), "rsync {src} -> {dest} failed: {status}");
    Ok(())
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

fn remote_mv(cfg: &Config, from: &str, to: &str) -> Result<()> {
    remote_run(cfg, &format!("mv {} {}", sh_quote(from), sh_quote(to)))
}

/// Run a state-changing command on the NAS (echoed instead when --dry-run).
fn remote_run(cfg: &Config, command: &str) -> Result<()> {
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
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
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
    fn parse_folder_accepts_plain_and_normalizes() {
        for raw in ["2026-07-01", "2026-07-01/", "2026-07-01.checked-out"] {
            let f = Folder::parse(raw, CHECKED_OUT_SUFFIX).unwrap();
            assert_eq!(f.name, "2026-07-01");
            assert_eq!(f.year, "2026");
        }
    }

    #[test]
    fn parse_folder_strips_custom_suffix() {
        let f = Folder::parse("2026-07-01.out", ".out").unwrap();
        assert_eq!(f.name, "2026-07-01");
    }

    #[test]
    fn parse_folder_rejects_bad_names() {
        for raw in ["july", "20-07-01", "2026", "2026-07/evil", ""] {
            assert!(
                Folder::parse(raw, CHECKED_OUT_SUFFIX).is_err(),
                "should reject {raw:?}"
            );
        }
    }

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

    #[test]
    fn sh_quote_escapes_single_quotes() {
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
    }
}
