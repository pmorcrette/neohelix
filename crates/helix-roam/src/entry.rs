//! Every headline of a file, as the agenda reads it.
//!
//! The graph holds nodes: headlines with an `:ID:`. Org's agenda does not
//! care about ids; a `TODO` filed by a capture is a task whether or not it
//! is a node. So the agenda reads entries instead, one per headline, with
//! what its views match on: the state, priority and tags (the entry's own
//! and those it inherits), the planning line, the active timestamps of its
//! text, its properties and its category.

use std::path::{Path, PathBuf};

use crate::node::{Timestamp, TodoState};
use crate::parser::{
    find_ignore_case, parse_headline, parse_id, parse_timestamp, starts_with_ignore_case,
    FileSettings,
};
use crate::Uuid;

/// A headline and what the agenda knows of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub title: String,
    pub file_path: PathBuf,
    /// Zero-based line of the headline.
    pub line: usize,
    /// The line after the entry's own text: the next headline, at any level,
    /// or the end of the file.
    pub end: usize,
    pub level: usize,
    pub todo: Option<TodoState>,
    pub priority: Option<char>,
    /// The headline's own tags.
    pub tags: Vec<String>,
    /// Tags from the headlines above and the file's `#+FILETAGS:`, not
    /// repeating the entry's own.
    pub inherited_tags: Vec<String>,
    pub scheduled: Option<Timestamp>,
    pub deadline: Option<Timestamp>,
    pub closed: Option<Timestamp>,
    /// The active timestamps in the entry's text, planning line aside:
    /// appointments, which the agenda shows on their day.
    pub timestamps: Vec<Timestamp>,
    /// The `CLOCK:` lines of its logbook, start and end (`None` while
    /// running), in minutes since the epoch.
    pub clocks: Vec<(crate::clock::Moment, Option<crate::clock::Moment>)>,
    /// Its `%%(diary-…)` lines, headline included.
    pub diary: Vec<String>,
    /// The property drawer, keys lowercased.
    pub properties: Vec<(String, String)>,
    /// Titles of the headlines above, outermost first.
    pub outline_path: Vec<String>,
    /// The `:CATEGORY:` property, inherited, or the file's `#+CATEGORY:`,
    /// or the file's name without its extension.
    pub category: String,
    /// The `:ID:`, when the entry is also a node.
    pub id: Option<Uuid>,
}

impl Entry {
    /// Every tag the entry carries, its own first.
    pub fn all_tags(&self) -> impl Iterator<Item = &String> {
        self.tags.iter().chain(&self.inherited_tags)
    }

    /// A property of the entry's drawer, by name in any case.
    pub fn property(&self, name: &str) -> Option<&str> {
        self.properties
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Whether the entry is a task not yet done.
    pub fn is_open_task(&self) -> bool {
        self.todo.as_ref().is_some_and(|state| !state.done)
    }
}

/// The entries of a file's text.
pub fn entries(text: &str, path: &Path, settings: &FileSettings) -> Vec<Entry> {
    let lines: Vec<&str> = text.lines().collect();
    let file_category = settings.category.clone().unwrap_or_else(|| {
        path.file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    let mut file_tags = Vec::new();

    let mut entries: Vec<Entry> = Vec::new();
    // The headlines above the one being read: level, title, tags, category.
    let mut above: Vec<(usize, String, Vec<String>, Option<String>)> = Vec::new();
    let mut block = false;

    for (at, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if entries.is_empty() {
            if let Some(value) = keyword(trimmed, "filetags") {
                file_tags.extend(
                    value
                        .split([':', ' ', '\t'])
                        .filter(|tag| !tag.is_empty())
                        .map(str::to_string),
                );
            }
        }
        // Stars inside a block are its text, not headlines.
        if starts_with_ignore_case(trimmed, "#+begin_") {
            block = true;
        } else if starts_with_ignore_case(trimmed, "#+end_") {
            block = false;
        }
        if block {
            continue;
        }
        let Some(headline) = parse_headline(line, settings) else {
            continue;
        };
        if let Some(previous) = entries.last_mut() {
            previous.end = at;
        }

        above.retain(|(level, ..)| *level < headline.level);
        let mut inherited_tags: Vec<String> = Vec::new();
        for tag in file_tags
            .iter()
            .chain(above.iter().flat_map(|(_, _, tags, _)| tags))
        {
            if !headline.tags.contains(tag) && !inherited_tags.contains(tag) {
                inherited_tags.push(tag.clone());
            }
        }
        let outline_path = above.iter().map(|(_, title, ..)| title.clone()).collect();
        let inherited_category = above
            .iter()
            .rev()
            .find_map(|(.., category)| category.clone());

        let mut entry = Entry {
            title: headline.title.clone(),
            file_path: path.to_path_buf(),
            line: at,
            end: lines.len(),
            level: headline.level,
            todo: headline.todo,
            priority: headline.priority,
            tags: headline.tags.clone(),
            inherited_tags,
            scheduled: None,
            deadline: None,
            closed: None,
            timestamps: Vec::new(),
            clocks: Vec::new(),
            diary: Vec::new(),
            properties: Vec::new(),
            outline_path,
            category: String::new(),
            id: None,
        };
        // A timestamp in the headline is the entry's too, and so is a
        // diary expression.
        entry.timestamps = active_timestamps(line);
        if headline.title.starts_with("%%(") {
            entry.diary.push(headline.title.clone());
        }
        read_section(&lines, at + 1, settings, &mut entry);
        let own_category = entry.property("category").map(str::to_string);
        entry.category = own_category
            .clone()
            .or(inherited_category)
            .unwrap_or_else(|| file_category.clone());
        above.push((headline.level, headline.title, headline.tags, own_category));
        entries.push(entry);
    }
    entries
}

/// `#+KEY: value`, the key in any case.
fn keyword<'a>(trimmed: &'a str, key: &str) -> Option<&'a str> {
    let rest = trimmed.strip_prefix("#+")?;
    let (name, value) = rest.split_once(':')?;
    name.eq_ignore_ascii_case(key).then(|| value.trim())
}

/// Reads the planning line, the property drawer and the timestamps of the
/// section starting at line `from`, up to the next headline.
fn read_section(lines: &[&str], from: usize, settings: &FileSettings, entry: &mut Entry) {
    let mut drawer = false;
    let mut block = false;
    for (offset, line) in lines[from.min(lines.len())..].iter().enumerate() {
        let trimmed = line.trim();
        if !block && parse_headline(line, settings).is_some() {
            break;
        }
        if starts_with_ignore_case(trimmed, "#+begin_") {
            block = true;
        } else if starts_with_ignore_case(trimmed, "#+end_") {
            block = false;
        }
        if block {
            continue;
        }
        if offset == 0 && is_planning(trimmed) {
            let stamp = |label: &str| {
                let at = find_ignore_case(trimmed, label)?;
                parse_timestamp(trimmed[at + label.len()..].trim_start())
            };
            entry.scheduled = stamp("SCHEDULED:");
            entry.deadline = stamp("DEADLINE:");
            entry.closed = stamp("CLOSED:");
            continue;
        }
        if trimmed.eq_ignore_ascii_case(":PROPERTIES:") && entry.properties.is_empty() {
            drawer = true;
            continue;
        }
        if drawer {
            if trimmed.eq_ignore_ascii_case(":END:") {
                drawer = false;
            } else if let Some((key, value)) = property(trimmed) {
                if key == "id" {
                    entry.id = Some(parse_id(&value));
                }
                entry.properties.push((key, value));
            }
            continue;
        }
        if trimmed.starts_with("%%(") {
            entry.diary.push(trimmed.to_string());
            continue;
        }
        if let Some(clock) = crate::clock::parse_clock(trimmed) {
            entry.clocks.push(clock);
            continue;
        }
        entry.timestamps.extend(active_timestamps(line));
    }
}

/// Whether a line is a planning line: `SCHEDULED:`, `DEADLINE:`, `CLOSED:`.
fn is_planning(trimmed: &str) -> bool {
    ["SCHEDULED:", "DEADLINE:", "CLOSED:"]
        .iter()
        .any(|label| starts_with_ignore_case(trimmed, label))
}

/// Reads `:KEY: value` in a drawer, the key lowercased.
fn property(trimmed: &str) -> Option<(String, String)> {
    let rest = trimmed.strip_prefix(':')?;
    let (key, value) = rest.split_once(':')?;
    if key.is_empty() || key.contains(char::is_whitespace) {
        return None;
    }
    Some((key.to_ascii_lowercase(), value.trim().to_string()))
}

/// The active timestamps in a line: `<2026-09-28 Mon 10:00>`, ranges
/// included.
fn active_timestamps(line: &str) -> Vec<Timestamp> {
    let mut found = Vec::new();
    let mut rest = line;
    while let Some(at) = rest.find('<') {
        let candidate = &rest[at..];
        match parse_timestamp(candidate).filter(|stamp| stamp.active) {
            Some(stamp) => {
                found.push(stamp);
                // Past the stamp, and past the range's end when it has one.
                let mut skip = candidate.find('>').map_or(1, |end| end + 1);
                if candidate[skip..].starts_with("--<") {
                    skip += candidate[skip + 2..].find('>').map_or(0, |end| end + 3);
                }
                rest = &candidate[skip..];
            }
            None => rest = &candidate[1..],
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "\
#+FILETAGS: :notes:
#+CATEGORY: perso
* Projets :work:
:PROPERTIES:
:CATEGORY: pro
:END:
** TODO [#A] Rapport :urgent:
DEADLINE: <2026-10-01 Thu> SCHEDULED: <2026-09-29 Tue>
:PROPERTIES:
:ID: 11111111-2222-3333-4444-555555555555
:Effort: 1:00
:END:
Réunion <2026-09-30 Wed 14:00> puis <2026-10-02 Fri>--<2026-10-03 Sat>.
#+begin_src org
* not a headline <2026-12-01 Tue>
#+end_src
* DONE Fini
CLOSED: [2026-09-27 Sun 18:00]
:LOGBOOK:
CLOCK: [2026-09-27 Sun 17:00]--[2026-09-27 Sun 17:45] =>  0:45
:END:
Pas daté [2026-09-01 Tue].
";

    #[test]
    fn every_headline_is_an_entry_with_what_it_inherits() {
        let settings = FileSettings::scan(TEXT);
        let found = entries(TEXT, Path::new("/n/gtd.org"), &settings);
        let titles: Vec<&str> = found.iter().map(|entry| entry.title.as_str()).collect();
        assert_eq!(titles, ["Projets", "Rapport", "Fini"]);

        let report = &found[1];
        assert_eq!((report.line, report.end), (6, 16));
        assert_eq!(report.priority, Some('A'));
        assert_eq!(report.tags, ["urgent"]);
        assert_eq!(report.inherited_tags, ["notes", "work"]);
        assert_eq!(report.category, "pro");
        assert_eq!(report.outline_path, ["Projets"]);
        assert_eq!(report.deadline.map(|stamp| stamp.day), Some(1));
        assert_eq!(report.scheduled.map(|stamp| stamp.day), Some(29));
        assert_eq!(report.property("EFFORT"), Some("1:00"));
        assert!(report.id.is_some());
        // The one in the block is text.
        let days: Vec<u32> = report.timestamps.iter().map(|stamp| stamp.day).collect();
        assert_eq!(days, [30, 2]);
        assert!(report.is_open_task());

        assert!(report.clocks.is_empty());

        let done = &found[2];
        assert_eq!(done.category, "perso");
        assert_eq!(done.inherited_tags, ["notes"]);
        assert_eq!(done.closed.map(|stamp| stamp.day), Some(27));
        assert!(done.timestamps.is_empty());
        assert_eq!(done.clocks.len(), 1);
        assert_eq!(done.clocks[0].1.map(|end| end - done.clocks[0].0), Some(45));
        assert!(!done.is_open_task());
    }

    #[test]
    fn a_file_without_a_category_is_named_after_the_file() {
        let text = "* TODO Laver\n";
        let found = entries(text, Path::new("/n/inbox.org"), &FileSettings::default());
        assert_eq!(found[0].category, "inbox");
        assert_eq!(found[0].id, None);
    }
}
