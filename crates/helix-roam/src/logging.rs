//! What changing an entry records, and what marking a repeating entry done
//! actually does.
//!
//! Until this module, cycling a TODO state rewrote one word and nothing else:
//! no `CLOSED:` stamp, no log line, and — the part a user would notice first —
//! a weekly task marked done simply stayed done. In Org, a task with a
//! repeater goes back to its TODO state with its date moved on; that is what
//! the repeater is for.
//!
//! How much gets recorded is the file's choice, through `#+STARTUP:` (see
//! [`crate::startup`]) and the `!` and `@` marks on its `#+TODO:` keywords.
//! The log lines use Org's own headings, so a file keeps reading the same in
//! both editors:
//!
//! ```text
//! - State "DONE"       from "TODO"       [2026-09-23 Wed 10:00]
//! - CLOSING NOTE [2026-09-23 Wed 10:00] \\
//!   the note
//! - Rescheduled from "[2026-09-20 Sun]" on [2026-09-23 Wed 10:00]
//! ```
//!
//! Where a change asks for a note, this module cannot ask for one: it returns
//! a [`PendingNote`] and the editor prompts, then calls [`write_note`].

use crate::date::{log_stamp, Date, Time};
use crate::parser::{parse_repeater, FileSettings};
use crate::restructure::{self, Error, Planning};
use crate::startup::Startup;
use crate::{Repeater, RepeaterKind, RepeaterUnit};

/// What a change records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Log {
    /// A timestamped line.
    Time,
    /// A timestamped line and a note the user writes.
    Note,
}

/// What entering and leaving one TODO keyword records.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeywordLog {
    pub enter: Option<Log>,
    pub leave: Option<Log>,
}

impl KeywordLog {
    /// Reads what follows the `(` of `WAIT(w@/!)`: a fast-access key, then
    /// what entering records, then after a `/` what leaving records.
    pub fn parse(spec: &str) -> Self {
        let marks = spec.trim_start_matches(|c| !matches!(c, '!' | '@' | '/'));
        let (enter, leave) = marks.split_once('/').unwrap_or((marks, ""));

        let read = |marks: &str| {
            if marks.contains('@') {
                Some(Log::Note)
            } else if marks.contains('!') {
                Some(Log::Time)
            } else {
                None
            }
        };
        Self {
            enter: read(enter),
            leave: read(leave),
        }
    }
}

/// A log line waiting for the note the user has not written yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingNote {
    /// The headline the note belongs to.
    pub line: usize,
    /// The first line of the entry, `State "DONE" from "TODO" [...]`.
    pub heading: String,
    /// Whether it goes into `:LOGBOOK:`.
    pub into_drawer: bool,
}

/// What a state change did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateChange {
    pub text: String,
    /// The state the entry is left in, which for a repeating entry marked
    /// done is not the state asked for.
    pub state: Option<String>,
    /// Whether a repeater moved the entry's dates on.
    pub repeated: bool,
    pub note: Option<PendingNote>,
}

/// Moves the headline at `line` to `to`, recording what the file asks for.
///
/// The order follows Org's `org-todo`: leaving a done state drops the
/// `CLOSED:` stamp; entering one adds it when `logdone` is set; a keyword's
/// own `!` or `@` adds a state line; and a repeater, if the entry has one,
/// turns "done" into "done this time" — dates move on, the state goes back,
/// and the completion is logged instead of closed.
pub fn change_state(
    text: &str,
    line: usize,
    settings: &FileSettings,
    startup: &Startup,
    to: Option<&str>,
    now: (Date, Time),
) -> Result<StateChange, Error> {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (start, end, _) = restructure::subtree_range(&lines, line).ok_or(Error::NoSubtree)?;
    let from = current_state(&lines[start], settings);

    let is_done =
        |state: Option<&str>| state.is_some_and(|s| settings.done_keywords.iter().any(|d| d == s));
    let is_active =
        |state: Option<&str>| state.is_some_and(|s| settings.todo_keywords.iter().any(|t| t == s));
    let now_done = is_done(to) && !is_done(from.as_deref());

    // A keyword's own marks: entering the new state, else leaving the old.
    let keyword_log = to
        .and_then(|to| settings.keyword_log(to).enter)
        .or_else(|| {
            from.as_deref()
                .and_then(|from| settings.keyword_log(from).leave)
        });
    let stamp = log_stamp(now.0, now.1);

    let mut text = restructure::set_todo(text, start, settings, to)?;

    if now_done {
        let entry_end = next_headline(&lines, start).min(end);
        if let Some(repeated) = repeat(&text, start, entry_end, now) {
            text = repeated;
            let reset = restructure::property_value(&text, start, "REPEAT_TO_STATE")
                .filter(|state| is_active(Some(state)))
                .or_else(|| settings.todo_keywords.first().cloned());
            text = restructure::set_todo(&text, start, settings, reset.as_deref())?;

            let mut note = None;
            let log = match (startup.log_repeat, keyword_log) {
                (Some(Log::Note), _) | (_, Some(Log::Note)) => Some(Log::Note),
                (None, None) => None,
                _ => Some(Log::Time),
            };
            if startup.log_repeat.is_some() {
                text = restructure::put_property(&text, start, "LAST_REPEAT", &stamp);
            }
            if let Some(log) = log {
                let heading = state_heading(to, from.as_deref(), &stamp);
                (text, note) = record(text, start, heading, log, startup.log_into_drawer);
            }

            return Ok(StateChange {
                text,
                state: reset,
                repeated: true,
                note,
            });
        }
    }

    // Leaving done, or dropping the state altogether, un-closes the entry.
    let reopened = to.is_none() || (is_active(to) && !is_active(from.as_deref()));
    if reopened && restructure::planning_value(&text, start, Planning::Closed).is_some() {
        text = restructure::set_planning(&text, start, Planning::Closed, None)?;
    }

    let mut note = None;
    if now_done {
        if let Some(log_done) = startup.log_done {
            text = restructure::set_planning(&text, start, Planning::Closed, Some(&stamp))?;
            if log_done == Log::Note && keyword_log.is_none() {
                note = Some(PendingNote {
                    line: start,
                    heading: format!("CLOSING NOTE {stamp}"),
                    into_drawer: startup.log_into_drawer,
                });
            }
        }
    }
    if let (Some(log), Some(_)) = (keyword_log, to) {
        let heading = state_heading(to, from.as_deref(), &stamp);
        let (logged, pending) = record(text, start, heading, log, startup.log_into_drawer);
        text = logged;
        note = pending.or(note);
    }

    Ok(StateChange {
        text,
        state: to.map(str::to_string),
        repeated: false,
        note,
    })
}

/// A changed planning stamp, and the note it is waiting for, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replanned {
    pub text: String,
    pub logged: bool,
    pub note: Option<PendingNote>,
}

/// Sets or clears `SCHEDULED:` or `DEADLINE:`, logging the change when the
/// file asks for `logreschedule` or `logredeadline`.
///
/// Only a change to a date that was already there is logged: setting a first
/// date is not a reschedule.
pub fn replan(
    text: &str,
    line: usize,
    which: Planning,
    stamp: Option<&str>,
    startup: &Startup,
    now: (Date, Time),
) -> Result<Replanned, Error> {
    let old = restructure::planning_value(text, line, which);
    let changed = restructure::set_planning(text, line, which, stamp)?;

    let log = match which {
        Planning::Scheduled => startup.log_reschedule,
        Planning::Deadline => startup.log_redeadline,
        Planning::Closed => None,
    };
    let (Some(log), Some(old)) = (log, old) else {
        return Ok(Replanned {
            text: changed,
            logged: false,
            note: None,
        });
    };
    if stamp == Some(old.as_str()) {
        return Ok(Replanned {
            text: changed,
            logged: false,
            note: None,
        });
    }

    let lines: Vec<String> = changed.lines().map(str::to_string).collect();
    let (start, _, _) = restructure::subtree_range(&lines, line).ok_or(Error::NoSubtree)?;
    let was = quoted_stamp(&old);
    let when = log_stamp(now.0, now.1);
    let heading = match (which, stamp) {
        (Planning::Deadline, Some(_)) => format!("New deadline from {was} on {when}"),
        (Planning::Deadline, None) => format!("Removed deadline, was {was} on {when}"),
        (_, Some(_)) => format!("Rescheduled from {was} on {when}"),
        (_, None) => format!("Not scheduled, was {was} on {when}"),
    };
    let (text, note) = record(changed, start, heading, log, startup.log_into_drawer);

    Ok(Replanned {
        text,
        logged: true,
        note,
    })
}

/// Writes a note under its heading, or the heading alone when the note is
/// empty — which is how Org stores a note the user finished without typing.
pub fn write_note(text: &str, pending: &PendingNote, note: &str) -> String {
    let note = note.trim();
    let entry = if note.is_empty() {
        format!("- {}", pending.heading)
    } else {
        format!("- {} \\\\\n  {note}", pending.heading)
    };
    restructure::log_entry_into(text, pending.line, &entry, pending.into_drawer)
}

/// `State "DONE"       from "TODO"       [stamp]`, padded as Org pads it.
///
/// Org's heading is `State %-12s from %-12S %t`: both states are quoted and
/// then padded to twelve columns, which is what lines a logbook up.
pub fn state_heading(to: Option<&str>, from: Option<&str>, stamp: &str) -> String {
    let quote = |state: Option<&str>| state.map_or_else(String::new, |s| format!("\"{s}\""));
    format!("State {:<12} from {:<12} {stamp}", quote(to), quote(from))
}

/// Writes a time entry now, or hands back the heading to write with a note.
fn record(
    text: String,
    line: usize,
    heading: String,
    log: Log,
    into_drawer: bool,
) -> (String, Option<PendingNote>) {
    match log {
        Log::Time => (
            restructure::log_entry_into(&text, line, &format!("- {heading}"), into_drawer),
            None,
        ),
        Log::Note => (
            text,
            Some(PendingNote {
                line,
                heading,
                into_drawer,
            }),
        ),
    }
}

/// An old planning stamp as a log line quotes it: made inactive, so that
/// the log line does not put the entry back on the agenda for its old date.
fn quoted_stamp(stamp: &str) -> String {
    let inner = stamp
        .strip_prefix(['<', '['])
        .and_then(|rest| rest.strip_suffix(['>', ']']))
        .unwrap_or(stamp);
    format!("\"[{inner}]\"")
}

/// The TODO keyword on a headline line, if it carries one.
fn current_state(headline: &str, settings: &FileSettings) -> Option<String> {
    let rest = headline.trim_start_matches('*').trim_start();
    let first = rest.split_whitespace().next()?;
    settings.is_todo_keyword(first).then(|| first.to_string())
}

/// The line of the next headline of any level after `start`.
///
/// A repeater belongs to the entry it is written in, not to a child that
/// happens to sit inside the same subtree.
fn next_headline(lines: &[String], start: usize) -> usize {
    lines[start + 1..]
        .iter()
        .position(|line| restructure::headline_level(line).is_some())
        .map_or(lines.len(), |offset| start + 1 + offset)
}

/// Moves on every active repeating timestamp in lines `start..end`, or
/// returns `None` if there is none — in which case the entry is not
/// repeating, and marking it done simply closes it.
fn repeat(text: &str, start: usize, end: usize, now: (Date, Time)) -> Option<String> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut moved = false;

    for line in lines.iter_mut().take(end).skip(start + 1) {
        let mut out = String::with_capacity(line.len());
        let mut rest = line.as_str();
        while let Some(open) = rest.find('<') {
            out.push_str(&rest[..open]);
            let Some(close) = rest[open..].find('>').map(|close| open + close) else {
                break;
            };
            match repeated(&rest[open + 1..close], now) {
                Some(inner) => {
                    out.push('<');
                    out.push_str(&inner);
                    out.push('>');
                    moved = true;
                }
                None => out.push_str(&rest[open..=close]),
            }
            rest = &rest[close + 1..];
        }
        out.push_str(rest);
        *line = out;
    }

    moved.then(|| restructure::rejoin(&lines, text))
}

/// A moment, in minutes since the epoch, which is fine enough for Org's
/// minute-resolution timestamps and makes comparing with "now" one test.
fn minutes(date: Date, minute_of_day: i64) -> i64 {
    date.to_days() * 1440 + minute_of_day
}

/// The inside of a repeating timestamp, moved on once by its repeater.
///
/// `+1w` moves one week from the date written; `++1w` moves in weeks until
/// the date is in the future, so a task missed for a month does not come back
/// four times; `.+1w` moves to a week from today, for things that repeat from
/// when they were last done.
fn repeated(inner: &str, now: (Date, Time)) -> Option<String> {
    let tokens: Vec<&str> = inner.split_whitespace().collect();
    let date = Date::parse_iso(tokens.first()?)?;
    let repeater = tokens.iter().find_map(|token| parse_repeater(token))?;
    if repeater.count <= 0 {
        return None;
    }

    let clock_at = tokens.iter().position(|token| clock(token).is_some());
    let (from, to) = clock_at.map_or((0, None), |at| clock(tokens[at]).unwrap());
    let now_minutes = minutes(now.0, (now.1.hour * 60 + now.1.minute) as i64);
    let at = minutes(date, from);

    let moved = match repeater.kind {
        RepeaterKind::Cumulate => step(at, repeater),
        RepeaterKind::CatchUp => {
            let mut next = step(at, repeater);
            // Bounded, so a malformed stamp cannot hang the editor.
            for _ in 0..100_000 {
                if next > now_minutes {
                    break;
                }
                next = step(next, repeater);
            }
            next
        }
        RepeaterKind::Restart => match repeater.unit {
            RepeaterUnit::Hour => step(now_minutes, repeater),
            _ => step(minutes(now.0, from), repeater),
        },
    };

    let day = Date::from_days(moved.div_euclid(1440));
    let minute_of_day = moved.rem_euclid(1440);
    let mut rebuilt = vec![day.to_iso()];
    for (index, token) in tokens.iter().enumerate().skip(1) {
        if Some(index) == clock_at {
            let start = format_clock(minute_of_day);
            rebuilt.push(match to {
                // A range keeps its length.
                Some(to) => format!("{start}-{}", format_clock(minute_of_day + to - from)),
                None => start,
            });
        } else if token.chars().all(char::is_alphabetic) {
            rebuilt.push(day.weekday().to_string());
        } else {
            rebuilt.push(token.to_string());
        }
    }

    Some(rebuilt.join(" "))
}

/// One repeater interval on from `at`.
fn step(at: i64, repeater: Repeater) -> i64 {
    match repeater.unit {
        RepeaterUnit::Hour => at + repeater.count * 60,
        _ => {
            let date = Date::from_days(at.div_euclid(1440));
            minutes(repeater.advance(date), at.rem_euclid(1440))
        }
    }
}

/// Reads `10:00` or `10:00-11:30` as minutes of the day.
fn clock(token: &str) -> Option<(i64, Option<i64>)> {
    let read = |text: &str| {
        let (hour, minute) = text.split_once(':')?;
        let hour: i64 = hour.parse().ok()?;
        let minute: i64 = minute.parse().ok()?;
        (hour < 24 && minute < 60 && !text.starts_with(['+', '-'])).then_some(hour * 60 + minute)
    };

    match token.split_once('-') {
        Some((from, to)) => Some((read(from)?, Some(read(to)?))),
        None => Some((read(token)?, None)),
    }
}

/// `09:05`. A range end past midnight is written as Org would, past 24:00.
fn format_clock(minute_of_day: i64) -> String {
    format!("{:02}:{:02}", minute_of_day / 60, minute_of_day % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> (Date, Time) {
        (
            Date::parse_iso("2026-09-23").unwrap(),
            Time {
                hour: 10,
                minute: 0,
            },
        )
    }

    fn change(text: &str, options: &[&str], to: Option<&str>) -> StateChange {
        let settings = FileSettings::scan(text);
        change_state(
            text,
            0,
            &settings,
            &Startup::from_options(options),
            to,
            now(),
        )
        .unwrap()
    }

    #[test]
    fn keyword_marks_are_read() {
        assert_eq!(
            KeywordLog::parse("w@/!"),
            KeywordLog {
                enter: Some(Log::Note),
                leave: Some(Log::Time)
            }
        );
        assert_eq!(KeywordLog::parse("d!").enter, Some(Log::Time));
        assert_eq!(KeywordLog::parse("!").enter, Some(Log::Time));
        assert_eq!(KeywordLog::parse("/!").leave, Some(Log::Time));
        assert_eq!(KeywordLog::parse("t"), KeywordLog::default());
    }

    #[test]
    fn without_logging_only_the_keyword_changes() {
        let done = change("* TODO Task\nbody\n", &[], Some("DONE"));
        assert_eq!(done.text, "* DONE Task\nbody\n");
        assert_eq!(done.note, None);
    }

    #[test]
    fn logdone_closes_the_entry_and_reopening_removes_it() {
        let text = "* TODO Task\nSCHEDULED: <2026-09-20 Sun>\n";
        let done = change(text, &["logdone"], Some("DONE"));
        assert_eq!(
            done.text,
            "* DONE Task\nCLOSED: [2026-09-23 Wed 10:00] SCHEDULED: <2026-09-20 Sun>\n"
        );

        let reopened = change(&done.text, &["logdone"], Some("TODO"));
        assert_eq!(reopened.text, "* TODO Task\nSCHEDULED: <2026-09-20 Sun>\n");
    }

    #[test]
    fn a_closed_stamp_goes_even_without_logdone() {
        let text = "* DONE Task\nCLOSED: [2026-09-20 Sun 10:00]\n";
        assert_eq!(change(text, &[], None).text, "* Task\n");
    }

    #[test]
    fn lognotedone_asks_for_a_closing_note() {
        let done = change("* TODO Task\n", &["lognotedone"], Some("DONE"));
        assert_eq!(done.text, "* DONE Task\nCLOSED: [2026-09-23 Wed 10:00]\n");
        let pending = done.note.unwrap();
        assert_eq!(pending.heading, "CLOSING NOTE [2026-09-23 Wed 10:00]");

        assert_eq!(
            write_note(&done.text, &pending, "shipped"),
            "* DONE Task\nCLOSED: [2026-09-23 Wed 10:00]\n:LOGBOOK:\n\
             - CLOSING NOTE [2026-09-23 Wed 10:00] \\\\\n  shipped\n:END:\n"
        );
    }

    #[test]
    fn a_keyword_mark_logs_the_state_change() {
        let text = "#+TODO: TODO WAIT(w!) | DONE\n* TODO Task\n";
        let settings = FileSettings::scan(text);
        let waited =
            change_state(text, 1, &settings, &Startup::default(), Some("WAIT"), now()).unwrap();
        assert_eq!(
            waited.text,
            "#+TODO: TODO WAIT(w!) | DONE\n* WAIT Task\n:LOGBOOK:\n\
             - State \"WAIT\"       from \"TODO\"       [2026-09-23 Wed 10:00]\n:END:\n"
        );
    }

    #[test]
    fn a_leaving_mark_logs_when_the_new_state_has_none() {
        let text = "#+TODO: TODO WAIT(w@/!) | DONE\n* WAIT Task\n";
        let settings = FileSettings::scan(text);
        let left = change_state(
            text,
            1,
            &settings,
            &Startup::from_options(&["nologdrawer"]),
            Some("DONE"),
            now(),
        )
        .unwrap();
        assert_eq!(
            left.text,
            "#+TODO: TODO WAIT(w@/!) | DONE\n* DONE Task\n\
             - State \"DONE\"       from \"WAIT\"       [2026-09-23 Wed 10:00]\n"
        );
    }

    #[test]
    fn a_repeating_task_marked_done_comes_back_with_its_date_moved_on() {
        let text = "* TODO Water the plants\nSCHEDULED: <2026-09-20 Sun +1w>\n";
        let done = change(text, &["logdone"], Some("DONE"));
        assert!(done.repeated);
        assert_eq!(done.state.as_deref(), Some("TODO"));
        assert_eq!(
            done.text,
            "* TODO Water the plants\nSCHEDULED: <2026-09-27 Sun +1w>\n\
             :PROPERTIES:\n:LAST_REPEAT: [2026-09-23 Wed 10:00]\n:END:\n\
             :LOGBOOK:\n- State \"DONE\"       from \"TODO\"       [2026-09-23 Wed 10:00]\n:END:\n"
        );
    }

    #[test]
    fn nologrepeat_moves_the_date_and_records_nothing() {
        let text = "* TODO Water the plants\nSCHEDULED: <2026-09-20 Sun +1w>\n";
        let done = change(text, &["nologrepeat"], Some("DONE"));
        assert_eq!(
            done.text,
            "* TODO Water the plants\nSCHEDULED: <2026-09-27 Sun +1w>\n"
        );
    }

    #[test]
    fn each_repeater_kind_moves_from_where_it_says() {
        let at = |stamp: &str| repeated(stamp, now()).unwrap();
        // One step from the date written, even if that is still in the past.
        assert_eq!(at("2026-09-01 Tue +1w"), "2026-09-08 Tue +1w");
        // Steps until it is in the future.
        assert_eq!(at("2026-09-01 Tue ++1w"), "2026-09-29 Tue ++1w");
        // From today.
        assert_eq!(at("2026-09-01 Tue .+1w"), "2026-09-30 Wed .+1w");
        // Months clamp to the end of a shorter month.
        assert_eq!(at("2026-01-31 Sat +1m"), "2026-02-28 Sat +1m");
        // A time and a warning period travel with the date.
        assert_eq!(
            at("2026-09-22 Tue 09:00-10:30 +1d -2d"),
            "2026-09-23 Wed 09:00-10:30 +1d -2d"
        );
        // Hours move the clock, and roll the day over.
        assert_eq!(at("2026-09-23 Wed 22:00 +3h"), "2026-09-24 Thu 01:00 +3h");
        // `++` compares the time of day too: 09:00 today has passed at 10:00.
        assert_eq!(at("2026-09-22 Tue 09:00 ++1d"), "2026-09-24 Thu 09:00 ++1d");
    }

    #[test]
    fn a_zero_repeater_and_an_inactive_stamp_do_not_repeat() {
        assert_eq!(repeated("2026-09-20 Sun +0d", now()), None);
        let text = "* TODO Task\n[2026-09-20 Sun +1w]\n";
        let done = change(text, &[], Some("DONE"));
        assert!(!done.repeated);
        assert_eq!(done.text, "* DONE Task\n[2026-09-20 Sun +1w]\n");
    }

    #[test]
    fn a_child_repeater_does_not_make_the_parent_repeat() {
        let text = "* TODO Parent\n** TODO Child\nSCHEDULED: <2026-09-20 Sun +1w>\n";
        let done = change(text, &[], Some("DONE"));
        assert!(!done.repeated);
        assert!(done.text.contains("<2026-09-20 Sun +1w>"));
    }

    #[test]
    fn repeat_to_state_decides_where_the_task_goes_back_to() {
        let text = "#+TODO: TODO NEXT | DONE\n* NEXT Task\nSCHEDULED: <2026-09-20 Sun +1w>\n\
                    :PROPERTIES:\n:REPEAT_TO_STATE: NEXT\n:END:\n";
        let settings = FileSettings::scan(text);
        let done = change_state(
            text,
            1,
            &settings,
            &Startup::from_options(&["nologrepeat"]),
            Some("DONE"),
            now(),
        )
        .unwrap();
        assert_eq!(done.state.as_deref(), Some("NEXT"));
    }

    #[test]
    fn lognoterepeat_leaves_the_state_line_waiting_for_a_note() {
        let text = "* TODO Task\nSCHEDULED: <2026-09-20 Sun +1w>\n";
        let done = change(text, &["lognoterepeat"], Some("DONE"));
        let pending = done.note.unwrap();
        assert_eq!(
            pending.heading,
            "State \"DONE\"       from \"TODO\"       [2026-09-23 Wed 10:00]"
        );
    }

    #[test]
    fn rescheduling_is_logged_only_when_asked_and_only_for_a_change() {
        let text = "* TODO Task\nSCHEDULED: <2026-09-20 Sun>\n";
        let quiet = replan(
            text,
            0,
            Planning::Scheduled,
            Some("<2026-09-25 Fri>"),
            &Startup::default(),
            now(),
        )
        .unwrap();
        assert!(!quiet.logged);

        let startup = Startup::from_options(&["logreschedule"]);
        let logged = replan(
            text,
            0,
            Planning::Scheduled,
            Some("<2026-09-25 Fri>"),
            &startup,
            now(),
        )
        .unwrap();
        assert_eq!(
            logged.text,
            "* TODO Task\nSCHEDULED: <2026-09-25 Fri>\n:LOGBOOK:\n\
             - Rescheduled from \"[2026-09-20 Sun]\" on [2026-09-23 Wed 10:00]\n:END:\n"
        );

        let first = replan(
            "* TODO Task\n",
            0,
            Planning::Scheduled,
            Some("<2026-09-25 Fri>"),
            &startup,
            now(),
        )
        .unwrap();
        assert!(!first.logged, "a first date is not a reschedule");

        let removed = replan(text, 0, Planning::Scheduled, None, &startup, now()).unwrap();
        assert!(removed
            .text
            .contains("Not scheduled, was \"[2026-09-20 Sun]\""));
    }

    #[test]
    fn a_state_heading_without_a_previous_state_still_lines_up() {
        assert_eq!(
            state_heading(Some("TODO"), None, "[s]"),
            "State \"TODO\"       from              [s]"
        );
    }
}
