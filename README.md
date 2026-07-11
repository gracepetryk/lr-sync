# lr-sync

Move dated photo folders between a local Lightroom tree (e.g.
`~/Pictures/Lightroom`) and an archive on a NAS (e.g. `nas:/photos/archive`),
to manage limited laptop disk space. Folders live under a year directory on
both sides, e.g. `<root>/2026/2026-07-01`.

```
lr-sync pull 2026-07-01              # NAS -> laptop
lr-sync push 2026-07-01 2026-07-02   # laptop -> NAS (one or more folders)
```

- `pull` copies a folder to the laptop and marks it checked out on the NAS.
  Lightroom "loses" the checked-out folder, so it can be re-pointed at the
  local copy. Nothing is ever deleted from the NAS.
- `push` syncs local changes back (incremental, so it's fast), restores the
  folder's name on the NAS, and removes the local copy. An interrupted push
  is safe to rerun: the folder stays checked out until the transfer succeeds.
- Photos culled locally while checked out are optionally set aside in a
  separate tree on the NAS — they don't reappear in Lightroom, but nothing
  is lost.

Both commands show what they're about to do and ask before syncing.
`-n`/`--dry-run` previews everything without changing anything.

## Install

```sh
cargo install --path .
lr-sync configure   # set local/remote paths, written to ~/.config/lr-sync
```

### zsh completion

Completes subcommands and folder names (local folders for `push`, NAS folders
for `pull`). Add this repo to your `fpath` before `compinit` in `~/.zshrc`:

```sh
fpath=(/path/to/lr-sync $fpath)
autoload -Uz compinit && compinit
```
