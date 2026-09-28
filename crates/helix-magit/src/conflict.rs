//! Conflicted files: the regions git marks, and taking a side.
//!
//! A conflicted file holds each disputed region as
//!
//! ```text
//! <<<<<<< ours
//! …
//! ||||||| base        (only with merge.conflictStyle = diff3 or zdiff3)
//! …
//! =======
//! …
//! >>>>>>> theirs
//! ```
//!
//! and the index holds the three whole versions as stages 1 (base), 2
//! (ours) and 3 (theirs). Resolving is editing the file until no region is
//! left, then adding it.

use std::path::Path;

use crate::command::GitCommand;

/// The length git's markers have unless `conflict-marker-size` says
/// otherwise.
const MARKER: usize = 7;

/// One disputed region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    /// The line of `<<<<<<<`.
    pub start: usize,
    /// The line after `>>>>>>>`.
    pub end: usize,
    pub ours: Vec<String>,
    /// Present when the file was written with the base section.
    pub base: Option<Vec<String>>,
    pub theirs: Vec<String>,
    /// What follows the markers: usually `HEAD` and the other branch.
    pub ours_label: String,
    pub theirs_label: String,
}

/// A side of a conflict to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Ours,
    Theirs,
    Base,
    /// Ours, then theirs.
    Both,
}

impl Side {
    pub fn parse(word: &str) -> Option<Side> {
        match word {
            "ours" | "o" | "upper" => Some(Side::Ours),
            "theirs" | "t" | "lower" => Some(Side::Theirs),
            "base" | "b" => Some(Side::Base),
            "both" | "all" => Some(Side::Both),
            _ => None,
        }
    }

    /// The index stage holding this side of the whole file.
    pub fn stage(self) -> Option<u8> {
        match self {
            Side::Base => Some(1),
            Side::Ours => Some(2),
            Side::Theirs => Some(3),
            Side::Both => None,
        }
    }
}

/// The marker a line is, with what follows it.
fn marker(line: &str, symbol: char) -> Option<&str> {
    let rest = line.strip_prefix(&symbol.to_string().repeat(MARKER))?;
    if rest.starts_with(symbol) {
        return None;
    }
    if rest.is_empty() {
        Some("")
    } else {
        rest.strip_prefix(' ')
    }
}

/// Every complete region in `text`. An unfinished one — a `<<<<<<<` with
/// no `>>>>>>>` after it — is not a region, so a half-edited file is not
/// misread.
pub fn regions(text: &str) -> Vec<Region> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        let Some(ours_label) = marker(lines[at], '<') else {
            at += 1;
            continue;
        };
        let start = at;
        let mut ours = Vec::new();
        let mut base: Option<Vec<String>> = None;
        let mut theirs = Vec::new();
        // 0: ours, 1: base, 2: theirs.
        let mut part = 0;
        let mut end = None;
        let mut theirs_label = String::new();
        let mut index = at + 1;
        while index < lines.len() {
            let line = lines[index];
            if marker(line, '<').is_some() {
                break;
            }
            if part == 0 && marker(line, '|').is_some() {
                part = 1;
                base = Some(Vec::new());
            } else if part < 2 && marker(line, '=').is_some() {
                part = 2;
            } else if part == 2 && marker(line, '>').is_some() {
                theirs_label = marker(line, '>').unwrap_or("").to_string();
                end = Some(index + 1);
                break;
            } else {
                let line = line.to_string();
                match part {
                    0 => ours.push(line),
                    1 => base.get_or_insert_with(Vec::new).push(line),
                    _ => theirs.push(line),
                }
            }
            index += 1;
        }
        match end {
            Some(end) => {
                out.push(Region {
                    start,
                    end,
                    ours,
                    base,
                    theirs,
                    ours_label: ours_label.to_string(),
                    theirs_label,
                });
                at = end;
            }
            None => at = index.max(at + 1),
        }
    }
    out
}

/// The region containing `line`, or the next one after it.
pub fn region_at(regions: &[Region], line: usize) -> Option<&Region> {
    regions
        .iter()
        .find(|region| line >= region.start && line < region.end)
}

/// The lines a region becomes when `side` is kept; `None` when the base is
/// asked for and the file has no base section.
pub fn kept(region: &Region, side: Side) -> Option<Vec<String>> {
    Some(match side {
        Side::Ours => region.ours.clone(),
        Side::Theirs => region.theirs.clone(),
        Side::Base => region.base.clone()?,
        Side::Both => region.ours.iter().chain(&region.theirs).cloned().collect(),
    })
}

/// The first line that is still a conflict marker, if any: what makes a
/// file not yet resolved.
pub fn first_marker(text: &str) -> Option<usize> {
    text.lines().position(|line| {
        marker(line, '<').is_some() || marker(line, '>').is_some() || marker(line, '|').is_some()
    })
}

/// One side of a conflicted path as the index holds it; `None` when that
/// side does not have the file (added on one side, deleted on the other).
pub fn stage_content(workdir: &Path, path: &Path, side: Side) -> Option<String> {
    let stage = side.stage()?;
    let spec = format!(":{stage}:{}", path.display());
    let output = GitCommand::new(workdir, vec!["show".into(), spec])
        .run()
        .ok()?;
    output.success.then_some(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MERGE: &str = "\
keep
<<<<<<< HEAD
ours 1
ours 2
=======
theirs
>>>>>>> topic
middle
<<<<<<< HEAD
a
||||||| base
b
=======
c
>>>>>>> topic
end
";

    #[test]
    fn regions_are_read_with_or_without_a_base() {
        let found = regions(MERGE);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].start, found[0].end), (1, 7));
        assert_eq!(found[0].ours, ["ours 1", "ours 2"]);
        assert_eq!(found[0].theirs, ["theirs"]);
        assert_eq!(found[0].base, None);
        assert_eq!(found[0].ours_label, "HEAD");
        assert_eq!(found[0].theirs_label, "topic");
        assert_eq!(found[1].base.as_deref(), Some(&["b".to_string()][..]));
        assert_eq!((found[1].start, found[1].end), (8, 15));
    }

    #[test]
    fn a_side_is_kept_per_region() {
        let found = regions(MERGE);
        assert_eq!(kept(&found[0], Side::Theirs).unwrap(), ["theirs"]);
        assert_eq!(
            kept(&found[0], Side::Both).unwrap(),
            ["ours 1", "ours 2", "theirs"]
        );
        assert_eq!(kept(&found[0], Side::Base), None);
        assert_eq!(kept(&found[1], Side::Base).unwrap(), ["b"]);
    }

    #[test]
    fn the_cursor_finds_its_region() {
        let found = regions(MERGE);
        assert_eq!(region_at(&found, 3).map(|r| r.start), Some(1));
        assert_eq!(region_at(&found, 7), None, "between regions");
        assert_eq!(region_at(&found, 14).map(|r| r.start), Some(8));
    }

    #[test]
    fn lookalikes_and_unfinished_regions_are_not_regions() {
        // Eight characters, or a lone opening marker, are not conflicts.
        assert!(regions("<<<<<<<< not\n========\n>>>>>>>> one\n").is_empty());
        assert!(regions("<<<<<<< HEAD\nhalf\n=======\n").is_empty());
        // A markdown underline of `=` is only a separator inside a region.
        assert!(regions("Title\n=======\n").is_empty());
    }

    #[test]
    fn a_leftover_marker_is_found() {
        assert_eq!(first_marker("a\nb\n"), None);
        assert_eq!(first_marker("a\n>>>>>>> topic\n"), Some(1));
    }
}
