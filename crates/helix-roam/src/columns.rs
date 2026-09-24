//! Column view: each headline's properties laid out as a table beside it.
//!
//! Org draws the table over the headlines with overlays. Helix has no
//! overlays, but it does have virtual text, so here each headline gets its
//! row appended after its own text: the headline is the `ITEM` column, and
//! the others follow it, padded into line. What a column shows comes from
//! `#+COLUMNS:`, in Org's format:
//!
//! ```org
//! #+COLUMNS: %25ITEM %TODO %Effort{:} %CLOCKSUM
//! ```
//!
//! A column with a summary (`{:}` adds durations, `{+}` numbers, `{min}` and
//! `{max}` pick one) shows, on a headline with children, the summary of
//! the subtree rather than the headline's own value — which is what makes a
//! column view answer "how much is left under this".

use crate::parser::{parse_headline, FileSettings};
use crate::restructure::headline_level;

/// How a column sums a subtree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Summary {
    /// `{+}`: numbers added.
    Sum,
    /// `{:}`: `H:MM` durations added.
    Time,
    /// `{min}`
    Min,
    /// `{max}`
    Max,
}

/// One column of the format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// The property, upper-cased: `ITEM`, `TODO`, `EFFORT`, …
    pub property: String,
    /// What the header shows: `%ITEM(Task)` gives `Task`.
    pub title: String,
    pub width: Option<usize>,
    pub summary: Option<Summary>,
}

/// Org's default, `org-columns-default-format`.
pub const DEFAULT_FORMAT: &str = "%25ITEM %TODO %3PRIORITY %TAGS";

/// Reads `%25ITEM %TODO %Effort(Estimate){:}`.
pub fn parse_format(format: &str) -> Vec<Column> {
    format
        .split_whitespace()
        .filter_map(|spec| {
            let spec = spec.strip_prefix('%')?;
            let digits = spec.chars().take_while(char::is_ascii_digit).count();
            let width = spec[..digits].parse().ok();
            let rest = &spec[digits..];

            let name_end = rest.find(['(', '{']).unwrap_or(rest.len());
            let property = rest[..name_end].to_ascii_uppercase();
            if property.is_empty() {
                return None;
            }
            let mut after = &rest[name_end..];

            let mut title = rest[..name_end].to_string();
            if let Some(inner) = after.strip_prefix('(') {
                let close = inner.find(')')?;
                title = inner[..close].to_string();
                after = &inner[close + 1..];
            }
            let summary = after
                .strip_prefix('{')
                .and_then(|inner| inner.strip_suffix('}'))
                .and_then(|kind| match kind {
                    "+" => Some(Summary::Sum),
                    ":" => Some(Summary::Time),
                    "min" => Some(Summary::Min),
                    "max" => Some(Summary::Max),
                    _ => None,
                });

            Some(Column {
                property,
                title,
                width,
                summary,
            })
        })
        .collect()
}

/// The format the file declares, or Org's default.
pub fn format_of(text: &str) -> Vec<Column> {
    let declared = text.lines().find_map(|line| {
        let rest = line.trim_start().strip_prefix("#+")?;
        let (key, value) = rest.split_once(':')?;
        key.eq_ignore_ascii_case("columns")
            .then(|| value.trim().to_string())
    });
    parse_format(declared.as_deref().unwrap_or(DEFAULT_FORMAT))
}

/// A headline's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub line: usize,
    pub level: usize,
    /// One cell per column, `ITEM` included.
    pub cells: Vec<String>,
}

/// The value a column has on the headline at `at`, before any summary.
fn own_value(
    column: &Column,
    lines: &[&str],
    at: usize,
    settings: &FileSettings,
    clocked: i64,
) -> String {
    let Some(headline) = parse_headline(lines[at], settings) else {
        return String::new();
    };
    match column.property.as_str() {
        "ITEM" => headline.title,
        "TODO" => headline.todo.map(|state| state.keyword).unwrap_or_default(),
        "PRIORITY" => headline.priority.map(String::from).unwrap_or_default(),
        "TAGS" | "ALLTAGS" if headline.tags.is_empty() => String::new(),
        "TAGS" | "ALLTAGS" => format!(":{}:", headline.tags.join(":")),
        "CLOCKSUM" if clocked > 0 => crate::clock::format_duration(clocked),
        "CLOCKSUM" => String::new(),
        property => drawer_value(lines, at, property).unwrap_or_default(),
    }
}

/// A property from the drawer under the headline at `at`.
fn drawer_value(lines: &[&str], at: usize, property: &str) -> Option<String> {
    let mut line = at + 1;
    if lines
        .get(line)
        .is_some_and(|l| crate::restructure::is_planning_line(l))
    {
        line += 1;
    }
    if !lines.get(line)?.trim().eq_ignore_ascii_case(":PROPERTIES:") {
        return None;
    }
    lines[line + 1..]
        .iter()
        .take_while(|l| !l.trim().eq_ignore_ascii_case(":END:"))
        .find_map(|l| {
            let (key, value) = l.trim().strip_prefix(':')?.split_once(':')?;
            key.eq_ignore_ascii_case(property)
                .then(|| value.trim().to_string())
        })
}

/// Minutes in `1:30`, `90` read as minutes being Org's `0:90`, or `2h`.
fn minutes(value: &str) -> Option<i64> {
    let value = value.trim();
    if let Some((hours, mins)) = value.split_once(':') {
        return Some(hours.parse::<i64>().ok()? * 60 + mins.parse::<i64>().ok()?);
    }
    if let Some(hours) = value.strip_suffix('h') {
        return Some((hours.parse::<f64>().ok()? * 60.0).round() as i64);
    }
    if let Some(mins) = value.strip_suffix("min") {
        return mins.trim().parse().ok();
    }
    None
}

fn number(value: &str) -> Option<f64> {
    value.trim().parse().ok()
}

fn format_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// Combines the values of a subtree the way `summary` asks.
fn summarise(summary: Summary, values: &[String]) -> String {
    match summary {
        Summary::Time => {
            let total: i64 = values.iter().filter_map(|v| minutes(v)).sum();
            if values.iter().all(|v| minutes(v).is_none()) {
                String::new()
            } else {
                crate::clock::format_duration(total)
            }
        }
        Summary::Sum | Summary::Min | Summary::Max => {
            let numbers: Vec<f64> = values.iter().filter_map(|v| number(v)).collect();
            if numbers.is_empty() {
                return String::new();
            }
            let value = match summary {
                Summary::Sum => numbers.iter().sum(),
                Summary::Min => numbers.iter().copied().fold(f64::INFINITY, f64::min),
                _ => numbers.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            };
            format_number(value)
        }
    }
}

/// Every headline's row. A summarised column on a headline with children
/// shows the summary of its subtree: its children's own values, and their
/// children's in turn.
pub fn rows(text: &str, columns: &[Column]) -> Vec<Row> {
    let settings = FileSettings::scan(text);
    let lines: Vec<&str> = text.lines().collect();
    let clocked = crate::clock::entry_times(text, None, None);

    let mut rows: Vec<Row> = lines
        .iter()
        .enumerate()
        .filter_map(|(at, line)| {
            let level = headline_level(line)?;
            let own = clocked
                .iter()
                .find(|entry| entry.line == at)
                .map_or(0, |entry| entry.total);
            Some(Row {
                line: at,
                level,
                cells: columns
                    .iter()
                    .map(|column| own_value(column, &lines, at, &settings, own))
                    .collect(),
            })
        })
        .collect();

    // Deepest first, so a child's summary is ready when its parent needs it.
    for index in (0..rows.len()).rev() {
        let level = rows[index].level;
        // The children are the shallowest rows of the subtree: a child two
        // levels down with nothing in between still counts as a child.
        let subtree: Vec<usize> = (index + 1..rows.len())
            .take_while(|&i| rows[i].level > level)
            .collect();
        let shallowest = subtree.iter().map(|&i| rows[i].level).min();
        let direct: Vec<usize> = subtree
            .into_iter()
            .filter(|&i| Some(rows[i].level) == shallowest)
            .collect();
        if direct.is_empty() {
            continue;
        }
        for (col, column) in columns.iter().enumerate() {
            // CLOCKSUM is already a subtree total.
            let Some(summary) = column.summary.filter(|_| column.property != "CLOCKSUM") else {
                continue;
            };
            let values: Vec<String> = direct.iter().map(|&i| rows[i].cells[col].clone()).collect();
            let summed = summarise(summary, &values);
            if !summed.is_empty() {
                rows[index].cells[col] = summed;
            }
        }
    }

    rows
}

/// The text each headline gets appended, by line, and the header naming the
/// columns. `widths[i]` is how wide the headline line itself is, so the
/// columns after it line up.
///
/// Widths are counted in characters, which lines up anything Latin; a
/// headline in a script with double-width characters will push its row out.
pub fn layout(text: &str, columns: &[Column]) -> (String, Vec<(usize, String)>) {
    let rows = rows(text, columns);
    let lines: Vec<&str> = text.lines().collect();
    let item = columns.iter().position(|column| column.property == "ITEM");

    let item_width = item.and_then(|i| columns[i].width).unwrap_or_else(|| {
        rows.iter()
            .map(|row| lines[row.line].trim_end().chars().count())
            .max()
            .unwrap_or(0)
    });
    let widths: Vec<usize> = columns
        .iter()
        .enumerate()
        .map(|(col, column)| {
            column.width.unwrap_or_else(|| {
                rows.iter()
                    .map(|row| row.cells[col].chars().count())
                    .chain(std::iter::once(column.title.chars().count()))
                    .max()
                    .unwrap_or(0)
            })
        })
        .collect();

    let cells = |values: &[String]| {
        columns
            .iter()
            .enumerate()
            .filter(|(col, _)| Some(*col) != item)
            .map(|(col, _)| {
                let value: String = values[col].chars().take(widths[col]).collect();
                format!("{value:<width$}", width = widths[col])
            })
            .collect::<Vec<_>>()
            .join(" │ ")
    };

    let annotations = rows
        .iter()
        .map(|row| {
            let shown = lines[row.line].trim_end().chars().count();
            let pad = item_width.saturating_sub(shown) + 1;
            (
                row.line,
                format!("{}│ {} │", " ".repeat(pad), cells(&row.cells)),
            )
        })
        .collect();

    let titles: Vec<String> = columns.iter().map(|column| column.title.clone()).collect();
    let header = cells(&titles);
    (header, annotations)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_are_read_as_org_writes_them() {
        let columns = parse_format("%25ITEM %TODO %Effort(Estimate){:} %3PRIORITY");
        assert_eq!(columns.len(), 4);
        assert_eq!(columns[0].width, Some(25));
        assert_eq!(columns[0].property, "ITEM");
        assert_eq!(columns[2].property, "EFFORT");
        assert_eq!(columns[2].title, "Estimate");
        assert_eq!(columns[2].summary, Some(Summary::Time));
        assert_eq!(columns[3].width, Some(3));
    }

    #[test]
    fn a_file_without_columns_uses_org_s_default() {
        assert_eq!(format_of("* A\n"), parse_format(DEFAULT_FORMAT));
        assert_eq!(format_of("#+columns: %ITEM %X\n")[1].property, "X");
    }

    const PLAN: &str = "\
#+COLUMNS: %ITEM %TODO %Effort{:} %Cost{+}
* Project
** TODO Design
:PROPERTIES:
:Effort: 1:30
:Cost: 100
:END:
** TODO Build
:PROPERTIES:
:Effort: 2:00
:Cost: 250.5
:END:
*** Part
:PROPERTIES:
:Effort: 0:45
:END:
* Other
";

    #[test]
    fn values_come_from_the_headline_and_its_drawer() {
        let columns = format_of(PLAN);
        let rows = rows(PLAN, &columns);
        let design = rows.iter().find(|r| r.cells[0] == "Design").unwrap();
        assert_eq!(design.cells, ["Design", "TODO", "1:30", "100"]);
    }

    #[test]
    fn summaries_add_up_the_children() {
        let columns = format_of(PLAN);
        let rows = rows(PLAN, &columns);
        let get = |title: &str| {
            rows.iter()
                .find(|r| r.cells[0] == title)
                .unwrap()
                .cells
                .clone()
        };

        // Build has a child, so its effort is its child's: 0:45.
        assert_eq!(get("Build")[2], "0:45");
        // Project sums its direct children's (already summarised) values.
        assert_eq!(get("Project")[2], "2:15");
        assert_eq!(get("Project")[3], "350.5");
        // Nothing below, nothing to sum.
        assert_eq!(get("Other")[2], "");
    }

    #[test]
    fn rows_line_up_after_their_headlines() {
        let text = "#+COLUMNS: %ITEM %TODO\n* A\n* TODO Longer one\n";
        let (header, annotations) = layout(text, &format_of(text));
        assert_eq!(header, "TODO");
        assert_eq!(annotations[0], (1, format!("{}│      │", " ".repeat(15))));
        assert_eq!(annotations[1], (2, " │ TODO │".to_string()));
    }

    #[test]
    fn clocked_time_is_a_column_too() {
        let text = "#+COLUMNS: %ITEM %CLOCKSUM\n* A\n:LOGBOOK:\nCLOCK: [2026-09-24 Thu 09:00]--[2026-09-24 Thu 10:15] =>  1:15\n:END:\n";
        let rows = rows(text, &format_of(text));
        assert_eq!(rows[0].cells[1], "1:15");
    }
}
