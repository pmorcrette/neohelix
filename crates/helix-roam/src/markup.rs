//! Emphasis, structure blocks, footnotes and citations.
//!
//! Text transformations over character offsets rather than lines, because
//! these act on what is selected rather than on the entry the cursor is in.

/// The six markers Org wraps a span in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emphasis {
    Bold,
    Italic,
    Underline,
    Verbatim,
    Code,
    Strike,
}

impl Emphasis {
    pub fn marker(self) -> char {
        match self {
            Emphasis::Bold => '*',
            Emphasis::Italic => '/',
            Emphasis::Underline => '_',
            Emphasis::Verbatim => '=',
            Emphasis::Code => '~',
            Emphasis::Strike => '+',
        }
    }

    /// Reads a name as a command would be given it.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "bold" | "b" => Emphasis::Bold,
            "italic" | "i" => Emphasis::Italic,
            "underline" | "u" => Emphasis::Underline,
            "verbatim" | "v" => Emphasis::Verbatim,
            "code" | "c" => Emphasis::Code,
            "strike" | "strikethrough" | "s" => Emphasis::Strike,
            _ => return None,
        })
    }

    pub fn names() -> &'static [&'static str] {
        &["bold", "italic", "underline", "verbatim", "code", "strike"]
    }
}

/// Wraps `from..to` in the marker, or unwraps it if it is already wrapped.
///
/// Both spellings of "already wrapped" count: the markers inside the
/// selection, and the selection sitting between them. A user who selects a
/// word and one who selects the word with its stars mean the same thing.
///
/// Whitespace at either edge is left outside the markers. Org will not render
/// a closing marker that follows a space, so wrapping a word that a selection
/// happened to take a trailing space with would produce markup that shows its
/// own stars.
pub fn toggle_emphasis(text: &str, from: usize, to: usize, kind: Emphasis) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let (mut from, mut to) = (from.min(to), from.max(to).min(chars.len()));

    while from < to && chars[from].is_whitespace() {
        from += 1;
    }
    while to > from && chars[to - 1].is_whitespace() {
        to -= 1;
    }
    if from >= to {
        return None;
    }

    let marker = kind.marker();

    // The selection holds its own markers.
    if to - from >= 2 && chars[from] == marker && chars[to - 1] == marker {
        let mut out: String = chars[..from].iter().collect();
        out.extend(&chars[from + 1..to - 1]);
        out.extend(&chars[to..]);
        return Some(out);
    }

    // The markers sit just outside it.
    if from > 0 && to < chars.len() && chars[from - 1] == marker && chars[to] == marker {
        let mut out: String = chars[..from - 1].iter().collect();
        out.extend(&chars[from..to]);
        out.extend(&chars[to + 1..]);
        return Some(out);
    }

    let mut out: String = chars[..from].iter().collect();
    out.push(marker);
    out.extend(&chars[from..to]);
    out.push(marker);
    out.extend(&chars[to..]);
    Some(out)
}

/// The blocks a structure template can insert.
pub fn block_names() -> &'static [&'static str] {
    &[
        "src", "quote", "example", "verse", "center", "comment", "export",
    ]
}

/// Renders a block's two delimiter lines.
fn delimiters(name: &str, argument: Option<&str>) -> (String, String) {
    let upper = name.to_uppercase();
    let open = match argument.map(str::trim).filter(|a| !a.is_empty()) {
        Some(argument) => format!("#+BEGIN_{upper} {argument}"),
        None => format!("#+BEGIN_{upper}"),
    };
    (open, format!("#+END_{upper}"))
}

/// Puts an empty block after `line`, returning the line to type on.
pub fn insert_block(
    text: &str,
    line: usize,
    name: &str,
    argument: Option<&str>,
) -> (String, usize) {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (open, close) = delimiters(name, argument);
    let at = (line + 1).min(lines.len());

    lines.splice(at..at, [open, String::new(), close]);
    (rejoin(&lines, text), at + 1)
}

/// Wraps `first..=last` in a block, returning the line the content now starts on.
pub fn wrap_block(
    text: &str,
    first: usize,
    last: usize,
    name: &str,
    argument: Option<&str>,
) -> (String, usize) {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (open, close) = delimiters(name, argument);
    let (first, last) = (first.min(last), last.max(first).min(lines.len()));

    lines.insert((last + 1).min(lines.len()), close);
    lines.insert(first, open);
    (rejoin(&lines, text), first + 1)
}

fn rejoin(lines: &[String], original: &str) -> String {
    let joined = lines.join("\n");
    if original.ends_with('\n') && !joined.is_empty() {
        format!("{joined}\n")
    } else {
        joined
    }
}

/// The footnote label under `offset`, from either a reference or a definition.
pub fn footnote_at(text: &str, offset: usize) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let at = offset.min(chars.len().saturating_sub(1));

    // Walk back to the `[` that opens the bracket the cursor is inside.
    let open = (0..=at).rev().find(|&i| chars[i] == '[')?;
    let label: String = chars[open + 1..]
        .iter()
        .take_while(|c| **c != ']')
        .collect();

    let label = label.strip_prefix("fn:")?;
    let name: String = label
        .chars()
        .take_while(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
        .collect();

    (!name.is_empty()).then_some(name)
}

/// Where a footnote's definition is, as a line.
pub fn footnote_definition(text: &str, label: &str) -> Option<usize> {
    let wanted = format!("[fn:{label}]");

    text.lines()
        .position(|line| line.trim_start().starts_with(&wanted))
}

/// Where a footnote is referred to, as a line.
///
/// The definition is skipped: jumping from a definition to itself is not
/// jumping anywhere.
pub fn footnote_reference(text: &str, label: &str) -> Option<usize> {
    let wanted = format!("[fn:{label}");

    text.lines().enumerate().find_map(|(at, line)| {
        (!line.trim_start().starts_with(&format!("[fn:{label}]")) && line.contains(&wanted))
            .then_some(at)
    })
}

/// Every footnote label the text refers to, in the order it refers to them.
pub fn footnote_labels(text: &str) -> Vec<String> {
    let mut labels = Vec::new();
    let mut rest = text;

    while let Some(at) = rest.find("[fn:") {
        let after = &rest[at + 4..];
        let name: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
            .collect();
        if !name.is_empty() && !labels.contains(&name) {
            labels.push(name);
        }
        rest = after;
    }

    labels
}

/// Adds a reference at `offset` and its definition at the end of the buffer.
///
/// Returns the text and the character offset of the new definition, so the
/// caller can put the cursor where the note is written rather than where it
/// is referred to. Org keeps definitions under a `* Footnotes` heading when
/// the file has one; this appends to the end, which is what a file without
/// that heading gets from Org too.
pub fn insert_footnote(text: &str, offset: usize) -> (String, usize) {
    let used = footnote_labels(text);
    let label = (1..)
        .map(|n| n.to_string())
        .find(|candidate| !used.contains(candidate))
        .expect("the integers do not run out");

    let reference = format!("[fn:{label}]");
    let chars: Vec<char> = text.chars().collect();
    let at = offset.min(chars.len());

    let mut out: String = chars[..at].iter().collect();
    out.push_str(&reference);
    out.extend(&chars[at..]);

    if !out.ends_with('\n') {
        out.push('\n');
    }
    let definition = out.chars().count();
    out.push_str(&format!("\n{reference} "));

    (out, definition + 1 + reference.chars().count() + 1)
}

/// Renumbers the numeric footnotes so they run in the order they are referred to.
///
/// Named footnotes are left alone: a name is a name, and renumbering it would
/// be renaming it.
pub fn renumber_footnotes(text: &str) -> String {
    let numeric: Vec<String> = footnote_labels(text)
        .into_iter()
        .filter(|label| label.chars().all(|c| c.is_ascii_digit()))
        .collect();

    // Old label to new, in the order the references appear.
    let renamed: Vec<(String, String)> = numeric
        .iter()
        .enumerate()
        .map(|(index, old)| (old.clone(), (index + 1).to_string()))
        .filter(|(old, new)| old != new)
        .collect();

    if renamed.is_empty() {
        return text.to_string();
    }

    // Two passes through a placeholder, or renaming 1 to 2 would collide with
    // the 2 that is about to become 3.
    let mut out = text.to_string();
    for (old, new) in &renamed {
        out = out.replace(&format!("[fn:{old}"), &format!("[fn:\u{0}{new}"));
    }
    out.replace("[fn:\u{0}", "[fn:")
}

/// The citation key under `offset`, if the cursor is inside a `[cite:@key]`.
pub fn citation_at(text: &str, offset: usize) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let at = offset.min(chars.len().saturating_sub(1));

    let open = (0..=at).rev().find(|&i| chars[i] == '[')?;
    let close = (at..chars.len()).find(|&i| chars[i] == ']')?;
    let inside: String = chars[open + 1..close].iter().collect();
    if !inside.starts_with("cite") {
        return None;
    }

    // The key the cursor is nearest: the last `@` at or before it.
    let marker = (open..=at.min(close)).rev().find(|&i| chars[i] == '@')?;
    let key: String = chars[marker + 1..close]
        .iter()
        .take_while(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
        .collect();

    (!key.is_empty()).then_some(key)
}

/// Puts `[cite:@key]` at `offset`.
pub fn insert_citation(text: &str, offset: usize, key: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let at = offset.min(chars.len());

    let mut out: String = chars[..at].iter().collect();
    out.push_str(&format!("[cite:@{}]", key.trim().trim_start_matches('@')));
    out.extend(&chars[at..]);
    out
}

/// The keys a BibTeX file defines.
///
/// Only the keys: this reads a bibliography to complete over, not to render,
/// and a full BibTeX parser is a different piece of work.
pub fn bib_keys(bib: &str) -> Vec<String> {
    let mut keys = Vec::new();

    for line in bib.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix('@') else {
            continue;
        };
        let Some((kind, rest)) = rest.split_once(['{', '(']) else {
            continue;
        };
        // `@string`, `@preamble` and `@comment` define no entry.
        if matches!(
            kind.trim().to_ascii_lowercase().as_str(),
            "string" | "preamble" | "comment"
        ) {
            continue;
        }

        let key: String = rest
            .trim()
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != ',')
            .collect();
        if !key.is_empty() {
            keys.push(key);
        }
    }

    keys
}
