//! Places a node is named without being linked.
//!
//! The graph knows what points at what. This answers a different question:
//! where did someone write a node's title as plain prose, so the link could
//! still be made? Upstream shows these in the Org-Roam buffer; the search
//! itself is here so it can be tested without an editor.

use std::path::{Path, PathBuf};

/// One occurrence of a node's name outside a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnlinkedReference {
    pub path: PathBuf,
    /// Zero-based line the occurrence sits on.
    pub line: usize,
    /// Byte offset of the occurrence within the line.
    pub column: usize,
    /// The whole line, for showing context.
    pub text: String,
    /// Which of the node's names matched — its title or one of its aliases.
    pub matched: String,
}

/// Finds the names in `names` written as prose in `text`.
///
/// An occurrence inside any `[[…]]` is skipped: there the text is a link's
/// target or a description someone chose, not prose waiting to become a link.
/// That covers links to this node without needing to know its id.
pub fn find_in_text(text: &str, path: &Path, names: &[String]) -> Vec<UnlinkedReference> {
    let mut found = Vec::new();

    for (line_number, line) in text.lines().enumerate() {
        let spans = link_spans(line);

        for name in names {
            if name.is_empty() {
                continue;
            }
            for column in match_positions(line, name) {
                // Inside `[[…]]` the text is a link's own business.
                if spans.iter().any(|span| span.contains(&column)) {
                    continue;
                }
                found.push(UnlinkedReference {
                    path: path.to_path_buf(),
                    line: line_number,
                    column,
                    text: line.to_string(),
                    matched: name.clone(),
                });
            }
        }
    }

    found.sort_by_key(|reference| (reference.line, reference.column));
    found
}

/// Byte ranges covered by `[[…]]` links on a line.
fn link_spans(line: &str) -> Vec<std::ops::Range<usize>> {
    let mut spans = Vec::new();
    let mut rest = line;
    let mut base = 0;

    while let Some(open) = rest.find("[[") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("]]") else {
            break;
        };
        let end = open + 2 + close + 2;
        spans.push(base + open..base + end);
        base += end;
        rest = &rest[end..];
    }

    spans
}

/// Byte offsets where `name` occurs as a whole word, ignoring case.
///
/// Whole-word only: a node called "Rust" should not match "trusted", which is
/// the difference between a useful list and an unusable one.
fn match_positions(line: &str, name: &str) -> Vec<usize> {
    let haystack = line.to_lowercase();
    let needle = name.to_lowercase();
    if needle.is_empty() || needle.len() > haystack.len() {
        return Vec::new();
    }

    let mut positions = Vec::new();
    let mut from = 0;

    while let Some(offset) = haystack[from..].find(&needle) {
        let start = from + offset;
        let end = start + needle.len();

        let before_ok = start == 0
            || !haystack[..start]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric);
        let after_ok = end == haystack.len()
            || !haystack[end..]
                .chars()
                .next()
                .is_some_and(char::is_alphanumeric);

        // Case folding can change byte length, so only offsets that are still
        // char boundaries in the original line are usable.
        if before_ok && after_ok && line.is_char_boundary(start) {
            positions.push(start);
        }

        from = end.max(start + 1);
    }

    positions
}
