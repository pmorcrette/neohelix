//! Plain lists and their checkboxes.
//!
//! Text transformations, like the rest of the editing layer, so what a command
//! does to a buffer is settled by tests rather than by driving the editor.

/// What a list line is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Columns of indentation, which decide nesting.
    pub indent: usize,
    /// `-`, `+`, `*`, or the number of an ordered item.
    pub bullet: Bullet,
    /// `[ ]`, `[X]` or `[-]`, when the item has one.
    pub checkbox: Option<Checkbox>,
    /// Everything after the bullet and the checkbox.
    pub content: String,
}

/// How an item is marked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bullet {
    /// `-`, `+` or `*`.
    Unordered(char),
    /// `1.` or `1)`, with the separator kept so it can be written back.
    Ordered(usize, char),
}

/// A checkbox's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checkbox {
    Empty,
    Done,
    /// `[-]`: some children done, not all.
    Partial,
}

impl Checkbox {
    fn render(self) -> &'static str {
        match self {
            Checkbox::Empty => "[ ]",
            Checkbox::Done => "[X]",
            Checkbox::Partial => "[-]",
        }
    }
}

impl Item {
    /// Renders the item back to a line.
    pub fn render(&self) -> String {
        let bullet = match &self.bullet {
            Bullet::Unordered(c) => c.to_string(),
            Bullet::Ordered(n, separator) => format!("{n}{separator}"),
        };
        let checkbox = match self.checkbox {
            Some(state) => format!("{} ", state.render()),
            None => String::new(),
        };

        format!(
            "{}{bullet} {checkbox}{}",
            " ".repeat(self.indent),
            self.content
        )
        .trim_end()
        .to_string()
    }
}

/// Reads a list item, if the line is one.
pub fn parse_item<'a>(line: &'a str) -> Option<Item> {
    let indent = line.len() - line.trim_start().len();
    let rest = line.trim_start();

    // A bullet with nothing after it is still an item — that is what a
    // freshly inserted one looks like, and failing to read it back would
    // break every operation that runs after an insertion.
    let after_bullet = |rest: &'a str, marker: char| -> Option<&'a str> {
        let tail = rest.strip_prefix(marker)?;
        if tail.is_empty() {
            Some(tail)
        } else {
            tail.strip_prefix(' ')
        }
    };

    // `* item` at the start of a line is a headline, not a list item; an
    // indented one is a list. The `*` branch must not short-circuit the
    // ordered one below it: an indented `1.` is still an ordered item.
    let starred = (indent > 0)
        .then(|| after_bullet(rest, '*'))
        .flatten()
        .map(|after| (Bullet::Unordered('*'), after));

    let (bullet, after) = if let Some(after) = after_bullet(rest, '-') {
        (Bullet::Unordered('-'), after)
    } else if let Some(after) = after_bullet(rest, '+') {
        (Bullet::Unordered('+'), after)
    } else if let Some(found) = starred {
        found
    } else {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            return None;
        }
        let separator = rest[digits.len()..].chars().next()?;
        if !matches!(separator, '.' | ')') {
            return None;
        }
        let tail = &rest[digits.len() + 1..];
        let after = if tail.is_empty() {
            tail
        } else {
            tail.strip_prefix(' ')?
        };
        (Bullet::Ordered(digits.parse().ok()?, separator), after)
    };

    let (checkbox, content) = match after.get(..3) {
        Some("[ ]") => (Some(Checkbox::Empty), after[3..].trim_start()),
        Some("[X]") | Some("[x]") => (Some(Checkbox::Done), after[3..].trim_start()),
        Some("[-]") => (Some(Checkbox::Partial), after[3..].trim_start()),
        _ => (None, after),
    };

    Some(Item {
        indent,
        bullet,
        checkbox,
        content: content.to_string(),
    })
}

/// Inserts a sibling item after the one at `line`, and says where it went.
///
/// The new item copies the bullet style and the checkbox's presence, since an
/// item added to a checklist is itself a thing to tick.
pub fn insert_item(text: &str, line: usize) -> Option<(String, usize)> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let at = line.min(lines.len().saturating_sub(1));
    let item = parse_item(lines.get(at)?)?;

    // After this item and anything nested under it.
    let mut end = at + 1;
    while end < lines.len() {
        match parse_item(&lines[end]) {
            Some(next) if next.indent > item.indent => end += 1,
            // A blank line inside a list belongs to it.
            None if lines[end].trim().is_empty() && end + 1 < lines.len() => break,
            _ => break,
        }
    }

    let new = Item {
        indent: item.indent,
        bullet: match item.bullet {
            Bullet::Unordered(c) => Bullet::Unordered(c),
            Bullet::Ordered(n, separator) => Bullet::Ordered(n + 1, separator),
        },
        checkbox: item.checkbox.map(|_| Checkbox::Empty),
        content: String::new(),
    };

    lines.insert(end, new.render());
    let text = renumber_from(&lines.join("\n"), end, text.ends_with('\n'));
    Some((text, end))
}

/// Renumbers the ordered list containing `line`.
pub fn renumber(text: &str, line: usize) -> String {
    renumber_from(text, line, text.ends_with('\n'))
}

fn renumber_from(text: &str, line: usize, trailing_newline: bool) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let at = line.min(lines.len().saturating_sub(1));

    let Some(item) = lines.get(at).and_then(|l| parse_item(l)) else {
        return rejoin(&lines, trailing_newline);
    };
    if matches!(item.bullet, Bullet::Unordered(_)) {
        return rejoin(&lines, trailing_newline);
    }

    // The run of items at this indent, in this list.
    let mut start = at;
    while start > 0 {
        match parse_item(&lines[start - 1]) {
            Some(previous) if previous.indent >= item.indent => start -= 1,
            _ => break,
        }
    }

    let mut number = 1;
    let mut index = start;
    while index < lines.len() {
        let Some(mut current) = parse_item(&lines[index]) else {
            break;
        };
        if current.indent < item.indent {
            break;
        }
        if current.indent == item.indent {
            if let Bullet::Ordered(_, separator) = current.bullet {
                current.bullet = Bullet::Ordered(number, separator);
                lines[index] = current.render();
                number += 1;
            }
        }
        index += 1;
    }

    rejoin(&lines, trailing_newline)
}

/// Moves the item at `line`, and anything nested under it, in or out a level.
pub fn shift_item(text: &str, line: usize, deeper: bool) -> Option<String> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let at = line.min(lines.len().saturating_sub(1));
    let item = parse_item(lines.get(at)?)?;

    if !deeper && item.indent == 0 {
        return None;
    }
    let step = 2;

    let mut index = at;
    while index < lines.len() {
        let Some(mut current) = parse_item(&lines[index]) else {
            break;
        };
        if index > at && current.indent <= item.indent {
            break;
        }
        current.indent = if deeper {
            current.indent + step
        } else {
            current.indent.saturating_sub(step)
        };
        lines[index] = current.render();
        index += 1;
    }

    Some(rejoin(&lines, text.ends_with('\n')))
}

/// Toggles the checkbox at `line`, then brings every cookie up to date.
///
/// An item with no checkbox gains one: asking to tick something that cannot be
/// ticked plainly means "make this tickable".
pub fn toggle_checkbox(text: &str, line: usize) -> Option<String> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let at = line.min(lines.len().saturating_sub(1));
    let mut item = parse_item(lines.get(at)?)?;

    item.checkbox = Some(match item.checkbox {
        Some(Checkbox::Done) => Checkbox::Empty,
        _ => Checkbox::Done,
    });
    lines[at] = item.render();

    Some(update_cookies(&rejoin(&lines, text.ends_with('\n'))))
}

/// Recomputes every `[n/m]` and `[p%]` cookie in the text.
///
/// A cookie counts the checkboxes directly under its line — the items one
/// level in, not every descendant — which is what Org counts.
pub fn update_cookies(text: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();

    for index in 0..lines.len() {
        if find_cookie(&lines[index]).is_none() {
            continue;
        }
        let (done, total) = count_children(&lines, index);
        if total == 0 {
            continue;
        }
        lines[index] = write_cookie(&lines[index], done, total);
    }

    rejoin(&lines, text.ends_with('\n'))
}

/// Byte range of a `[n/m]` or `[p%]` cookie on a line.
fn find_cookie(line: &str) -> Option<(usize, usize)> {
    let mut from = 0;
    while let Some(open) = line[from..].find('[') {
        let start = from + open;
        let close = line[start..].find(']')?;
        let inner = &line[start + 1..start + close];
        let is_cookie = inner
            .strip_suffix('%')
            .is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()))
            || inner.split_once('/').is_some_and(|(a, b)| {
                a.chars().all(|c| c.is_ascii_digit()) && b.chars().all(|c| c.is_ascii_digit())
            });

        if is_cookie {
            return Some((start, start + close + 1));
        }
        from = start + 1;
    }
    None
}

/// Done and total checkboxes one level under `index`.
fn count_children(lines: &[String], index: usize) -> (usize, usize) {
    // A headline's children are the top-level items under it; a list item's
    // are the items one step further in.
    let parent_indent = parse_item(&lines[index]).map(|item| item.indent);

    let mut done = 0;
    let mut total = 0;
    let mut child_indent = None;

    for line in &lines[index + 1..] {
        let Some(item) = parse_item(line) else {
            if line.trim().is_empty() {
                continue;
            }
            // A headline ends the list it follows.
            if line.starts_with('*') {
                break;
            }
            continue;
        };

        if let Some(parent) = parent_indent {
            if item.indent <= parent {
                break;
            }
        }
        let level = *child_indent.get_or_insert(item.indent);
        if item.indent < level {
            break;
        }
        if item.indent > level {
            continue;
        }
        if let Some(state) = item.checkbox {
            total += 1;
            if state == Checkbox::Done {
                done += 1;
            }
        }
    }

    (done, total)
}

/// Writes the counts into the line's cookie, keeping its form.
fn write_cookie(line: &str, done: usize, total: usize) -> String {
    let Some((start, end)) = find_cookie(line) else {
        return line.to_string();
    };

    let replacement = if line[start..end].ends_with("%]") {
        let percent = if total == 0 { 0 } else { done * 100 / total };
        format!("[{percent}%]")
    } else {
        format!("[{done}/{total}]")
    };

    format!("{}{replacement}{}", &line[..start], &line[end..])
}

fn rejoin(lines: &[String], trailing_newline: bool) -> String {
    let joined = lines.join("\n");
    if trailing_newline && !joined.is_empty() {
        format!("{joined}\n")
    } else {
        joined
    }
}
