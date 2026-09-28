//! Diary entries: the `%%(…)` lines Org reads from Emacs's calendar.
//!
//! `%%(diary-anniversary 10 1 1990) Pierre's birthday (%d years)` shows on
//! every first of October. The expression is Lisp, and there is no Lisp
//! here; the forms people write in their notes are recognised by name and
//! evaluated here, and any other is left out of the agenda rather than
//! guessed at.
//!
//! Dates in them are in Emacs's default American order, month day year,
//! except `org-anniversary`, which takes year month day.

use crate::Date;

/// When `line` (a `%%(…) text` line) falls on `day`, the text to show;
/// `%d` in it becomes the years since an anniversary's date.
pub fn occurs(line: &str, day: Date) -> Option<String> {
    let rest = line.trim().strip_prefix("%%(")?;
    let close = rest.find(')')?;
    let words: Vec<&str> = rest[..close].split_whitespace().collect();
    let text = rest[close + 1..].trim();
    let (form, args) = words.split_first()?;

    let number = |at: usize| -> Option<i64> { args.get(at)?.parse().ok() };
    // `t` in a date's place stands for any value.
    let matches = |at: usize, value: i64| -> Option<bool> {
        match *args.get(at)? {
            "t" => Some(true),
            given => Some(given.parse::<i64>().ok()? == value),
        }
    };
    let (year, month, date) = (day.year as i64, day.month as i64, day.day as i64);

    let years = match *form {
        "diary-anniversary" | "org-anniversary" => {
            let (m, d, y) = if *form == "org-anniversary" {
                (number(1)?, number(2)?, number(0)?)
            } else {
                (number(0)?, number(1)?, number(2)?)
            };
            // The 29th of February comes round on the 1st of March.
            let on = if m == 2 && d == 29 && days_in_month(year, 2) == 28 {
                month == 3 && date == 1
            } else {
                month == m && date == d
            };
            if !on || year < y {
                return None;
            }
            Some(year - y)
        }
        "diary-date" => {
            let on = matches(0, month)? && matches(1, date)? && matches(2, year)?;
            if !on {
                return None;
            }
            None
        }
        "diary-block" => {
            let start = Date::parse_iso(&format!(
                "{:04}-{:02}-{:02}",
                number(2)?,
                number(0)?,
                number(1)?
            ))?;
            let end = Date::parse_iso(&format!(
                "{:04}-{:02}-{:02}",
                number(5)?,
                number(3)?,
                number(4)?
            ))?;
            if day < start || day > end {
                return None;
            }
            None
        }
        "diary-cyclic" => {
            let every = number(0)?;
            let start = Date::parse_iso(&format!(
                "{:04}-{:02}-{:02}",
                number(3)?,
                number(1)?,
                number(2)?
            ))?;
            let since = day.to_days() - start.to_days();
            if every <= 0 || since < 0 || since % every != 0 {
                return None;
            }
            Some(since / every)
        }
        "diary-float" => {
            // (diary-float MONTH DAYNAME N): the Nth DAYNAME (0 is Sunday)
            // of the month, counted from its end when N is negative.
            if !matches(0, month)? {
                return None;
            }
            let dayname = number(1)?;
            let nth = number(2)?;
            if weekday(day) != dayname || nth == 0 {
                return None;
            }
            let from_start = (date - 1) / 7 + 1;
            let from_end = -((days_in_month(year, day.month) as i64 - date) / 7 + 1);
            if nth != from_start && nth != from_end {
                return None;
            }
            None
        }
        _ => return None,
    };

    let text = match years {
        Some(years) => text.replace("%d", &years.to_string()),
        None => text.to_string(),
    };
    Some(text)
}

/// 0 for Sunday to 6 for Saturday.
fn weekday(day: Date) -> i64 {
    // The 1st of January 1970 was a Thursday.
    (day.to_days() + 4).rem_euclid(7)
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        _ => 28,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(year: i32, month: u32, day: u32) -> Date {
        Date { year, month, day }
    }

    #[test]
    fn anniversaries_count_their_years() {
        let line = "%%(diary-anniversary 10 1 1990) Pierre (%d years)";
        assert_eq!(
            occurs(line, on(2026, 10, 1)).as_deref(),
            Some("Pierre (36 years)")
        );
        assert_eq!(occurs(line, on(2026, 10, 2)), None);
        assert_eq!(occurs(line, on(1989, 10, 1)), None);
        let org = "%%(org-anniversary 2000 2 29) Leap (%d)";
        assert_eq!(occurs(org, on(2026, 3, 1)).as_deref(), Some("Leap (26)"));
        assert_eq!(occurs(org, on(2028, 2, 29)).as_deref(), Some("Leap (28)"));
    }

    #[test]
    fn dates_blocks_cycles_and_floats() {
        let date = "%%(diary-date t 15 t) Loyer";
        assert!(occurs(date, on(2026, 9, 15)).is_some());
        assert!(occurs(date, on(2026, 9, 16)).is_none());

        let block = "%%(diary-block 9 28 2026 10 2 2026) Congés";
        assert!(occurs(block, on(2026, 9, 30)).is_some());
        assert!(occurs(block, on(2026, 10, 3)).is_none());

        let cyclic = "%%(diary-cyclic 14 9 1 2026) Poubelles";
        assert!(occurs(cyclic, on(2026, 9, 15)).is_some());
        assert!(occurs(cyclic, on(2026, 9, 16)).is_none());

        // The 4th Thursday of November, and the last Monday of May.
        let thanks = "%%(diary-float 11 4 4) Thanksgiving";
        assert!(occurs(thanks, on(2026, 11, 26)).is_some());
        assert!(occurs(thanks, on(2026, 11, 19)).is_none());
        let memorial = "%%(diary-float 5 1 -1) Memorial Day";
        assert!(occurs(memorial, on(2026, 5, 25)).is_some());

        assert!(occurs("%%(calendar-unknown 1) x", on(2026, 1, 1)).is_none());
    }
}
