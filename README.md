# lr-sync

Move dated photo folders between a local Lightroom tree (e.g.
`~/Pictures/Lightroom`) and an archive tree on a NAS (e.g.
`nas:/photos/archive`) with rsync, to manage limited laptop disk space.
Folders are named `YYYY-MM-DD…` and live under a year directory on both
sides, e.g. `<root>/2026/2026-07-01`.

```
lr-sync pull 2026-07-01   # NAS -> laptop
lr-sync push 2026-07-01   # laptop -> NAS
```

- `pull` copies `<remote>/2026/2026-07-01` to `<local>/2026/2026-07-01`, then
  renames the NAS folder to `2026-07-01.checked-out`. It **never deletes
  files on the NAS**. The rename makes Lightroom "lose" the remote folder so
  it can be re-pointed at the local copy, and marks the folder as checked
  out.
- `push` rsyncs local changes back into the checked-out NAS folder
  (incremental, so it's fast), restores the folder's original name, and
  removes the local copy as files transfer (`--remove-source-files`). The
  rename happens only after a successful transfer, so an interrupted push
  leaves the folder safely checked out.
- Files culled locally while checked out (present on the NAS, deleted
  locally) are listed and, after a `[Y/n]` confirmation, moved to
  `<culled>/<year>/<folder>/` on the NAS before the push sync — they don't
  reappear in Lightroom, but nothing is ever deleted.
  Name collisions in the culled tree get a `.collision` / `.collision-2` /
  ... suffix.

The local, remote, and culled roots plus the checked-out suffix live in
`~/.config/lr-sync/config`; run `lr-sync configure` to create it
interactively (the culled tree defaults to a sibling of the remote dir named
`culled/`, the suffix to `.checked-out`). Per-invocation overrides:
`--local DIR` / `--remote HOST:DIR` / `--culled DIR` / `--suffix SUFFIX`.
Both commands announce what they're about to do (with the exact rsync
command, dimmed) and confirm before syncing. `-v`/`--verbose` also prints
each ssh command as it runs; `-n`/`--dry-run` implies `--verbose`, passes
`--dry-run` through to rsync, and executes nothing else (renames, culled
moves are only echoed).

## Install

```sh
cargo install --path .
lr-sync configure
```

### zsh completion

Completion covers subcommands and folder names (local folders for `push`,
NAS folders for `pull`). Add this repo to your `fpath` before `compinit` in
`~/.zshrc`:

```sh
fpath=(/path/to/lr-sync $fpath)
autoload -Uz compinit && compinit
```
