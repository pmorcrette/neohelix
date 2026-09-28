//! Sorting entries, list items and table rows by a chosen key.
//!
//! The three share a shape: find the run of siblings the cursor sits in, read
//! one value per sibling, reorder, write back. They differ only in what a
//! sibling is and where its value comes from, so the key lives here once.
//!
//! Siblings whose value is missing are never sorted. They keep their order
//! among themselves and stay at the bottom, reversed or not: an entry with no
//! deadline has not got an early deadline, and letting it lead a reversed
//! sort would say that it had.

use std::cmp::Ordering;

use crate::list::{parse_item, renumber};
use crate::parser::{parse_headline, parse_planning, parse_timestamp, FileSettings};
use crate::restructure::{headline_level, rejoin, subtree_range, Error};
use crate::table::{parse_table, rewrite, Row};

/// What to sort siblings by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SortKey {
    /// The text, compared without regard to case.
    Alphabetical,
    /// The number the text starts with.
    Numeric,
    /// The order the file declares its TODO keywords in.
    Todo,
    /// The order the file declares its priorities in.
    Priority,
    /// The `SCHEDULED:` date.
    Scheduled,
    /// The `DEADLINE:` date.
    Deadline,
    /// The first active timestamp anywhere in the entry.
    Timestamp,
    /// A property's value.
    Property(String),
}

impl SortKey {
    /// Reads a key from what the user typed.
    ///
    /// A property is written `property:CATEGORY` rather than bare, so that
    /// adding a key later cannot silently change what an existing name means.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();

        if let Some(name) = text.strip_prefix("property:") {
            let name = name.trim();
            return (!name.is_empty()).then(|| SortKey::Property(name.to_string()));
        }

        Some(match text.to_ascii_lowercase().as_str() {
            "alpha" | "alphabetical" | "title" | "text" => SortKey::Alphabetical,
            "numeric" | "number" => SortKey::Numeric,
            "todo" => SortKey::Todo,
            "priority" => SortKey::Priority,
            "scheduled" => SortKey::Scheduled,
            "deadline" => SortKey::Deadline,
            "timestamp" | "time" => SortKey::Timestamp,
            _ => return None,
        })
    }

    /// Every key name, for completion and for an error message worth reading.
    pub fn names() -> &'static [&'static str] {
        &[
            "alpha",
            "numeric",
            "todo",
            "priority",
            "scheduled",
            "deadline",
            "timestamp",
            "property:KEY",
        ]
    }
}

/// One sibling's value, ordered against the others'.
#[derive(Debug, Clone, PartialEq)]
enum Cell {
    Text(String),
    Number(f64),
}

impl Cell {
    fn compare(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Cell::Text(a), Cell::Text(b)) => a.cmp(b),
            (Cell::Number(a), Cell::Number(b)) => a.total_cmp(b),
            // Only a column that is not uniform mixes the two. Numbers first
            // is arbitrary, but it is at least the same answer every time.
            (Cell::Number(_), Cell::Text(_)) => Ordering::Less,
            (Cell::Text(_), Cell::Number(_)) => Ordering::Greater,
        }
    }

    /// A value read as a number when it is one, and as text otherwise.
    fn read(text: &str) -> Self {
        match text.trim().parse::<f64>() {
            Ok(number) => Cell::Number(number),
            Err(_) => Cell::Text(text.trim().to_lowercase()),
        }
    }
}

/// Reorders `blocks` by their values, leaving the valueless ones at the end.
fn reorder<T>(blocks: Vec<(Option<Cell>, T)>, reverse: bool) -> Vec<T> {
    let (mut valued, missing): (Vec<_>, Vec<_>) =
        blocks.into_iter().partition(|(value, _)| value.is_some());

    // Reversing through the comparison rather than reversing the result keeps
    // the sort stable: siblings that tie stay in the order they were written.
    valued.sort_by(|a, b| {
        let order = match (&a.0, &b.0) {
            (Some(a), Some(b)) => a.compare(b),
            _ => Ordering::Equal,
        };
        if reverse {
            order.reverse()
        } else {
            order
        }
    });

    valued
        .into_iter()
        .chain(missing)
        .map(|(_, block)| block)
        .collect()
}

/// Sorts the children of the entry at the cursor.
///
/// With the cursor above any headline it sorts the file's top-level entries
/// instead. Whatever sits between an entry's headline and its first child —
/// a planning line, a drawer, a paragraph — belongs to the parent and stays
/// where it is.
pub fn sort_entries(
    text: &str,
    line: usize,
    key: &SortKey,
    reverse: bool,
) -> Result<String, Error> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let settings = FileSettings::scan(text);

    let (scope_start, scope_end, level) = match subtree_range(&lines, line) {
        Some((start, end, level)) => (start + 1, end, level + 1),
        None => {
            let first = lines
                .iter()
                .position(|l| headline_level(l).is_some())
                .ok_or(Error::NoSubtree)?;
            (first, lines.len(), 1)
        }
    };

    let children = ranges(&lines, scope_start, scope_end, |l| {
        headline_level(l) == Some(level)
    });
    if children.len() < 2 {
        return Err(Error::NoSibling);
    }

    let blocks: Vec<_> = children
        .iter()
        .map(|&(start, end)| {
            let block = lines[start..end].to_vec();
            (entry_value(&block, key, &settings), block)
        })
        .collect();

    let first = children[0].0;
    let sorted: Vec<String> = reorder(blocks, reverse).concat();
    lines.splice(first..scope_end, sorted);

    Ok(rejoin(&lines, text))
}

/// The value a key reads out of one entry's lines.
fn entry_value(entry: &[String], key: &SortKey, settings: &FileSettings) -> Option<Cell> {
    let headline = parse_headline(entry.first()?, settings)?;

    match key {
        SortKey::Alphabetical => Some(Cell::Text(headline.title.to_lowercase())),
        SortKey::Numeric => leading_number(&headline.title).map(Cell::Number),

        SortKey::Todo => {
            let state = headline.todo?;
            let rank = settings
                .todo_keywords
                .iter()
                .position(|kw| *kw == state.keyword)
                .or_else(|| {
                    settings
                        .done_keywords
                        .iter()
                        .position(|kw| *kw == state.keyword)
                        .map(|at| settings.todo_keywords.len() + at)
                })?;
            Some(Cell::Number(rank as f64))
        }

        SortKey::Priority => {
            let letter = headline.priority?;
            let rank = settings.priorities.iter().position(|c| *c == letter)?;
            Some(Cell::Number(rank as f64))
        }

        SortKey::Scheduled | SortKey::Deadline => entry.iter().find_map(|line| {
            let (scheduled, deadline) = parse_planning(line.trim())?;
            let stamp = match key {
                SortKey::Scheduled => scheduled,
                _ => deadline,
            }?;
            Some(Cell::Number(stamp.day().to_days() as f64))
        }),

        SortKey::Timestamp => entry.iter().find_map(|line| {
            let at = line.find('<')?;
            let stamp = parse_timestamp(&line[at..])?;
            Some(Cell::Number(stamp.day().to_days() as f64))
        }),

        SortKey::Property(name) => entry_property(entry, name).map(|value| Cell::read(&value)),
    }
}

/// A property's value, read from the entry's own drawer.
///
/// Inherited properties are not consulted: every sibling would inherit the
/// same value from the same parent, so sorting by one would do nothing.
pub(crate) fn entry_property(entry: &[String], name: &str) -> Option<String> {
    let mut inside = false;

    for line in entry {
        let trimmed = line.trim();

        if trimmed.eq_ignore_ascii_case(":PROPERTIES:") {
            inside = true;
            continue;
        }
        if trimmed.eq_ignore_ascii_case(":END:") {
            return None;
        }
        if !inside {
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix(':') {
            if let Some((key, value)) = rest.split_once(':') {
                if key.trim().eq_ignore_ascii_case(name) {
                    return Some(value.trim().to_string());
                }
            }
        }
    }

    None
}

/// The number a string starts with, ignoring what follows it.
fn leading_number(text: &str) -> Option<f64> {
    let text = text.trim_start();
    let digits = text
        .find(|c: char| !c.is_ascii_digit() && c != '-' && c != '+' && c != '.')
        .unwrap_or(text.len());

    text[..digits].parse().ok()
}

/// Splits `start..end` at every line `is_boundary` accepts.
///
/// Anything before the first boundary is not a sibling and is left out, so the
/// caller can keep it where it is.
fn ranges(
    lines: &[String],
    start: usize,
    end: usize,
    is_boundary: impl Fn(&str) -> bool,
) -> Vec<(usize, usize)> {
    let heads: Vec<usize> = (start..end).filter(|&at| is_boundary(&lines[at])).collect();

    heads
        .iter()
        .enumerate()
        .map(|(index, &head)| (head, heads.get(index + 1).copied().unwrap_or(end)))
        .collect()
}

/// Sorts the list items that are siblings of the one at the cursor.
///
/// An item carries its continuation lines and its sub-items with it, and the
/// list is renumbered afterwards so an ordered list stays in sequence.
pub fn sort_list(text: &str, line: usize, key: &SortKey, reverse: bool) -> Option<String> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let indent = parse_item(lines.get(line)?)?.indent;

    let (start, end) = list_region(&lines, line, indent);
    let items = ranges(&lines, start, end, |l| {
        parse_item(l).is_some_and(|item| item.indent == indent)
    });
    if items.len() < 2 {
        return None;
    }

    let blocks: Vec<_> = items
        .iter()
        .map(|&(at, to)| {
            let block = lines[at..to].to_vec();
            let value = parse_item(&lines[at]).and_then(|item| match key {
                SortKey::Numeric => leading_number(&item.content).map(Cell::Number),
                _ => Some(Cell::Text(item.content.to_lowercase())),
            });
            (value, block)
        })
        .collect();

    let first = items[0].0;
    let sorted: Vec<String> = reorder(blocks, reverse).concat();
    lines.splice(first..end, sorted);

    Some(renumber(&rejoin(&lines, text), first))
}

/// The run of lines the list at `indent` occupies around `line`.
///
/// A single blank line is bridged, because Org lets one separate two items;
/// anything more ends the list, and so does a line shallower than the item.
fn list_region(lines: &[String], line: usize, indent: usize) -> (usize, usize) {
    let belongs = |at: usize| {
        let text: &str = &lines[at];
        if text.trim().is_empty() {
            return false;
        }
        match parse_item(text) {
            Some(item) => item.indent >= indent,
            None => text.len() - text.trim_start().len() > indent,
        }
    };

    let mut start = line;
    while start > 0 {
        let previous = start - 1;
        // Look past one blank line, but only if something below it still
        // belongs to this list.
        let candidate = if lines[previous].trim().is_empty() {
            previous.checked_sub(1)
        } else {
            Some(previous)
        };

        match candidate.filter(|&at| belongs(at)) {
            Some(at) => start = at,
            None => break,
        }
    }

    let mut end = line + 1;
    while end < lines.len() {
        let candidate = if lines[end].trim().is_empty() {
            (end + 1 < lines.len()).then(|| end + 1)
        } else {
            Some(end)
        };

        match candidate.filter(|&at| belongs(at)) {
            Some(at) => end = at + 1,
            None => break,
        }
    }

    (start, end)
}

/// Sorts the table rows between the separators the cursor's row sits between.
///
/// Sorting a section rather than the whole table is what leaves a header
/// alone without having to guess which rows are one. Only [`SortKey::Alphabetical`],
/// [`SortKey::Numeric`] and [`SortKey::Timestamp`] mean anything in a cell;
/// the entry keys fall back to comparing it as text. A cell the key cannot
/// read — no number where one was asked for — sorts to the bottom of its
/// section, as a missing value does everywhere else here.
pub fn sort_table(
    text: &str,
    line: usize,
    column: usize,
    key: &SortKey,
    reverse: bool,
) -> Option<String> {
    let table = parse_table(text, line)?;
    let at = line.checked_sub(table.start)?;

    // A separator is a boundary, not a row, so there is nothing to sort from
    // one.
    if matches!(table.rows.get(at)?, Row::Separator) {
        return None;
    }

    let start = table.rows[..at]
        .iter()
        .rposition(|row| matches!(row, Row::Separator))
        .map_or(0, |sep| sep + 1);
    let end = table.rows[at..]
        .iter()
        .position(|row| matches!(row, Row::Separator))
        .map_or(table.rows.len(), |offset| at + offset);

    if end - start < 2 {
        return None;
    }

    let blocks: Vec<_> = table.rows[start..end]
        .iter()
        .map(|row| {
            let cell = match row {
                Row::Cells(cells) => cells.get(column).filter(|text| !text.trim().is_empty()),
                Row::Separator => None,
            };
            (cell.and_then(|text| cell_value(text, key)), row.clone())
        })
        .collect();

    let mut rows = table.rows.clone();
    rows.splice(start..end, reorder(blocks, reverse));

    Some(rewrite(text, &table, rows))
}

/// The value a key reads out of one table cell.
fn cell_value(text: &str, key: &SortKey) -> Option<Cell> {
    match key {
        SortKey::Numeric => leading_number(text).map(Cell::Number),
        SortKey::Timestamp => {
            parse_timestamp(text.trim()).map(|stamp| Cell::Number(stamp.day().to_days() as f64))
        }
        _ => Some(Cell::Text(text.trim().to_lowercase())),
    }
}
