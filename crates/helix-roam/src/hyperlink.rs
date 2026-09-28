//! Org's link syntax, as the editor needs to understand it.
//!
//! The indexer only ever asked one question of a link — does it point at a
//! node — and [`crate::parser::LinkTarget`] answers exactly that. Following a
//! link needs more: what kind of thing it points at, and where in the text it
//! sits so the cursor can be tested against it.

use std::ops::Range;

use uuid::Uuid;

use crate::parser::{find_close, find_from, parse_id};

/// Where inside a file a `file:` link wants to land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileSearch {
    /// `::42` — a line number, one-based as Org writes it.
    Line(usize),
    /// `::*Headline`
    Headline(String),
    /// `::text to find`
    Text(String),
}

/// What a link points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkKind {
    /// `id:` — a node in the graph.
    Id(Uuid),
    /// `file:`, or a bare path.
    File {
        path: String,
        search: Option<FileSearch>,
    },
    /// Anything with a URL scheme the system should open.
    Url(String),
    /// `mailto:`
    Mailto(String),
    /// `*Headline` — a headline in this file.
    Headline(String),
    /// `#custom-id` — a `:CUSTOM_ID:` in this file.
    CustomId(String),
    /// A dedicated `<<target>>`, or a fuzzy match on text.
    Target(String),
    /// `roam:` — a v1 title link, which Task 1.17 migrates.
    Roam(String),
    /// A scheme nothing here handles, kept whole so it can be reported.
    Other { scheme: String, rest: String },
}

/// A link found in a buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgLink {
    pub kind: LinkKind,
    /// The `[[target][description]]` description, when it has one.
    pub description: Option<String>,
    /// Byte range of the whole `[[…]]` within the text it was found in.
    pub range: Range<usize>,
}

impl OrgLink {
    /// The text to show for this link: its description, or its target.
    pub fn label(&self) -> String {
        self.description
            .clone()
            .unwrap_or_else(|| match &self.kind {
                LinkKind::Id(id) => id.to_string(),
                LinkKind::File { path, .. } => path.clone(),
                LinkKind::Url(url) => url.clone(),
                LinkKind::Mailto(who) => who.clone(),
                LinkKind::Headline(title) => format!("*{title}"),
                LinkKind::CustomId(id) => format!("#{id}"),
                LinkKind::Target(target) => target.clone(),
                LinkKind::Roam(title) => title.clone(),
                LinkKind::Other { scheme, rest } => format!("{scheme}:{rest}"),
            })
    }
}

/// Schemes that mean "hand this to the system".
const URL_SCHEMES: [&str; 6] = ["http", "https", "ftp", "ftps", "news", "irc"];

/// Reads a link target, expanding any abbreviation that applies.
///
/// `abbreviations` comes from the file's `#+LINK:` lines, so the same text can
/// mean different things in different files — which is why this takes them
/// rather than consulting a global.
pub fn parse_target(raw: &str, abbreviations: &[(String, String)]) -> LinkKind {
    let raw = expand_abbreviation(raw.trim(), abbreviations);
    let raw = raw.as_str();

    if let Some(rest) = raw.strip_prefix('*') {
        return LinkKind::Headline(rest.trim().to_string());
    }
    if let Some(rest) = raw.strip_prefix('#') {
        return LinkKind::CustomId(rest.trim().to_string());
    }

    let Some((scheme, rest)) = split_scheme(raw) else {
        // No scheme at all: a path if it looks like one, a target otherwise.
        return if raw.contains('/') || raw.starts_with('.') || raw.ends_with(".org") {
            let (path, search) = split_file_search(raw);
            LinkKind::File { path, search }
        } else {
            LinkKind::Target(raw.to_string())
        };
    };

    match scheme {
        "id" => LinkKind::Id(parse_id(rest.trim())),
        "roam" => LinkKind::Roam(rest.trim().to_string()),
        "mailto" => LinkKind::Mailto(rest.trim().to_string()),
        "file" => {
            let (path, search) = split_file_search(rest);
            LinkKind::File { path, search }
        }
        _ if URL_SCHEMES.contains(&scheme) => LinkKind::Url(raw.to_string()),
        _ => LinkKind::Other {
            scheme: scheme.to_string(),
            rest: rest.to_string(),
        },
    }
}

/// Splits `scheme:rest`, rejecting a lone `:` that is not a scheme.
fn split_scheme(raw: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = raw.split_once(':')?;
    let looks_like_scheme = !scheme.is_empty()
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));

    looks_like_scheme.then_some((scheme, rest))
}

/// Splits `path::search` into its parts.
fn split_file_search(rest: &str) -> (String, Option<FileSearch>) {
    let Some((path, search)) = rest.split_once("::") else {
        return (rest.to_string(), None);
    };

    let search = if let Ok(line) = search.parse::<usize>() {
        FileSearch::Line(line)
    } else if let Some(headline) = search.strip_prefix('*') {
        FileSearch::Headline(headline.trim().to_string())
    } else {
        FileSearch::Text(search.to_string())
    };

    (path.to_string(), Some(search))
}

/// Applies a `#+LINK:` abbreviation, if one matches.
///
/// `%s` is where the tail goes; without it the tail is appended, which is what
/// Org does for the common `#+LINK: gh https://github.com/` shape.
fn expand_abbreviation(raw: &str, abbreviations: &[(String, String)]) -> String {
    let Some((prefix, tail)) = raw.split_once(':') else {
        return raw.to_string();
    };

    let Some((_, expansion)) = abbreviations.iter().find(|(name, _)| name == prefix) else {
        return raw.to_string();
    };

    if expansion.contains("%s") {
        expansion.replace("%s", tail)
    } else {
        format!("{expansion}{tail}")
    }
}

/// Every link in `text`, with its byte range.
pub fn find_links(text: &str, abbreviations: &[(String, String)]) -> Vec<OrgLink> {
    let mut links = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;

    while let Some(open) = find_from(bytes, i, b"[[") {
        let start = open + 2;
        let Some(target_end) = find_close(bytes, start) else {
            break;
        };

        let target = &text[start..target_end];
        // `]]` ends a bare link; `][` starts a description.
        let (description, end) = if bytes.get(target_end + 1) == Some(&b'[') {
            let description_start = target_end + 2;
            match find_close(bytes, description_start) {
                Some(description_end) => (
                    Some(text[description_start..description_end].to_string()),
                    description_end + 2,
                ),
                None => (None, target_end + 2),
            }
        } else {
            (None, target_end + 2)
        };

        if !target.is_empty() {
            links.push(OrgLink {
                kind: parse_target(target, abbreviations),
                description,
                range: open..end.min(text.len()),
            });
        }

        i = end.max(target_end + 1);
    }

    links
}

/// The link containing byte offset `at`, if the cursor is inside one.
pub fn link_at(text: &str, at: usize, abbreviations: &[(String, String)]) -> Option<OrgLink> {
    find_links(text, abbreviations)
        .into_iter()
        .find(|link| link.range.contains(&at))
}

/// The first link starting strictly after `at`.
pub fn next_link(text: &str, at: usize, abbreviations: &[(String, String)]) -> Option<OrgLink> {
    find_links(text, abbreviations)
        .into_iter()
        .find(|link| link.range.start > at)
}

/// The last link starting strictly before `at`.
pub fn previous_link(text: &str, at: usize, abbreviations: &[(String, String)]) -> Option<OrgLink> {
    find_links(text, abbreviations)
        .into_iter()
        .filter(|link| link.range.start < at)
        .next_back()
}

/// Renders a link back to Org syntax.
pub fn format_link(target: &str, description: Option<&str>) -> String {
    match description {
        Some(description) if !description.is_empty() => format!("[[{target}][{description}]]"),
        _ => format!("[[{target}]]"),
    }
}
