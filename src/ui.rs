//! Interactive terminal prompts.

use anyhow::{Result, ensure};
use colored::Colorize;
use std::io::{self, Write};

/// Ask a yes/no question; enter (or EOF) takes the default "yes".
pub fn confirm(question: &str) -> Result<bool> {
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

/// Like [`confirm`], but shows `hints` dimmed and indented (one per line)
/// below the prompt, with the cursor left at the answer position:
///
///   Pull 123 photos from mediabox? [Y/n]: _
///       (rsync ...)
///       (rsync ...)
///
/// The hints are cleared once answered. Without a usable tty width the same
/// layout is printed without cursor movement (answered on the line below).
pub fn confirm_with_hint(question: &str, hints: &[String]) -> Result<bool> {
    let hint_lines: Vec<String> = hints.iter().map(|h| format!("    ({h})")).collect();
    let cols = terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .filter(|w| *w > 0);

    let Some(cols) = cols else {
        println!("{} {}", question.bold(), "[Y/n]:".dimmed());
        for line in &hint_lines {
            println!("{}", line.dimmed());
        }
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

    // print prompt + hints, then park the cursor back at the answer position
    // (one row per wrapped hint line up, then just past the prompt)
    let prompt_cols = question.chars().count() + " [Y/n]: ".len();
    let hint_rows: usize = hint_lines
        .iter()
        .map(|l| l.chars().count().div_ceil(cols).max(1))
        .sum();
    let hint_block = hint_lines
        .iter()
        .map(|l| l.dimmed().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    print!(
        "{} {}\n{hint_block}\x1b[{hint_rows}A\x1b[{col}G",
        question.bold(),
        "[Y/n]:".dimmed(),
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
pub fn prompt(label: &str, default: Option<&str>) -> Result<String> {
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
