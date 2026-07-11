# CLAUDE.md

`lr-sync` is a Rust CLI for moving dated photo folders between a local
Lightroom tree and an archive tree on a NAS with rsync, to manage limited
laptop disk space. The roots are user configuration, never hardcoded.

## Commands

```sh
cargo build          # binary at target/debug/lr-sync
cargo test           # unit tests in src/main.rs
cargo clippy         # keep clean
```

## Layout

- `src/main.rs` — the whole CLI (clap derive; shells out to `rsync` and `ssh`)
- `_lr-sync` — zsh completion; calls the hidden `lr-sync list-folders <pull|push>`
  subcommand for dynamic folder candidates (local tree for `push`, NAS via ssh
  for `pull`)

## Behavior invariants

Folders are named `YYYY-MM-DD…` and live under a year directory on both sides
(e.g. `<root>/2026/2026-07-01`); the year is derived from the folder name.

- `pull` copies NAS → local, then renames the NAS folder to
  `<folder>.checked-out`. **`pull` must NEVER delete files from the NAS** — no
  `--delete`, no `--remove-source-files` in that direction. The rename exists
  to make Lightroom "lose" the remote folder (so it prompts to re-point at the
  local copy) and to make the eventual `push` a cheap incremental rsync.
- `push` rsyncs local → the `.checked-out` NAS folder with
  `--remove-source-files`, renames it back to its original name only **after**
  a successful transfer (so an interrupted push leaves the folder marked
  checked out and keeps untransferred files locally), then prunes the emptied
  local directory tree.
- `push` never uses `--delete`. Files culled locally (on the NAS but missing
  locally) are moved before the sync to `<culled root>/<year>/<folder>/`
  (flag `--culled`), preserving relative paths, so they don't reappear in
  Lightroom but are never lost. The files are listed and confirmed
  interactively first (default yes; declining leaves them in place).
  Collisions in the culled tree get `.collision`, `.collision-2`, ...
  suffixes (never overwrite).
- If both `<folder>` and `<folder>.checked-out` exist on the NAS, refuse to do
  anything and tell the user to resolve it manually.
- Pulling an already-checked-out folder resumes from the `.checked-out` copy.
- Local/remote/culled roots and the checked-out suffix come from
  `~/.config/lr-sync/config` (`$XDG_CONFIG_HOME` respected; `key = value`
  lines, written by `lr-sync configure`), overridable per-invocation with
  `--local DIR` / `--remote HOST:DIR` / `--culled DIR` / `--suffix SUFFIX`.
  **No user-specific paths or hostnames in the Rust code** — local and remote
  have no built-in defaults (missing config is an error pointing at
  `configure`); culled's default is derived (sibling of the remote dir named
  `culled/`); the suffix falls back to the `CHECKED_OUT_SUFFIX` constant
  (`.checked-out`). Every command supports `-n`/`--dry-run`.

## Conventions

- Remote paths passed through ssh must go through `sh_quote`.
- Anything that changes state on the NAS goes through `remote_run` (or
  `remote_script` for batches, which pipes via stdin to avoid arg limits) so
  `--dry-run` stays honest.
- `rsync` confirms with "Push <folder> to <host>? [Y/n]:" and the exact
  command dimmed and indented on the line below (cleared after answering;
  plain layout without cursor movement when there's no usable tty width or
  under `--verbose`). Declining aborts, which is always safe: the rename-back
  happens after the sync. `--verbose` also traces each ssh command;
  `--dry-run` implies `--verbose` and shows up as a literal `--dry-run` in
  the rsync command rather than skipping it. Colors come from the `colored`
  crate (auto-disabled when piped / `NO_COLOR`).
- After changing Rust behavior that completion depends on, `cargo install
  --path .` — the completion shells out to the installed binary, not
  `target/debug`.
