//! Dated folder names (`YYYY-MM-DD…`) and the year directory derived from
//! them.

use anyhow::{Result, ensure};

pub struct Folder {
    pub name: String,
    pub year: String,
}

impl Folder {
    /// Accepts "2026-07-01", "2026-07-01/", or the checked-out form and
    /// normalizes to the plain folder name. The year directory comes from the
    /// first four characters.
    pub fn parse(raw: &str, checked_out_suffix: &str) -> Result<Folder> {
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
}
