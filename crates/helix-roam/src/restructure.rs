//! Moving nodes between shapes: subtree to file, file to subtree, and back.
//!
//! These are text transformations rather than editor commands, so that what
//! they do to a buffer can be pinned by tests instead of by driving the UI.
//! Each takes the text of a file and returns the text it becomes.
//!
//! The behaviour follows Org-Roam's own. Notably, extracting a subtree does
//! *not* leave a link where it was: the subtree carries its `:ID:` into the
//! new file, so links that already pointed at it keep resolving, and nothing
//! needs to be left behind.

use std::fmt;

use uuid::Uuid;

use crate::parser::{find_ignore_case, starts_with_ignore_case, FileSettings};

/// Why a restructuring could not be done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Promoting needs exactly one root heading and nothing beside it.
    NotPromotable(&'static str),
    /// Extracting needs a headline at or above the cursor.
    NoSubtree,
    /// A file-level node is already what extracting would produce.
    AlreadyTopLevel,
    /// Demoting needs a `#+title:` to turn into a heading.
    NoTitle,
    /// The chosen target node was not found in its file.
    NoTarget,
    /// The entry has no property drawer to edit.
    NoDrawer,
    /// There is no sibling to swap with in that direction.
    NoSibling,
    /// The file does not declare that priority letter.
    UndeclaredPriority(char),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotPromotable(why) => write!(f, "cannot promote: {why}"),
            Error::NoSubtree => write!(f, "no subtree at the cursor"),
            Error::AlreadyTopLevel => write!(f, "already a top-level node"),
            Error::NoTitle => write!(f, "the file has no #+title: to demote"),
            Error::NoTarget => write!(f, "the target node was not found in its file"),
            Error::NoSibling => write!(f, "there is no sibling that way"),
            Error::UndeclaredPriority(letter) => {
                write!(f, "this file does not declare priority [#{letter}]")
            }
            Error::NoDrawer => write!(f, "the entry has no property drawer; give it an :ID: first"),
        }
    }
}

impl std::error::Error for Error {}

/// Number of leading stars, when the line is a headline.
fn headline_level(line: &str) -> Option<usize> {
    let stars = line.bytes().take_while(|&b| b == b'*').count();
    if stars == 0 {
        return None;
    }
    // `**bold**` is not a headline: the stars must be followed by a space.
    let rest = &line[stars..];
    (rest.is_empty() || rest.starts_with([' ', '\t'])).then_some(stars)
}

/// The text of a headline line after its stars.
fn headline_text(line: &str) -> &str {
    line.trim_start_matches('*').trim()
}

/// Splits `Title :tag1:tag2:` into its title and its tags.
fn split_tags(text: &str) -> (&str, Vec<String>) {
    let trimmed = text.trim_end();
    if !trimmed.ends_with(':') || trimmed.len() < 2 {
        return (trimmed, Vec::new());
    }

    let Some(start) = trimmed[..trimmed.len() - 1].rfind(|c: char| c.is_whitespace()) else {
        return (trimmed, Vec::new());
    };
    let run = &trimmed[start + 1..];

    let inner = run.trim_matches(':');
    if inner.is_empty()
        || !inner
            .split(':')
            .all(|tag| !tag.is_empty() && tag.chars().all(|c| c.is_alphanumeric() || c == '_'))
    {
        return (trimmed, Vec::new());
    }

    (
        trimmed[..start].trim_end(),
        inner.split(':').map(str::to_string).collect(),
    )
}

/// `#+key:` value, if the line is that keyword.
fn keyword_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line.trim_start().strip_prefix("#+")?;
    let (found, value) = rest.split_once(':')?;
    found.trim().eq_ignore_ascii_case(key).then(|| value.trim())
}

/// Shifts every headline in `lines` by one level.
fn shift_levels(lines: &mut [String], deeper: bool) {
    for line in lines {
        if headline_level(line).is_some() {
            if deeper {
                line.insert(0, '*');
            } else {
                line.remove(0);
            }
        }
    }
}

/// Turns a file whose whole content is one level-1 heading into a file node.
///
/// The heading's title becomes `#+title:` and its tags `#+filetags:`; every
/// remaining heading moves up one level.
pub fn promote_buffer(text: &str) -> Result<String, Error> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();

    let Some(root) = lines.iter().position(|l| headline_level(l).is_some()) else {
        return Err(Error::NotPromotable("there is no heading"));
    };
    if headline_level(&lines[root]) != Some(1) {
        return Err(Error::NotPromotable("the first heading is not level 1"));
    }
    if lines[root + 1..]
        .iter()
        .any(|l| headline_level(l) == Some(1))
    {
        return Err(Error::NotPromotable("there are multiple root headings"));
    }
    // Anything before the heading other than keywords, a drawer or blank lines
    // would be file-level text that has nowhere to go.
    if lines[..root].iter().any(|l| is_stray_text(l)) {
        return Err(Error::NotPromotable("there is text above the heading"));
    }

    let (title, tags) = split_tags(headline_text(&lines[root]));
    let title = title.to_string();

    lines.remove(root);
    shift_levels(&mut lines[root..], false);

    // The keywords go where the heading was, after whatever preamble exists.
    let mut inserted = vec![format!("#+title: {title}")];
    if !tags.is_empty() {
        inserted.push(format!("#+filetags: :{}:", tags.join(":")));
    }
    lines.splice(root..root, inserted);

    Ok(rejoin(&lines, text))
}

/// Whether a preamble line is neither a keyword, a drawer line nor blank.
fn is_stray_text(line: &str) -> bool {
    let trimmed = line.trim();
    !(trimmed.is_empty()
        || trimmed.starts_with("#+")
        || trimmed.starts_with('#')
        || trimmed.starts_with(':'))
}

/// Turns a file node into a single level-1 heading holding the whole file.
///
/// The heading is inserted at the very top so that a preamble property drawer
/// ends up beneath it and becomes the heading's own, which is what Org-Roam
/// does and why the drawer is not moved explicitly.
pub fn demote_buffer(text: &str) -> Result<String, Error> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();

    let title = lines
        .iter()
        .find_map(|line| keyword_value(line, "title"))
        .ok_or(Error::NoTitle)?
        .to_string();
    let tags: Vec<String> = lines
        .iter()
        .find_map(|line| keyword_value(line, "filetags"))
        .map(|value| {
            value
                .split(':')
                .filter(|tag| !tag.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    shift_levels(&mut lines, true);
    lines.retain(|line| {
        keyword_value(line, "title").is_none() && keyword_value(line, "filetags").is_none()
    });

    let heading = if tags.is_empty() {
        format!("* {title}")
    } else {
        format!("* {title}  :{}:", tags.join(":"))
    };
    lines.insert(0, heading);

    Ok(rejoin(&lines, text))
}

/// A subtree taken out of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extraction {
    /// The source file with the subtree removed.
    pub remaining: String,
    /// The new file's whole content, as a file-level node.
    pub extracted: String,
    /// The node's id, created if the subtree did not have one.
    pub id: Uuid,
    /// The subtree's headline text, for naming the new file.
    pub title: String,
}

/// Cuts the subtree containing `line` and turns it into a file-level node.
///
/// `new_id` supplies the id when the subtree has none; it is a parameter
/// rather than generated here so the caller stays in control of randomness
/// and the result is testable.
pub fn extract_subtree(text: &str, line: usize, new_id: Uuid) -> Result<Extraction, Error> {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();

    // The headline at or above the cursor.
    let start = lines[..=line.min(lines.len().saturating_sub(1))]
        .iter()
        .rposition(|l| headline_level(l).is_some())
        .ok_or(Error::NoSubtree)?;
    let level = headline_level(&lines[start]).ok_or(Error::NoSubtree)?;

    let end = lines[start + 1..]
        .iter()
        .position(|l| headline_level(l).is_some_and(|other| other <= level))
        .map_or(lines.len(), |offset| start + 1 + offset);

    let mut subtree: Vec<String> = lines[start..end].to_vec();
    // Raise it until its headline is level 1, so it can become a file node.
    for _ in 1..level {
        shift_levels(&mut subtree, false);
    }

    let (title, _) = split_tags(headline_text(&subtree[0]));
    let title = title.to_string();

    let id = existing_id(&subtree).unwrap_or(new_id);
    if existing_id(&subtree).is_none() {
        // A node needs an id before it can be linked to, and the drawer sits
        // directly under its headline.
        subtree.splice(
            1..1,
            [
                ":PROPERTIES:".to_string(),
                format!(":ID:       {id}"),
                ":END:".to_string(),
            ],
        );
    }

    let extracted = promote_buffer(&subtree.join("\n"))?;

    let mut remaining = lines;
    remaining.drain(start..end);

    Ok(Extraction {
        remaining: rejoin(&remaining, text),
        extracted: if extracted.ends_with('\n') {
            extracted
        } else {
            format!("{extracted}\n")
        },
        id,
        title,
    })
}

/// The `:ID:` of the drawer directly under the first headline, if any.
fn existing_id(subtree: &[String]) -> Option<Uuid> {
    let mut in_drawer = false;
    for line in subtree.iter().skip(1) {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case(":PROPERTIES:") {
            in_drawer = true;
            continue;
        }
        if !in_drawer {
            // The drawer must follow the headline; anything else ends the search.
            if trimmed.is_empty() {
                continue;
            }
            return None;
        }
        if trimmed.eq_ignore_ascii_case(":END:") {
            return None;
        }
        if let Some(rest) = trimmed.strip_prefix(':') {
            if let Some((key, value)) = rest.split_once(':') {
                if key.eq_ignore_ascii_case("id") {
                    return Uuid::parse_str(value.trim()).ok();
                }
            }
        }
    }
    None
}

/// A subtree taken out of a file and sent to an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archived {
    /// The source file with the subtree removed.
    pub remaining: String,
    /// The subtree, with a note recording where it came from.
    pub archived: String,
    /// Where it should go, from `#+ARCHIVE:` or the default.
    pub target: String,
}

/// Cuts the subtree at `line` and prepares it for the archive.
///
/// `origin` names the file it came from, which goes into the archived copy:
/// a subtree that has lost its context is hard to place again.
pub fn archive_subtree(
    text: &str,
    line: usize,
    settings: &FileSettings,
    origin: &str,
) -> Result<Archived, Error> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (start, end, _) = subtree_range(&lines, line).ok_or(Error::NoSubtree)?;

    let mut archived: Vec<String> = lines[start..end].to_vec();
    // The note goes under the headline, where Org puts it.
    archived.insert(1, format!(":ARCHIVE_FILE: {origin}"));

    lines.drain(start..end);

    // `#+ARCHIVE:` may carry a `::headline` part, which naming a file does not
    // need; only the file half is used here.
    let target = settings
        .archive
        .as_deref()
        .map(|archive| {
            archive
                .split("::")
                .next()
                .unwrap_or(archive)
                .trim()
                .to_string()
        })
        .filter(|target| !target.is_empty())
        .unwrap_or_else(|| format!("{origin}_archive"));

    Ok(Archived {
        remaining: rejoin(&lines, text),
        archived: format!("{}\n", archived.join("\n")),
        target,
    })
}

/// Moves the headline at `line` to the next TODO state the file declares.
///
/// The sequence is the file's own, followed by "no state at all", so cycling
/// past the last keyword clears it the way Org does. Which words are keywords
/// comes from [`FileSettings`], never from a guess.
pub fn cycle_todo(
    text: &str,
    line: usize,
    settings: &FileSettings,
    forward: bool,
) -> Result<String, Error> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (start, _, level) = subtree_range(&lines, line).ok_or(Error::NoSubtree)?;

    // The cycle: every keyword, then nothing.
    let mut states: Vec<Option<&str>> = settings
        .todo_keywords
        .iter()
        .chain(&settings.done_keywords)
        .map(|keyword| Some(keyword.as_str()))
        .collect();
    states.push(None);

    let rest = headline_text(&lines[start]);
    let current = rest
        .split_once(char::is_whitespace)
        .map(|(first, _)| first)
        .unwrap_or(rest);
    let current = settings.is_todo_keyword(current).then_some(current);

    let at = states
        .iter()
        .position(|state| *state == current)
        .unwrap_or(states.len() - 1);
    let next = if forward {
        states[(at + 1) % states.len()]
    } else {
        states[(at + states.len() - 1) % states.len()]
    };

    // Rebuild the headline: stars, new state, then what was already there.
    let without_state = match current {
        Some(keyword) => rest[keyword.len()..].trim_start(),
        None => rest,
    };
    let stars = "*".repeat(level);
    lines[start] = match next {
        Some(state) => format!("{stars} {state} {without_state}")
            .trim_end()
            .to_string(),
        None => format!("{stars} {without_state}").trim_end().to_string(),
    };

    Ok(rejoin(&lines, text))
}

/// Sets or clears the priority cookie on the headline at `line`.
pub fn set_priority(
    text: &str,
    line: usize,
    settings: &FileSettings,
    priority: Option<char>,
) -> Result<String, Error> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (start, _, level) = subtree_range(&lines, line).ok_or(Error::NoSubtree)?;

    if let Some(letter) = priority {
        if !settings.priorities.contains(&letter) {
            return Err(Error::UndeclaredPriority(letter));
        }
    }

    let rest = headline_text(&lines[start]);
    // Keyword, then cookie, then title: only the cookie changes.
    let (keyword, after_keyword) = match rest.split_once(char::is_whitespace) {
        Some((first, tail)) if settings.is_todo_keyword(first) => (Some(first), tail.trim_start()),
        _ => (None, rest),
    };
    let after_cookie = after_keyword
        .strip_prefix("[#")
        .and_then(|tail| tail.split_once(']'))
        .filter(|(letter, _)| {
            letter.chars().count() == 1
                && letter
                    .chars()
                    .next()
                    .is_some_and(|c| settings.priorities.contains(&c))
        })
        .map_or(after_keyword, |(_, tail)| tail.trim_start());

    let mut headline = "*".repeat(level);
    if let Some(keyword) = keyword {
        headline.push(' ');
        headline.push_str(keyword);
    }
    if let Some(letter) = priority {
        headline.push_str(&format!(" [#{letter}]"));
    }
    if !after_cookie.is_empty() {
        headline.push(' ');
        headline.push_str(after_cookie);
    }
    lines[start] = headline;

    Ok(rejoin(&lines, text))
}

/// Moves the priority up or down the file's declared range.
///
/// `raise` means towards `A`, which is how Org names it even though the letter
/// goes down.
pub fn change_priority(
    text: &str,
    line: usize,
    settings: &FileSettings,
    raise: bool,
) -> Result<String, Error> {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (start, _, _) = subtree_range(&lines, line).ok_or(Error::NoSubtree)?;

    let rest = headline_text(&lines[start]);
    let after_keyword = match rest.split_once(char::is_whitespace) {
        Some((first, tail)) if settings.is_todo_keyword(first) => tail.trim_start(),
        _ => rest,
    };
    let current = after_keyword
        .strip_prefix("[#")
        .and_then(|tail| tail.split_once(']'))
        .and_then(|(letter, _)| letter.chars().next())
        .filter(|c| settings.priorities.contains(c));

    let next = match current {
        None => settings.priorities.first().copied(),
        Some(letter) => {
            let at = settings.priorities.iter().position(|c| *c == letter);
            at.and_then(|at| {
                if raise {
                    at.checked_sub(1).and_then(|at| settings.priorities.get(at))
                } else {
                    settings.priorities.get(at + 1)
                }
                .copied()
            })
            // Off either end, the cookie goes away rather than sticking.
            .or(None)
        }
    };

    set_priority(text, line, settings, next)
}

/// Which planning timestamp to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Planning {
    Scheduled,
    Deadline,
}

impl Planning {
    fn label(self) -> &'static str {
        match self {
            Planning::Scheduled => "SCHEDULED:",
            Planning::Deadline => "DEADLINE:",
        }
    }
}

/// Sets or clears a planning timestamp on the entry at `line`.
///
/// `stamp` is the whole `<…>`, so the caller decides whether it carries a time
/// or a repeater; `None` removes the entry.
pub fn set_planning(
    text: &str,
    line: usize,
    which: Planning,
    stamp: Option<&str>,
) -> Result<String, Error> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (start, end, _) = subtree_range(&lines, line).ok_or(Error::NoSubtree)?;

    // The planning line is the one directly under the headline, if any.
    let planning_at = (start + 1 < end)
        .then(|| start + 1)
        .filter(|at| is_planning_line(&lines[*at]));

    let mut parts: Vec<(Planning, String)> = Vec::new();
    if let Some(at) = planning_at {
        for other in [Planning::Scheduled, Planning::Deadline] {
            if other != which {
                if let Some(existing) = planning_part(&lines[at], other) {
                    parts.push((other, existing));
                }
            }
        }
    }
    if let Some(stamp) = stamp {
        parts.push((which, stamp.to_string()));
    }
    // Org writes DEADLINE before SCHEDULED.
    parts.sort_by_key(|(which, _)| match which {
        Planning::Deadline => 0,
        Planning::Scheduled => 1,
    });

    let rendered = parts
        .iter()
        .map(|(which, stamp)| format!("{} {stamp}", which.label()))
        .collect::<Vec<_>>()
        .join(" ");

    match (planning_at, rendered.is_empty()) {
        (Some(at), true) => {
            lines.remove(at);
        }
        (Some(at), false) => lines[at] = rendered,
        (None, false) => lines.insert(start + 1, rendered),
        (None, true) => {}
    }

    Ok(rejoin(&lines, text))
}

/// Whether a line is a planning line rather than body text.
fn is_planning_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    ["SCHEDULED:", "DEADLINE:", "CLOSED:"]
        .iter()
        .any(|label| starts_with_ignore_case(trimmed, label))
}

/// The `<…>` stamp a planning line carries for `which`, if any.
fn planning_part(line: &str, which: Planning) -> Option<String> {
    let at = find_ignore_case(line, which.label())?;
    let rest = line[at + which.label().len()..].trim_start();

    let open = rest.chars().next()?;
    let close = match open {
        '<' => '>',
        '[' => ']',
        _ => return None,
    };
    let end = rest.find(close)?;

    Some(rest[..=end].to_string())
}

/// The lines of the subtree containing `line`, and its level.
///
/// Extracting, refiling and the structure commands all ask this same
/// question, so it is answered in one place.
fn subtree_range(lines: &[String], line: usize) -> Option<(usize, usize, usize)> {
    let at = line.min(lines.len().saturating_sub(1));
    let start = lines[..=at]
        .iter()
        .rposition(|l| headline_level(l).is_some())?;
    let level = headline_level(&lines[start])?;

    let end = lines[start + 1..]
        .iter()
        .position(|l| headline_level(l).is_some_and(|other| other <= level))
        .map_or(lines.len(), |offset| start + 1 + offset);

    Some((start, end, level))
}

/// Inserts a sibling headline after the subtree containing `line`.
///
/// Returns the new text and the line the new headline is on, so the caller can
/// put the cursor there.
pub fn insert_heading(text: &str, line: usize) -> (String, usize) {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();

    let (at, level) = match subtree_range(&lines, line) {
        Some((_, end, level)) => (end, level),
        // Above any headline, a new one starts at the top level, after the
        // preamble rather than inside it.
        None => (lines.len(), 1),
    };

    lines.insert(at, format!("{} ", "*".repeat(level)));
    (rejoin(&lines, text), at)
}

/// Moves one headline a level in or out, leaving its children where they are.
pub fn shift_heading(text: &str, line: usize, deeper: bool) -> Result<String, Error> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (start, _, level) = subtree_range(&lines, line).ok_or(Error::NoSubtree)?;

    if !deeper && level <= 1 {
        return Err(Error::AlreadyTopLevel);
    }
    shift_levels(&mut lines[start..=start], deeper);

    Ok(rejoin(&lines, text))
}

/// Moves a headline and everything under it a level in or out.
pub fn shift_subtree(text: &str, line: usize, deeper: bool) -> Result<String, Error> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (start, end, level) = subtree_range(&lines, line).ok_or(Error::NoSubtree)?;

    if !deeper && level <= 1 {
        return Err(Error::AlreadyTopLevel);
    }
    shift_levels(&mut lines[start..end], deeper);

    Ok(rejoin(&lines, text))
}

/// Swaps the subtree containing `line` with its previous or next sibling.
///
/// Returns the new text and the line the moved subtree now starts on.
pub fn move_subtree(text: &str, line: usize, up: bool) -> Result<(String, usize), Error> {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (start, end, level) = subtree_range(&lines, line).ok_or(Error::NoSubtree)?;

    let sibling = if up {
        // The nearest headline above at the same level, and not past a
        // shallower one, which would be leaving the parent.
        lines[..start]
            .iter()
            .rposition(|l| headline_level(l).is_some_and(|other| other <= level))
            .filter(|at| headline_level(&lines[*at]) == Some(level))
            .map(|sibling_start| (sibling_start, start))
    } else {
        (end < lines.len() && headline_level(&lines[end]) == Some(level)).then(|| {
            let (_, sibling_end, _) = subtree_range(&lines, end).unwrap();
            (end, sibling_end)
        })
    };

    let Some((other_start, other_end)) = sibling else {
        return Err(Error::NoSibling);
    };

    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let moved_to;
    if up {
        out.extend_from_slice(&lines[..other_start]);
        moved_to = out.len();
        out.extend_from_slice(&lines[start..end]);
        out.extend_from_slice(&lines[other_start..other_end]);
        out.extend_from_slice(&lines[end..]);
    } else {
        out.extend_from_slice(&lines[..start]);
        out.extend_from_slice(&lines[other_start..other_end]);
        moved_to = out.len();
        out.extend_from_slice(&lines[start..end]);
        out.extend_from_slice(&lines[other_end..]);
    }

    Ok((rejoin(&out, text), moved_to))
}

/// Sets a single-valued property on the entry at `line`.
///
/// Unlike [`edit_property`], which manages a list, this replaces the value
/// outright — which is what `:EFFORT:` and a user's own keys want.
pub fn set_property(text: &str, line: usize, property: &str, value: &str) -> Result<String, Error> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (start, end) = locate_drawer(&lines, line)?;

    let rendered = format!(":{property}: {value}");
    match find_property(&lines, start, end, property) {
        Some(at) => lines[at] = rendered,
        None => lines.insert(end, rendered),
    }

    Ok(rejoin(&lines, text))
}

/// Removes a property from the entry at `line`.
///
/// Returns `None` when it was not there, so the caller can say so rather than
/// marking the buffer modified for nothing.
pub fn remove_property(text: &str, line: usize, property: &str) -> Result<Option<String>, Error> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (start, end) = locate_drawer(&lines, line)?;

    let Some(at) = find_property(&lines, start, end, property) else {
        return Ok(None);
    };
    lines.remove(at);

    Ok(Some(rejoin(&lines, text)))
}

/// The value of a property on the entry at `line`.
pub fn property_value(text: &str, line: usize, property: &str) -> Option<String> {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (start, end) = locate_drawer(&lines, line).ok()?;
    let at = find_property(&lines, start, end, property)?;

    lines[at]
        .trim()
        .strip_prefix(':')
        .and_then(|rest| rest.split_once(':'))
        .map(|(_, value)| value.trim().to_string())
}

/// Every property key used anywhere in `text`, for completion.
pub fn property_keys(text: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let mut in_drawer = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case(":PROPERTIES:") {
            in_drawer = true;
            continue;
        }
        if trimmed.eq_ignore_ascii_case(":END:") {
            in_drawer = false;
            continue;
        }
        if !in_drawer {
            continue;
        }
        if let Some((key, _)) = trimmed.strip_prefix(':').and_then(|r| r.split_once(':')) {
            let key = key.to_uppercase();
            if !key.is_empty() && !keys.contains(&key) {
                keys.push(key);
            }
        }
    }

    keys.sort_unstable();
    keys
}

/// The drawer of the entry containing `line`.
fn locate_drawer(lines: &[String], line: usize) -> Result<(usize, usize), Error> {
    let at = line.min(lines.len().saturating_sub(1));
    let headline = lines[..=at]
        .iter()
        .rposition(|l| headline_level(l).is_some());
    drawer_range(lines, headline.map_or(0, |at| at + 1)).ok_or(Error::NoDrawer)
}

/// Line holding `property` within a drawer, if any.
fn find_property(lines: &[String], start: usize, end: usize, property: &str) -> Option<usize> {
    lines[start..end]
        .iter()
        .position(|l| {
            l.trim()
                .strip_prefix(':')
                .and_then(|rest| rest.split_once(':'))
                .is_some_and(|(key, _)| key.eq_ignore_ascii_case(property))
        })
        .map(|offset| start + offset)
}

/// Inserts an empty drawer under the entry at `line`.
pub fn insert_drawer(text: &str, line: usize, name: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let at = line.min(lines.len().saturating_sub(1));

    // After the headline and its property drawer, if it has one.
    let headline = lines[..=at]
        .iter()
        .rposition(|l| headline_level(l).is_some());
    let after = match headline {
        Some(headline_at) => match drawer_range(&lines, headline_at + 1) {
            Some((_, end)) => end + 1,
            None => headline_at + 1,
        },
        None => 0,
    };

    let name = name.trim().trim_matches(':').to_uppercase();
    lines.splice(
        after..after,
        [format!(":{name}:"), String::new(), ":END:".to_string()],
    );

    rejoin(&lines, text)
}

/// Appends an entry to the `:LOGBOOK:` drawer of the entry at `line`.
///
/// The drawer is created when there is none. Entries go at the top, newest
/// first, which is how Org writes them.
pub fn log_entry(text: &str, line: usize, entry: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let at = line.min(lines.len().saturating_sub(1));
    let headline = lines[..=at]
        .iter()
        .rposition(|l| headline_level(l).is_some());

    // The logbook sits after the headline's property drawer, if it has one.
    let search_from = headline.map_or(0, |at| at + 1);
    let after_properties = match drawer_range(&lines, search_from) {
        Some((_, end)) => end + 1,
        None => search_from,
    };

    let existing = lines[after_properties..]
        .iter()
        .position(|l| l.trim().eq_ignore_ascii_case(":LOGBOOK:"))
        .map(|offset| after_properties + offset)
        // Only a logbook directly under the entry belongs to it.
        .filter(|at| {
            lines[after_properties..*at]
                .iter()
                .all(|l| l.trim().is_empty())
        });

    match existing {
        Some(at) => lines.insert(at + 1, entry.to_string()),
        None => {
            lines.splice(
                after_properties..after_properties,
                [
                    ":LOGBOOK:".to_string(),
                    entry.to_string(),
                    ":END:".to_string(),
                ],
            );
        }
    }

    rejoin(&lines, text)
}

/// Renames the entry at `line`.
///
/// A headline's title is its own line; a file node's is `#+title:`. Only the
/// title changes here — the links pointing at the node keep working because
/// they carry its id, not its name.
pub fn rename_entry(text: &str, line: usize, new_title: &str) -> Result<String, Error> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let at = line.min(lines.len().saturating_sub(1));
    let headline = lines[..=at]
        .iter()
        .rposition(|l| headline_level(l).is_some());

    match headline {
        Some(at) => {
            let stars = "*".repeat(headline_level(&lines[at]).unwrap_or(1));
            let (_, tags) = split_tags(headline_text(&lines[at]));
            lines[at] = if tags.is_empty() {
                format!("{stars} {new_title}")
            } else {
                format!("{stars} {new_title}  :{}:", tags.join(":"))
            };
        }
        None => {
            let at = lines
                .iter()
                .position(|l| keyword_value(l, "title").is_some())
                .ok_or(Error::NoTitle)?;
            lines[at] = format!("#+title: {new_title}");
        }
    }

    Ok(rejoin(&lines, text))
}

/// Retitles the `[[id:…][…]]` links in `text` that still describe `id` by its
/// old name.
///
/// A description someone wrote by hand is left alone: only the ones that
/// repeated the title are updated, so renaming does not silently rewrite
/// prose.
pub fn retitle_links(text: &str, id: Uuid, old_title: &str, new_title: &str) -> Option<String> {
    let needle = format!("[[id:{id}][{old_title}]]");
    if !text.contains(&needle) {
        return None;
    }

    Some(text.replace(&needle, &format!("[[id:{id}][{new_title}]]")))
}

/// Adds or removes a tag on the entry at `line`.
///
/// Tags are not a drawer property: a headline carries them as a trailing
/// `:a:b:` run, and a file-level node carries them in `#+filetags:`. Both are
/// handled here so the caller does not have to know which kind of entry it is
/// looking at.
pub fn edit_tag(text: &str, line: usize, tag: &str, add: bool) -> Result<Option<String>, Error> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let at = line.min(lines.len().saturating_sub(1));
    let headline = lines[..=at]
        .iter()
        .rposition(|l| headline_level(l).is_some());

    let (mut tags, target) = match headline {
        Some(at) => {
            let (_, tags) = split_tags(headline_text(&lines[at]));
            (tags, Some(at))
        }
        None => {
            let at = lines
                .iter()
                .position(|l| keyword_value(l, "filetags").is_some());
            let tags = at
                .map(|at| {
                    keyword_value(&lines[at], "filetags")
                        .unwrap_or_default()
                        .split(':')
                        .filter(|tag| !tag.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            (tags, at)
        }
    };

    let present = tags.iter().any(|t| t == tag);
    if add == present {
        return Ok(None);
    }
    if add {
        tags.push(tag.to_string());
    } else {
        tags.retain(|t| t != tag);
    }

    match headline {
        Some(at) => {
            let stars = "*".repeat(headline_level(&lines[at]).unwrap_or(1));
            let (title, _) = split_tags(headline_text(&lines[at]));
            lines[at] = if tags.is_empty() {
                format!("{stars} {title}")
            } else {
                format!("{stars} {title}  :{}:", tags.join(":"))
            };
        }
        None => {
            let rendered = format!("#+filetags: :{}:", tags.join(":"));
            match (target, tags.is_empty()) {
                (Some(at), true) => {
                    lines.remove(at);
                }
                (Some(at), false) => lines[at] = rendered,
                // No keyword yet: it goes after `#+title:` when there is one.
                (None, _) => {
                    let after = lines
                        .iter()
                        .position(|l| keyword_value(l, "title").is_some())
                        .map_or(0, |at| at + 1);
                    lines.insert(after, rendered);
                }
            }
        }
    }

    Ok(Some(rejoin(&lines, text)))
}

/// Adds or removes a value in a multi-valued property of the entry at `line`.
///
/// `:ROAM_ALIASES:` and `:ROAM_REFS:` hold space-separated lists whose entries
/// are quoted when they contain a space, so editing one means reading the list,
/// changing it and writing it back rather than appending text.
///
/// Returns `None` when there was nothing to do — the value was already there,
/// or was not there to remove — so the caller can say so instead of marking
/// the buffer modified for nothing.
pub fn edit_property(
    text: &str,
    line: usize,
    property: &str,
    value: &str,
    add: bool,
) -> Result<Option<String>, Error> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let at = line.min(lines.len().saturating_sub(1));

    let headline = lines[..=at]
        .iter()
        .rposition(|l| headline_level(l).is_some());
    let search_from = headline.map_or(0, |at| at + 1);

    let Some((drawer_start, drawer_end)) = drawer_range(&lines, search_from) else {
        return Err(Error::NoDrawer);
    };

    // The property's current values, and where its line is.
    let existing = lines[drawer_start..drawer_end]
        .iter()
        .position(|l| {
            l.trim()
                .strip_prefix(':')
                .and_then(|rest| rest.split_once(':'))
                .is_some_and(|(key, _)| key.eq_ignore_ascii_case(property))
        })
        .map(|offset| drawer_start + offset);

    let mut values: Vec<String> = existing
        .map(|at| {
            let raw = lines[at]
                .trim()
                .strip_prefix(':')
                .and_then(|rest| rest.split_once(':'))
                .map(|(_, value)| value)
                .unwrap_or_default();
            parse_quoted_values(raw)
        })
        .unwrap_or_default();

    let present = values.iter().any(|v| v == value);
    if add == present {
        return Ok(None);
    }

    if add {
        values.push(value.to_string());
    } else {
        values.retain(|v| v != value);
    }

    let rendered = format!(":{property}: {}", render_quoted_values(&values));

    match (existing, values.is_empty()) {
        // Removing the last value removes the property line with it.
        (Some(at), true) => {
            lines.remove(at);
        }
        (Some(at), false) => lines[at] = rendered,
        (None, _) => lines.insert(drawer_end, rendered),
    }

    Ok(Some(rejoin(&lines, text)))
}

/// Start and end (exclusive, not counting `:END:`) of the drawer opening at or
/// just after `from`.
fn drawer_range(lines: &[String], from: usize) -> Option<(usize, usize)> {
    let start = lines[from..]
        .iter()
        .position(|l| l.trim().eq_ignore_ascii_case(":PROPERTIES:"))
        .map(|offset| from + offset + 1)?;

    // The drawer must follow the headline, not appear further down the file.
    if lines[from..start - 1].iter().any(|l| !l.trim().is_empty()) {
        return None;
    }

    let end = lines[start..]
        .iter()
        .position(|l| l.trim().eq_ignore_ascii_case(":END:"))
        .map(|offset| start + offset)?;

    Some((start, end))
}

/// Splits `"a b" c` into its values, honouring the quotes Org uses.
fn parse_quoted_values(raw: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut quoted = false;

    for c in raw.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    values.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        values.push(current);
    }

    values
}

/// Renders values back, quoting the ones that need it.
fn render_quoted_values(values: &[String]) -> String {
    values
        .iter()
        .map(|value| {
            if value.chars().any(char::is_whitespace) {
                format!("\"{value}\"")
            } else {
                value.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The id and headline of the entry containing `line`, when it has one.
///
/// "Entry" is the headline at or above the line, or the preamble when there is
/// none. This is not the same question as finding the node whose indexed line
/// is nearest: [`crate::Node::line`] points at the `:ID:` property rather than
/// at the headline, so a cursor sitting on the headline itself is *before* its
/// own node by that measure.
pub fn entry_at(text: &str, line: usize) -> Option<(Uuid, String)> {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let at = line.min(lines.len().saturating_sub(1));

    let headline = lines[..=at]
        .iter()
        .rposition(|l| headline_level(l).is_some());

    let (scope, title) = match headline {
        Some(start) => {
            let (title, _) = split_tags(headline_text(&lines[start]));
            (lines[start..].to_vec(), title.to_string())
        }
        None => {
            // The preamble node, whose drawer sits at the top of the file.
            let mut scope = vec![String::new()];
            scope.extend(lines.iter().cloned());
            let title = lines
                .iter()
                .find_map(|l| keyword_value(l, "title"))
                .unwrap_or_default()
                .to_string();
            (scope, title)
        }
    };

    existing_id(&scope).map(|id| (id, title))
}

/// What giving an entry an id did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdOutcome {
    /// The entry already had one; nothing was changed.
    Existing(Uuid),
    /// A drawer was added, and here is the file that results.
    Created { text: String, id: Uuid },
}

/// Ensures the entry containing `line` has an `:ID:`.
///
/// The entry is the headline at or above the line, or the file's preamble when
/// there is none — Org-Roam treats both as nodes, so both can be linked to.
pub fn ensure_id(text: &str, line: usize, new_id: Uuid) -> Result<IdOutcome, Error> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();

    let headline = lines[..=line.min(lines.len().saturating_sub(1))]
        .iter()
        .rposition(|l| headline_level(l).is_some());

    // The drawer goes directly under the headline, or at the very top for the
    // preamble node.
    let insert_at = headline.map_or(0, |at| at + 1);
    let scope: Vec<String> = match headline {
        Some(at) => lines[at..].to_vec(),
        None => {
            let mut scope = vec![String::new()];
            scope.extend(lines.iter().cloned());
            scope
        }
    };

    if let Some(id) = existing_id(&scope) {
        return Ok(IdOutcome::Existing(id));
    }

    lines.splice(
        insert_at..insert_at,
        [
            ":PROPERTIES:".to_string(),
            format!(":ID:       {new_id}"),
            ":END:".to_string(),
        ],
    );

    Ok(IdOutcome::Created {
        text: rejoin(&lines, text),
        id: new_id,
    })
}

/// A subtree moved from one file to another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refiling {
    /// The source file with the subtree removed.
    pub source: String,
    /// The target file with the subtree added beneath its node.
    pub target: String,
    /// The moved subtree's headline, for reporting what happened.
    pub title: String,
}

/// Moves the subtree at `line` in `source` under the node `target_id` names.
///
/// The subtree is re-levelled to sit directly beneath the target and is placed
/// after everything already under it, so refiling twice does not interleave.
/// Ids travel with the text, so links into the moved subtree keep resolving —
/// which is the whole reason this is a move rather than a copy and a delete.
pub fn refile_subtree(
    source: &str,
    line: usize,
    target: &str,
    target_id: Uuid,
) -> Result<Refiling, Error> {
    let source_lines: Vec<String> = source.lines().map(str::to_string).collect();

    let start = source_lines[..=line.min(source_lines.len().saturating_sub(1))]
        .iter()
        .rposition(|l| headline_level(l).is_some())
        .ok_or(Error::NoSubtree)?;
    let level = headline_level(&source_lines[start]).ok_or(Error::NoSubtree)?;
    let end = source_lines[start + 1..]
        .iter()
        .position(|l| headline_level(l).is_some_and(|other| other <= level))
        .map_or(source_lines.len(), |offset| start + 1 + offset);

    let mut subtree: Vec<String> = source_lines[start..end].to_vec();
    let (title, _) = split_tags(headline_text(&subtree[0]));
    let title = title.to_string();

    let mut target_lines: Vec<String> = target.lines().map(str::to_string).collect();
    let (target_level, insert_at) = locate_target(&target_lines, target_id)?;

    // Re-level so the subtree's root sits one below its new parent.
    let wanted = target_level + 1;
    while headline_level(&subtree[0]).is_some_and(|l| l > wanted) {
        shift_levels(&mut subtree, false);
    }
    while headline_level(&subtree[0]).is_some_and(|l| l < wanted) {
        shift_levels(&mut subtree, true);
    }

    let mut moved = source_lines;
    moved.drain(start..end);
    target_lines.splice(insert_at..insert_at, subtree);

    Ok(Refiling {
        source: rejoin(&moved, source),
        target: rejoin(&target_lines, target),
        title,
    })
}

/// The target node's level, and the line its subtree ends at.
///
/// A file-level node has no headline of its own, so it is level 0 and its
/// subtree is the whole file.
fn locate_target(lines: &[String], target_id: Uuid) -> Result<(usize, usize), Error> {
    let wanted = target_id.to_string();
    let id_line = lines
        .iter()
        .position(|line| {
            line.trim()
                .strip_prefix(':')
                .and_then(|rest| rest.split_once(':'))
                .is_some_and(|(key, value)| {
                    key.eq_ignore_ascii_case("id") && value.trim() == wanted
                })
        })
        .ok_or(Error::NoTarget)?;

    let headline = lines[..id_line]
        .iter()
        .rposition(|line| headline_level(line).is_some());

    match headline.and_then(|at| headline_level(&lines[at]).map(|level| (at, level))) {
        Some((at, level)) => {
            let end = lines[at + 1..]
                .iter()
                .position(|l| headline_level(l).is_some_and(|other| other <= level))
                .map_or(lines.len(), |offset| at + 1 + offset);
            Ok((level, end))
        }
        // No headline above the id: this is the file-level node.
        None => Ok((0, lines.len())),
    }
}

/// Rewrites `[[roam:Title]]` links as `[[id:…]]` ones.
///
/// `resolve` maps a title to a node, and a title it does not know is left
/// alone rather than turned into a link that goes nowhere.
pub fn replace_roam_links(text: &str, resolve: impl Fn(&str) -> Option<Uuid>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(at) = rest.find("[[roam:") {
        out.push_str(&rest[..at]);
        let after = &rest[at + "[[roam:".len()..];

        let Some(close) = after.find("]]") else {
            out.push_str(&rest[at..]);
            return out;
        };

        let body = &after[..close];
        // `[[roam:Title][description]]` keeps its description.
        let (target, description) = match body.split_once("][") {
            Some((target, description)) => (target, description),
            None => (body, body),
        };

        match resolve(target.trim()) {
            Some(id) => out.push_str(&format!("[[id:{id}][{description}]]")),
            None => out.push_str(&rest[at..at + "[[roam:".len() + close + 2]),
        }
        rest = &after[close + 2..];
    }

    out.push_str(rest);
    out
}

/// Joins lines back, preserving whether the original ended with a newline.
fn rejoin(lines: &[String], original: &str) -> String {
    let joined = lines.join("\n");
    if original.ends_with('\n') && !joined.is_empty() {
        format!("{joined}\n")
    } else {
        joined
    }
}
