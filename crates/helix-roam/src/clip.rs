//! Cut, copy, paste and clone subtrees as structure rather than as text.
//!
//! What separates these from a plain yank is that a subtree knows its own
//! depth. Pasting a level-3 entry under a level-1 heading has to move the
//! whole tree up by two levels, or the outline the paste lands in stops being
//! an outline. The level therefore travels with the text, in a [`Clip`].
//!
//! Cloning is the other half: the standard way of laying out a recurring set
//! of entries is to write one, then ask for five copies a week apart.

use crate::parser::{parse_repeater, starts_with_ignore_case};
use crate::restructure::{headline_level, rejoin, subtree_range, Error};
use crate::{Date, Repeater, RepeaterUnit};

/// A subtree lifted out of a buffer, remembering the level it came from.
///
/// The level is kept beside the text because pasting needs it: without it the
/// clip is just lines, and putting them back is a guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    /// The subtree's lines, its own headline first.
    pub text: String,
    /// The level of that headline where it was taken from.
    pub level: usize,
}

/// Reads the subtree at the cursor without changing the buffer.
///
/// The copy loses its `:ID:`, because it is about to become a second entry
/// and two nodes sharing a UUID is a corrupt graph. Cutting keeps the id:
/// there the entry is moving, not being duplicated, and every link already
/// pointing at it has to keep resolving.
pub fn copy_subtree(text: &str, line: usize) -> Option<Clip> {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (start, end, level) = subtree_range(&lines, line)?;

    Some(Clip {
        text: without_ids(&lines[start..end]).join("\n"),
        level,
    })
}

/// Removes the subtree at the cursor and hands it back as a clip.
pub fn cut_subtree(text: &str, line: usize) -> Option<(String, Clip)> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (start, end, level) = subtree_range(&lines, line)?;

    let clip = Clip {
        text: lines[start..end].join("\n"),
        level,
    };
    lines.drain(start..end);

    Some((rejoin(&lines, text), clip))
}

/// Pastes a clip as a sibling of the entry at the cursor.
///
/// The clip is shifted so its own headline lands at the level of the entry it
/// follows; everything under it moves by the same amount, so the tree keeps
/// its shape. Returns the new text and the line the pasted headline is on.
pub fn paste_subtree(text: &str, line: usize, clip: &Clip) -> (String, usize) {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();

    // Above any headline there is no sibling to match, so the clip becomes a
    // top-level entry at the end of the file — the same choice `insert_heading`
    // makes in that position.
    let (at, level) = match subtree_range(&lines, line) {
        Some((_, end, level)) => (end, level),
        None => (lines.len(), 1),
    };

    let mut body: Vec<String> = clip.text.lines().map(str::to_string).collect();
    retarget(&mut body, clip.level, level);
    lines.splice(at..at, body);

    (rejoin(&lines, text), at)
}

/// Moves every headline in a subtree so its root sits at `to`.
///
/// Each heading keeps its depth relative to the root, which is what makes the
/// result a subtree rather than a flattened list of headings.
fn retarget(lines: &mut [String], from: usize, to: usize) {
    if from == to {
        return;
    }

    for line in lines {
        let Some(level) = headline_level(line) else {
            continue;
        };
        // The root is the shallowest line in a well-formed clip, so this only
        // clamps when the clip is not one.
        let moved = (level + to).saturating_sub(from).max(1);
        *line = format!("{}{}", "*".repeat(moved), &line[level..]);
    }
}

/// Reads a clone's time shift, written the way a repeater is: `+1w`, `+2m`.
pub fn parse_shift(text: &str) -> Result<Repeater, Error> {
    let shift = parse_repeater(text.trim()).ok_or(Error::UnsupportedShift)?;

    // An hour step says nothing about which day a copy lands on, and a set of
    // clones is a calendar of days.
    if shift.unit == RepeaterUnit::Hour {
        return Err(Error::UnsupportedShift);
    }

    Ok(shift)
}

/// Appends `times` copies of the subtree at the cursor, shifting their dates.
///
/// Copy *n* is shifted by *n* intervals from the original rather than by one
/// interval from the copy before it, so a monthly set started on the 31st
/// lands on the 31st of every month that has one instead of drifting back
/// after the first short month.
///
/// Two things are deliberately not carried into the copies:
///
/// * The `:ID:`. Two nodes sharing a UUID is a corrupt graph, and the entry
///   the copy was made from already owns that one. A copy is a fresh entry;
///   `org-create-id` gives it an identity when it needs one.
/// * Inactive timestamps. `[…]` records when something happened — a `CLOSED:`
///   line, a log entry — and moving it forward would turn a record into a
///   falsehood. Only `<…>` timestamps, which are the ones the agenda reads,
///   are shifted.
pub fn clone_subtree(
    text: &str,
    line: usize,
    times: usize,
    shift: Option<Repeater>,
) -> Result<String, Error> {
    if let Some(shift) = shift {
        if shift.unit == RepeaterUnit::Hour {
            return Err(Error::UnsupportedShift);
        }
    }

    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (start, end, _) = subtree_range(&lines, line).ok_or(Error::NoSubtree)?;
    if times == 0 {
        return Ok(text.to_string());
    }

    let original = without_ids(&lines[start..end]);

    let mut copies = Vec::with_capacity(original.len() * times);
    for step in 1..=times as i64 {
        let step_shift = shift.map(|shift| Repeater {
            count: shift.count * step,
            ..shift
        });

        copies.extend(original.iter().map(|line| match step_shift {
            Some(shift) => shift_active_timestamps(line, shift),
            None => line.clone(),
        }));
    }

    lines.splice(end..end, copies);
    Ok(rejoin(&lines, text))
}

/// Drops `:ID:` lines, and any property drawer left empty by dropping them.
///
/// Shared by copying and cloning: both produce an entry that is new, however
/// much of an existing one it is made of.
fn without_ids(lines: &[String]) -> Vec<String> {
    let mut kept: Vec<String> = Vec::with_capacity(lines.len());

    for line in lines {
        let trimmed = line.trim();

        if starts_with_ignore_case(trimmed, ":ID:") {
            continue;
        }

        // A drawer that held nothing but the `:ID:` is now empty, and an empty
        // drawer in every copy is noise.
        if starts_with_ignore_case(trimmed, ":END:")
            && kept
                .last()
                .is_some_and(|prev| starts_with_ignore_case(prev.trim(), ":PROPERTIES:"))
        {
            kept.pop();
            continue;
        }

        kept.push(line.clone());
    }

    kept
}

/// Moves every active timestamp in a line on by one `shift` interval.
fn shift_active_timestamps(line: &str, shift: Repeater) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;

    while let Some(open) = rest.find('<') {
        let (before, from_open) = rest.split_at(open);
        out.push_str(before);

        let Some(close) = from_open.find('>') else {
            out.push_str(from_open);
            return out;
        };

        match shifted(&from_open[1..close], shift) {
            Some(moved) => {
                out.push('<');
                out.push_str(&moved);
                out.push('>');
            }
            // Not a timestamp — a diary sexp, or a `<` that means `<`.
            None => out.push_str(&from_open[..=close]),
        }

        rest = &from_open[close + 1..];
    }

    out.push_str(rest);
    out
}

/// Rewrites the inside of a `<…>` timestamp at a new date.
fn shifted(inner: &str, shift: Repeater) -> Option<String> {
    let mut parts = inner.split_whitespace();
    let moved = shift.advance(Date::parse_iso(parts.next()?)?);

    let mut rebuilt = moved.to_iso();
    for part in parts {
        rebuilt.push(' ');

        // The day name is a fact about the date, so it is recomputed. A time,
        // a repeater and a warning period are the entry's own and travel as
        // they were written; none of them is purely alphabetic.
        if part.chars().all(char::is_alphabetic) {
            rebuilt.push_str(moved.weekday());
        } else {
            rebuilt.push_str(part);
        }
    }

    Some(rebuilt)
}
