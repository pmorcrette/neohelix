//! Org tables.
//!
//! Aligning is the operation everything else depends on: inserting a row or a
//! column produces a ragged table, and it is the realignment that makes the
//! result readable. So the module is built around it.

use unicode_width::UnicodeWidthStr;

/// A row of a table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// `| a | b |`
    Cells(Vec<String>),
    /// `|---+---|`
    Separator,
}

/// A table found in a buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub rows: Vec<Row>,
    /// Line the table starts on.
    pub start: usize,
    /// Line after its last row.
    pub end: usize,
    /// Columns of indentation the table sits at.
    pub indent: usize,
}

/// Whether a line belongs to a table.
fn is_table_line(line: &str) -> bool {
    line.trim_start().starts_with('|')
}

/// Whether a table line is a separator rather than cells.
fn is_separator(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("|-")
        || (trimmed.starts_with('|')
            && trimmed
                .trim_matches(|c| c == '|')
                .chars()
                .all(|c| matches!(c, '-' | '+' | '|')))
            && trimmed.contains('-')
}

/// Splits `| a | b |` into its cells.
fn split_cells(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let inner = trimmed
        .strip_prefix('|')
        .and_then(|rest| rest.strip_suffix('|'))
        .unwrap_or(trimmed.strip_prefix('|').unwrap_or(trimmed));

    inner
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

/// Reads the table containing `line`, if there is one.
pub fn parse_table(text: &str, line: usize) -> Option<Table> {
    let lines: Vec<&str> = text.lines().collect();
    let at = line.min(lines.len().saturating_sub(1));
    if !is_table_line(lines.get(at)?) {
        return None;
    }

    let mut start = at;
    while start > 0 && is_table_line(lines[start - 1]) {
        start -= 1;
    }
    let mut end = at + 1;
    while end < lines.len() && is_table_line(lines[end]) {
        end += 1;
    }

    let indent = lines[start].len() - lines[start].trim_start().len();
    let rows = lines[start..end]
        .iter()
        .map(|line| {
            if is_separator(line) {
                Row::Separator
            } else {
                Row::Cells(split_cells(line))
            }
        })
        .collect();

    Some(Table {
        rows,
        start,
        end,
        indent,
    })
}

impl Table {
    /// How many columns the widest row has.
    pub fn columns(&self) -> usize {
        self.rows
            .iter()
            .filter_map(|row| match row {
                Row::Cells(cells) => Some(cells.len()),
                Row::Separator => None,
            })
            .max()
            .unwrap_or(0)
    }

    /// Display width of each column, from its widest cell.
    fn widths(&self) -> Vec<usize> {
        // Zero, not one: a column whose cells are all empty renders as `|  |`
        // the way Org writes it, rather than gaining a space it never had.
        let mut widths = vec![0; self.columns()];
        for row in &self.rows {
            if let Row::Cells(cells) = row {
                for (index, cell) in cells.iter().enumerate() {
                    widths[index] = widths[index].max(cell.width());
                }
            }
        }
        widths
    }

    /// Whether a column holds only numbers, and so is right-aligned.
    ///
    /// Org's rule, and the reason a column of figures lines up on its digits.
    /// A column with no values at all is not numeric: there is nothing to
    /// suggest it.
    fn is_numeric(&self, column: usize) -> bool {
        // Everything above the first separator is a header: its labels are
        // words by nature, and counting them would make every numeric column
        // look textual.
        let body = match self
            .rows
            .iter()
            .position(|row| matches!(row, Row::Separator))
        {
            Some(at) => &self.rows[at + 1..],
            None => &self.rows[..],
        };

        let mut seen = false;
        for row in body {
            let Row::Cells(cells) = row else { continue };
            let Some(cell) = cells.get(column) else {
                continue;
            };
            if cell.is_empty() {
                continue;
            }
            seen = true;
            if cell.parse::<f64>().is_err() {
                return false;
            }
        }
        seen
    }

    /// Renders the table back, aligned.
    pub fn render(&self) -> Vec<String> {
        let widths = self.widths();
        let numeric: Vec<bool> = (0..widths.len()).map(|c| self.is_numeric(c)).collect();
        let pad = " ".repeat(self.indent);

        self.rows
            .iter()
            .map(|row| match row {
                Row::Separator => {
                    let middle = widths
                        .iter()
                        .map(|width| "-".repeat(width + 2))
                        .collect::<Vec<_>>()
                        .join("+");
                    format!("{pad}|{middle}|")
                }
                Row::Cells(cells) => {
                    let middle = widths
                        .iter()
                        .enumerate()
                        .map(|(index, width)| {
                            let cell = cells.get(index).map(String::as_str).unwrap_or("");
                            let padding = width.saturating_sub(cell.width());
                            if numeric[index] {
                                format!(" {}{cell} ", " ".repeat(padding))
                            } else {
                                format!(" {cell}{} ", " ".repeat(padding))
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("|");
                    format!("{pad}|{middle}|")
                }
            })
            .collect()
    }
}

/// Replaces the table containing `line` with `rows`.
fn rewrite(text: &str, table: &Table, rows: Vec<Row>) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let rebuilt = Table {
        rows,
        start: table.start,
        end: table.end,
        indent: table.indent,
    };

    lines.splice(table.start..table.end, rebuilt.render());

    let joined = lines.join("\n");
    if text.ends_with('\n') && !joined.is_empty() {
        format!("{joined}\n")
    } else {
        joined
    }
}

/// Realigns the table containing `line`.
pub fn align(text: &str, line: usize) -> Option<String> {
    let table = parse_table(text, line)?;
    let rows = table.rows.clone();
    Some(rewrite(text, &table, rows))
}

/// Inserts an empty row below the one at `line`.
pub fn insert_row(text: &str, line: usize) -> Option<String> {
    let table = parse_table(text, line)?;
    let mut rows = table.rows.clone();
    let at = (line - table.start + 1).min(rows.len());

    rows.insert(at, Row::Cells(vec![String::new(); table.columns()]));
    Some(rewrite(text, &table, rows))
}

/// Inserts a separator below the row at `line`.
pub fn insert_separator(text: &str, line: usize) -> Option<String> {
    let table = parse_table(text, line)?;
    let mut rows = table.rows.clone();
    let at = (line - table.start + 1).min(rows.len());

    rows.insert(at, Row::Separator);
    Some(rewrite(text, &table, rows))
}

/// Removes the row at `line`.
pub fn delete_row(text: &str, line: usize) -> Option<String> {
    let table = parse_table(text, line)?;
    let at = line.checked_sub(table.start)?;
    if at >= table.rows.len() {
        return None;
    }

    let mut rows = table.rows.clone();
    rows.remove(at);
    // A table with no rows left is no table at all.
    if rows.iter().all(|row| matches!(row, Row::Separator)) {
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        lines.drain(table.start..table.end);
        let joined = lines.join("\n");
        return Some(if text.ends_with('\n') && !joined.is_empty() {
            format!("{joined}\n")
        } else {
            joined
        });
    }

    Some(rewrite(text, &table, rows))
}

/// Inserts an empty column at `column`, zero-based.
pub fn insert_column(text: &str, line: usize, column: usize) -> Option<String> {
    let table = parse_table(text, line)?;
    let at = column.min(table.columns());

    let rows = table
        .rows
        .iter()
        .map(|row| match row {
            Row::Separator => Row::Separator,
            Row::Cells(cells) => {
                let mut cells = cells.clone();
                cells.resize(table.columns(), String::new());
                cells.insert(at, String::new());
                Row::Cells(cells)
            }
        })
        .collect();

    Some(rewrite(text, &table, rows))
}

/// Removes the column at `column`.
pub fn delete_column(text: &str, line: usize, column: usize) -> Option<String> {
    let table = parse_table(text, line)?;
    if column >= table.columns() {
        return None;
    }

    let rows = table
        .rows
        .iter()
        .map(|row| match row {
            Row::Separator => Row::Separator,
            Row::Cells(cells) => {
                let mut cells = cells.clone();
                if column < cells.len() {
                    cells.remove(column);
                }
                Row::Cells(cells)
            }
        })
        .collect();

    Some(rewrite(text, &table, rows))
}

/// Which column a byte offset within a table line falls in.
///
/// Used to act on "the column the cursor is in" without the caller counting
/// pipes itself.
pub fn column_at(line: &str, byte: usize) -> usize {
    line[..byte.min(line.len())]
        .chars()
        .filter(|c| *c == '|')
        .count()
        .saturating_sub(1)
}
