//! Org again, as Org's `org` backend writes it: the `#+INCLUDE`s already
//! resolved by the caller, and what export leaves out taken out — the
//! subtrees tagged `noexport` (or `#+EXCLUDE_TAGS:`), the `COMMENT`
//! headlines, comment lines and comment blocks. What remains is the text
//! as written, which is the point: another Org reader gets one file.

use super::*;

pub(super) fn document(text: &str) -> String {
    let settings = FileSettings::scan(text);
    let exclude: Vec<String> = text
        .lines()
        .find_map(|line| keyword(line, "exclude_tags"))
        .map(|value| value.split_whitespace().map(str::to_string).collect())
        .unwrap_or_else(|| vec!["noexport".to_string()]);

    let mut out = Vec::new();
    // The level of a subtree being left out, until a headline as shallow.
    let mut skipping: Option<usize> = None;
    let mut in_comment_block = false;
    for line in text.lines() {
        if let Some(level) = headline_level(line) {
            if skipping.is_some_and(|skip| level > skip) {
                continue;
            }
            skipping = None;
            if let Some(headline) = crate::parser::parse_headline(line, &settings) {
                let commented =
                    headline.title == "COMMENT" || headline.title.starts_with("COMMENT ");
                if commented || headline.tags.iter().any(|tag| exclude.contains(tag)) {
                    skipping = Some(level);
                    continue;
                }
            }
        } else if skipping.is_some() {
            continue;
        }

        let trimmed = line.trim_start();
        if in_comment_block {
            if trimmed.to_ascii_lowercase().starts_with("#+end_comment") {
                in_comment_block = false;
            }
            continue;
        }
        if trimmed.to_ascii_lowercase().starts_with("#+begin_comment") {
            in_comment_block = true;
            continue;
        }
        // `# comment`, but not `#+KEYWORD:`.
        if trimmed == "#" || trimmed.starts_with("# ") {
            continue;
        }
        out.push(line);
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}
