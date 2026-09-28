//! Attachments: files kept with an entry, in a directory of its own.
//!
//! Where that directory is follows Org's defaults since 9.3: a `:DIR:`
//! property if the entry has one, and otherwise `data/` next to the Org file,
//! then the entry's `:ID:` split after its first two characters —
//! `data/6b/a7b810-9dad-…`. Splitting keeps any one directory from holding
//! every attachment in the notes. An entry with attachments carries the
//! `ATTACH` tag, and `[[attachment:name]]` links to a file in its directory.

use std::path::{Path, PathBuf};

use crate::restructure::property_value;

/// The tag Org puts on an entry with attachments (`org-attach-auto-tag`).
pub const TAG: &str = "ATTACH";

/// `data/6b/a7b810-9dad-…` for an id.
pub fn id_dir(id: &str) -> PathBuf {
    let id = id.trim();
    let split = id.char_indices().nth(2).map_or(id.len(), |(at, _)| at);
    Path::new("data").join(&id[..split]).join(&id[split..])
}

/// Where the entry at `line` keeps its attachments, or `None` when it has
/// neither a `:DIR:` nor an `:ID:` to say.
pub fn attach_dir(text: &str, line: usize, org_dir: &Path) -> Option<PathBuf> {
    if let Some(dir) = property_value(text, line, "DIR").filter(|dir| !dir.is_empty()) {
        return Some(org_dir.join(dir));
    }
    let id = property_value(text, line, "ID").filter(|id| !id.is_empty())?;
    Some(org_dir.join(id_dir(&id)))
}

/// The files attached to the entry, sorted by name. A missing directory is
/// an entry with nothing attached, not an error.
pub fn list(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                .map(|entry| entry.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_split_after_two_characters() {
        assert_eq!(
            id_dir("6ba7b810-9dad-11d1-80b4-00c04fd430c8"),
            Path::new("data/6b/a7b810-9dad-11d1-80b4-00c04fd430c8")
        );
    }

    #[test]
    fn dir_wins_over_the_id() {
        let text = "* A\n:PROPERTIES:\n:ID: 6ba7b810\n:DIR: files/a\n:END:\n";
        assert_eq!(
            attach_dir(text, 0, Path::new("/n")),
            Some(PathBuf::from("/n/files/a"))
        );

        let text = "* A\n:PROPERTIES:\n:ID: 6ba7b810\n:END:\n";
        assert_eq!(
            attach_dir(text, 2, Path::new("/n")),
            Some(PathBuf::from("/n/data/6b/a7b810"))
        );

        assert_eq!(attach_dir("* A\n", 0, Path::new("/n")), None);
    }

    #[test]
    fn the_preamble_is_an_entry_too() {
        let text = ":PROPERTIES:\n:ID: abcdef\n:END:\n#+title: File\n";
        assert_eq!(
            attach_dir(text, 3, Path::new("/n")),
            Some(PathBuf::from("/n/data/ab/cdef"))
        );
    }

    #[test]
    fn a_missing_directory_lists_nothing() {
        assert!(list(Path::new("/definitely/not/here")).is_empty());
    }
}
