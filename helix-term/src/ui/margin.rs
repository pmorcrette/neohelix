//! The margin of the log and the status: each commit's author and age.
//!
//! `Z` cycles it through the age, the short age, the date, and nothing;
//! the margin menu (`L`) sets a style, hides or shows it, and the author.
//! The choice holds for the rest of the session, separately for the log
//! and the status, as a fresh log should not forget that the margin was
//! turned off.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use helix_core::unicode::width::UnicodeWidthStr;

/// What the margin shows next to a commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Margin {
    Off,
    /// The author and how long ago: `Ann  3 days`.
    Age,
    /// The same, shorter: `Ann  3d`.
    ShortAge,
    /// The author and the date: `Ann  2026-09-25`.
    Date,
}

impl Margin {
    /// The next style `Z` switches to.
    pub fn next(self) -> Self {
        match self {
            Margin::Age => Margin::ShortAge,
            Margin::ShortAge => Margin::Date,
            Margin::Date => Margin::Off,
            Margin::Off => Margin::Age,
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Margin::Off => "Margin hidden",
            Margin::Age => "Margin: author and age",
            Margin::ShortAge => "Margin: author and short age",
            Margin::Date => "Margin: author and date",
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            1 => Margin::Age,
            2 => Margin::Date,
            3 => Margin::ShortAge,
            _ => Margin::Off,
        }
    }

    const fn to_u8(self) -> u8 {
        match self {
            Margin::Off => 0,
            Margin::Age => 1,
            Margin::Date => 2,
            Margin::ShortAge => 3,
        }
    }
}

/// A view's margin style, kept for the session, and the style it had
/// when last shown, which showing it again brings back.
pub struct MarginSetting(AtomicU8, AtomicU8);

impl MarginSetting {
    const fn new(margin: Margin) -> Self {
        Self(
            AtomicU8::new(margin.to_u8()),
            AtomicU8::new(Margin::Age.to_u8()),
        )
    }

    pub fn get(&self) -> Margin {
        Margin::from_u8(self.0.load(Ordering::Relaxed))
    }

    pub fn set(&self, margin: Margin) {
        if margin != Margin::Off {
            self.1.store(margin.to_u8(), Ordering::Relaxed);
        }
        self.0.store(margin.to_u8(), Ordering::Relaxed);
    }

    /// Hides the margin, or shows it again in the style it had.
    pub fn toggle(&self) -> Margin {
        let margin = match self.get() {
            Margin::Off => Margin::from_u8(self.1.load(Ordering::Relaxed)),
            _ => Margin::Off,
        };
        self.set(margin);
        margin
    }
}

/// Whether the margin names the author, or only says when.
static AUTHOR: AtomicBool = AtomicBool::new(true);

pub fn author_shown() -> bool {
    AUTHOR.load(Ordering::Relaxed)
}

/// Shows or hides the author in every margin; returns whether shown now.
pub fn toggle_author() -> bool {
    !AUTHOR.fetch_xor(true, Ordering::Relaxed)
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
    column_with(stamps, margin, now, author_shown())
}

/// [`column`], with or without the authors.
pub fn column_with(
    stamps: &[Option<&Stamp>],
    margin: Margin,
    now: i64,
    with_author: bool,
) -> Vec<String> {
    if margin == Margin::Off {
        return vec![String::new(); stamps.len()];
    }
    let when = |stamp: &Stamp| match margin {
        Margin::Age => helix_magit::log::age(stamp.time, now),
        Margin::ShortAge => short_age(stamp.time, now),
        _ => stamp.date.clone(),
    };
    let author = |stamp: &Stamp| -> String {
        let mut name = String::new();
        if !with_author {
            return name;
        }
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

/// How long ago, in the fewest characters: `3d`, `2w`, `5mo`.
pub fn short_age(time: i64, now: i64) -> String {
    let seconds = (now - time).max(0);
    let units = [
        (365 * 86_400, "y"),
        (30 * 86_400, "mo"),
        (7 * 86_400, "w"),
        (86_400, "d"),
        (3_600, "h"),
        (60, "m"),
    ];
    units
        .iter()
        .find(|(length, _)| seconds >= *length)
        .map_or_else(
            || "now".to_string(),
            |(length, unit)| format!("{}{unit}", seconds / length),
        )
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
        assert_eq!(
            column_with(&rows, Margin::ShortAge, 3 * 86_400 + 60, false)[0],
            "   3d"
        );
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
        assert_eq!(Margin::Age.next(), Margin::ShortAge);
        assert_eq!(Margin::ShortAge.next(), Margin::Date);
        assert_eq!(Margin::Date.next(), Margin::Off);
        assert_eq!(Margin::Off.next(), Margin::Age);
    }
}
