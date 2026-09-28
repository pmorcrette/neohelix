//! What is due, and when.
//!
//! The agenda is a view over the index rather than a store of its own: it
//! takes the entries (every headline, node or not) the indexer already
//! holds and answers which of them land on which day. Keeping it a pure
//! function of entries and a date range is what lets the answer be tested
//! without an editor.

use crate::clock::Moment;
use crate::entry::Entry;
use crate::node::Timestamp;
use crate::Date;

/// Why an entry appears on a day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reason {
    /// Its `DEADLINE:` falls on or before the day.
    Deadline,
    /// Its `SCHEDULED:` falls on the day.
    Scheduled,
    /// An active timestamp in its text falls on the day: an appointment.
    Timestamp,
}

/// One line of an agenda.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgendaItem<'a> {
    pub day: Date,
    pub reason: Reason,
    pub entry: &'a Entry,
    /// The time of day, `(hour, minute)`, when the timestamp has one.
    pub time: Option<(u32, u32)>,
    /// Days until the deadline, negative once it is past.
    ///
    /// `None` for a scheduled entry, which is not late, only due.
    pub days_left: Option<i64>,
}

/// How far ahead a deadline starts showing up.
///
/// Org's default warning period is 14 days, and a deadline nobody sees until
/// the morning it is due is not a deadline.
pub const DEADLINE_WARNING_DAYS: i64 = 14;

fn time_of(stamp: &Timestamp) -> Option<(u32, u32)> {
    Some((stamp.hour?, stamp.minute.unwrap_or(0)))
}

/// The agenda for `days` days starting at `from`.
///
/// Items are ordered by day, then those with a time of day by their time,
/// then deadlines before scheduled items before appointments, then by title
/// — so the same set of notes always produces the same agenda.
pub fn agenda<'a>(
    entries: impl IntoIterator<Item = &'a Entry>,
    from: Date,
    days: i64,
) -> Vec<AgendaItem<'a>> {
    let to = from.offset_by(days.max(1) - 1);
    let mut items = Vec::new();

    for entry in entries {
        // A finished task is not pending, whatever its dates say.
        if entry.todo.as_ref().is_some_and(|state| state.done) {
            continue;
        }
        let item = |day, reason, stamp: &Timestamp, days_left| AgendaItem {
            day,
            reason,
            entry,
            time: time_of(stamp),
            days_left,
        };

        if let Some(scheduled) = entry.scheduled.filter(|stamp| stamp.active) {
            for day in scheduled.occurrences(from, to) {
                items.push(item(day, Reason::Scheduled, &scheduled, None));
            }
        }

        for stamp in &entry.timestamps {
            for day in stamp.occurrences(from, to) {
                items.push(item(day, Reason::Timestamp, stamp, None));
            }
        }

        if let Some(deadline) = entry.deadline.filter(|stamp| stamp.active) {
            // A deadline announces itself before it arrives, so the window
            // reaches past the view's last day by the warning period.
            let horizon = to.offset_by(DEADLINE_WARNING_DAYS);
            let upcoming = deadline.occurrences(from, horizon);

            // Nothing upcoming and a date already gone: it is overdue, and an
            // overdue deadline is carried onto today rather than vanishing.
            if upcoming.is_empty() && deadline.repeater.is_none() && deadline.day() < from {
                let late = deadline.day().to_days() - from.to_days();
                items.push(item(from, Reason::Deadline, &deadline, Some(late)));
            }

            for due in upcoming {
                // Inside the view it sits on its own day; beyond it, the
                // warning shows on the first day instead.
                let day = if due <= to { due } else { from };
                let left = due.to_days() - from.to_days();
                items.push(item(day, Reason::Deadline, &deadline, Some(left)));
            }
        }
    }

    items.sort_by(|a, b| {
        a.day
            .cmp(&b.day)
            // Timed items first, in time order; `None` sorts before `Some`.
            .then(a.time.is_none().cmp(&b.time.is_none()))
            .then(a.time.cmp(&b.time))
            .then(a.reason.cmp(&b.reason))
            .then(a.entry.title.cmp(&b.entry.title))
    });
    items.dedup_by(|a, b| {
        a.day == b.day
            && a.reason == b.reason
            && a.entry.file_path == b.entry.file_path
            && a.entry.line == b.entry.line
    });

    items
}

/// What log mode adds to a day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Logged {
    /// The entry was closed then.
    Closed,
    /// Time was clocked on it, from then until `end`.
    Clocked { end: Moment },
}

/// A line of log mode: something that happened on a day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogItem<'a> {
    pub day: Date,
    /// When it happened, `(hour, minute)`.
    pub time: (u32, u32),
    pub entry: &'a Entry,
    pub what: Logged,
}

fn day_of(moment: Moment) -> Date {
    Date::from_days(moment.div_euclid(1440))
}

fn time_of_moment(moment: Moment) -> (u32, u32) {
    let minutes = moment.rem_euclid(1440) as u32;
    (minutes / 60, minutes % 60)
}

/// What was closed and what was clocked over `days` days from `from`, as
/// Org's log mode adds to the agenda: finished work included, which the
/// agenda otherwise leaves out.
pub fn log<'a>(
    entries: impl IntoIterator<Item = &'a Entry>,
    from: Date,
    days: i64,
) -> Vec<LogItem<'a>> {
    let to = from.offset_by(days.max(1) - 1);
    let mut items = Vec::new();
    for entry in entries {
        if let Some(closed) = entry.closed {
            if closed.day() >= from && closed.day() <= to {
                items.push(LogItem {
                    day: closed.day(),
                    time: (closed.hour.unwrap_or(0), closed.minute.unwrap_or(0)),
                    entry,
                    what: Logged::Closed,
                });
            }
        }
        for &(start, end) in &entry.clocks {
            // A running clock has not been spent yet.
            let Some(end) = end else { continue };
            let day = day_of(start);
            if day >= from && day <= to {
                items.push(LogItem {
                    day,
                    time: time_of_moment(start),
                    entry,
                    what: Logged::Clocked { end },
                });
            }
        }
    }
    items.sort_by(|a, b| {
        (a.day, a.time)
            .cmp(&(b.day, b.time))
            .then(a.entry.title.cmp(&b.entry.title))
    });
    items
}

/// The time clocked on each entry between `from` and `to` (moments), a
/// running clock counting until `now`; only entries with some, in the
/// order of their files. Org's clock report of an agenda span.
pub fn clock_report<'a>(
    entries: impl IntoIterator<Item = &'a Entry>,
    from: Moment,
    to: Moment,
    now: Moment,
) -> Vec<(&'a Entry, i64)> {
    let mut report: Vec<(&Entry, i64)> = entries
        .into_iter()
        .filter_map(|entry| {
            let minutes: i64 = entry
                .clocks
                .iter()
                .map(|&(start, end)| {
                    let end = end.unwrap_or(now);
                    (end.min(to) - start.max(from)).max(0)
                })
                .sum();
            (minutes > 0).then_some((entry, minutes))
        })
        .collect();
    report.sort_by(|a, b| (&a.0.file_path, a.0.line).cmp(&(&b.0.file_path, b.0.line)));
    report
}

/// The diary lines of `entries` that fall within `days` days from `from`:
/// the day, the entry, and the text to show (the entry's title when the
/// line has none).
pub fn diary<'a>(
    entries: impl IntoIterator<Item = &'a Entry>,
    from: Date,
    days: i64,
) -> Vec<(Date, &'a Entry, String)> {
    let mut found = Vec::new();
    for entry in entries {
        for line in &entry.diary {
            for offset in 0..days.max(1) {
                let day = from.offset_by(offset);
                if let Some(text) = crate::diary::occurs(line, day) {
                    let text = if text.is_empty() {
                        entry.title.clone()
                    } else {
                        text
                    };
                    found.push((day, entry, text));
                }
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.cmp(&b.2)));
    found
}

/// The projects with nothing to do next: Org's stuck projects.
///
/// `entries` are one file's, in order. A project is an entry `is_project`
/// accepts; it is stuck unless something in its subtree has one of the
/// `todo` keywords or one of the `tags`.
pub fn stuck_projects<'a>(
    entries: &'a [Entry],
    is_project: impl Fn(&Entry) -> bool,
    todo: &[String],
    tags: &[String],
) -> Vec<&'a Entry> {
    entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| is_project(entry))
        .filter(|&(at, project)| {
            let moving = entries[at + 1..]
                .iter()
                .take_while(|below| below.level > project.level)
                .any(|below| {
                    below
                        .todo
                        .as_ref()
                        .is_some_and(|state| todo.contains(&state.keyword))
                        || below.tags.iter().any(|tag| tags.contains(tag))
                });
            !moving
        })
        .map(|(_, project)| project)
        .collect()
}

/// What to narrow a TODO list to.
///
/// Every field left unset is a filter not applied, so the default lists
/// everything unfinished.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TodoFilter {
    /// Only this TODO keyword, e.g. `WAITING`.
    pub keyword: Option<String>,
    /// Only entries carrying this tag, inherited or their own.
    pub tag: Option<String>,
    /// Only this priority letter.
    pub priority: Option<char>,
}

impl TodoFilter {
    /// Reads `WAITING`, `:work:` or `#A`, in any order and any number.
    ///
    /// A single prompt rather than three, because narrowing a list is one
    /// thought, and the sigils say which is which without asking.
    pub fn parse(input: &str) -> Self {
        let mut filter = Self::default();

        for token in input.split_whitespace() {
            if let Some(tag) = token.strip_prefix(':') {
                filter.tag = Some(tag.trim_matches(':').to_string());
            } else if let Some(letter) = token.strip_prefix('#') {
                filter.priority = letter.chars().next().map(|c| c.to_ascii_uppercase());
            } else {
                filter.keyword = Some(token.to_uppercase());
            }
        }

        filter
    }

    fn matches(&self, entry: &Entry) -> bool {
        self.keyword.as_ref().is_none_or(|wanted| {
            entry
                .todo
                .as_ref()
                .is_some_and(|state| &state.keyword == wanted)
        }) && self
            .tag
            .as_ref()
            .is_none_or(|wanted| entry.all_tags().any(|tag| tag == wanted))
            && self
                .priority
                .is_none_or(|wanted| entry.priority == Some(wanted))
    }
}

/// Every unfinished entry with a TODO state, whatever its dates.
///
/// This is the other half of an agenda: the things that have to happen but
/// were never given a day.
pub fn todo_list<'a>(entries: impl IntoIterator<Item = &'a Entry>) -> Vec<&'a Entry> {
    filtered_todo_list(entries, &TodoFilter::default())
}

/// The unfinished entries matching `filter`.
pub fn filtered_todo_list<'a>(
    entries: impl IntoIterator<Item = &'a Entry>,
    filter: &TodoFilter,
) -> Vec<&'a Entry> {
    let mut found: Vec<&Entry> = entries
        .into_iter()
        .filter(|entry| entry.is_open_task())
        .filter(|entry| filter.matches(entry))
        .collect();

    // Priority first, then title, so the list is stable and reads usefully.
    found.sort_by(|a, b| {
        a.priority
            .unwrap_or('Z')
            .cmp(&b.priority.unwrap_or('Z'))
            .then(a.title.cmp(&b.title))
    });
    found
}
