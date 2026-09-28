//! Habits: a repeating task whose history is the point.
//!
//! An entry is a habit when it has `:STYLE: habit` and a `SCHEDULED:` with a
//! repeater, usually `.+` ("again N days after I last did it"). A maximum
//! can follow the repeater: `.+2d/4d` means due two days after the last
//! time, and late after four. The history is what the state changes logged
//! into the entry record — Task 1.21 makes marking a repeating task done log
//! `State "DONE" from "TODO" [date]` by default.
//!
//! The consistency graph shows the three weeks before today and the week
//! after, one cell a day: done or not, and how due the habit was that day.

use crate::parser::{parse_repeater, FileSettings};
use crate::restructure::{self, headline_level, Planning};
use crate::{Date, RepeaterUnit};

/// A habit, as its entry describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Habit {
    /// The next time it is due, from `SCHEDULED:`.
    pub scheduled: Date,
    /// Days from one completion to the next due date.
    pub interval: i64,
    /// Days from one completion to the last day before it is late, when the
    /// repeater gives a maximum.
    pub max: Option<i64>,
    /// The days it was done, oldest first.
    pub done: Vec<Date>,
}

/// How due a habit was on a day, which Org shows as a colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    /// Before the due date: blue in Org.
    NotYet,
    /// Due, and not late yet: green.
    Due,
    /// The last day before it is late: yellow.
    LastDay,
    /// Late: red.
    Overdue,
}

/// One day of the consistency graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub day: Date,
    pub due: Due,
    pub done: bool,
    pub today: bool,
}

impl Cell {
    /// Org's glyphs: `*` for a day it was done, `!` for today if not.
    pub fn glyph(&self) -> char {
        if self.done {
            '*'
        } else if self.today {
            '!'
        } else {
            ' '
        }
    }
}

/// Days in a repeater step. Months and years are approximate, as they are
/// in Org's habit graph, which counts in days.
fn days(count: i64, unit: RepeaterUnit) -> i64 {
    match unit {
        RepeaterUnit::Hour => (count / 24).max(1),
        RepeaterUnit::Day => count,
        RepeaterUnit::Week => count * 7,
        RepeaterUnit::Month => count * 30,
        RepeaterUnit::Year => count * 365,
    }
}

/// `4d` from the `/4d` after a repeater.
fn max_days(token: &str) -> Option<i64> {
    let (_, max) = token.split_once('/')?;
    let digits: String = max.chars().take_while(char::is_ascii_digit).collect();
    let unit = match max[digits.len()..].chars().next()? {
        'h' => RepeaterUnit::Hour,
        'd' => RepeaterUnit::Day,
        'w' => RepeaterUnit::Week,
        'm' => RepeaterUnit::Month,
        'y' => RepeaterUnit::Year,
        _ => return None,
    };
    Some(days(digits.parse().ok()?, unit))
}

/// The habit the entry containing `line` describes, or `None` when it is
/// not one.
pub fn parse(text: &str, line: usize) -> Option<Habit> {
    let style = restructure::property_value(text, line, "STYLE")?;
    if !style.eq_ignore_ascii_case("habit") {
        return None;
    }
    let stamp = restructure::planning_value(text, line, Planning::Scheduled)?;
    let inner = stamp.trim_matches(['<', '>']);
    let mut tokens = inner.split_whitespace();
    let scheduled = Date::parse_iso(tokens.next()?)?;
    let token = tokens.find(|token| parse_repeater(token).is_some())?;
    let repeater = parse_repeater(token)?;

    let settings = FileSettings::scan(text);
    let lines: Vec<&str> = text.lines().collect();
    let start = (0..=line.min(lines.len().saturating_sub(1)))
        .rev()
        .find(|&at| headline_level(lines[at]).is_some())?;
    let end = (start + 1..lines.len())
        .find(|&at| headline_level(lines[at]).is_some())
        .unwrap_or(lines.len());

    let mut done: Vec<Date> = lines[start + 1..end]
        .iter()
        .filter_map(|line| completed_on(line, &settings))
        .collect();
    done.sort();
    done.dedup();

    Some(Habit {
        scheduled,
        interval: days(repeater.count, repeater.unit),
        max: max_days(token),
        done,
    })
}

/// The day a log line records a move into a done state.
fn completed_on(line: &str, settings: &FileSettings) -> Option<Date> {
    let rest = line.trim_start().strip_prefix("- State \"")?;
    let (state, rest) = rest.split_once('"')?;
    if !settings.done_keywords.iter().any(|done| done == state) {
        return None;
    }
    // The stamp is the last bracketed thing on the line.
    let open = rest.rfind('[')?;
    Date::parse_iso(rest[open + 1..].split_whitespace().next()?)
}

/// The graph from `before` days before `today` to `after` days after.
///
/// For a day in the past, the habit was due `interval` days after the last
/// time it was done before that day; for a day to come, it is due when
/// `SCHEDULED:` says. Days before any recorded completion and before the
/// scheduled date count as not yet due.
pub fn consistency(habit: &Habit, today: Date, before: i64, after: i64) -> Vec<Cell> {
    (-before..=after)
        .map(|offset| {
            let day = today.offset_by(offset);
            let from = if day > today {
                Some(habit.scheduled)
            } else {
                habit
                    .done
                    .iter()
                    .rev()
                    .find(|done| **done < day)
                    .map(|last| last.offset_by(habit.interval))
                    .or((day >= habit.scheduled).then_some(habit.scheduled))
            };
            let due = match from {
                None => Due::NotYet,
                Some(from) => {
                    let last = from.offset_by(habit.max.map_or(0, |max| max - habit.interval));
                    if day < from {
                        Due::NotYet
                    } else if day < last {
                        Due::Due
                    } else if day == last {
                        Due::LastDay
                    } else {
                        Due::Overdue
                    }
                }
            };
            Cell {
                day,
                due,
                done: habit.done.contains(&day),
                today: day == today,
            }
        })
        .collect()
}

/// Org's default window: three weeks back, one forward.
pub const DAYS_BEFORE: i64 = 21;
pub const DAYS_AFTER: i64 = 7;

#[cfg(test)]
mod tests {
    use super::*;

    fn date(iso: &str) -> Date {
        Date::parse_iso(iso).unwrap()
    }

    const WATERING: &str = "\
* TODO Water the plants
SCHEDULED: <2026-09-26 Sat .+2d/4d>
:PROPERTIES:
:STYLE: habit
:LAST_REPEAT: [2026-09-24 Thu 08:00]
:END:
:LOGBOOK:
- State \"DONE\"       from \"TODO\"       [2026-09-24 Thu 08:00]
- State \"DONE\"       from \"TODO\"       [2026-09-18 Fri 09:00]
- Note taken on [2026-09-17 Thu 10:00] \\\\
  not a completion
- State \"WAIT\"       from \"TODO\"       [2026-09-16 Wed 10:00]
:END:
* Next
";

    #[test]
    fn a_habit_is_read_with_its_history() {
        let habit = parse(WATERING, 0).unwrap();
        assert_eq!(habit.scheduled, date("2026-09-26"));
        assert_eq!(habit.interval, 2);
        assert_eq!(habit.max, Some(4));
        assert_eq!(habit.done, [date("2026-09-18"), date("2026-09-24")]);
    }

    #[test]
    fn only_a_habit_style_entry_is_a_habit() {
        assert!(parse("* TODO Plain\nSCHEDULED: <2026-09-26 Sat .+2d>\n", 0).is_none());
        // A habit needs a repeater.
        assert!(parse(
            "* TODO X\nSCHEDULED: <2026-09-26 Sat>\n:PROPERTIES:\n:STYLE: habit\n:END:\n",
            0
        )
        .is_none());
    }

    #[test]
    fn each_day_is_as_due_as_the_last_completion_made_it() {
        let habit = parse(WATERING, 0).unwrap();
        let cells = consistency(&habit, date("2026-09-25"), 7, 3);
        let at = |iso: &str| *cells.iter().find(|cell| cell.day == date(iso)).unwrap();

        // Done on the 18th: not due until the 20th, late after the 22nd.
        assert_eq!(at("2026-09-19").due, Due::NotYet);
        assert_eq!(at("2026-09-20").due, Due::Due);
        assert_eq!(at("2026-09-22").due, Due::LastDay);
        assert_eq!(at("2026-09-23").due, Due::Overdue);
        assert!(at("2026-09-24").done);
        assert_eq!(at("2026-09-24").glyph(), '*');
        // Today, the day after: not due yet.
        assert_eq!(at("2026-09-25").due, Due::NotYet);
        assert_eq!(at("2026-09-25").glyph(), '!');
        // To come: from SCHEDULED.
        assert_eq!(at("2026-09-26").due, Due::Due);
        assert_eq!(at("2026-09-28").due, Due::LastDay);
    }

    #[test]
    fn before_any_history_nothing_is_due() {
        let habit = parse(WATERING, 0).unwrap();
        let cells = consistency(&habit, date("2026-09-25"), 10, 0);
        assert_eq!(cells[0].day, date("2026-09-15"));
        assert_eq!(cells[0].due, Due::NotYet);
    }
}
