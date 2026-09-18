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
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotPromotable(why) => write!(f, "cannot promote: {why}"),
            Error::NoSubtree => write!(f, "no subtree at the cursor"),
            Error::AlreadyTopLevel => write!(f, "already a top-level node"),
            Error::NoTitle => write!(f, "the file has no #+title: to demote"),
            Error::NoTarget => write!(f, "the target node was not found in its file"),
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
