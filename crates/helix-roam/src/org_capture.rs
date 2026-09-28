//! Org's `org-capture`: a note, a task or a list item filed from anywhere
//! into a chosen place, without going there first.
//!
//! A template says what to write and where. What is written is Org's
//! template text with its `%`-escapes:
//!
//! | Escape | Becomes |
//! |--|--|
//! | `%?` | where the cursor lands |
//! | `%t` `%T` | today as an active timestamp, without and with the time |
//! | `%u` `%U` | the same, inactive |
//! | `%a` | a link to where the capture started |
//! | `%i` | the text selected there |
//! | `%f` `%F` | that file's name, and its full path |
//! | `%^{Prompt}` | an answer asked for; `%^{Prompt\|default\|other}` offers choices |
//! | `%\1` … | the first, … answer again |
//! | `%%` | a `%` |
//!
//! Any other `%` stays as written, so "50% done" needs no escaping.
//!
//! Where it goes is a file, optionally a path of headlines in it (created
//! when missing) and optionally a date tree under that (`2026`, then
//! `2026-09 September`, then `2026-09-28 Monday`). What goes there is an
//! entry (a headline, made a child of the place), a list item, a checkbox
//! item, or plain text. Everything is computed on the text, so the editor
//! applies one insertion and tests can check exactly what it would be.

use crate::date::{Date, Time};

/// What a template writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    /// A headline, with whatever follows it.
    #[default]
    Entry,
    /// A plain list item, `- …`.
    Item,
    /// A checkbox item, `- [ ] …`.
    CheckItem,
    /// Text as it is.
    Plain,
}

impl Kind {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "entry" => Kind::Entry,
            "item" => Kind::Item,
            "checkitem" | "check-item" => Kind::CheckItem,
            "plain" => Kind::Plain,
            _ => return None,
        })
    }
}

/// Where a capture is filed, inside its file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Place {
    /// Headlines from the top of the file down; empty is the file itself.
    pub outline: Vec<String>,
    /// File under a date tree for the capture's day, beneath the outline.
    pub datetree: bool,
    /// First rather than last among what is already there.
    pub prepend: bool,
}

/// Everything the escapes can refer to, gathered where the capture started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    pub date: Date,
    pub time: Time,
    /// `%a`: a link to where the capture started, when there is one.
    pub link: Option<String>,
    /// `%i`: the text selected there.
    pub initial: String,
    /// `%F`: that file's path; `%f` is its last component.
    pub file: Option<std::path::PathBuf>,
    /// The answers to the template's `%^{…}` prompts, in order.
    pub answers: Vec<String>,
}

/// One `%^{…}` question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub prompt: String,
    /// What an empty answer means, when the template says.
    pub default: Option<String>,
    /// The answers offered: the default and the others.
    pub choices: Vec<String>,
}

/// Where the cursor lands, kept through the expansion and the reshaping of
/// the text as a character nothing writes.
const CURSOR: char = '\u{E000}';

/// The questions a template asks, in the order it asks them.
pub fn questions(template: &str) -> Vec<Question> {
    let mut found = Vec::new();
    let mut rest = template;
    while let Some(at) = rest.find("%^{") {
        let after = &rest[at + 3..];
        let Some(end) = after.find('}') else {
            break;
        };
        let mut parts = after[..end].split('|');
        let prompt = parts.next().unwrap_or("").to_string();
        let choices: Vec<String> = parts.map(str::to_string).collect();
        found.push(Question {
            prompt,
            default: choices.first().cloned(),
            choices,
        });
        rest = &after[end + 1..];
    }
    found
}

fn timestamp(date: Date, time: Option<Time>, active: bool) -> String {
    let (open, close) = if active { ('<', '>') } else { ('[', ']') };
    match time {
        Some(time) => format!(
            "{open}{} {} {:02}:{:02}{close}",
            date.to_iso(),
            date.weekday(),
            time.hour,
            time.minute
        ),
        None => format!("{open}{} {}{close}", date.to_iso(), date.weekday()),
    }
}

/// The template's text with its escapes replaced. The cursor's place is
/// marked with [`CURSOR`]; [`place`] takes it out.
pub fn expand(template: &str, cx: &Context) -> String {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.char_indices().peekable();
    let mut asked = 0;
    let mut cursor_placed = false;
    while let Some((at, c)) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let Some(&(_, next)) = chars.peek() else {
            out.push('%');
            break;
        };
        let simple = match next {
            '%' => Some("%".to_string()),
            '?' if !cursor_placed => {
                cursor_placed = true;
                Some(CURSOR.to_string())
            }
            '?' => Some(String::new()),
            't' => Some(timestamp(cx.date, None, true)),
            'T' => Some(timestamp(cx.date, Some(cx.time), true)),
            'u' => Some(timestamp(cx.date, None, false)),
            'U' => Some(timestamp(cx.date, Some(cx.time), false)),
            'a' => Some(cx.link.clone().unwrap_or_default()),
            'f' => Some(
                cx.file
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
            'F' => Some(
                cx.file
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
            ),
            'i' => {
                // Continuation lines line up under the first, as in Org.
                let line_start = out.rfind('\n').map_or(0, |at| at + 1);
                let indent: String = out[line_start..]
                    .chars()
                    .take_while(|c| c.is_whitespace())
                    .collect();
                Some(cx.initial.replace('\n', &format!("\n{indent}")))
            }
            _ => None,
        };
        if let Some(text) = simple {
            chars.next();
            out.push_str(&text);
            continue;
        }
        let rest = &template[at + 1..];
        if let Some(inner) = rest.strip_prefix("^{") {
            if let Some(end) = inner.find('}') {
                let answer = cx.answers.get(asked).cloned().unwrap_or_default();
                asked += 1;
                out.push_str(&answer);
                // Skip `^{…}`.
                let skip = 2 + inner[..=end].chars().count();
                for _ in 0..skip {
                    chars.next();
                }
                continue;
            }
        }
        if let Some(digits) = rest.strip_prefix('\\') {
            let number: String = digits.chars().take_while(char::is_ascii_digit).collect();
            if let Ok(n) = number.parse::<usize>() {
                out.push_str(cx.answers.get(n.wrapping_sub(1)).map_or("", String::as_str));
                for _ in 0..1 + number.len() {
                    chars.next();
                }
                continue;
            }
        }
        out.push('%');
    }
    out
}

/// One insertion into the target's text, and where the cursor goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Insertion {
    /// A byte offset into the target's text.
    pub at: usize,
    pub text: String,
    /// The cursor's byte offset in the text after the insertion.
    pub cursor: usize,
}

/// A headline's level: its stars, when the line is one.
fn level_of(line: &str) -> Option<usize> {
    let stars = line.chars().take_while(|&c| c == '*').count();
    (stars > 0 && line[stars..].starts_with(' ')).then_some(stars)
}

/// A headline's title for matching: without stars, keyword, priority and
/// tags. The keyword is dropped only when it is one of the usual ones, as
/// matching is about the words the template names.
fn title_of(line: &str) -> String {
    let stars = line.chars().take_while(|&c| c == '*').count();
    let mut title = line[stars..].trim();
    for keyword in ["TODO ", "DONE ", "NEXT ", "WAITING ", "CANCELLED "] {
        if let Some(rest) = title.strip_prefix(keyword) {
            title = rest.trim_start();
        }
    }
    if title.starts_with("[#") && title.get(3..4) == Some("]") {
        title = title[4..].trim_start();
    }
    // Trailing `:tag:list:`.
    if let Some(space) = title.rfind(char::is_whitespace) {
        let tail = &title[space + 1..];
        if tail.len() > 1 && tail.starts_with(':') && tail.ends_with(':') {
            title = title[..space].trim_end();
        }
    }
    title.to_string()
}

/// The lines of `text` with their byte offsets.
fn lines(text: &str) -> Vec<(usize, &str)> {
    let mut at = 0;
    text.split_inclusive('\n')
        .map(|line| {
            let start = at;
            at += line.len();
            (start, line.trim_end_matches(['\n', '\r']))
        })
        .collect()
}

/// A place in the outline being walked: a headline, or the file itself.
#[derive(Debug, Clone, Copy)]
struct Node {
    /// Stars; 0 for the file.
    level: usize,
    /// Index of the headline's line; `None` for the file.
    line: Option<usize>,
    /// Index of the first line after its subtree.
    end: usize,
}

impl Node {
    /// The first line after the headline, and the first line of its first
    /// child, or `end` without one.
    fn body_end(self, lines: &[(usize, &str)]) -> usize {
        let from = self.line.map_or(0, |line| line + 1);
        (from..self.end)
            .find(|&index| level_of(lines[index].1).is_some())
            .unwrap_or(self.end)
    }

    /// Its children, as nodes.
    fn children(self, lines: &[(usize, &str)]) -> Vec<Node> {
        let from = self.line.map_or(0, |line| line + 1);
        let mut children = Vec::new();
        let mut index = from;
        while index < self.end {
            match level_of(lines[index].1) {
                Some(level) if level > self.level => {
                    let end = (index + 1..self.end)
                        .find(|&next| level_of(lines[next].1).is_some_and(|l| l <= level))
                        .unwrap_or(self.end);
                    children.push(Node {
                        level,
                        line: Some(index),
                        end,
                    });
                    index = end;
                }
                _ => index += 1,
            }
        }
        children
    }
}

/// The byte offset of line `index`, or the end of the text.
fn offset(lines: &[(usize, &str)], text: &str, index: usize) -> usize {
    lines.get(index).map_or(text.len(), |(start, _)| *start)
}

/// Makes `entry` a subtree whose top headline has `level` stars.
fn shift_entry(entry: &str, level: usize) -> String {
    let entry = if level_of(entry.lines().next().unwrap_or("")).is_some() {
        entry.to_string()
    } else {
        format!("* {entry}")
    };
    let top = level_of(entry.lines().next().unwrap_or("")).unwrap_or(1);
    entry
        .split_inclusive('\n')
        .map(|line| match level_of(line) {
            Some(stars) => {
                let new = (stars + level).saturating_sub(top).max(1);
                format!("{}{}", "*".repeat(new), &line[stars..])
            }
            None => line.to_string(),
        })
        .collect()
}

/// Makes `text` a list item: the marker on its first line, the others
/// indented under it.
fn as_item(text: &str, checkbox: bool) -> String {
    let marker = if checkbox { "- [ ] " } else { "- " };
    let is_item = |line: &str| {
        let line = line.trim_start();
        line.starts_with("- ") || line.starts_with("+ ")
    };
    let mut out = String::new();
    for (index, line) in text.split_inclusive('\n').enumerate() {
        if index == 0 {
            if is_item(line) {
                out.push_str(line.trim_start());
            } else {
                out.push_str(marker);
                out.push_str(line);
            }
        } else if line.trim().is_empty() {
            out.push_str(line);
        } else {
            out.push_str("  ");
            out.push_str(line);
        }
    }
    out
}

/// Where `expanded` goes in `text`, filed at `place` as `kind`, dated
/// `date` for a date tree. Headlines the place names and the date tree
/// needs are created as part of the same insertion.
pub fn place(text: &str, place: &Place, kind: Kind, expanded: &str, date: Date) -> Insertion {
    let lines = lines(text);
    let mut node = Node {
        level: 0,
        line: None,
        end: lines.len(),
    };
    // What has to be created on the way, as headlines (level, title).
    let mut missing: Vec<(usize, String)> = Vec::new();
    // Where a missing path starts: after the last existing ancestor's
    // subtree, or before a later date in a date tree.
    let mut create_at: Option<usize> = None;

    for title in &place.outline {
        if create_at.is_some() {
            missing.push((node.level + 1 + missing.len(), title.clone()));
            continue;
        }
        match node
            .children(&lines)
            .into_iter()
            .find(|child| title_of(lines[child.line.unwrap_or(0)].1) == title.trim())
        {
            Some(child) => node = child,
            None => {
                create_at = Some(node.end);
                missing.push((node.level + 1, title.clone()));
            }
        }
    }

    if place.datetree {
        let year = date.to_iso()[..4].to_string();
        let month = format!("{} {}", &date.to_iso()[..7], month_name(date));
        let day = format!("{} {}", date.to_iso(), day_name(date));
        for title in [year, month, day] {
            let base = node.level + 1 + missing.len();
            if create_at.is_some() {
                missing.push((base, title));
                continue;
            }
            let children = node.children(&lines);
            let key = title.split_whitespace().next().unwrap_or("").to_string();
            let found = children.iter().find(|child| {
                title_of(lines[child.line.unwrap_or(0)].1)
                    .split_whitespace()
                    .next()
                    == Some(key.as_str())
            });
            match found {
                Some(child) => node = *child,
                None => {
                    // Kept in date order: before the first later one.
                    let later = children.iter().find(|child| {
                        title_of(lines[child.line.unwrap_or(0)].1)
                            .split_whitespace()
                            .next()
                            .is_some_and(|other| other > key.as_str())
                    });
                    create_at = Some(later.and_then(|child| child.line).unwrap_or(node.end));
                    missing.push((base, title));
                }
            }
        }
    }

    let parent_level = node.level + missing.len();
    let mut body = match kind {
        Kind::Entry => shift_entry(expanded, parent_level + 1),
        Kind::Item => as_item(expanded, false),
        Kind::CheckItem => as_item(expanded, true),
        Kind::Plain => expanded.to_string(),
    };
    if !body.ends_with('\n') {
        body.push('\n');
    }

    let mut block = String::new();
    for (level, title) in &missing {
        block.push_str(&format!("{} {title}\n", "*".repeat(*level)));
    }
    block.push_str(&body);

    let line = match create_at {
        Some(line) => line,
        None => match (kind, place.prepend) {
            // An entry goes among the place's children: first or last.
            (Kind::Entry, true) => node.body_end(&lines),
            (Kind::Entry, false) => node.end,
            // Anything else goes into the place's own text, after it.
            _ => node.body_end(&lines),
        },
    };
    let mut at = offset(&lines, text, line);
    // A file without a final newline needs one before what is added.
    if at == text.len() && !text.is_empty() && !text.ends_with('\n') {
        block.insert(0, '\n');
    }
    // An item or plain text after a list goes after its last line, not
    // after the blank lines that close the entry's text.
    if kind != Kind::Entry && create_at.is_none() {
        let first = node.line.map_or(0, |line| line + 1);
        let mut end = line;
        while end > first && lines[end - 1].1.trim().is_empty() {
            end -= 1;
        }
        at = offset(&lines, text, end);
    }

    let (text, cursor) = match block.find(CURSOR) {
        Some(mark) => (block.replacen(CURSOR, "", 1), mark),
        // Without `%?`, at the end of what was written, before its newline.
        None => {
            let end = block.trim_end_matches('\n').len();
            (block, end)
        }
    };
    Insertion {
        at,
        cursor: at + cursor,
        text,
    }
}

fn month_name(date: Date) -> &'static str {
    const NAMES: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    NAMES[(date.month as usize).saturating_sub(1).min(11)]
}

fn day_name(date: Date) -> &'static str {
    match date.weekday() {
        "Mon" => "Monday",
        "Tue" => "Tuesday",
        "Wed" => "Wednesday",
        "Thu" => "Thursday",
        "Fri" => "Friday",
        "Sat" => "Saturday",
        _ => "Sunday",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx() -> Context {
        Context {
            date: Date::parse_iso("2026-09-28").unwrap(),
            time: Time { hour: 9, minute: 5 },
            link: Some("[[file:a.org::3][Somewhere]]".into()),
            initial: "line one\nline two".into(),
            file: Some("/notes/a.org".into()),
            answers: vec!["Alice".into(), "high".into()],
        }
    }

    fn inserted(text: &str, insertion: &Insertion) -> String {
        let mut out = text.to_string();
        out.insert_str(insertion.at, &insertion.text);
        out
    }

    #[test]
    fn escapes_are_replaced_and_unknown_ones_kept() {
        let out = expand(
            "* TODO Call %^{Who} (%^{Priority|high|low}) %?\n  %U %t\n  %a\n  - %i\n  %\\1 in %f; 50% done %%",
            &cx(),
        );
        assert_eq!(
            out,
            format!(
                "* TODO Call Alice (high) {CURSOR}\n  [2026-09-28 Mon 09:05] <2026-09-28 Mon>\n  \
                 [[file:a.org::3][Somewhere]]\n  - line one\n  line two\n  Alice in a.org; 50% done %"
            )
        );
        let asked = questions("%^{Who} and %^{Priority|high|low}");
        assert_eq!(asked.len(), 2);
        assert_eq!(asked[0].prompt, "Who");
        assert_eq!(asked[1].default.as_deref(), Some("high"));
        assert_eq!(asked[1].choices, ["high", "low"]);
    }

    #[test]
    fn an_entry_goes_under_a_headline_created_if_missing() {
        let text = "#+title: Inbox\n* Tasks\n** One\n* Notes\n";
        let place = Place {
            outline: vec!["Tasks".into()],
            ..Place::default()
        };
        let entry = expand("* TODO New %?", &cx());
        let insertion = super::place(text, &place, Kind::Entry, &entry, cx().date);
        let after = inserted(text, &insertion);
        assert_eq!(
            after,
            "#+title: Inbox\n* Tasks\n** One\n** TODO New \n* Notes\n"
        );
        assert_eq!(&after[insertion.cursor..insertion.cursor + 1], "\n");

        // Prepended, it comes first; a missing path is created at the end.
        let first = Place {
            prepend: true,
            ..place.clone()
        };
        let after = inserted(
            text,
            &super::place(text, &first, Kind::Entry, "* A", cx().date),
        );
        assert_eq!(after, "#+title: Inbox\n* Tasks\n** A\n** One\n* Notes\n");
        let deep = Place {
            outline: vec!["Projects".into(), "Home".into()],
            ..Place::default()
        };
        let after = inserted(
            text,
            &super::place(text, &deep, Kind::Entry, "Paint", cx().date),
        );
        assert!(
            after.ends_with("* Notes\n* Projects\n** Home\n*** Paint\n"),
            "{after}"
        );
    }

    #[test]
    fn items_and_the_file_itself_as_targets() {
        let text = "* Shopping\n- milk\n\n* Other";
        let place = Place {
            outline: vec!["Shopping".into()],
            ..Place::default()
        };
        let after = inserted(
            text,
            &super::place(text, &place, Kind::CheckItem, "eggs", cx().date),
        );
        assert_eq!(after, "* Shopping\n- milk\n- [ ] eggs\n\n* Other");

        // The whole file, without a final newline: one is added first.
        let after = inserted(
            text,
            &super::place(text, &Place::default(), Kind::Entry, "* Last", cx().date),
        );
        assert_eq!(after, "* Shopping\n- milk\n\n* Other\n* Last\n");
        let after = inserted(
            "",
            &super::place("", &Place::default(), Kind::Entry, "First", cx().date),
        );
        assert_eq!(after, "* First\n");
    }

    #[test]
    fn a_date_tree_is_found_or_built_in_order() {
        let place = Place {
            outline: vec!["Journal".into()],
            datetree: true,
            ..Place::default()
        };
        let date = cx().date;
        let empty = "* Journal\n";
        let after = inserted(
            empty,
            &super::place(empty, &place, Kind::Entry, "* Met Bob", date),
        );
        assert_eq!(
            after,
            "* Journal\n** 2026\n*** 2026-09 September\n**** 2026-09-28 Monday\n***** Met Bob\n"
        );

        // The day exists: the entry goes last under it.
        let again = inserted(
            &after,
            &super::place(&after, &place, Kind::Entry, "Lunch", date),
        );
        assert!(again.ends_with("***** Met Bob\n***** Lunch\n"), "{again}");

        // A later day already there: the new one goes before it.
        let later = "* Journal\n** 2026\n*** 2026-09 September\n**** 2026-09-30 Wednesday\n";
        let after = inserted(
            later,
            &super::place(later, &place, Kind::Entry, "Early", date),
        );
        assert_eq!(
            after,
            "* Journal\n** 2026\n*** 2026-09 September\n**** 2026-09-28 Monday\n***** Early\n\
             **** 2026-09-30 Wednesday\n"
        );
    }

    #[test]
    fn titles_match_without_keyword_priority_or_tags() {
        assert_eq!(title_of("** TODO [#A] Tasks :work:"), "Tasks");
        assert_eq!(title_of("* Tasks"), "Tasks");
        assert_eq!(shift_entry("* A\n** B\nbody\n", 3), "*** A\n**** B\nbody\n");
    }
}
