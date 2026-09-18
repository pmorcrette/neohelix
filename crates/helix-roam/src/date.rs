//! Calendar arithmetic, without a date crate.
//!
//! Daily notes need three things: today's date, the date `n` days from a given
//! one, and a name to read and write. That is little enough that a dependency
//! would cost more than it saves, and the conversion below is the standard
//! days-from-civil algorithm rather than something invented here.

/// A calendar day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl Date {
    /// Today, in UTC.
    ///
    /// UTC rather than local time: the fork has no timezone database, and a
    /// daily note named for the wrong day would be worse than one that is
    /// consistently UTC. This is worth revisiting when the agenda lands.
    pub fn today() -> Self {
        let days = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| (since.as_secs() / 86_400) as i64)
            .unwrap_or(0);

        Self::from_days(days)
    }

    /// The date `offset` days away.
    pub fn offset_by(self, offset: i64) -> Self {
        Self::from_days(self.to_days() + offset)
    }

    /// `2026-09-18`, which is both Org's format and the daily-note file name.
    pub fn to_iso(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// Reads `2026-09-18`, rejecting anything that is not a real date.
    pub fn parse_iso(text: &str) -> Option<Self> {
        let mut parts = text.trim().split('-');
        let year = parts.next()?.parse().ok()?;
        let month = parts.next()?.parse().ok()?;
        let day = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }

        let date = Self { year, month, day };
        // Round-tripping catches `2026-02-31`, which parses but is not a day.
        (Self::from_days(date.to_days()) == date).then_some(date)
    }

    /// Days since the Unix epoch. Howard Hinnant's `days_from_civil`.
    pub fn to_days(self) -> i64 {
        let year = self.year as i64 - i64::from(self.month <= 2);
        let era = if year >= 0 { year } else { year - 399 } / 400;
        let year_of_era = year - era * 400;

        let month = self.month as i64;
        let day_of_year =
            (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + self.day as i64 - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;

        era * 146_097 + day_of_era - 719_468
    }

    /// The inverse: `civil_from_days`.
    pub fn from_days(days: i64) -> Self {
        let days = days + 719_468;
        let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
        let day_of_era = days - era * 146_097;
        let year_of_era =
            (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;

        let year = year_of_era + era * 400;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let shifted_month = (5 * day_of_year + 2) / 153;

        let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
        let month = (shifted_month + if shifted_month < 10 { 3 } else { -9 }) as u32;

        Self {
            year: (year + i64::from(month <= 2)) as i32,
            month,
            day,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_and_dates_round_trip() {
        for date in [
            Date {
                year: 1970,
                month: 1,
                day: 1,
            },
            Date {
                year: 2026,
                month: 9,
                day: 18,
            },
            Date {
                year: 2000,
                month: 2,
                day: 29,
            },
            Date {
                year: 1899,
                month: 12,
                day: 31,
            },
            Date {
                year: 2100,
                month: 3,
                day: 1,
            },
        ] {
            assert_eq!(Date::from_days(date.to_days()), date, "{date:?}");
        }
    }

    #[test]
    fn the_epoch_is_day_zero() {
        assert_eq!(
            Date {
                year: 1970,
                month: 1,
                day: 1
            }
            .to_days(),
            0
        );
    }

    #[test]
    fn crossing_a_month_and_a_leap_day_works() {
        let end_of_month = Date {
            year: 2026,
            month: 1,
            day: 31,
        };
        assert_eq!(
            end_of_month.offset_by(1),
            Date {
                year: 2026,
                month: 2,
                day: 1
            }
        );

        // 2024 is a leap year, 2026 is not.
        let before_leap = Date {
            year: 2024,
            month: 2,
            day: 28,
        };
        assert_eq!(
            before_leap.offset_by(1),
            Date {
                year: 2024,
                month: 2,
                day: 29
            }
        );
        let non_leap = Date {
            year: 2026,
            month: 2,
            day: 28,
        };
        assert_eq!(
            non_leap.offset_by(1),
            Date {
                year: 2026,
                month: 3,
                day: 1
            }
        );
    }

    #[test]
    fn iso_round_trips_and_rejects_impossible_days() {
        let date = Date {
            year: 2026,
            month: 9,
            day: 18,
        };
        assert_eq!(date.to_iso(), "2026-09-18");
        assert_eq!(Date::parse_iso("2026-09-18"), Some(date));

        // Parses as numbers but is not a day.
        assert_eq!(Date::parse_iso("2026-02-31"), None);
        assert_eq!(Date::parse_iso("2026-13-01"), None);
        assert_eq!(Date::parse_iso("not a date"), None);
        assert_eq!(Date::parse_iso("2026-09-18-01"), None);
    }

    #[test]
    fn today_is_a_plausible_date() {
        let today = Date::today();
        assert!((2020..2200).contains(&today.year), "{today:?}");
        assert!((1..=12).contains(&today.month));
        assert!((1..=31).contains(&today.day));
    }
}
