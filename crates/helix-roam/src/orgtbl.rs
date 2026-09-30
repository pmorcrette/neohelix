//! Tables sent into other syntaxes: Org's radio tables and the
//! `orgtbl-to-…` translators behind them.
//!
//! A table in another language's file (a LaTeX document, an HTML page, a
//! program) is edited as an Org table, usually inside a comment, and sent
//! into the place that reads it:
//!
//! ```text
//! % BEGIN RECEIVE ORGTBL sales
//! % END RECEIVE ORGTBL sales
//! \begin{comment}
//! #+ORGTBL: SEND sales orgtbl-to-latex :splice nil
//! | Month | Days |
//! |-------+------|
//! | Jan   |   23 |
//! \end{comment}
//! ```
//!
//! Sending writes the translated table between the two `RECEIVE` lines,
//! whatever comment they are in. The translators are also what exporting a
//! table to a file uses (`:TABLE_EXPORT_FORMAT: orgtbl-to-csv`).

use crate::table::{parse_table, strip_leader, Row, Table};

/// A translator and its parameters: `orgtbl-to-latex :splice t :skip 1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Translator {
    pub kind: Kind,
    pub splice: bool,
    /// Table lines left out from the top, separators counting.
    pub skip: usize,
    /// Columns left out, from 1.
    pub skipcols: Vec<usize>,
    /// What a separator becomes: `None` for the translator's own, and
    /// `Some(None)` (`:hline nil`) to leave separators out.
    pub hline: Option<Option<String>>,
    pub lstart: Option<String>,
    pub lend: Option<String>,
    pub sep: Option<String>,
    pub tstart: Option<String>,
    pub tend: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Latex,
    Html,
    Csv,
    Tsv,
    Texinfo,
    Orgtbl,
    Generic,
}

impl Kind {
    fn from_name(name: &str) -> Option<Kind> {
        let name = name.strip_prefix("orgtbl-to-").unwrap_or(name);
        Some(match name {
            "latex" => Kind::Latex,
            "html" => Kind::Html,
            "csv" => Kind::Csv,
            "tsv" => Kind::Tsv,
            "texinfo" => Kind::Texinfo,
            "orgtbl" | "org" => Kind::Orgtbl,
            "generic" => Kind::Generic,
            _ => return None,
        })
    }

    /// The kind a file's extension suggests, for exporting to it.
    pub fn from_extension(extension: &str) -> Option<Kind> {
        Some(match extension.to_ascii_lowercase().as_str() {
            "csv" => Kind::Csv,
            "tsv" | "txt" | "tab" => Kind::Tsv,
            "tex" | "latex" => Kind::Latex,
            "html" | "htm" => Kind::Html,
            "texi" | "texinfo" => Kind::Texinfo,
            "org" => Kind::Orgtbl,
            _ => return None,
        })
    }
}

impl Translator {
    pub fn new(kind: Kind) -> Translator {
        Translator {
            kind,
            splice: false,
            skip: 0,
            skipcols: Vec::new(),
            hline: None,
            lstart: None,
            lend: None,
            sep: None,
            tstart: None,
            tend: None,
        }
    }

    /// Reads `orgtbl-to-latex :splice t :skip 1 :hline "\\hline"`.
    pub fn parse(spec: &str) -> Result<Translator, String> {
        let words = plist_words(spec);
        let mut words = words.into_iter();
        let name = words.next().ok_or("no translator named")?;
        let kind = Kind::from_name(&name).ok_or(format!("unknown translator `{name}`"))?;
        let mut translator = Translator::new(kind);
        while let Some(key) = words.next() {
            let value = words.next().ok_or_else(|| format!("{key} has no value"))?;
            let text = || (value != "nil").then(|| value.clone());
            match key.as_str() {
                ":splice" => translator.splice = value != "nil",
                ":skip" => {
                    translator.skip = value
                        .parse()
                        .map_err(|_| format!(":skip {value} is not a number"))?
                }
                ":skipcols" => {
                    translator.skipcols = value
                        .trim_matches(|c| c == '(' || c == ')')
                        .split_whitespace()
                        .map(|n| n.parse().map_err(|_| format!(":skipcols has `{n}`")))
                        .collect::<Result<_, _>>()?
                }
                ":hline" => translator.hline = Some(text()),
                ":lstart" => translator.lstart = text(),
                ":lend" => translator.lend = text(),
                ":sep" => translator.sep = text(),
                ":tstart" => translator.tstart = text(),
                ":tend" => translator.tend = text(),
                other => return Err(format!("unknown parameter {other}")),
            }
        }
        Ok(translator)
    }

    /// The table's rows translated, one string per output line.
    pub fn translate(&self, table: &Table) -> String {
        let rows = self.rows(table);
        let lines = match self.kind {
            Kind::Latex => self.latex(&rows),
            Kind::Html => self.html(&rows),
            Kind::Csv => self.delimited(&rows, ","),
            Kind::Tsv => self.delimited(&rows, "\t"),
            Kind::Texinfo => self.texinfo(&rows),
            Kind::Orgtbl => self.orgtbl(&rows),
            Kind::Generic => self.generic(&rows),
        };
        let mut out = lines.join("\n");
        out.push('\n');
        out
    }

    /// The rows left once `:skip` and `:skipcols` have taken theirs, cells
    /// filled to the table's width.
    fn rows(&self, table: &Table) -> Vec<Row> {
        let columns = table.columns();
        table
            .rows
            .iter()
            .skip(self.skip)
            .map(|row| match row {
                Row::Separator => Row::Separator,
                Row::Cells(cells) => {
                    let mut cells = cells.clone();
                    cells.resize(columns, String::new());
                    Row::Cells(
                        cells
                            .into_iter()
                            .enumerate()
                            .filter(|(at, _)| !self.skipcols.contains(&(at + 1)))
                            .map(|(_, cell)| cell)
                            .collect(),
                    )
                }
            })
            .collect()
    }

    fn latex(&self, rows: &[Row]) -> Vec<String> {
        let mut out = Vec::new();
        if !self.splice {
            let align: String = (0..width(rows))
                .map(|column| if is_numeric(rows, column) { 'r' } else { 'l' })
                .collect();
            out.push(
                self.tstart
                    .clone()
                    .unwrap_or(format!("\\begin{{tabular}}{{{align}}}")),
            );
        }
        let hline = match &self.hline {
            None => Some("\\hline".to_string()),
            Some(hline) => hline.clone(),
        };
        let sep = self.sep.clone().unwrap_or(" & ".to_string());
        let lend = self.lend.clone().unwrap_or("\\\\".to_string());
        let lstart = self.lstart.clone().unwrap_or_default();
        for row in trim_hlines(rows, self.splice) {
            match row {
                Row::Separator => out.extend(hline.clone()),
                Row::Cells(cells) => {
                    let cells: Vec<String> = cells.iter().map(|c| latex_escape(c)).collect();
                    out.push(format!("{lstart}{}{lend}", cells.join(&sep)));
                }
            }
        }
        if !self.splice {
            out.push(self.tend.clone().unwrap_or("\\end{tabular}".to_string()));
        }
        out
    }

    fn html(&self, rows: &[Row]) -> Vec<String> {
        let mut out = Vec::new();
        if !self.splice {
            out.push(self.tstart.clone().unwrap_or("<table>".to_string()));
        }
        // Rows above the first separator are the header, when rows follow it.
        let header_end = rows
            .iter()
            .position(|row| matches!(row, Row::Separator))
            .filter(|&at| rows[at..].iter().any(|row| matches!(row, Row::Cells(_))))
            .unwrap_or(0);
        let (head, body) = rows.split_at(header_end);
        let group = |out: &mut Vec<String>, rows: &[Row], tag: &str, cell: &str| {
            let cells: Vec<&Vec<String>> = rows
                .iter()
                .filter_map(|row| match row {
                    Row::Cells(cells) => Some(cells),
                    Row::Separator => None,
                })
                .collect();
            if cells.is_empty() {
                return;
            }
            out.push(format!("<{tag}>"));
            for cells in cells {
                let inner: String = cells
                    .iter()
                    .map(|c| format!("<{cell}>{}</{cell}>", html_escape(c)))
                    .collect();
                out.push(format!("<tr>{inner}</tr>"));
            }
            out.push(format!("</{tag}>"));
        };
        group(&mut out, head, "thead", "th");
        group(&mut out, body, "tbody", "td");
        if !self.splice {
            out.push(self.tend.clone().unwrap_or("</table>".to_string()));
        }
        out
    }

    fn delimited(&self, rows: &[Row], default_sep: &str) -> Vec<String> {
        let sep = self.sep.clone().unwrap_or(default_sep.to_string());
        rows.iter()
            .filter_map(|row| match row {
                Row::Separator => self.hline.clone().flatten(),
                Row::Cells(cells) => Some(
                    cells
                        .iter()
                        .map(|cell| {
                            if self.kind == Kind::Csv {
                                csv_quote(cell)
                            } else {
                                cell.clone()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(&sep),
                ),
            })
            .collect()
    }

    fn texinfo(&self, rows: &[Row]) -> Vec<String> {
        let mut out = Vec::new();
        let columns = width(rows).max(1);
        if !self.splice {
            let fraction = format!("{:.2}", 1.0 / columns as f64);
            out.push(format!(
                "@multitable @columnfractions {}",
                vec![fraction.as_str(); columns].join(" ")
            ));
        }
        let header_end = rows
            .iter()
            .position(|row| matches!(row, Row::Separator))
            .unwrap_or(0);
        for (at, row) in rows.iter().enumerate() {
            if let Row::Cells(cells) = row {
                let command = if at < header_end {
                    "@headitem"
                } else {
                    "@item"
                };
                let cells: Vec<String> = cells.iter().map(|c| texinfo_escape(c)).collect();
                out.push(format!("{command} {}", cells.join(" @tab ")));
            }
        }
        if !self.splice {
            out.push("@end multitable".to_string());
        }
        out
    }

    fn orgtbl(&self, rows: &[Row]) -> Vec<String> {
        Table {
            rows: rows.to_vec(),
            start: 0,
            end: 0,
            prefix: String::new(),
        }
        .render()
    }

    fn generic(&self, rows: &[Row]) -> Vec<String> {
        let mut out = Vec::new();
        if let (false, Some(start)) = (self.splice, &self.tstart) {
            out.push(start.clone());
        }
        let sep = self.sep.clone().unwrap_or("\t".to_string());
        for row in rows {
            match row {
                Row::Separator => out.extend(self.hline.clone().flatten()),
                Row::Cells(cells) => out.push(format!(
                    "{}{}{}",
                    self.lstart.clone().unwrap_or_default(),
                    cells.join(&sep),
                    self.lend.clone().unwrap_or_default()
                )),
            }
        }
        if let (false, Some(end)) = (self.splice, &self.tend) {
            out.push(end.clone());
        }
        out
    }
}

/// Words of a translator spec, quoted strings kept whole and unescaped,
/// parenthesised lists kept whole.
fn plist_words(spec: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut chars = spec.trim().chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut word = String::new();
        if c == '"' {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => {
                        if let Some(next) = chars.next() {
                            word.push(match next {
                                'n' => '\n',
                                't' => '\t',
                                other => other,
                            });
                        }
                    }
                    '"' => break,
                    c => word.push(c),
                }
            }
        } else if c == '(' {
            for c in chars.by_ref() {
                word.push(c);
                if c == ')' {
                    break;
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                word.push(c);
                chars.next();
            }
        }
        words.push(word);
    }
    words
}

fn width(rows: &[Row]) -> usize {
    rows.iter()
        .filter_map(|row| match row {
            Row::Cells(cells) => Some(cells.len()),
            Row::Separator => None,
        })
        .max()
        .unwrap_or(0)
}

/// Whether a column's body holds only numbers, as Org decides alignment.
fn is_numeric(rows: &[Row], column: usize) -> bool {
    let body = match rows.iter().position(|row| matches!(row, Row::Separator)) {
        Some(at) if at + 1 < rows.len() => &rows[at + 1..],
        _ => rows,
    };
    let mut seen = false;
    for row in body {
        if let Row::Cells(cells) = row {
            match cells.get(column).map(|c| c.trim()) {
                Some("") | None => {}
                Some(cell) if cell.parse::<f64>().is_ok() => seen = true,
                Some(_) => return false,
            }
        }
    }
    seen
}

/// Separators at the very top or bottom say nothing inside a table's own
/// rules; a spliced table keeps them all.
fn trim_hlines(rows: &[Row], splice: bool) -> &[Row] {
    if splice {
        return rows;
    }
    let first = rows
        .iter()
        .position(|row| matches!(row, Row::Cells(_)))
        .unwrap_or(rows.len());
    let last = rows
        .iter()
        .rposition(|row| matches!(row, Row::Cells(_)))
        .map_or(first, |at| at + 1);
    &rows[first..last.max(first)]
}

fn latex_escape(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '&' | '%' | '$' | '#' | '_' | '{' | '}' => {
                out.push('\\');
                out.push(c);
            }
            '~' => out.push_str("\\textasciitilde{}"),
            '^' => out.push_str("\\textasciicircum{}"),
            '\\' => out.push_str("\\textbackslash{}"),
            c => out.push(c),
        }
    }
    out
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn texinfo_escape(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if matches!(c, '@' | '{' | '}') {
            out.push('@');
        }
        out.push(c);
    }
    out
}

/// A CSV field, quoted when it holds a comma, a quote or a line break.
pub fn csv_quote(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

/// `#+ORGTBL: SEND name translator params`, possibly behind a comment.
fn send_line(line: &str) -> Option<(&str, &str)> {
    let rest = strip_leader(line);
    let keyword = rest.get(..9)?;
    if !keyword.eq_ignore_ascii_case("#+ORGTBL:") {
        return None;
    }
    let rest = rest[9..].trim_start();
    let rest = rest
        .get(..4)
        .filter(|send| send.eq_ignore_ascii_case("SEND"))
        .map(|_| rest[4..].trim_start())?;
    let (name, spec) = rest.split_once(char::is_whitespace)?;
    Some((name, spec.trim()))
}

/// Whether `line` is the `BEGIN` or `END RECEIVE ORGTBL name` marker.
fn receive_marker(line: &str, which: &str, name: &str) -> bool {
    let wanted = format!("{which} RECEIVE ORGTBL");
    line.find(&wanted).is_some_and(|at| {
        let rest = line[at + wanted.len()..].trim_start();
        rest.split_whitespace().next() == Some(name)
    })
}

/// What sending a table did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    pub text: String,
    pub name: String,
    /// How many places received it.
    pub receivers: usize,
}

/// Sends the table at `line` to its receivers: the text between every
/// `BEGIN RECEIVE ORGTBL name` and the `END RECEIVE ORGTBL name` after it.
pub fn send(text: &str, line: usize) -> Result<Sent, String> {
    let table = parse_table(text, line).ok_or("No table at the cursor")?;
    let lines: Vec<&str> = text.lines().collect();
    let (name, spec) = table
        .start
        .checked_sub(1)
        .and_then(|above| send_line(lines[above]))
        .ok_or("The table has no #+ORGTBL: SEND line above it")?;
    let translator = Translator::parse(spec)?;
    let translated = translator.translate(&table);
    let translated: Vec<&str> = translated.lines().collect();

    let mut out: Vec<String> = Vec::new();
    let mut receivers = 0;
    let mut at = 0;
    while at < lines.len() {
        out.push(lines[at].to_string());
        if receive_marker(lines[at], "BEGIN", name) {
            let end = (at + 1..lines.len()).find(|&i| receive_marker(lines[i], "END", name));
            let Some(end) = end else {
                return Err(format!("BEGIN RECEIVE ORGTBL {name} has no END"));
            };
            // What is received is replaced: never the table sent.
            if at < table.end && end >= table.start {
                return Err(format!("RECEIVE ORGTBL {name} encloses its own table"));
            }
            out.extend(translated.iter().map(|line| line.to_string()));
            out.push(lines[end].to_string());
            receivers += 1;
            at = end + 1;
            continue;
        }
        at += 1;
    }
    if receivers == 0 {
        return Err(format!(
            "Nothing receives {name}: add BEGIN/END RECEIVE ORGTBL {name}"
        ));
    }
    let mut text_out = out.join("\n");
    if text.ends_with('\n') {
        text_out.push('\n');
    }
    Ok(Sent {
        text: text_out,
        name: name.to_string(),
        receivers,
    })
}

/// A radio table's skeleton for a file of `language`: the receiving
/// markers, and the table to send in a comment, as Org's
/// `orgtbl-insert-radio-table` writes it.
pub fn radio_skeleton(name: &str, language: &str) -> String {
    let (open, close, translator) = match language {
        "latex" | "tex" => ("\\begin{comment}", "\\end{comment}", "orgtbl-to-latex"),
        "html" => ("<!--", "-->", "orgtbl-to-html"),
        "c" | "cpp" | "java" | "javascript" | "typescript" | "rust" | "go" | "css" => {
            ("/*", "*/", "orgtbl-to-csv")
        }
        _ => ("", "", "orgtbl-to-tsv"),
    };
    let marker = |which: &str| match language {
        "latex" | "tex" => format!("% {which} RECEIVE ORGTBL {name}"),
        "html" => format!("<!-- {which} RECEIVE ORGTBL {name} -->"),
        "c" | "cpp" | "java" | "javascript" | "typescript" | "rust" | "go" => {
            format!("// {which} RECEIVE ORGTBL {name}")
        }
        "css" => format!("/* {which} RECEIVE ORGTBL {name} */"),
        _ => format!("# {which} RECEIVE ORGTBL {name}"),
    };
    let mut out = vec![marker("BEGIN"), marker("END")];
    if !open.is_empty() {
        out.push(open.to_string());
    }
    out.push(format!(
        "#+ORGTBL: SEND {name} {translator} :splice nil :skip 0"
    ));
    out.push("| | |".to_string());
    if !close.is_empty() {
        out.push(close.to_string());
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "\
% BEGIN RECEIVE ORGTBL sales
old
% END RECEIVE ORGTBL sales
\\begin{comment}
#+ORGTBL: SEND sales orgtbl-to-latex
| Month | Days |
|-------+------|
| Jan   |   23 |
| Feb_2 |   21 |
\\end{comment}
";

    #[test]
    fn sends_latex_between_the_markers() {
        let sent = send(DOC, 6).unwrap();
        assert_eq!(sent.receivers, 1);
        assert!(
            sent.text.starts_with(
                "% BEGIN RECEIVE ORGTBL sales\n\\begin{tabular}{lr}\nMonth & Days\\\\\n\\hline\n\
                 Jan & 23\\\\\nFeb\\_2 & 21\\\\\n\\end{tabular}\n% END RECEIVE ORGTBL sales\n"
            ),
            "{}",
            sent.text
        );
        // Sending again replaces what the first send wrote.
        let again = send(&sent.text, 11).unwrap();
        assert_eq!(again.text, sent.text);
    }

    #[test]
    fn translators_and_their_parameters() {
        let table = parse_table("| a | b,c |\n|---+---|\n| 1 | \"x\" |\n", 0).unwrap();
        let csv = Translator::parse("orgtbl-to-csv")
            .unwrap()
            .translate(&table);
        assert_eq!(csv, "a,\"b,c\"\n1,\"\"\"x\"\"\"\n");
        let tsv = Translator::parse("orgtbl-to-tsv :skip 2")
            .unwrap()
            .translate(&table);
        assert_eq!(tsv, "1\t\"x\"\n");
        let html = Translator::parse("orgtbl-to-html")
            .unwrap()
            .translate(&table);
        assert_eq!(
            html,
            "<table>\n<thead>\n<tr><th>a</th><th>b,c</th></tr>\n</thead>\n<tbody>\n\
             <tr><td>1</td><td>\"x\"</td></tr>\n</tbody>\n</table>\n"
        );
        let spliced = Translator::parse("orgtbl-to-latex :splice t :skipcols (2) :hline nil")
            .unwrap()
            .translate(&table);
        assert_eq!(spliced, "a\\\\\n1\\\\\n");
        let generic = Translator::parse(
            "orgtbl-to-generic :lstart \"- \" :sep \" / \" :hline \"--\" :tstart \"<<\" :tend \">>\"",
        )
        .unwrap()
        .translate(&table);
        assert_eq!(generic, "<<\n- a / b,c\n--\n- 1 / \"x\"\n>>\n");
        assert!(Translator::parse("orgtbl-to-nothing").is_err());
    }

    #[test]
    fn a_table_behind_line_comments_sends_too() {
        let text = "# BEGIN RECEIVE ORGTBL t\n# END RECEIVE ORGTBL t\n# #+ORGTBL: SEND t orgtbl-to-csv\n# | x | y |\n# | 1 | 2 |\n";
        let sent = send(text, 4).unwrap();
        assert_eq!(
            sent.text,
            "# BEGIN RECEIVE ORGTBL t\nx,y\n1,2\n# END RECEIVE ORGTBL t\n# #+ORGTBL: SEND t orgtbl-to-csv\n# | x | y |\n# | 1 | 2 |\n"
        );
        assert!(send("| a |\n", 0).is_err());
        let enclosing = "# BEGIN RECEIVE ORGTBL t\n# #+ORGTBL: SEND t orgtbl-to-csv\n# | x |\n# END RECEIVE ORGTBL t\n";
        assert!(send(enclosing, 2).unwrap_err().contains("encloses"));
    }
}
