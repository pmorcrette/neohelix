//! Clocking: recording the time spent on an entry, and reporting it.
//!
//! A clock is a line in the entry's `:LOGBOOK:`:
//!
//! ```org
//! :LOGBOOK:
//! CLOCK: [2026-09-24 Thu 09:00]--[2026-09-24 Thu 10:30] =>  1:30
//! CLOCK: [2026-09-24 Thu 14:00]
//! :END:
//! ```
//!
//! The second is running. The file is the only record: whether a clock is
//! running is read from the text rather than remembered, so a clock started
//! before a restart, or in another editor, is still a clock.

use crate::date::{log_stamp, Date, Time};
use crate::dynamic::DynamicBlock;
use crate::parser::{parse_headline, FileSettings};
use crate::restructure::{self, headline_level, rejoin};
use crate::table::{Row, Table};

/// Minutes since the Unix epoch: the unit every clock is compared and
/// summed in.
pub type Moment = i64;

/// A moment from a date and a time of day.
pub fn moment(date: Date, time: Time) -> Moment {
    date.to_days() * 1440 + (time.hour * 60 + time.minute) as Moment
}

/// A `CLOCK:` line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clock {
    pub line: usize,
    pub start: Moment,
    /// `None` while the clock is running.
    pub end: Option<Moment>,
}

impl Clock {
    /// How long it ran, or has run so far.
    pub fn minutes(&self, now: Moment) -> i64 {
        (self.end.unwrap_or(now) - self.start).max(0)
    }
}

/// Reads `[2026-09-24 Thu 09:00]`.
fn parse_stamp(stamp: &str) -> Option<Moment> {
    let inner = stamp.trim().strip_prefix('[')?.strip_suffix(']')?;
    let mut parts = inner.split_whitespace();
    let date = Date::parse_iso(parts.next()?)?;
    let clock = parts.find(|part| part.contains(':'))?;
    let (hour, minute) = clock.split_once(':')?;
    let time = Time {
        hour: hour.parse().ok()?,
        minute: minute.parse().ok()?,
    };
    (time.hour < 24 && time.minute < 60).then(|| moment(date, time))
}

/// Reads a `CLOCK:` line, running or closed.
pub fn parse_clock(line: &str) -> Option<(Moment, Option<Moment>)> {
    let rest = line.trim_start().strip_prefix("CLOCK:")?.trim();
    let first_end = rest.find(']')?;
    let start = parse_stamp(&rest[..=first_end])?;

    let after = rest[first_end + 1..].trim_start();
    let Some(second) = after.strip_prefix("--") else {
        return Some((start, None));
    };
    let second_end = second.find(']')?;
    let end = parse_stamp(&second[..=second_end])?;
    Some((start, Some(end)))
}

/// Every clock in `text`.
pub fn clocks(text: &str) -> Vec<Clock> {
    text.lines()
        .enumerate()
        .filter_map(|(line, raw)| {
            let (start, end) = parse_clock(raw)?;
            Some(Clock { line, start, end })
        })
        .collect()
}

/// The running clock in `text`, if there is one.
pub fn running(text: &str) -> Option<Clock> {
    clocks(text).into_iter().find(|clock| clock.end.is_none())
}

/// `1:30`, as Org writes a duration: hours unpadded, minutes on two digits.
pub fn format_duration(minutes: i64) -> String {
    format!("{}:{:02}", minutes / 60, minutes % 60)
}

/// The stamp a clock line writes, `[2026-09-24 Thu 09:00]`.
fn stamp_at(at: Moment) -> String {
    let date = Date::from_days(at.div_euclid(1440));
    let minutes = at.rem_euclid(1440);
    log_stamp(
        date,
        Time {
            hour: (minutes / 60) as u32,
            minute: (minutes % 60) as u32,
        },
    )
}

/// Starts a clock on the entry at `line`.
///
/// Refuses while another clock in the same text is running: Org has one
/// clock at a time, and clocking in elsewhere is the caller's decision to
/// clock that one out first, not something to do silently here.
pub fn clock_in(text: &str, line: usize, now: Moment) -> Result<String, ClockError> {
    if let Some(clock) = running(text) {
        return Err(ClockError::AlreadyRunning(clock.line));
    }
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let at = line.min(lines.len().saturating_sub(1));
    if lines.is_empty() || !lines[..=at].iter().any(|l| headline_level(l).is_some()) {
        return Err(ClockError::NoEntry);
    }

    Ok(restructure::log_entry(
        text,
        line,
        &format!("CLOCK: {}", stamp_at(now)),
    ))
}

/// Stops the running clock, returning the text and how long it ran.
pub fn clock_out(text: &str, now: Moment) -> Result<(String, i64), ClockError> {
    let clock = running(text).ok_or(ClockError::NotRunning)?;
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();

    let minutes = (now - clock.start).max(0);
    let indent: String = lines[clock.line]
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();
    // `=> %2d:%02d`: the hours are padded to two columns.
    lines[clock.line] = format!(
        "{indent}CLOCK: {}--{} => {:>2}:{:02}",
        stamp_at(clock.start),
        stamp_at(now),
        minutes / 60,
        minutes % 60
    );

    Ok((rejoin(&lines, text), minutes))
}

/// Throws the running clock away, and its `:LOGBOOK:` if that leaves it
/// empty — a clock started by mistake should leave no trace.
pub fn clock_cancel(text: &str) -> Result<String, ClockError> {
    let clock = running(text).ok_or(ClockError::NotRunning)?;
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    lines.remove(clock.line);

    let above = clock.line.checked_sub(1);
    let empty_drawer = above.is_some_and(|above| {
        lines[above].trim().eq_ignore_ascii_case(":LOGBOOK:")
            && lines
                .get(clock.line)
                .is_some_and(|below| below.trim().eq_ignore_ascii_case(":END:"))
    });
    if let (true, Some(above)) = (empty_drawer, above) {
        lines.drain(above..=clock.line);
    }

    Ok(rejoin(&lines, text))
}

/// The headline line of the entry a clock belongs to.
pub fn entry_of(text: &str, clock: &Clock) -> Option<usize> {
    let lines: Vec<&str> = text.lines().collect();
    (0..=clock.line)
        .rev()
        .find(|&at| headline_level(lines[at]).is_some())
}

/// Why a clock command did nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClockError {
    /// A clock is already running, on this line.
    AlreadyRunning(usize),
    NotRunning,
    /// There is no headline to clock into.
    NoEntry,
}

impl std::fmt::Display for ClockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClockError::AlreadyRunning(line) => {
                write!(f, "a clock is already running, on line {}", line + 1)
            }
            ClockError::NotRunning => write!(f, "no clock is running"),
            ClockError::NoEntry => write!(f, "no headline to clock into"),
        }
    }
}

// ── Reports ────────────────────────────────────────────────────────────────

/// Time clocked on one headline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryTime {
    pub line: usize,
    pub level: usize,
    pub title: String,
    /// On this entry's own clocks.
    pub own: i64,
    /// On this entry and everything below it.
    pub total: i64,
}

/// Minutes of `[start, end)` inside `[from, to)`.
fn overlap(start: Moment, end: Moment, from: Option<Moment>, to: Option<Moment>) -> i64 {
    let start = from.map_or(start, |from| start.max(from));
    let end = to.map_or(end, |to| end.min(to));
    (end - start).max(0)
}

/// Clocked time per headline, counting closed clocks only, clipped to
/// `[from, to)` when given — so a clock running past midnight counts half
/// in each day, which is what a report for "today" needs.
pub fn entry_times(text: &str, from: Option<Moment>, to: Option<Moment>) -> Vec<EntryTime> {
    let settings = FileSettings::scan(text);
    let lines: Vec<&str> = text.lines().collect();

    let mut entries: Vec<EntryTime> = lines
        .iter()
        .enumerate()
        .filter_map(|(line, raw)| {
            let headline = parse_headline(raw, &settings)?;
            Some(EntryTime {
                line,
                level: headline.level,
                title: headline.title,
                own: 0,
                total: 0,
            })
        })
        .collect();

    for clock in clocks(text) {
        let Some(end) = clock.end else {
            continue;
        };
        let minutes = overlap(clock.start, end, from, to);
        if let Some(entry) = entries
            .iter_mut()
            .rev()
            .find(|entry| entry.line < clock.line)
        {
            entry.own += minutes;
        }
    }

    // A subtree's total is its own time plus its descendants'.
    for index in 0..entries.len() {
        let level = entries[index].level;
        let below: i64 = entries[index + 1..]
            .iter()
            .take_while(|entry| entry.level > level)
            .map(|entry| entry.own)
            .sum();
        entries[index].total = entries[index].own + below;
    }

    entries
}

/// The span a `:block` parameter names, relative to `today`.
///
/// Weeks start on Monday, as they do in Org.
pub fn block_span(block: &str, today: Date) -> Option<(Moment, Moment)> {
    let day = |date: Date| date.to_days() * 1440;
    // 1970-01-01 was a Thursday: Monday is 3 days before an epoch-aligned week.
    let monday = today.offset_by(-((today.to_days() + 3).rem_euclid(7)));
    let first_of_month = Date { day: 1, ..today };
    let next_month = if today.month == 12 {
        Date {
            year: today.year + 1,
            month: 1,
            day: 1,
        }
    } else {
        Date {
            month: today.month + 1,
            day: 1,
            ..today
        }
    };

    Some(match block {
        "today" => (day(today), day(today.offset_by(1))),
        "yesterday" => (day(today.offset_by(-1)), day(today)),
        "thisweek" => (day(monday), day(monday.offset_by(7))),
        "lastweek" => (day(monday.offset_by(-7)), day(monday)),
        "thismonth" => (day(first_of_month), day(next_month)),
        _ => return None,
    })
}

/// Org's `clocktable` dynamic block.
///
/// Reads `:maxlevel` (default 2), `:scope` (`file`, the default, or
/// `subtree`), `:block` (`today`, `yesterday`, `thisweek`, `lastweek`,
/// `thismonth`) and `:tstart`/`:tend` as ISO dates. Each deeper level gets
/// a column of its own, the way Org lays the table out, so a subtree's total
/// and its children's times do not sit in the same column.
pub fn clocktable(text: &str, block: &DynamicBlock, today: Date, now: Moment) -> Vec<String> {
    let max_level = block.number("maxlevel").unwrap_or(2).max(1);

    let (mut from, mut to) = match block
        .param("block")
        .and_then(|b| block_span(b.trim(), today))
    {
        Some((from, to)) => (Some(from), Some(to)),
        None => (None, None),
    };
    let iso = |key: &str| {
        block
            .param(key)
            .map(|value| value.trim().trim_matches(['"', '<', '>', '[', ']']))
            .and_then(|value| value.split_whitespace().next())
            .and_then(Date::parse_iso)
            .map(|date| date.to_days() * 1440)
    };
    from = iso("tstart").or(from);
    to = iso("tend").or(to);

    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let scope = match block.param("scope").map(str::trim) {
        Some("subtree") => restructure::subtree_range(&lines, block.start),
        _ => None,
    };

    let entries: Vec<EntryTime> = entry_times(text, from, to)
        .into_iter()
        .filter(|entry| scope.is_none_or(|(start, end, _)| (start..end).contains(&entry.line)))
        .collect();
    let base = scope.map_or(1, |(_, _, level)| level);

    let shown: Vec<&EntryTime> = entries
        .iter()
        .filter(|entry| entry.total > 0 && entry.level - base < max_level)
        .collect();
    let total: i64 = entries
        .iter()
        .filter(|entry| entry.level == base)
        .map(|entry| entry.total)
        .sum();
    let depth = shown
        .iter()
        .map(|entry| entry.level - base + 1)
        .max()
        .unwrap_or(1);

    let cells = |first: String, level: usize, time: String| {
        let mut row = vec![first, String::new()];
        row.extend((1..depth).map(|_| String::new()));
        row[level] = time;
        Row::Cells(row)
    };

    let mut header = vec!["Headline".to_string(), "Time".to_string()];
    header.extend((1..depth).map(|_| String::new()));
    let mut rows = vec![
        Row::Cells(header),
        Row::Separator,
        cells(
            "*Total time*".to_string(),
            1,
            format!("*{}*", format_duration(total)),
        ),
        Row::Separator,
    ];
    for entry in shown {
        let level = entry.level - base + 1;
        // `\_` and two spaces per level below the first, as Org indents.
        let indent = if level == 1 {
            String::new()
        } else {
            format!("\\_{}", " ".repeat(2 * (level - 1)))
        };
        rows.push(cells(
            format!("{indent}{}", entry.title),
            level,
            format_duration(entry.total),
        ));
    }

    let mut out = vec![format!("#+CAPTION: Clock summary at {}", stamp_at(now))];
    out.extend(
        Table {
            rows,
            start: 0,
            end: 0,
            indent: 0,
        }
        .render(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(date: &str, hour: u32, minute: u32) -> Moment {
        moment(Date::parse_iso(date).unwrap(), Time { hour, minute })
    }

    #[test]
    fn clock_lines_are_read_running_or_closed() {
        assert_eq!(
            parse_clock("CLOCK: [2026-09-24 Thu 09:00]"),
            Some((at("2026-09-24", 9, 0), None))
        );
        assert_eq!(
            parse_clock("  CLOCK: [2026-09-24 Thu 09:00]--[2026-09-24 Thu 10:30] =>  1:30"),
            Some((at("2026-09-24", 9, 0), Some(at("2026-09-24", 10, 30))))
        );
        assert_eq!(parse_clock("CLOCK: nonsense"), None);
    }

    #[test]
    fn clocking_in_and_out_writes_the_logbook_as_org_does() {
        let text = "* Task\nSCHEDULED: <2026-09-24 Thu>\nBody.\n";
        let running = clock_in(text, 0, at("2026-09-24", 9, 0)).unwrap();
        assert_eq!(
            running,
            "* Task\nSCHEDULED: <2026-09-24 Thu>\n:LOGBOOK:\nCLOCK: [2026-09-24 Thu 09:00]\n:END:\nBody.\n"
        );

        let (stopped, minutes) = clock_out(&running, at("2026-09-24", 10, 30)).unwrap();
        assert_eq!(minutes, 90);
        assert_eq!(
            stopped,
            "* Task\nSCHEDULED: <2026-09-24 Thu>\n:LOGBOOK:\n\
             CLOCK: [2026-09-24 Thu 09:00]--[2026-09-24 Thu 10:30] =>  1:30\n:END:\nBody.\n"
        );

        // The next clock goes on top.
        let again = clock_in(&stopped, 0, at("2026-09-24", 14, 0)).unwrap();
        assert!(
            again.contains(
                ":LOGBOOK:\nCLOCK: [2026-09-24 Thu 14:00]\nCLOCK: [2026-09-24 Thu 09:00]--"
            ),
            "{again}"
        );
    }

    #[test]
    fn a_clock_spanning_midnight_is_measured_across_it() {
        let text = "* Task\n:LOGBOOK:\nCLOCK: [2026-09-24 Thu 23:30]\n:END:\n";
        let (_, minutes) = clock_out(text, at("2026-09-25", 1, 15)).unwrap();
        assert_eq!(minutes, 105);
    }

    #[test]
    fn two_clocks_cannot_run_at_once() {
        let text = "* A\n:LOGBOOK:\nCLOCK: [2026-09-24 Thu 09:00]\n:END:\n* B\n";
        assert_eq!(
            clock_in(text, 4, at("2026-09-24", 10, 0)),
            Err(ClockError::AlreadyRunning(2))
        );
    }

    #[test]
    fn cancelling_leaves_no_trace() {
        let text = "* Task\nBody.\n";
        let running = clock_in(text, 0, at("2026-09-24", 9, 0)).unwrap();
        assert_eq!(clock_cancel(&running).unwrap(), text);

        let kept = "* Task\n:LOGBOOK:\nCLOCK: [2026-09-24 Thu 11:00]\n- A note\n:END:\n";
        assert_eq!(
            clock_cancel(kept).unwrap(),
            "* Task\n:LOGBOOK:\n- A note\n:END:\n"
        );
        assert_eq!(clock_cancel(text), Err(ClockError::NotRunning));
    }

    const LOGGED: &str = "\
* Project
:LOGBOOK:
CLOCK: [2026-09-24 Thu 08:00]--[2026-09-24 Thu 08:30] =>  0:30
:END:
** Task A
:LOGBOOK:
CLOCK: [2026-09-24 Thu 09:00]--[2026-09-24 Thu 10:30] =>  1:30
CLOCK: [2026-09-23 Wed 23:00]--[2026-09-24 Thu 00:30] =>  1:30
:END:
** Task B
*** Deep
:LOGBOOK:
CLOCK: [2026-09-22 Tue 10:00]--[2026-09-22 Tue 11:00] =>  1:00
:END:
* Idle
";

    #[test]
    fn subtrees_add_up_their_children() {
        let times = entry_times(LOGGED, None, None);
        let total = |title: &str| times.iter().find(|e| e.title == title).unwrap().total;
        assert_eq!(total("Project"), 30 + 180 + 60);
        assert_eq!(total("Task A"), 180);
        assert_eq!(total("Task B"), 60);
        assert_eq!(total("Idle"), 0);
    }

    #[test]
    fn a_span_clips_clocks_that_cross_it() {
        let today = Date::parse_iso("2026-09-24").unwrap();
        let (from, to) = block_span("today", today).unwrap();
        let times = entry_times(LOGGED, Some(from), Some(to));
        let a = times.iter().find(|e| e.title == "Task A").unwrap();
        // 1:30 in the morning, and the half hour after midnight.
        assert_eq!(a.total, 120);
    }

    #[test]
    fn weeks_start_on_monday() {
        let thursday = Date::parse_iso("2026-09-24").unwrap();
        let (from, to) = block_span("thisweek", thursday).unwrap();
        assert_eq!(Date::from_days(from / 1440).to_iso(), "2026-09-21");
        assert_eq!(Date::from_days(to / 1440).to_iso(), "2026-09-28");
    }

    #[test]
    fn the_clocktable_lays_levels_out_in_columns() {
        let text = format!("#+BEGIN: clocktable :maxlevel 2\n#+END:\n{LOGGED}");
        let block = crate::dynamic::blocks(&text).remove(0);
        let table = clocktable(
            &text,
            &block,
            Date::parse_iso("2026-09-24").unwrap(),
            at("2026-09-24", 12, 0),
        );
        assert_eq!(
            table,
            [
                "#+CAPTION: Clock summary at [2026-09-24 Thu 12:00]",
                "| Headline     | Time   |      |",
                "|--------------+--------+------|",
                "| *Total time* | *4:30* |      |",
                "|--------------+--------+------|",
                "| Project      | 4:30   |      |",
                "| \\_  Task A   |        | 3:00 |",
                "| \\_  Task B   |        | 1:00 |",
            ]
        );
    }

    #[test]
    fn a_clocktable_for_today_counts_only_today() {
        let text = format!("#+BEGIN: clocktable :block today\n#+END:\n{LOGGED}");
        let block = crate::dynamic::blocks(&text).remove(0);
        let table = clocktable(
            &text,
            &block,
            Date::parse_iso("2026-09-24").unwrap(),
            at("2026-09-24", 12, 0),
        );
        assert_eq!(table[3], "| *Total time* | *2:30* |      |");
    }
}
