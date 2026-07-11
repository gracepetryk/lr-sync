//! Folder paths relative to the tree roots. The path is taken literally and
//! mirrored on both sides (`2026/2026-07-01`, `2026/07/01`,
//! `2026/2026-07/2026-07-01`, ...); naming a parent layer (a whole month, or
//! a whole year) moves everything under it as one unit.

use anyhow::{Result, ensure};

pub struct Folder {
    /// Path of the folder relative to the tree roots,
    /// e.g. "2026/2026-07-01" or "2026/2026-07/2026-07-01".
    pub rel: String,
}

impl Folder {
    /// Accepts a root-relative path, with or without a trailing slash or the
    /// checked-out suffix. Only path safety is checked (relative, no empty
    /// or `.`/`..` components) — push and cull move and delete through the
    /// derived paths, so they must stay inside the trees.
    pub fn parse(raw: &str, checked_out_suffix: &str) -> Result<Folder> {
        let path = raw.trim_end_matches('/');
        let path = path.strip_suffix(checked_out_suffix).unwrap_or(path);
        ensure!(
            !path.starts_with('/')
                && !path.is_empty()
                && path
                    .split('/')
                    .all(|c| !c.is_empty() && c != "." && c != ".."),
            "folder must be a path relative to the tree root, e.g. 2026/07/01 (got: {raw})"
        );
        Ok(Folder {
            rel: path.to_string(),
        })
    }

    /// The layers above this folder, outermost first
    /// ("2026/07/01" -> "2026", "2026/07").
    pub fn ancestors(&self) -> impl Iterator<Item = &str> {
        self.rel.match_indices('/').map(|(i, _)| &self.rel[..i])
    }
}

pub fn parse_folders(raw: &[String], checked_out_suffix: &str) -> Result<Vec<Folder>> {
    raw.iter()
        .map(|r| Folder::parse(r, checked_out_suffix))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CHECKED_OUT_SUFFIX;

    #[test]
    fn parse_folder_takes_paths_literally() {
        for raw in [
            "2026/2026-07-01",
            "2026/07/01",
            "2026/07/01/",
            "2026/07/01.checked-out",
            "2026/2026-07/2026-07-01",
            "2026",
            "2026/07",
        ] {
            let f = Folder::parse(raw, CHECKED_OUT_SUFFIX).unwrap();
            assert_eq!(
                f.rel,
                raw.trim_end_matches('/').trim_end_matches(".checked-out")
            );
        }
    }

    #[test]
    fn ancestors_lists_layers_outermost_first() {
        let f = Folder::parse("2026/07/01", CHECKED_OUT_SUFFIX).unwrap();
        assert_eq!(f.ancestors().collect::<Vec<_>>(), ["2026", "2026/07"]);
        let f = Folder::parse("2026", CHECKED_OUT_SUFFIX).unwrap();
        assert_eq!(f.ancestors().count(), 0);
    }

    #[test]
    fn parse_folder_strips_custom_suffix() {
        let f = Folder::parse("2026/07/01.out", ".out").unwrap();
        assert_eq!(f.rel, "2026/07/01");
    }

    #[test]
    fn parse_folder_rejects_unsafe_paths() {
        for raw in ["/2026/07/01", "2026//01", "2026/../etc", "2026/.", "", "/"] {
            assert!(
                Folder::parse(raw, CHECKED_OUT_SUFFIX).is_err(),
                "should reject {raw:?}"
            );
        }
    }
}
