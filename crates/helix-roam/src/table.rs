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
    /// What each row starts with before its first `|`: the indentation, or
    /// in another language's file the comment the table lives in (`# `,
    /// `// `), which is Org's `orgtbl-mode`.
    pub prefix: String,
}

/// The comment starts a table may sit behind in another language's file.
const LEADERS: &[&str] = &["//", "#", "--", ";;", ";", "%", "*", "!", "'"];

/// What comes before a table line's first `|`, when the line is one: only
/// indentation, or a comment's start between indentation and spaces.
pub(crate) fn table_prefix(line: &str) -> Option<&str> {
    let pipe = line.find('|')?;
    let before = &line[..pipe];
    let leader = before.trim();
    if leader.is_empty() {
        return Some(before);
    }
    // The comment's start stands alone: `# |` is a table, `#+x |` is not.
    let lead_at = before.len() - before.trim_start().len();
    let after = &before[lead_at + leader.len()..];
    (LEADERS.contains(&leader) && !after.is_empty() && after.trim().is_empty()).then_some(before)
}

/// A line with its table prefix taken off, or as it is.
pub(crate) fn without_prefix(line: &str) -> &str {
    match table_prefix(line) {
        Some(prefix) => &line[prefix.len()..],
        None => line.trim_start(),
    }
}

/// A line without its indentation and the comment it may sit behind:
/// `# #+TBLFM: …` reads as `#+TBLFM: …`.
pub(crate) fn strip_leader(line: &str) -> &str {
    let trimmed = line.trim_start();
    for leader in LEADERS {
        if let Some(rest) = trimmed.strip_prefix(leader) {
            if rest.starts_with(char::is_whitespace) {
                return rest.trim_start();
            }
        }
    }
    trimmed
}

/// The comment a line is behind (`#`, `//`), or nothing for a plain line;
/// rows of one table share it.
fn leader(line: &str) -> Option<&str> {
    table_prefix(line).map(str::trim)
}

/// Whether a line belongs to a table.
pub(crate) fn is_table_line(line: &str) -> bool {
    table_prefix(line).is_some()
}

/// Whether a table line is a separator rather than cells.
fn is_separator(line: &str) -> bool {
    let trimmed = without_prefix(line);
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
    let trimmed = without_prefix(line).trim();
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

    // Rows of one table sit behind the same comment, or behind none.
    let kind = leader(lines[at]);
    let same = |line: &str| is_table_line(line) && leader(line) == kind;
    let mut start = at;
    while start > 0 && same(lines[start - 1]) {
        start -= 1;
    }
    let mut end = at + 1;
    while end < lines.len() && same(lines[end]) {
        end += 1;
    }

    let prefix = table_prefix(lines[start]).unwrap_or_default().to_string();
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
        prefix,
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
        let pad = &self.prefix;

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
pub(crate) fn rewrite(text: &str, table: &Table, rows: Vec<Row>) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let rebuilt = Table {
        rows,
        start: table.start,
        end: table.end,
        prefix: table.prefix.clone(),
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

/// Rows of CSV or TSV text. Without a `separator`, Org's guess: a tab when
/// the first line has one, else a comma when it has one, else runs of
/// spaces. Commas and tabs follow CSV's quoting: `"a, b"` is one field and
/// `""` inside quotes is a quote.
pub fn parse_delimited(text: &str, separator: Option<char>) -> Vec<Vec<String>> {
    let first = text.lines().next().unwrap_or("");
    let separator = separator.or(if first.contains('\t') {
        Some('\t')
    } else if first.contains(',') {
        Some(',')
    } else {
        None
    });
    let Some(separator) = separator else {
        return text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.split_whitespace().map(str::to_string).collect())
            .collect();
    };

    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' if quoted => quoted = false,
            '"' if field.is_empty() => quoted = true,
            c if c == separator && !quoted => row.push(std::mem::take(&mut field)),
            '\r' if !quoted => {}
            '\n' if !quoted => {
                row.push(std::mem::take(&mut field));
                if !(row.len() == 1 && row[0].is_empty()) {
                    rows.push(std::mem::take(&mut row));
                }
                row.clear();
            }
            c => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

/// A field as a table cell can hold it: on one line, its `|` written as
/// Org's `\vert{}`.
fn cell_text(field: &str) -> String {
    field
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('|', "\\vert{}")
}

/// An aligned table of `rows`, each line starting with `prefix`.
pub fn table_text(rows: &[Vec<String>], prefix: &str) -> String {
    let table = Table {
        rows: rows
            .iter()
            .map(|row| Row::Cells(row.iter().map(|field| cell_text(field)).collect()))
            .collect(),
        start: 0,
        end: 0,
        prefix: prefix.to_string(),
    };
    let mut text = table.render().join("\n");
    text.push('\n');
    text
}

/// An empty table of `columns` by `rows`, a separator under the first row,
/// as Org's `org-table-create` makes one.
pub fn create(columns: usize, rows: usize, prefix: &str) -> String {
    let columns = columns.max(1);
    let mut table = Table {
        rows: vec![Row::Cells(vec![String::new(); columns]); rows.max(1)],
        start: 0,
        end: 0,
        prefix: prefix.to_string(),
    };
    if rows > 1 {
        table.rows.insert(1, Row::Separator);
    }
    let mut text = table.render().join("\n");
    text.push('\n');
    text
}

/// The text of the field in column `column` (from zero) of the row at
/// `line`, or `None` when that is no field.
pub fn field_at(text: &str, line: usize, column: usize) -> Option<String> {
    let table = parse_table(text, line)?;
    match table.rows.get(line.checked_sub(table.start)?)? {
        Row::Cells(cells) => Some(cells.get(column).cloned().unwrap_or_default()),
        Row::Separator => None,
    }
}

/// The row nearest `line`, in the table there, whose field `column` still
/// reads `value`: where an edited field goes back to, rows above it having
/// perhaps moved it.
pub fn find_field(text: &str, line: usize, column: usize, value: &str) -> Option<usize> {
    let lines: Vec<&str> = text.lines().collect();
    let near = line.min(lines.len().checked_sub(1)?);
    // The table may have moved too: look outwards from where it was.
    let table = (0..lines.len())
        .flat_map(|d| [near.checked_sub(d), Some(near + d)])
        .flatten()
        .filter(|&at| at < lines.len())
        .find_map(|at| parse_table(text, at))?;
    (table.start..table.end)
        .filter(|&row| field_at(text, row, column).as_deref() == Some(value))
        .min_by_key(|row| row.abs_diff(line))
}

/// `text` with the field in column `column` of the row at `line` set to
/// `value`, made one line as a field must be, and the table realigned.
pub fn set_field(text: &str, line: usize, column: usize, value: &str) -> Option<String> {
    let table = parse_table(text, line)?;
    let at = line.checked_sub(table.start)?;
    let columns = table.columns();
    let mut rows = table.rows.clone();
    let Row::Cells(cells) = rows.get_mut(at)? else {
        return None;
    };
    cells.resize(columns.max(column + 1), String::new());
    cells[column] = cell_text(value);
    Some(rewrite(text, &table, rows))
}
