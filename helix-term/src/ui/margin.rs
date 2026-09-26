//! The margin of the log and the status: each commit's author and age.
//!
//! `Z` cycles it through the age, the date, and nothing. The choice holds
//! for the rest of the session, separately for the log and the status, as
//! a fresh log should not forget that the margin was turned off.

use std::sync::atomic::{AtomicU8, Ordering};

use helix_core::unicode::width::UnicodeWidthStr;

/// What the margin shows next to a commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Margin {
    Off,
    /// The author and how long ago: `Ann  3 days`.
    Age,
    /// The author and the date: `Ann  2026-09-25`.
    Date,
}

impl Margin {
    /// The next style `Z` switches to.
    pub fn next(self) -> Self {
        match self {
            Margin::Age => Margin::Date,
            Margin::Date => Margin::Off,
            Margin::Off => Margin::Age,
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Margin::Off => "Margin hidden",
            Margin::Age => "Margin: author and age",
            Margin::Date => "Margin: author and date",
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            1 => Margin::Age,
            2 => Margin::Date,
            _ => Margin::Off,
        }
    }

    const fn to_u8(self) -> u8 {
        match self {
            Margin::Off => 0,
            Margin::Age => 1,
            Margin::Date => 2,
        }
    }
}

/// A view's margin style, kept for the session.
pub struct MarginSetting(AtomicU8);

impl MarginSetting {
    const fn new(margin: Margin) -> Self {
        Self(AtomicU8::new(margin.to_u8()))
    }

    pub fn get(&self) -> Margin {
        Margin::from_u8(self.0.load(Ordering::Relaxed))
    }

    pub fn set(&self, margin: Margin) {
        self.0.store(margin.to_u8(), Ordering::Relaxed);
    }
}

/// The log shows the margin unless told otherwise, as Magit's does.
pub static LOG_MARGIN: MarginSetting = MarginSetting::new(Margin::Age);
/// The status keeps its lines short unless asked.
pub static STATUS_MARGIN: MarginSetting = MarginSetting::new(Margin::Off);

/// What the margin needs to know of a commit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stamp {
    pub author: String,
    /// Seconds since the epoch.
    pub time: i64,
    pub date: String,
}

/// Longest author name shown; longer ones are cut.
const AUTHOR_WIDTH: usize = 16;

/// The margin text of each row, aligned into columns: authors on the left,
/// ages or dates flush right. Rows without a commit get an empty string.
pub fn column(stamps: &[Option<&Stamp>], margin: Margin, now: i64) -> Vec<String> {
    if margin == Margin::Off {
        return vec![String::new(); stamps.len()];
    }
    let when = |stamp: &Stamp| match margin {
        Margin::Age => helix_magit::log::age(stamp.time, now),
        _ => stamp.date.clone(),
    };
    let author = |stamp: &Stamp| -> String {
        let mut name = String::new();
        for c in stamp.author.chars() {
            if (name.as_str().width() + c.to_string().width()) > AUTHOR_WIDTH {
                break;
            }
            name.push(c);
        }
        name
    };
    let pieces: Vec<Option<(String, String)>> = stamps
        .iter()
        .map(|stamp| stamp.map(|stamp| (author(stamp), when(stamp))))
        .collect();
    let author_width = pieces
        .iter()
        .flatten()
        .map(|(author, _)| author.as_str().width())
        .max()
        .unwrap_or(0);
    let when_width = pieces
        .iter()
        .flatten()
        .map(|(_, when)| when.as_str().width())
        .max()
        .unwrap_or(0);
    pieces
        .into_iter()
        .map(|piece| match piece {
            Some((author, when)) => format!(
                " {author}{}  {}{when}",
                " ".repeat(author_width - author.as_str().width()),
                " ".repeat(when_width - when.as_str().width()),
            ),
            None => String::new(),
        })
        .collect()
}

/// Seconds since the epoch, for ages.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(author: &str, time: i64) -> Stamp {
        Stamp {
            author: author.into(),
            time,
            date: "2026-09-25".into(),
        }
    }

    #[test]
    fn the_margin_is_aligned_in_columns() {
        let (ann, bartholomew) = (stamp("Ann", 0), stamp("Bartholomew", 3 * 86_400));
        let rows = [Some(&ann), None, Some(&bartholomew)];
        assert_eq!(
            column(&rows, Margin::Age, 3 * 86_400 + 60),
            [
                " Ann            3 days".to_string(),
                String::new(),
                " Bartholomew  1 minute".to_string(),
            ]
        );
        assert_eq!(
            column(&rows, Margin::Date, 0)[0],
            " Ann          2026-09-25"
        );
        assert_eq!(column(&rows, Margin::Off, 0), vec![String::new(); 3]);
    }

    #[test]
    fn long_authors_are_cut() {
        let long = stamp("An Extremely Long Author Name", 0);
        assert_eq!(
            column(&[Some(&long)], Margin::Age, 0)[0],
            " An Extremely Lon  just now"
        );
    }

    #[test]
    fn z_cycles_through_the_styles() {
        assert_eq!(Margin::Age.next(), Margin::Date);
        assert_eq!(Margin::Date.next(), Margin::Off);
        assert_eq!(Margin::Off.next(), Margin::Age);
    }
}
