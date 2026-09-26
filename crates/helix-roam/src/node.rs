use std::path::{Path, PathBuf};

use uuid::Uuid;

/// A headline's TODO state.
///
/// Carries whether it means "done" because only the declaring file knows, and
/// the graph spans files.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TodoState {
    /// The keyword as written, e.g. `NEXT`.
    pub keyword: String,
    /// Whether the file puts this keyword after the `|`.
    pub done: bool,
}

/// How often a timestamp comes round, and what happens when one is missed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RepeaterKind {
    /// `+1w` — shift by one interval, however long ago it was due.
    Cumulate,
    /// `++1w` — shift by whole intervals until it is in the future.
    CatchUp,
    /// `.+1w` — shift from today rather than from the old date.
    Restart,
}

/// The unit a repeater counts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RepeaterUnit {
    Hour,
    Day,
    Week,
    Month,
    Year,
}

/// `+1w`, `++2m`, `.+3d`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Repeater {
    pub kind: RepeaterKind,
    pub count: i64,
    pub unit: RepeaterUnit,
}

/// An Org timestamp, reduced to the parts a query needs.
///
/// Field order makes the derived ordering chronological. Repeaters, warning
/// periods and ranges are not modelled here — a timestamp carrying one is
/// still read for its date, and the full model belongs to the agenda work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    /// `None` for a date without a time of day.
    pub hour: Option<u32>,
    pub minute: Option<u32>,
    /// `<…>` is active and appears in the agenda; `[…]` is not.
    pub active: bool,
    /// How often it comes round, when it repeats.
    ///
    /// Last so that the derived ordering still compares dates first.
    pub repeater: Option<Repeater>,
    /// The last day of a `<a>--<b>` range.
    ///
    /// Only the day is kept: a range spanning days is a calendar fact, and
    /// the time halves of `<… 10:00>--<… 12:00>` say nothing about which days
    /// it covers.
    pub range_end: Option<crate::Date>,
}

impl Timestamp {
    /// The date alone, for comparing days rather than instants.
    pub fn date(&self) -> (i32, u32, u32) {
        (self.year, self.month, self.day)
    }

    /// The calendar day this timestamp falls on.
    pub fn day(&self) -> crate::Date {
        crate::Date {
            year: self.year,
            month: self.month,
            day: self.day,
        }
    }

    /// The days this timestamp falls on within `from..=to`.
    ///
    /// A timestamp without a repeater occurs once, if at all. A repeating one
    /// is stepped forward from its own date, so a weekly task set up last year
    /// still lands on the right weekday.
    ///
    /// Hour repeaters are treated as daily here: the agenda is a calendar of
    /// days, and something recurring within one day belongs to that day once.
    pub fn occurrences(&self, from: crate::Date, to: crate::Date) -> Vec<crate::Date> {
        let start = self.day();

        // A range covers every day between its ends, so it is listed on each
        // one rather than only on the day it opens.
        if let Some(end) = self.range_end.filter(|end| *end >= start) {
            let mut days = Vec::new();
            let mut at = start.max(from);
            while at <= end && at <= to {
                days.push(at);
                at = at.offset_by(1);
            }
            return days;
        }

        let Some(repeater) = self.repeater.filter(|r| r.count > 0) else {
            return (start >= from && start <= to)
                .then_some(start)
                .into_iter()
                .collect();
        };

        let mut days = Vec::new();
        let mut at = start;

        // Skip forward in whole intervals rather than day by day.
        while at < from {
            let next = repeater.advance(at);
            // A repeater that cannot move would loop forever.
            if next <= at {
                return days;
            }
            at = next;
        }

        while at <= to {
            days.push(at);
            let next = repeater.advance(at);
            if next <= at {
                break;
            }
            at = next;
        }

        days
    }
}

impl Repeater {
    /// The date one interval after `date`.
    ///
    /// Month and year steps clamp to the end of the target month, so the 31st
    /// of January plus a month is the 28th or 29th of February rather than a
    /// date that does not exist.
    pub fn advance(self, date: crate::Date) -> crate::Date {
        match self.unit {
            // A sub-day repeater still lands on the next day for a calendar.
            RepeaterUnit::Hour => date.offset_by(1),
            RepeaterUnit::Day => date.offset_by(self.count),
            RepeaterUnit::Week => date.offset_by(self.count * 7),
            RepeaterUnit::Month => add_months(date, self.count),
            RepeaterUnit::Year => add_months(date, self.count * 12),
        }
    }
}

/// Adds whole months, clamping the day to the target month's length.
fn add_months(date: crate::Date, months: i64) -> crate::Date {
    let total = date.year as i64 * 12 + (date.month as i64 - 1) + months;
    let year = total.div_euclid(12) as i32;
    let month = total.rem_euclid(12) as u32 + 1;

    crate::Date {
        year,
        month,
        day: date.day.min(days_in_month(year, month)),
    }
}

/// How many days a month has, leap years included.
fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 30,
    }
}

/// A single Org-Roam node.
///
/// In Org-Roam v2 a node is any headline — or the file-level preamble — that
/// carries an `:ID:` property. `title` is the headline text (or the
/// `#+title:` keyword for a file-level node), `tags` are the `:tag:` markers
/// attached to it, and `aliases` come from the `:ROAM_ALIASES:` property.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// The value of the node's `:ID:` property.
    pub id: Uuid,
    /// The headline text, or the `#+title:` keyword for a file-level node.
    pub title: String,
    /// Path of the Org file the node was parsed from.
    pub file_path: PathBuf,
    /// Tags attached to the node, without their surrounding colons.
    pub tags: Vec<String>,
    /// Alternative titles declared through `:ROAM_ALIASES:`.
    pub aliases: Vec<String>,
    /// Outline depth: 1 for a top-level headline, 0 for the file-level node.
    pub level: usize,
    /// The node's TODO state, when its headline carries one.
    pub todo: Option<TodoState>,
    /// The priority letter, when the headline carries a cookie the file
    /// declares.
    pub priority: Option<char>,
    /// `SCHEDULED:` from the planning line under the headline.
    pub scheduled: Option<Timestamp>,
    /// `DEADLINE:` from the same line.
    pub deadline: Option<Timestamp>,
    /// Titles of the headlines above this one, outermost first.
    ///
    /// Complete regardless of which ancestors are themselves nodes: a
    /// headline without an `:ID:` still names a level of the outline.
    pub outline_path: Vec<String>,
    /// Properties of the node's drawer, keys lowercased.
    ///
    /// Excludes the ones with fields of their own — `:ID:`,
    /// `:ROAM_ALIASES:` and `:ROAM_REFS:` — so nothing is stored twice.
    pub properties: Vec<(String, String)>,
    /// Properties this node takes from an enclosing entry, or from the file's
    /// `#+PROPERTY:` defaults.
    ///
    /// Kept apart from [`Node::properties`] rather than merged: a query that
    /// wants either can look at both, and one that wants only what the entry
    /// itself declares still can.
    pub inherited_properties: Vec<(String, String)>,
    /// Zero-based line of the node's `:ID:` property within `file_path`.
    ///
    /// This is what the node picker jumps to, so it points at the `:ID:`
    /// itself rather than at the headline above it.
    pub line: usize,
}

impl Node {
    /// Creates a node with no tags and no aliases.
    pub fn new(id: Uuid, title: impl Into<String>, file_path: impl Into<PathBuf>) -> Self {
        Self {
            id,
            title: title.into(),
            file_path: file_path.into(),
            tags: Vec::new(),
            aliases: Vec::new(),
            level: 0,
            todo: None,
            priority: None,
            scheduled: None,
            deadline: None,
            outline_path: Vec::new(),
            properties: Vec::new(),
            inherited_properties: Vec::new(),
            line: 0,
        }
    }

    /// Builder-style setter for [`Node::tags`].
    pub fn with_tags<T: Into<String>>(mut self, tags: impl IntoIterator<Item = T>) -> Self {
        self.tags = tags.into_iter().map(Into::into).collect();
        self
    }

    /// Builder-style setter for [`Node::aliases`].
    pub fn with_aliases<T: Into<String>>(mut self, aliases: impl IntoIterator<Item = T>) -> Self {
        self.aliases = aliases.into_iter().map(Into::into).collect();
        self
    }

    /// Builder-style setter for [`Node::line`].
    pub fn with_line(mut self, line: usize) -> Self {
        self.line = line;
        self
    }

    /// The path of the Org file this node was parsed from.
    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    /// Whether `title` matches the node's title or any of its aliases.
    ///
    /// Org-Roam resolves `[[roam:...]]` descriptions against both, so lookups
    /// by name have to consider aliases as well.
    /// The value of `key`, from this node or from what it inherits.
    pub fn property(&self, key: &str) -> Option<&str> {
        let key = key.to_lowercase();
        self.properties
            .iter()
            .chain(&self.inherited_properties)
            .find(|(found, _)| *found == key)
            .map(|(_, value)| value.as_str())
    }

    pub fn matches_title(&self, title: &str) -> bool {
        self.title == title || self.aliases.iter().any(|alias| alias == title)
    }
}
