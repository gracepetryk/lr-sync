//! lr-sync -- move dated photo folders between the local Lightroom tree and
//! the NAS archive.
//!
//!   lr-sync pull 2026-07-01   NAS -> laptop; NAS copy renamed *.checked-out
//!   lr-sync push 2026-07-01   laptop -> NAS; NAS name restored, local copy removed
//!
//! `pull` never deletes anything on the NAS. The rename to *.checked-out makes
//! Lightroom "lose" the remote folder (so it can be re-pointed at the local
//! copy) and lets `push` return the folder with a cheap incremental rsync.

mod commands;
mod config;
mod folder;
mod remote;
mod rsync;
mod ui;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use crate::config::{Config, configure};
use crate::folder::parse_folders;

#[derive(Parser)]
#[command(
    name = "lr-sync",
    about = "Move dated photo folders between the local Lightroom tree and the remote archive"
)]
pub struct Cli {
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

    /// Answer yes to every confirmation prompt
    #[arg(short = 'y', long, global = true)]
    yes: bool,

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
    /// Copy folders from the remote to the local tree; the remote copies are
    /// renamed with the checked-out suffix (never deleted)
    Pull {
        #[arg(required = true)]
        folders: Vec<String>,
    },

    /// Copy folders back to the remote, restore their original names, and
    /// remove the local copies as files transfer
    Push {
        #[arg(required = true)]
        folders: Vec<String>,
    },

    /// Create the config file by prompting for each value
    Configure,

    /// List candidate folders (used by shell completion)
    #[command(hide = true)]
    ListFolders {
        #[arg(value_parser = ["pull", "push"])]
        cmd: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if matches!(cli.command, Cmd::Configure) {
        return configure();
    }
    let cfg = Config::from_cli(&cli)?;
    match &cli.command {
        Cmd::Pull { folders } => {
            commands::pull(&cfg, &parse_folders(folders, &cfg.checked_out_suffix)?)
        }
        Cmd::Push { folders } => {
            commands::push(&cfg, &parse_folders(folders, &cfg.checked_out_suffix)?)
        }
        Cmd::ListFolders { cmd } => commands::list_folders(&cfg, cmd),
        Cmd::Configure => unreachable!(),
    }
}
