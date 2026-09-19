//! Dynamic blocks: a region whose contents are written by a named function.
//!
//! ```org
//! #+BEGIN: columnview :maxlevel 3 :columns "ITEM TODO EFFORT"
//! | Item      | Todo | Effort |
//! |-----------+------+--------|
//! | Write it  | TODO | 2h     |
//! #+END:
//! ```
//!
//! Everything between the two lines is output: refreshing a block throws it
//! away and asks the named generator for it again. The generator is supplied
//! by the caller rather than looked up here, so the library stays a text
//! transformation and the editor decides what names mean.
//!
//! The block syntax here was written from the form Org uses rather than from
//! Org's own reader, which was not reachable when this was written. If a
//! block in the wild is not recognised, that is the first thing to check.

use crate::parser::{parse_headline, FileSettings};
use crate::restructure::{headline_level, rejoin, subtree_range};
use crate::table::{Row, Table};

/// A `#+BEGIN:` … `#+END:` pair and what its opening line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicBlock {
    /// The generator's name, as written after `#+BEGIN:`.
    pub name: String,
    /// `:key value` pairs, in the order they were written.
    pub params: Vec<(String, String)>,
    /// Line the `#+BEGIN:` sits on.
    pub start: usize,
    /// Line the `#+END:` sits on.
    pub end: usize,
    /// Columns of indentation the block sits at.
    pub indent: usize,
}

impl DynamicBlock {
    /// A parameter's value, or `None` when the block does not give it.
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.as_str())
    }

    /// A parameter read as a number.
    pub fn number(&self, key: &str) -> Option<usize> {
        self.param(key)?.trim().parse().ok()
    }
}

/// Reads the `#+BEGIN:` line, if the line is one.
fn parse_begin(line: &str) -> Option<(String, Vec<(String, String)>)> {
    let trimmed = line.trim_start();
    let rest = trimmed
        .strip_prefix("#+BEGIN:")
        .or_else(|| trimmed.strip_prefix("#+begin:"))?;

    let mut tokens = split_tokens(rest).into_iter();
    let name = tokens.next()?;

    let mut params: Vec<(String, String)> = Vec::new();
    for token in tokens {
        match token.strip_prefix(':') {
            // `:flag` with nothing after it is still an answer: the parameter
            // is present, and its value is empty.
            Some(key) if !key.is_empty() => params.push((key.to_string(), String::new())),
            _ => {
                if let Some((_, value)) = params.last_mut() {
                    if !value.is_empty() {
                        value.push(' ');
                    }
                    value.push_str(&token);
                }
            }
        }
    }

    Some((name, params))
}

/// Whether the line closes a dynamic block.
fn is_end(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.eq_ignore_ascii_case("#+END:") || trimmed.eq_ignore_ascii_case("#+END")
}

/// Splits on whitespace, keeping a `"quoted value"` in one piece.
fn split_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;

    for c in text.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Every dynamic block in the text, in the order they appear.
pub fn blocks(text: &str) -> Vec<DynamicBlock> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    let mut at = 0;

    while at < lines.len() {
        let Some((name, params)) = parse_begin(lines[at]) else {
            at += 1;
            continue;
        };

        // An unclosed block is not a block: without an end there is nothing to
        // replace, and treating the rest of the file as its output would be a
        // destructive reading of a typo.
        let Some(offset) = lines[at + 1..].iter().position(|line| is_end(line)) else {
            at += 1;
            continue;
        };
        let end = at + 1 + offset;

        found.push(DynamicBlock {
            name,
            params,
            start: at,
            end,
            indent: lines[at].len() - lines[at].trim_start().len(),
        });
        at = end + 1;
    }

    found
}

/// The dynamic block containing `line`, its two delimiters included.
pub fn block_at(text: &str, line: usize) -> Option<DynamicBlock> {
    blocks(text)
        .into_iter()
        .find(|block| (block.start..=block.end).contains(&line))
}

/// Replaces a block's contents, leaving its two delimiters alone.
pub fn rewrite(text: &str, block: &DynamicBlock, content: &[String]) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let pad = " ".repeat(block.indent);

    let body: Vec<String> = content
        .iter()
        .map(|line| {
            if line.is_empty() {
                line.clone()
            } else {
                format!("{pad}{line}")
            }
        })
        .collect();

    lines.splice(block.start + 1..block.end, body);
    rejoin(&lines, text)
}

/// Regenerates the block at the cursor.
///
/// `generate` is given the whole buffer and the block, and returns the lines
/// to put between its delimiters; `None` means the generator did not
/// recognise the name, and the buffer is left as it was.
pub fn refresh(
    text: &str,
    line: usize,
    generate: impl Fn(&str, &DynamicBlock) -> Option<Vec<String>>,
) -> Option<String> {
    let block = block_at(text, line)?;
    let content = generate(text, &block)?;
    Some(rewrite(text, &block, &content))
}

/// Regenerates every dynamic block in the buffer.
///
/// Blocks are rewritten from the last to the first so that the line numbers
/// of the ones still to do are not moved by the ones already done.
pub fn refresh_all(
    text: &str,
    generate: impl Fn(&str, &DynamicBlock) -> Option<Vec<String>>,
) -> (String, usize) {
    let mut out = text.to_string();
    let mut done = 0;

    for block in blocks(text).into_iter().rev() {
        if let Some(content) = generate(&out, &block) {
            out = rewrite(&out, &block, &content);
            done += 1;
        }
    }

    (out, done)
}

/// The one generator the library ships: a table of the entries below a block.
///
/// `:maxlevel N` is the deepest headline level to include, counted from the
/// top of the file as Org's stars are, and defaults to 3. `:columns` names the
/// columns; `ITEM`, `TODO`, `PRIORITY` and `TAGS` come from the headline and
/// anything else is read as a property. The default is `ITEM TODO`.
///
/// The scope is the subtree the block sits in, or the whole file when it sits
/// above every headline.
///
/// Two things Org's version has are not here: the `#+COLUMNS:` format, which
/// this fork does not model, and the summary rows it computes from it.
pub fn columnview(text: &str, block: &DynamicBlock) -> Vec<String> {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let settings = FileSettings::scan(text);

    let max_level = block.number("maxlevel").unwrap_or(3);
    let columns: Vec<String> = match block.param("columns") {
        Some(given) => split_tokens(given),
        None => vec!["ITEM".to_string(), "TODO".to_string()],
    };

    let (start, end, root) = match subtree_range(&lines, block.start) {
        Some((start, end, level)) => (start, end, level),
        None => (0, lines.len(), 1),
    };

    let mut rows = vec![
        Row::Cells(columns.iter().map(|name| titlecase(name)).collect()),
        Row::Separator,
    ];

    for at in start..end {
        let Some(level) = headline_level(&lines[at]) else {
            continue;
        };
        if level > max_level {
            continue;
        }
        let Some(headline) = parse_headline(&lines[at], &settings) else {
            continue;
        };

        let entry = &lines[at..entry_end(&lines, at, end)];
        rows.push(Row::Cells(
            columns
                .iter()
                .map(|name| column(name, &headline, level - root, entry))
                .collect(),
        ));
    }

    // A view of nothing is an empty table, not a header with no body.
    if rows.len() == 2 {
        return Vec::new();
    }

    Table {
        rows,
        start: 0,
        end: 0,
        indent: 0,
    }
    .render()
}

/// Where the entry starting at `at` ends, bounded by the scope.
fn entry_end(lines: &[String], at: usize, bound: usize) -> usize {
    lines[at + 1..bound]
        .iter()
        .position(|line| headline_level(line).is_some())
        .map_or(bound, |offset| at + 1 + offset)
}

/// One cell of a column view.
fn column(
    name: &str,
    headline: &crate::parser::Headline,
    depth: usize,
    entry: &[String],
) -> String {
    match name.to_ascii_uppercase().as_str() {
        // Indenting by depth is what makes a flat table read as an outline.
        "ITEM" => format!("{}{}", " ".repeat(depth), headline.title),
        "TODO" => headline
            .todo
            .as_ref()
            .map(|state| state.keyword.clone())
            .unwrap_or_default(),
        "PRIORITY" => headline.priority.map(String::from).unwrap_or_default(),
        "TAGS" => headline.tags.join(":"),
        property => crate::sort::entry_property(entry, property).unwrap_or_default(),
    }
}

/// `EFFORT` as a column heading reads better as `Effort`.
fn titlecase(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}
