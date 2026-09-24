//! TODO dependencies: what has to be done before an entry can be.
//!
//! Three sources, from Org and its `org-depend` module:
//!
//! * An entry's children. Org enforces this only when
//!   `org-enforce-todo-dependencies` is set, which it is not by default, and
//!   the fork follows it: the editor's `todo-dependencies` setting.
//! * An `:ORDERED:` parent, whose children must be done in order.
//! * A `:BLOCKER:` property naming other entries by id, as `org-depend`
//!   writes it (`:BLOCKER: id1 id2`, or `previous-sibling`) or as `org-edna`
//!   does (`ids(id1 id2)`).
//!
//! The last two are written into the entry on purpose, so they apply
//! whatever the setting says: a `:BLOCKER:` that blocked nothing would be a
//! property that lies.
//!
//! Only going *to* a done state is ever blocked. A task can always be
//! reopened.

use uuid::Uuid;

use crate::parser::{parse_headline, FileSettings};
use crate::restructure::{headline_level, property_value, subtree_range};

/// Something that has to be done first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocker {
    /// An open child, by title.
    Child(String),
    /// An open earlier sibling under an `:ORDERED:` parent.
    Sibling(String),
    /// An entry named in `:BLOCKER:`; whether it is done is the caller's to
    /// find out, since it may be in another file.
    Named(Uuid),
    /// A `:BLOCKER:` word that is neither an id nor a keyword this reads.
    Unreadable(String),
}

/// Whether the headline on `line` has a TODO keyword that is not done.
fn is_open(line: &str, settings: &FileSettings) -> Option<String> {
    let headline = parse_headline(line, settings)?;
    let state = headline.todo?;
    (!state.done).then_some(headline.title)
}

/// The headline lines of the direct children of the entry at `start`.
fn children(lines: &[&str], start: usize, end: usize, level: usize) -> Vec<usize> {
    let below: Vec<(usize, usize)> = (start + 1..end)
        .filter_map(|at| headline_level(lines[at]).map(|l| (at, l)))
        .filter(|(_, l)| *l > level)
        .collect();
    let shallowest = below.iter().map(|(_, l)| *l).min();
    below
        .into_iter()
        .filter(|(_, l)| Some(*l) == shallowest)
        .map(|(at, _)| at)
        .collect()
}

/// What stands between the entry at `line` and a done state.
///
/// `children_block` is the editor's `todo-dependencies` setting.
pub fn blockers(
    text: &str,
    line: usize,
    settings: &FileSettings,
    children_block: bool,
) -> Vec<Blocker> {
    let owned: Vec<String> = text.lines().map(str::to_string).collect();
    let lines: Vec<&str> = owned.iter().map(String::as_str).collect();
    let Some((start, end, level)) = subtree_range(&owned, line) else {
        return Vec::new();
    };
    let mut found = Vec::new();

    if children_block {
        for child in children(&lines, start, end, level) {
            if let Some(title) = is_open(lines[child], settings) {
                found.push(Blocker::Child(title));
            }
        }
    }

    // Walk up: an ORDERED parent blocks on earlier siblings, and so does an
    // ORDERED grandparent on the parent's earlier siblings, since the parent
    // itself could not be done before them.
    let mut entry = start;
    while let Some(parent) = (0..entry).rev().find(|&at| {
        headline_level(lines[at]).is_some_and(|l| l < headline_level(lines[entry]).unwrap_or(1))
    }) {
        let ordered = property_value(text, parent, "ORDERED")
            .is_some_and(|value| !value.is_empty() && value != "nil");
        if ordered {
            let (_, parent_end, parent_level) =
                subtree_range(&owned, parent).unwrap_or((parent, entry, 1));
            for sibling in children(&lines, parent, parent_end, parent_level) {
                if sibling >= entry {
                    break;
                }
                if let Some(title) = is_open(lines[sibling], settings) {
                    found.push(Blocker::Sibling(title));
                }
            }
        }
        entry = parent;
    }

    if let Some(value) = property_value(text, start, "BLOCKER") {
        let inner = value
            .trim()
            .strip_prefix("ids(")
            .and_then(|rest| rest.strip_suffix(')'))
            .unwrap_or(value.trim());
        for word in inner.split_whitespace() {
            let word = word.trim_start_matches("id:");
            if word == "previous-sibling" {
                if let Some(previous) = previous_sibling(&lines, start) {
                    if let Some(title) = is_open(lines[previous], settings) {
                        found.push(Blocker::Sibling(title));
                    }
                }
            } else {
                match word.parse::<Uuid>() {
                    Ok(id) => found.push(Blocker::Named(id)),
                    Err(_) => found.push(Blocker::Unreadable(word.to_string())),
                }
            }
        }
    }

    found.dedup();
    found
}

/// The headline before `start` at the same level, under the same parent.
fn previous_sibling(lines: &[&str], start: usize) -> Option<usize> {
    let level = headline_level(lines[start])?;
    (0..start)
        .rev()
        .map(|at| (at, headline_level(lines[at])))
        .find(|(_, l)| l.is_some_and(|l| l <= level))
        .filter(|(_, l)| *l == Some(level))
        .map(|(at, _)| at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(text: &str, line: usize, children: bool) -> Vec<Blocker> {
        blockers(text, line, &FileSettings::scan(text), children)
    }

    const PROJECT: &str = "\
* TODO Project
** DONE Plan
** TODO Build
** Notes
";

    #[test]
    fn open_children_block_only_when_the_setting_says_so() {
        assert_eq!(of(PROJECT, 0, true), [Blocker::Child("Build".to_string())]);
        assert!(of(PROJECT, 0, false).is_empty());
    }

    #[test]
    fn an_ordered_parent_blocks_on_earlier_siblings_whatever_the_setting() {
        let text =
            "* Steps\n:PROPERTIES:\n:ORDERED: t\n:END:\n** TODO One\n** TODO Two\n** TODO Three\n";
        assert!(of(text, 4, false).is_empty());
        assert_eq!(
            of(text, 6, false),
            [
                Blocker::Sibling("One".to_string()),
                Blocker::Sibling("Two".to_string()),
            ]
        );
    }

    #[test]
    fn ordering_reaches_through_a_parent() {
        let text = "* Steps\n:PROPERTIES:\n:ORDERED: t\n:END:\n** TODO First\n** Second\n*** TODO Inside\n";
        assert_eq!(of(text, 6, false), [Blocker::Sibling("First".to_string())]);
    }

    #[test]
    fn a_blocker_names_entries_either_way_it_is_written() {
        let id = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
        let depend = format!("* TODO A\n:PROPERTIES:\n:BLOCKER: {id} nonsense\n:END:\n");
        assert_eq!(
            of(&depend, 0, false),
            [
                Blocker::Named(id.parse().unwrap()),
                Blocker::Unreadable("nonsense".to_string())
            ]
        );
        let edna = format!("* TODO A\n:PROPERTIES:\n:BLOCKER: ids({id})\n:END:\n");
        assert_eq!(of(&edna, 0, false), [Blocker::Named(id.parse().unwrap())]);
    }

    #[test]
    fn previous_sibling_is_a_blocker_keyword() {
        let text = "* TODO First\n* TODO Second\n:PROPERTIES:\n:BLOCKER: previous-sibling\n:END:\n";
        assert_eq!(of(text, 1, false), [Blocker::Sibling("First".to_string())]);
    }
}
