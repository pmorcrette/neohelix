//! Trailers: the `Key: value` lines at the end of a commit message.
//!
//! A trailer goes into the message's last paragraph when that paragraph is
//! already made of trailers, and into a paragraph of its own otherwise, as
//! `git interpret-trailers` places them. The comment lines git adds below
//! the message, and a `--verbose` diff below the scissors, stay where they
//! are.

use std::collections::HashMap;
use std::path::Path;

use crate::command::GitCommand;

/// The trailers offered, as Magit's message buffer offers them.
pub const KINDS: [&str; 10] = [
    "Signed-off-by",
    "Co-authored-by",
    "Reported-by",
    "Reviewed-by",
    "Tested-by",
    "Acked-by",
    "Suggested-by",
    "Co-developed-by",
    "Modified-by",
    "Cc",
];

/// The trailer `typed` names: one of [`KINDS`] matched without regard to
/// case, or the only one containing it; any other word is taken as it is,
/// as git accepts any key. Several matches are an error naming them.
pub fn resolve_kind(typed: &str) -> Result<String, String> {
    let typed = typed.trim().trim_end_matches(':');
    let lower = typed.to_lowercase();
    if let Some(kind) = KINDS.iter().find(|kind| kind.to_lowercase() == lower) {
        return Ok(kind.to_string());
    }
    let matching: Vec<&str> = KINDS
        .iter()
        .copied()
        .filter(|kind| kind.to_lowercase().contains(&lower))
        .collect();
    match matching.as_slice() {
        [kind] => Ok(kind.to_string()),
        [] if !typed.is_empty() && typed.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') => {
            Ok(typed.to_string())
        }
        [] => Err(format!("`{typed}` cannot be a trailer's key")),
        several => Err(format!("`{typed}` could be {}", several.join(", "))),
    }
}

/// The line git puts above a `--verbose` diff: nothing below it is part of
/// the message.
const SCISSORS: &str = "# ------------------------ >8 ------------------------";

fn is_trailer(line: &str) -> bool {
    line.split_once(": ").is_some_and(|(key, value)| {
        !key.is_empty()
            && !value.trim().is_empty()
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}

/// `message` with `trailer` added, or as it was when it already has it.
pub fn add(message: &str, trailer: &str) -> String {
    let trailer = trailer.trim();
    let lines: Vec<&str> = message.lines().collect();
    let scissors = lines
        .iter()
        .position(|line| *line == SCISSORS)
        .unwrap_or(lines.len());
    // The message ends at its last line that is neither blank nor a
    // comment; git strips the rest.
    let end = lines[..scissors]
        .iter()
        .rposition(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .map_or(0, |last| last + 1);
    let (body, rest) = lines.split_at(end);
    if body.iter().any(|line| line.trim() == trailer) {
        return message.to_string();
    }

    let mut out: Vec<&str> = body.to_vec();
    let last_paragraph = body
        .iter()
        .rposition(|line| line.trim().is_empty())
        .map(|blank| &body[blank + 1..]);
    match last_paragraph {
        // Below the subject, a paragraph of trailers already: one more.
        Some(paragraph) if !paragraph.is_empty() && paragraph.iter().all(|l| is_trailer(l)) => {}
        // No message yet: the subject line stays for the user to write.
        _ if body.is_empty() => out.extend(["", ""]),
        _ => out.push(""),
    }
    out.push(trailer);
    if rest.first().is_some_and(|line| !line.trim().is_empty()) {
        out.push("");
    }
    out.extend(rest);

    let mut text = out.join("\n");
    if message.ends_with('\n') || message.is_empty() {
        text.push('\n');
    }
    text
}

/// The people in the history, as `Name <email>`, most frequent first:
/// authors and committers of the last commits.
pub fn people(workdir: &Path) -> Vec<String> {
    let args = ["log", "-n", "2000", "--format=%an <%ae>%n%cn <%ce>"]
        .iter()
        .map(|arg| arg.to_string())
        .collect();
    let text = GitCommand::new(workdir, args)
        .run()
        .ok()
        .filter(|output| output.success)
        .map(|output| output.stdout)
        .unwrap_or_default();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let count = counts.entry(line).or_insert(0);
        if *count == 0 {
            order.push(line);
        }
        *count += 1;
    }
    // Stable: equally frequent people keep the order of recency.
    order.sort_by_key(|person| std::cmp::Reverse(counts[person]));
    order.into_iter().map(str::to_string).collect()
}

/// The user, as git would sign off: `Name <email>`.
pub fn me(workdir: &Path) -> Option<String> {
    let args = vec!["var".to_string(), "GIT_COMMITTER_IDENT".to_string()];
    let output = GitCommand::new(workdir, args).run().ok()?;
    let ident = output.stdout.trim();
    // `Name <email> 1727262000 +0200`: the date goes.
    let end = ident.rfind('>')?;
    output.success.then(|| ident[..=end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOB: &str = "Signed-off-by: Ann <ann@example.com>";

    #[test]
    fn a_trailer_gets_a_paragraph_of_its_own_below_the_message() {
        assert_eq!(
            add("Fix the thing\n\nBecause.\n", SOB),
            format!("Fix the thing\n\nBecause.\n\n{SOB}\n")
        );
        assert_eq!(
            add("Fix the thing\n", SOB),
            format!("Fix the thing\n\n{SOB}\n")
        );
    }

    #[test]
    fn a_trailer_joins_the_trailers_already_there_once() {
        let message = format!("Fix\n\nBody.\n\n{SOB}\n");
        let cc = "Cc: Bob <bob@example.com>";
        let with_cc = add(&message, cc);
        assert_eq!(with_cc, format!("Fix\n\nBody.\n\n{SOB}\n{cc}\n"));
        assert_eq!(add(&with_cc, SOB), with_cc);
    }

    #[test]
    fn a_kind_is_found_from_part_of_its_name() {
        assert_eq!(resolve_kind("signed").unwrap(), "Signed-off-by");
        assert_eq!(resolve_kind("cc").unwrap(), "Cc");
        assert_eq!(resolve_kind("Fixes:").unwrap(), "Fixes");
        assert!(resolve_kind("co").unwrap_err().contains("Co-authored-by"));
        assert!(resolve_kind("a b").is_err());
    }

    #[test]
    fn a_subject_that_looks_like_a_trailer_is_still_the_subject() {
        assert_eq!(add("fix: typo\n", SOB), format!("fix: typo\n\n{SOB}\n"));
    }

    #[test]
    fn comments_and_the_verbose_diff_stay_below() {
        let message = "Fix\n\n# Please enter the commit message\n# ------------------------ >8 ------------------------\ndiff --git a/x b/x\n";
        assert_eq!(
            add(message, SOB),
            format!(
                "Fix\n\n{SOB}\n\n# Please enter the commit message\n# ------------------------ >8 ------------------------\ndiff --git a/x b/x\n"
            )
        );
        // No message yet: room is kept for it.
        assert_eq!(
            add("\n# comment\n", SOB),
            format!("\n\n{SOB}\n\n# comment\n")
        );
    }
}
