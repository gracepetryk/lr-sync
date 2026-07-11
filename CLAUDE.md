# CLAUDE.md

`lr-sync` is a Rust CLI for moving dated photo folders between a local
Lightroom tree and an archive tree on a NAS with rsync, to manage limited
laptop disk space. The roots are user configuration, never hardcoded.

## Commands

```sh
cargo build          # binary at target/debug/lr-sync
cargo test           # unit tests live in each module's tests submodule
cargo clippy         # keep clean
```

## Layout

- `src/main.rs` — clap derive CLI (`Cli`/`Cmd`) and dispatch
- `src/commands.rs` — `pull`, `push`, `list_folders`, culled-file handling,
  local-fs helpers
- `src/config.rs` — `Config` (flags > config file), config-file parsing,
  the `configure` subcommand
- `src/folder.rs` — `Folder` name parsing (`YYYY-MM-DD…` → name + year)
- `src/remote.rs` — ssh helpers (`remote_run`, `remote_script`, `sh_quote`, …)
- `src/rsync.rs` — building, confirming, and running rsync commands
- `src/ui.rs` — interactive prompts (`confirm`, `confirm_with_hint`, `prompt`)
- `_lr-sync` — zsh completion; `push` uses native path completion (`_files`
  rooted at the hidden `lr-sync local-root`), `pull` descends one remote layer
  per tab via the hidden `lr-sync list-folders <parent>` (the dirs directly
  below `<parent>`, hidden dirs pruned, checked-out suffix stripped from every
  component so a checked-out parent stays navigable under its clean name) —
  both complete one layer per tab

## Behavior invariants

Folders are addressed by their path relative to the tree roots, taken
literally and mirrored on both sides — any layering works
(`2026/2026-07-01`, `2026/07/01`, `2026/2026-07/2026-07-01`). Names are not
validated (only path safety: relative, no `..`); a parent layer (`2026/07`,
or a bare `2026`) names everything under it as one unit — it checks out and
returns as a whole.

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
- `pull` and `push` accept multiple folders. Every folder is resolved (and
  culled files settled) before anything transfers, then the batch is confirmed
  once and rsynced folder by folder (each folder's rename/cleanup right after
  its transfer).
- If both `<folder>` and `<folder>.checked-out` exist on the NAS, refuse to do
  anything and tell the user to resolve it manually.
- `push` refuses when a parent layer of the target is checked out on the NAS
  (one ssh probe covering all layers): pushing below it would create a plain
  tree next to the renamed one, which Lightroom then sees twice.
- Pushing a parent un-checks-out any checked-out subdirs under the
  destination first (renamed back deepest-first in one remote script,
  refusing if a plain sibling exists), so the rsync merges into them; this
  runs before the cull comparison so their files aren't mistaken for culled.
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
- `rsync` confirms with "Push <n> photos to <host>? [Y/n]:" — the count skips
  sidecars/metadata (`is_photo`: no xmp/lua, dotfiles, Thumbs.db) — with the
  exact command(s) dimmed and indented on the lines below (cleared after
  answering;
  plain layout without cursor movement when there's no usable tty width or
  under `--verbose`). Declining aborts, which is always safe: the rename-back
  happens after the sync. `--verbose` also traces each ssh command;
  `--dry-run` implies `--verbose` and shows up as a literal `--dry-run` in
  the rsync command rather than skipping it. `-y`/`--yes` answers yes to
  every confirmation (rsync and culled-move) but still prints the commands
  and the culled-file listing. Colors come from the `colored`
  crate (auto-disabled when piped / `NO_COLOR`).
- After changing Rust behavior that completion depends on, `cargo install
  --path .` — the completion shells out to the installed binary, not
  `target/debug`.
