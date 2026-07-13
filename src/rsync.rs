//! Building, confirming, and running rsync commands.

use anyhow::{Context, Result, bail, ensure};
use colored::Colorize;
use std::process::Command;

use crate::config::Config;
use crate::ui::{Default, confirm, confirm_with_hint};

pub fn rsync_command(cfg: &Config, extra_args: &[&str], src: &str, dest: &str) -> Command {
    let mut cmd = Command::new("rsync");

    cmd.args(["-a", "-h", "--info=progress2"]);

    if cfg.dry_run {
        cmd.arg("--dry-run");
    }

    cmd.args(extra_args).arg(src).arg(dest);
    cmd
}

fn command_line(cmd: &Command) -> String {
    std::iter::once(cmd.get_program())
        .chain(cmd.get_args())
        .map(|w| w.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Announce `action`, show the exact rsync command(s) dimmed, and confirm the
/// whole batch; declining aborts (safe: nothing has run yet). With --yes
/// there is nothing to ask; each command is echoed as it runs instead.
pub fn confirm_rsyncs(cfg: &Config, action: &str, cmds: &[Command]) -> Result<()> {
    if cfg.yes {
        return Ok(());
    }
    let commands: Vec<String> = cmds.iter().map(command_line).collect();
    let question = format!("{action}?");

    // verbose already shows the full commands, so no need for the inline hint
    let proceed = if cfg.verbose {
        for command in &commands {
            println!("{}", format!("+ {command}").dimmed());
        }
        confirm(&question, Default::Yes)?
    } else {
        confirm_with_hint(&question, &commands)?
    };
    if !proceed {
        bail!("aborted");
    }
    Ok(())
}

/// Whether [`run_rsync`] echoes the command right before the transfer.
/// Echoed under --yes, where the batch confirmation didn't show the commands
/// upfront; silent otherwise, since the confirmation already did.
#[derive(PartialEq)]
pub enum Echo {
    Command,
    Silent,
}

impl Echo {
    pub fn should_echo(&self) -> bool {
        match self {
            Echo::Command => true,
            Echo::Silent => false,
        }
    }
}

impl From<bool> for Echo {
    fn from(value: bool) -> Self {
        if value { Echo::Command } else { Echo::Silent }
    }
}

pub fn run_rsync(echo: Echo, cmd: &mut Command) -> Result<()> {
    if echo.should_echo() {
        println!("{}", format!("+ {}", command_line(cmd)).dimmed());
    }

    let status = cmd.status().context("failed to run rsync")?;
    ensure!(status.success(), "{} failed: {status}", command_line(cmd));
    Ok(())
}
