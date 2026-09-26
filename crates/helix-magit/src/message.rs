//! The commit message buffer: its history and its style.
//!
//! The buffer holds the message above git's comment lines (and, with
//! `--verbose`, the diff below the scissors). Recalling an earlier message
//! swaps the part above the comments and leaves the rest alone.

use std::path::Path;

use crate::command::{GitCommand, SCISSORS};

/// Magit's `git-commit-summary-max-length`.
pub const SUMMARY_MAX: usize = 68;

/// Where the message part of `buffer` ends: at the first comment line, or
/// the scissors, or the end.
fn message_end(buffer: &str) -> usize {
    let mut offset = 0;
    for line in buffer.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        if bare.starts_with('#') || bare == SCISSORS {
            return offset;
        }
        offset += line.len();
    }
    buffer.len()
}

/// The message part of the buffer, as written, comments left out.
pub fn message_part(buffer: &str) -> &str {
    &buffer[..message_end(buffer)]
}

/// The buffer with its message part replaced by `message`, the comments
/// and anything below them kept. One blank line separates the two.
pub fn replace_message(buffer: &str, message: &str) -> String {
    let rest = &buffer[message_end(buffer)..];
    let message = message.trim_end();
    if rest.is_empty() {
        format!("{message}\n")
    } else if message.is_empty() {
        format!("\n{rest}")
    } else {
        format!("{message}\n\n{rest}")
    }
}

/// The messages of the last `limit` commits on HEAD, newest first.
pub fn recent_messages(workdir: &Path, limit: usize) -> Vec<String> {
    let output = GitCommand::new(
        workdir,
        vec![
            "log".into(),
            format!("--max-count={limit}"),
            "--format=%B%x00".into(),
        ],
    )
    .run();
    match output {
        Ok(output) if output.success => output
            .stdout
            .split('\0')
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// What git-commit's style checks would say of `message` (comments already
/// stripped): a summary over [`SUMMARY_MAX`] characters, and a second line
/// that is not blank. An `amend!` or `squash!` line is not the summary: the
/// line after it is.
pub fn style_problems(message: &str) -> Vec<String> {
    let mut lines = message.lines().peekable();
    // What an `amend!` commit's first line names is not its own message.
    if lines
        .peek()
        .is_some_and(|line| line.starts_with("amend! ") || line.starts_with("squash! "))
    {
        lines.next();
        if lines.peek().is_some_and(|line| line.trim().is_empty()) {
            lines.next();
        }
    }
    let mut problems = Vec::new();
    if let Some(summary) = lines.next() {
        let length = summary.chars().count();
        if length > SUMMARY_MAX {
            problems.push(format!(
                "the summary is {length} characters long, over {SUMMARY_MAX}"
            ));
        }
    }
    if lines.next().is_some_and(|line| !line.trim().is_empty()) {
        problems.push("the line after the summary is not blank".to_string());
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUFFER: &str = "Old subject\n\nOld body\n\n# Write the message above.\n# ## main\n";

    #[test]
    fn the_message_is_what_comes_before_the_comments() {
        assert_eq!(message_part(BUFFER), "Old subject\n\nOld body\n\n");
        assert_eq!(message_part("just a line"), "just a line");
        let verbose = format!("Subject\n{SCISSORS}\ndiff --git …\n");
        assert_eq!(message_part(&verbose), "Subject\n");
    }

    #[test]
    fn replacing_the_message_keeps_the_comments() {
        assert_eq!(
            replace_message(BUFFER, "New subject\n\nNew body\n"),
            "New subject\n\nNew body\n\n# Write the message above.\n# ## main\n"
        );
        // Back to nothing: the blank line the template starts with.
        assert_eq!(
            replace_message(BUFFER, ""),
            "\n# Write the message above.\n# ## main\n"
        );
        assert_eq!(replace_message("draft", "other"), "other\n");
    }

    #[test]
    fn style_checks_find_a_long_summary_and_a_crowded_second_line() {
        assert!(style_problems("Short\n\nBody").is_empty());
        assert!(style_problems("Short").is_empty());
        let long = "x".repeat(SUMMARY_MAX + 1);
        assert_eq!(
            style_problems(&long),
            [format!(
                "the summary is 69 characters long, over {SUMMARY_MAX}"
            )]
        );
        assert_eq!(
            style_problems("Summary\nno blank line"),
            ["the line after the summary is not blank"]
        );
        // An amend! commit's own first line is not checked.
        let amend = format!("amend! {long}\n\nNew subject\n\nBody");
        assert!(style_problems(&amend).is_empty());
    }
}
