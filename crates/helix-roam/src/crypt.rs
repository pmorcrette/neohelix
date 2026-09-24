//! Encrypted subtrees: an entry tagged `:crypt:` keeps its body as a PGP
//! message.
//!
//! What is encrypted follows Org's `org-crypt` since 9.4: everything below
//! the headline's planning line and property drawer, through the end of
//! the subtree. The headline, its dates and its properties stay readable,
//! so the entry keeps its `:ID:` — which is what lets an encrypted note stay
//! a node that other notes link to.
//!
//! This module finds bodies and swaps them; running `gpg` is the editor's.

use crate::parser::FileSettings;
use crate::restructure::{self, headline_level, is_planning_line, rejoin, subtree_range};

/// The tag that marks an entry to keep encrypted (`org-crypt-tag-matcher`).
pub const TAG: &str = "crypt";

/// How an armoured PGP message starts.
pub const ARMOR_BEGIN: &str = "-----BEGIN PGP MESSAGE-----";

/// The body of the entry at `line`: its headline, and the lines
/// `start..end` that encrypting replaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Body {
    pub headline: usize,
    pub start: usize,
    pub end: usize,
}

/// The body of the entry containing `line`, or `None` outside any headline.
///
/// Trailing blank lines stay outside, so the blank line before the next
/// headline survives a round trip through encryption.
pub fn body(text: &str, line: usize) -> Option<Body> {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (headline, end, _) = subtree_range(&lines, line)?;
    headline_level(&lines[headline])?;

    let mut start = headline + 1;
    if lines.get(start).is_some_and(|l| is_planning_line(l)) {
        start += 1;
    }
    if lines
        .get(start)
        .is_some_and(|l| l.trim().eq_ignore_ascii_case(":PROPERTIES:"))
    {
        if let Some(close) = lines[start + 1..end]
            .iter()
            .position(|l| l.trim().eq_ignore_ascii_case(":END:"))
        {
            start += close + 2;
        }
    }

    let mut end = end.max(start);
    while end > start && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    Some(Body {
        headline,
        start,
        end,
    })
}

/// The body's text, with a final newline.
pub fn body_text(text: &str, body: &Body) -> String {
    let mut out: String = text
        .lines()
        .skip(body.start)
        .take(body.end - body.start)
        .collect::<Vec<_>>()
        .join("\n");
    if body.end > body.start {
        out.push('\n');
    }
    out
}

/// Whether the body is already a PGP message.
pub fn is_encrypted(text: &str, body: &Body) -> bool {
    text.lines()
        .nth(body.start)
        .is_some_and(|line| line.trim() == ARMOR_BEGIN)
}

/// `text` with the body replaced by `new`.
pub fn replace(text: &str, body: &Body, new: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    lines.splice(body.start..body.end, new.lines().map(str::to_string));
    rejoin(&lines, text)
}

/// The headlines tagged `:crypt:` themselves, which is where a body starts
/// that holds any children they have.
pub fn tagged(text: &str) -> Vec<usize> {
    let settings = FileSettings::scan(text);
    text.lines()
        .enumerate()
        .filter(|(_, line)| headline_level(line).is_some())
        .filter_map(|(at, line)| {
            let headline = crate::parser::parse_headline(line, &settings)?;
            headline.tags.iter().any(|tag| tag == TAG).then_some(at)
        })
        .collect()
}

/// The `:crypt:` entries whose body is readable and not empty: what would
/// be written to disk in clear.
pub fn in_clear(text: &str) -> Vec<Body> {
    tagged(text)
        .into_iter()
        .filter_map(|headline| body(text, headline))
        .filter(|body| body.end > body.start && !is_encrypted(text, body))
        .collect()
}

/// The key to encrypt the entry at `line` to: its `:CRYPTKEY:`, or the
/// file's `#+PROPERTY: CRYPTKEY`. None means a passphrase instead.
pub fn key(text: &str, line: usize) -> Option<String> {
    restructure::property_value(text, line, "CRYPTKEY")
        .or_else(|| {
            FileSettings::scan(text)
                .properties
                .into_iter()
                .find(|(key, _)| key == "cryptkey")
                .map(|(_, value)| value)
        })
        .filter(|key| !key.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIARY: &str = "\
* Diary :crypt:
SCHEDULED: <2026-09-24 Thu>
:PROPERTIES:
:ID: 6ba7b810-9dad-11d1-80b4-00c04fd430c8
:END:
Dear diary.
** Private child
Also secret.

* Public
Visible.
";

    #[test]
    fn the_body_starts_below_the_meta_data_and_holds_the_children() {
        let found = body(DIARY, 0).unwrap();
        assert_eq!((found.headline, found.start, found.end), (0, 5, 8));
        assert_eq!(
            body_text(DIARY, &found),
            "Dear diary.\n** Private child\nAlso secret.\n"
        );
    }

    #[test]
    fn replacing_keeps_the_headline_properties_and_the_blank_line() {
        let found = body(DIARY, 5).unwrap();
        let out = replace(
            DIARY,
            &found,
            "-----BEGIN PGP MESSAGE-----\n\nabc\n-----END PGP MESSAGE-----\n",
        );
        assert_eq!(
            out,
            "* Diary :crypt:\nSCHEDULED: <2026-09-24 Thu>\n:PROPERTIES:\n:ID: 6ba7b810-9dad-11d1-80b4-00c04fd430c8\n:END:\n\
             -----BEGIN PGP MESSAGE-----\n\nabc\n-----END PGP MESSAGE-----\n\n* Public\nVisible.\n"
        );
        assert!(is_encrypted(&out, &body(&out, 0).unwrap()));
        assert!(in_clear(&out).is_empty());
    }

    #[test]
    fn only_tagged_entries_with_a_readable_body_are_in_clear() {
        assert_eq!(tagged(DIARY), [0]);
        assert_eq!(in_clear(DIARY).len(), 1);
        assert!(in_clear("* Empty :crypt:\n* Other\n").is_empty());
    }

    #[test]
    fn the_key_comes_from_the_entry_or_the_file() {
        assert_eq!(key(DIARY, 0), None);
        let with_key = DIARY.replace(":ID:", ":CRYPTKEY: me@example.org\n:ID:");
        assert_eq!(key(&with_key, 0).as_deref(), Some("me@example.org"));
        let file = format!("#+PROPERTY: CRYPTKEY team@example.org\n{DIARY}");
        assert_eq!(key(&file, 1).as_deref(), Some("team@example.org"));
    }
}
