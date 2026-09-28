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
//! | `%<%Y-%m-%d %H:%M>` | the date and time in that format |
//! | `%a` | a link to where the capture started |
//! | `%i` | the text selected there |
//! | `%f` `%F` | that file's name, and its full path |
//! | `%c` `%x` | the last yank, and the clipboard |
//! | `%k` `%K` | the entry being clocked, and a link to it |
//! | `%n` | the user's name |
//! | `%^{Prompt}` | an answer asked for; `%^{Prompt\|default\|other}` offers choices |
//! | `%^t` `%^T` `%^u` `%^U` | a date asked for, as those timestamps; `%^{Label}t` names it |
//! | `%^g` `%^G` | tags asked for, as `:a:b:` |
//! | `%^{Prop}p` | a value asked for, set as the entry's property `Prop` |
//! | `%\1` … | the first, … `%^{…}` answer again |
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
    /// Where the outline starts: the entry this line is in (the entry with
    /// an id, the one a regexp found, the one being clocked); `None` is the
    /// top of the file.
    pub line: Option<usize>,
    /// Headlines from there down; empty is that place itself.
    pub outline: Vec<String>,
    /// File under a date tree for the capture's day, beneath the outline.
    pub datetree: bool,
    /// What the date tree's levels are.
    pub tree: Tree,
    /// First rather than last among what is already there.
    pub prepend: bool,
}

/// The levels of a date tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tree {
    /// `2026` / `2026-09 September` / `2026-09-28 Monday`.
    #[default]
    Day,
    /// `2026` / `2026-W40` / `2026-09-28 Monday`, ISO weeks (Org's week tree).
    Week,
    /// `2026` / `2026-09 September`.
    Month,
}

/// The line of the entry whose property drawer gives it `:ID: id`.
pub fn id_line(text: &str, id: &str) -> Option<usize> {
    let lines: Vec<&str> = text.lines().collect();
    let found = lines.iter().position(|line| {
        let line = line.trim();
        line.len() > 4
            && line[..4].eq_ignore_ascii_case(":ID:")
            && line[4..].trim().eq_ignore_ascii_case(id.trim())
    })?;
    (0..=found)
        .rev()
        .find(|&index| level_of(lines[index]).is_some())
}

/// The ISO week of a date, with the year it belongs to: the one its
/// Thursday is in.
pub fn iso_week(date: Date) -> (i32, u32) {
    let weekday = match date.weekday() {
        "Mon" => 1,
        "Tue" => 2,
        "Wed" => 3,
        "Thu" => 4,
        "Fri" => 5,
        "Sat" => 6,
        _ => 7,
    };
    let thursday = date.offset_by(4 - weekday);
    let start = Date {
        year: thursday.year,
        month: 1,
        day: 1,
    };
    let ordinal = thursday.to_days() - start.to_days();
    (thursday.year, (ordinal / 7 + 1) as u32)
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
    /// `%c`: the last yank.
    pub kill: String,
    /// `%x`: the clipboard.
    pub clipboard: String,
    /// `%k` and `%K`: the title of the entry being clocked, and a link to it.
    pub clocked: Option<(String, String)>,
    /// `%n`: who is capturing.
    pub user: String,
    /// The answers to the template's questions, in order, as they are to
    /// be written: a date answer is already a timestamp, tags are `:a:b:`.
    pub answers: Vec<String>,
}

impl Context {
    /// A context with nothing but the moment in it.
    pub fn at(date: Date, time: Time) -> Self {
        Self {
            date,
            time,
            link: None,
            initial: String::new(),
            file: None,
            kill: String::new(),
            clipboard: String::new(),
            clocked: None,
            user: String::new(),
            answers: Vec::new(),
        }
    }
}

/// What a question asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asking {
    Text,
    /// A date, written as an active or inactive timestamp, with or without
    /// a time.
    Date {
        active: bool,
        time: bool,
    },
    Tags,
    /// A value for the entry's property of that name.
    Property(String),
}

/// One question a template asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub prompt: String,
    /// What an empty answer means, when the template says.
    pub default: Option<String>,
    /// The answers offered: the default and the others.
    pub choices: Vec<String>,
    pub asking: Asking,
}

/// Where the cursor lands, kept through the expansion and the reshaping of
/// the text as a character nothing writes.
const CURSOR: char = '\u{E000}';

/// The question an escape asks, read from just after its `%`, with how
/// many bytes the escape takes after the `%`.
fn interactive(rest: &str) -> Option<(Question, usize)> {
    let date = |c: char| match c {
        't' => Some((true, false)),
        'T' => Some((true, true)),
        'u' => Some((false, false)),
        'U' => Some((false, true)),
        _ => None,
    };
    let after_caret = rest.strip_prefix('^')?;
    let first = after_caret.chars().next()?;
    if first == '{' {
        let end = after_caret.find('}')?;
        let mut parts = after_caret[1..end].split('|');
        let prompt = parts.next().unwrap_or("").to_string();
        let choices: Vec<String> = parts.map(str::to_string).collect();
        let suffix = after_caret[end + 1..].chars().next();
        let (asking, extra) = match suffix.and_then(|c| date(c).map(|d| (c, d))) {
            Some((_, (active, time))) => (Asking::Date { active, time }, 1),
            None if suffix == Some('p') => (Asking::Property(prompt.clone()), 1),
            None => (Asking::Text, 0),
        };
        let question = Question {
            prompt,
            default: choices.first().cloned(),
            choices,
            asking,
        };
        return Some((question, 1 + end + 1 + extra));
    }
    let (prompt, asking) = match (first, date(first)) {
        (_, Some((active, time))) => ("Date", Asking::Date { active, time }),
        ('g' | 'G', None) => ("Tags", Asking::Tags),
        _ => return None,
    };
    Some((
        Question {
            prompt: prompt.to_string(),
            default: None,
            choices: Vec::new(),
            asking,
        },
        2,
    ))
}

/// The questions a template asks, in the order it asks them.
pub fn questions(template: &str) -> Vec<Question> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(offset) = template[at..].find('%') {
        let percent = at + offset;
        let rest = &template[percent + 1..];
        if rest.starts_with('%') {
            at = percent + 2;
            continue;
        }
        match interactive(rest) {
            Some((question, length)) => {
                found.push(question);
                at = percent + 1 + length;
            }
            None => at = percent + 1,
        }
    }
    found
}

/// An Org timestamp for a moment: active `<…>` or inactive `[…]`.
pub fn timestamp(date: Date, time: Option<Time>, active: bool) -> String {
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

/// Tags as typed (`work home`, `work:home`, `work, home`) as Org writes
/// them after a headline: `:work:home:`; nothing for none.
pub fn tags_answer(input: &str) -> String {
    let tags: Vec<&str> = input
        .split(|c: char| c == ':' || c == ',' || c.is_whitespace())
        .filter(|tag| !tag.is_empty())
        .collect();
    if tags.is_empty() {
        String::new()
    } else {
        format!(":{}:", tags.join(":"))
    }
}

/// `%<…>`: a date and time in a `strftime`-like format. `%Y` `%y` `%m`
/// `%d` `%e` `%H` `%M` `%a` `%A` `%b` `%B` `%j` and `%%` are understood;
/// anything else is kept.
pub fn format_moment(format: &str, date: Date, time: Time) -> String {
    let mut out = String::new();
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('Y') => out.push_str(&format!("{:04}", date.year)),
            Some('y') => out.push_str(&format!("{:02}", date.year.rem_euclid(100))),
            Some('m') => out.push_str(&format!("{:02}", date.month)),
            Some('d') => out.push_str(&format!("{:02}", date.day)),
            Some('e') => out.push_str(&format!("{:>2}", date.day)),
            Some('H') => out.push_str(&format!("{:02}", time.hour)),
            Some('M') => out.push_str(&format!("{:02}", time.minute)),
            Some('a') => out.push_str(date.weekday()),
            Some('A') => out.push_str(day_name(date)),
            Some('b') => out.push_str(&month_name(date)[..3]),
            Some('B') => out.push_str(month_name(date)),
            Some('j') => {
                let start = Date {
                    year: date.year,
                    month: 1,
                    day: 1,
                };
                out.push_str(&format!("{:03}", date.to_days() - start.to_days() + 1));
            }
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// The template's text with its escapes replaced. The cursor's place is
/// marked with [`CURSOR`]; [`place`] takes it out. Properties asked for go
/// into a drawer after the first line.
pub fn expand(template: &str, cx: &Context) -> String {
    let mut out = String::with_capacity(template.len());
    let mut asked = 0;
    let mut texts: Vec<String> = Vec::new();
    let mut properties: Vec<(String, String)> = Vec::new();
    let mut cursor_placed = false;
    let mut at = 0;
    while at < template.len() {
        let Some(offset) = template[at..].find('%') else {
            out.push_str(&template[at..]);
            break;
        };
        out.push_str(&template[at..at + offset]);
        let percent = at + offset;
        let rest = &template[percent + 1..];
        let Some(next) = rest.chars().next() else {
            out.push('%');
            break;
        };
        let clocked = |pick: fn(&(String, String)) -> &String| {
            cx.clocked.as_ref().map(pick).cloned().unwrap_or_default()
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
            'c' => Some(cx.kill.clone()),
            'x' => Some(cx.clipboard.clone()),
            'k' => Some(clocked(|(title, _)| title)),
            'K' => Some(clocked(|(_, link)| link)),
            'n' => Some(cx.user.clone()),
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
            out.push_str(&text);
            at = percent + 1 + next.len_utf8();
            continue;
        }
        if let Some((question, length)) = interactive(rest) {
            let answer = cx.answers.get(asked).cloned().unwrap_or_default();
            asked += 1;
            match question.asking {
                Asking::Property(name) => properties.push((name, answer)),
                Asking::Text => {
                    out.push_str(&answer);
                    texts.push(answer);
                }
                _ => out.push_str(&answer),
            }
            at = percent + 1 + length;
            continue;
        }
        if let Some(format) = rest.strip_prefix('<') {
            if let Some(end) = format.find('>') {
                out.push_str(&format_moment(&format[..end], cx.date, cx.time));
                at = percent + 2 + end + 1;
                continue;
            }
        }
        if let Some(digits) = rest.strip_prefix('\\') {
            let number: String = digits.chars().take_while(char::is_ascii_digit).collect();
            if let Ok(n) = number.parse::<usize>() {
                out.push_str(texts.get(n.wrapping_sub(1)).map_or("", String::as_str));
                at = percent + 2 + number.len();
                continue;
            }
        }
        out.push('%');
        at = percent + 1;
    }

    // An empty answer (no tags, say) leaves no blank at a line's end.
    let mut out = out
        .split('\n')
        .map(|line| line.trim_end_matches([' ', '\t']))
        .collect::<Vec<_>>()
        .join("\n");
    if !properties.is_empty() {
        // After the headline and its planning line, as Org wants them.
        let mut first_end = out.find('\n').unwrap_or(out.len());
        while first_end < out.len() {
            let next = &out[first_end + 1..];
            let line = next.lines().next().unwrap_or("").trim_start();
            if !["SCHEDULED:", "DEADLINE:", "CLOSED:"]
                .iter()
                .any(|keyword| line.starts_with(keyword))
            {
                break;
            }
            first_end += 1 + next.find('\n').unwrap_or(next.len());
        }
        let mut drawer = String::from("\n:PROPERTIES:\n");
        for (name, value) in &properties {
            drawer.push_str(&format!(":{name}: {value}\n"));
        }
        drawer.push_str(":END:");
        out.insert_str(first_end, &drawer);
    }
    out
}

/// The expanded text without its cursor mark, and where the mark was (a
/// byte offset): for showing a capture in a buffer of its own first.
pub fn take_cursor(expanded: &str) -> (String, Option<usize>) {
    match expanded.find(CURSOR) {
        Some(at) => (expanded.replacen(CURSOR, "", 1), Some(at)),
        None => (expanded.to_string(), None),
    }
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
    // Started from an entry: that entry is the place the outline starts at.
    if let Some(line) = place.line {
        if let Some(heading) = (0..=line.min(lines.len().saturating_sub(1)))
            .rev()
            .find(|&index| level_of(lines[index].1).is_some())
        {
            let level = level_of(lines[heading].1).unwrap_or(1);
            let end = (heading + 1..lines.len())
                .find(|&next| level_of(lines[next].1).is_some_and(|l| l <= level))
                .unwrap_or(lines.len());
            node = Node {
                level,
                line: Some(heading),
                end,
            };
        }
    }
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
        let titles = match place.tree {
            Tree::Day => vec![year, month, day],
            Tree::Month => vec![year, month],
            Tree::Week => {
                let (week_year, week) = iso_week(date);
                vec![
                    week_year.to_string(),
                    format!("{week_year}-W{week:02}"),
                    day,
                ]
            }
        };
        for title in titles {
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
            link: Some("[[file:a.org::3][Somewhere]]".into()),
            initial: "line one\nline two".into(),
            file: Some("/notes/a.org".into()),
            answers: vec!["Alice".into(), "high".into()],
            ..Context::at(
                Date::parse_iso("2026-09-28").unwrap(),
                Time { hour: 9, minute: 5 },
            )
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
    fn dates_tags_properties_and_the_rest_of_the_escapes() {
        let template = "* %^{Title} %^g\n%^{Effort|1:00}p%^{Due}t %^U %<%Y/%m/%d %a %H:%M %B %j> \
                        %c %x %k %K %n %\\1 %^{Who}";
        let asked = questions(template);
        let kinds: Vec<&Asking> = asked.iter().map(|q| &q.asking).collect();
        assert_eq!(
            kinds,
            [
                &Asking::Text,
                &Asking::Tags,
                &Asking::Property("Effort".into()),
                &Asking::Date {
                    active: true,
                    time: false
                },
                &Asking::Date {
                    active: false,
                    time: true
                },
                &Asking::Text,
            ]
        );
        assert_eq!(asked[3].prompt, "Due");
        assert_eq!(asked[4].prompt, "Date");
        assert_eq!(asked[2].default.as_deref(), Some("1:00"));

        let context = Context {
            kill: "yanked".into(),
            clipboard: "clip".into(),
            clocked: Some(("Report".into(), "[[id:1][Report]]".into())),
            user: "ann".into(),
            answers: vec![
                "Plan".into(),
                ":work:".into(),
                "2:00".into(),
                "<2026-10-01 Thu>".into(),
                "[2026-10-02 Fri 14:00]".into(),
                "Bob".into(),
            ],
            ..cx()
        };
        assert_eq!(
            expand(template, &context),
            "* Plan :work:\n:PROPERTIES:\n:Effort: 2:00\n:END:\n<2026-10-01 Thu> [2026-10-02 Fri 14:00] \
             2026/09/28 Mon 09:05 September 271 yanked clip Report [[id:1][Report]] ann Plan Bob"
        );
        assert_eq!(tags_answer("work home, x:y"), ":work:home:x:y:");
        assert_eq!(tags_answer("  "), "");
        // The drawer goes after a planning line.
        let planned = Context {
            answers: vec!["<2026-10-01 Thu>".into(), "1:00".into()],
            ..cx()
        };
        assert_eq!(
            expand("* A\nSCHEDULED: %^t\n%^{Effort}pbody", &planned),
            "* A\nSCHEDULED: <2026-10-01 Thu>\n:PROPERTIES:\n:Effort: 1:00\n:END:\nbody"
        );
        let untagged = Context {
            answers: vec!["Paint".into(), String::new()],
            ..cx()
        };
        assert_eq!(expand("* %^{Title} %^g\nbody", &untagged), "* Paint\nbody");
        // `%%^{x}` is a `%` followed by text, not a question.
        assert!(questions("100%%^{x}").is_empty());
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
    fn an_entry_by_id_or_line_and_week_and_month_trees() {
        let text = "* Projects\n** Home\n:PROPERTIES:\n:ID: abc-1\n:END:\n*** Paint\n* Other\n";
        assert_eq!(id_line(text, "ABC-1"), Some(1));
        assert_eq!(id_line(text, "nope"), None);
        let at_home = Place {
            line: id_line(text, "abc-1"),
            ..Place::default()
        };
        let after = inserted(
            text,
            &super::place(text, &at_home, Kind::Entry, "Fix roof", cx().date),
        );
        assert_eq!(
            after,
            "* Projects\n** Home\n:PROPERTIES:\n:ID: abc-1\n:END:\n*** Paint\n*** Fix roof\n* Other\n"
        );
        // From a line inside the entry, with an outline below it.
        let below = Place {
            line: Some(5),
            outline: vec!["Notes".into()],
            ..Place::default()
        };
        let after = inserted(
            text,
            &super::place(text, &below, Kind::Item, "brush", cx().date),
        );
        assert!(
            after.contains("*** Paint\n**** Notes\n- brush\n* Other"),
            "{after}"
        );

        // 2026-09-28 is a Monday of ISO week 40; 2027-01-01 is in 2026's week 53.
        assert_eq!(iso_week(cx().date), (2026, 40));
        assert_eq!(iso_week(Date::parse_iso("2027-01-01").unwrap()), (2026, 53));
        assert_eq!(iso_week(Date::parse_iso("2026-01-01").unwrap()), (2026, 1));
        let week = Place {
            datetree: true,
            tree: Tree::Week,
            ..Place::default()
        };
        let after = inserted(
            "",
            &super::place("", &week, Kind::Entry, "Standup", cx().date),
        );
        assert_eq!(
            after,
            "* 2026\n** 2026-W40\n*** 2026-09-28 Monday\n**** Standup\n"
        );
        let month = Place {
            datetree: true,
            tree: Tree::Month,
            ..Place::default()
        };
        let after = inserted("", &super::place("", &month, Kind::Item, "rent", cx().date));
        assert_eq!(after, "* 2026\n** 2026-09 September\n- rent\n");
    }

    #[test]
    fn titles_match_without_keyword_priority_or_tags() {
        assert_eq!(title_of("** TODO [#A] Tasks :work:"), "Tasks");
        assert_eq!(title_of("* Tasks"), "Tasks");
        assert_eq!(shift_entry("* A\n** B\nbody\n", 3), "*** A\n**** B\nbody\n");
    }
}
