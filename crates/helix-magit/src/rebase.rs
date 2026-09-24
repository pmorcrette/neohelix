//! Steering `git rebase --interactive` without an editor server.
//!
//! Git hands the todo-list to `$GIT_SEQUENCE_EDITOR` and waits for it to
//! exit. Pointing that at Helix would need Helix to be reachable from a
//! subprocess — the integrated terminal, a second instance, or a server of
//! its own. Instead the rebase is run twice:
//!
//! 1. with a sequence editor that copies git's list out and then empties it.
//!    An empty list is git's documented way of cancelling, so git stops
//!    with "nothing to do" and leaves the repository as it was — autostash
//!    included. The list is git's own, so `--autosquash`, `--rebase-merges`
//!    and anything else that shapes it apply.
//! 2. once the edited list is written, with a sequence editor that copies it
//!    back over git's. From there it is an ordinary rebase.
//!
//! Between the two, HEAD must not move, or the list would describe commits
//! that are no longer the ones being rebased; [`head`] is recorded at the
//! first run and checked before the second.
//!
//! `reword` needs a message editor, and every git subprocess here runs with
//! `GIT_EDITOR=true`, which would keep the old message. So it is run as
//! `edit`: the rebase stops at the commit, the message is amended with the
//! commit menu, and the rebase continued.

use std::path::Path;

use crate::command::GitCommand;

/// The todo-list actions a line can be set to, with git's one-letter forms.
pub const ACTIONS: [(&str, &str); 6] = [
    ("pick", "p"),
    ("reword", "r"),
    ("edit", "e"),
    ("squash", "s"),
    ("fixup", "f"),
    ("drop", "d"),
];

/// `'…'` for `sh`, which takes everything inside literally except `'`.
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

/// The sequence editor for the first run: keep git's list, then empty it.
///
/// Git runs the editor through `sh` with the list's path appended, so `$1`
/// is that path.
pub fn capture_editor(copy_to: &Path) -> String {
    format!("cp \"$1\" {} && : > \"$1\"", shell_quote(copy_to))
}

/// The sequence editor for the second run: install the edited list. Git
/// appends the path it wants the list at.
pub fn install_editor(edited: &Path) -> String {
    format!("cp {}", shell_quote(edited))
}

/// Where an interactive rebase that should include `commit` starts: its
/// parent, or `--root` when it has none.
pub fn base_for(workdir: &Path, commit: &str) -> String {
    let parent = format!("{commit}^");
    let has_parent = GitCommand::new(
        workdir,
        vec![
            "rev-parse".into(),
            "--verify".into(),
            "--quiet".into(),
            parent.clone(),
        ],
    )
    .run()
    .is_ok_and(|output| output.success);
    if has_parent {
        parent
    } else {
        "--root".to_string()
    }
}

/// HEAD's full hash, to check nothing moved between the two runs.
pub fn head(workdir: &Path) -> Option<String> {
    let output = GitCommand::new(workdir, vec!["rev-parse".into(), "HEAD".into()])
        .run()
        .ok()?;
    output
        .success
        .then(|| output.stdout.trim().to_string())
        .filter(|head| !head.is_empty())
}

/// The first run's command: `args` is the whole `rebase --interactive …`
/// command line.
pub fn capture_command(workdir: &Path, args: Vec<String>, copy_to: &Path) -> GitCommand {
    GitCommand::new(workdir, args).with_env("GIT_SEQUENCE_EDITOR", capture_editor(copy_to))
}

/// The second run's command.
pub fn install_command(workdir: &Path, args: Vec<String>, edited: &Path) -> GitCommand {
    GitCommand::new(workdir, args).with_env("GIT_SEQUENCE_EDITOR", install_editor(edited))
}

/// Whether the first run did what it should: git refused with "nothing to
/// do" because the list was emptied. Anything else is a real failure —
/// no upstream, a dirty tree without `--autostash` — worth reporting as is.
pub fn captured(stderr: &str) -> bool {
    stderr.contains("nothing to do")
}

/// The action of a todo line, as its full name, or `None` for a comment, a
/// blank line or a command that is not about a commit (`exec`, `label`, …).
pub fn action_of(line: &str) -> Option<&'static str> {
    let word = line.split_whitespace().next()?;
    ACTIONS
        .iter()
        .find(|(long, short)| word == *long || word == *short)
        .map(|(long, _)| *long)
}

/// The line with its action replaced, or `None` when it is not a commit
/// line or `action` is not one of [`ACTIONS`].
pub fn set_action(line: &str, action: &str) -> Option<String> {
    action_of(line)?;
    let (long, _) = ACTIONS
        .iter()
        .find(|(long, short)| action == *long || action == *short)?;
    let indent = &line[..line.len() - line.trim_start().len()];
    let rest = line.trim_start();
    let word_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    Some(format!("{indent}{long}{}", &rest[word_end..]))
}

/// The list as git will run it: `reword` becomes `edit` (see the module
/// documentation). Returns the list and how many lines were changed.
pub fn prepare(todo: &str) -> (String, usize) {
    let mut rewords = 0;
    let mut out: Vec<String> = todo
        .lines()
        .map(|line| {
            if action_of(line) == Some("reword") {
                rewords += 1;
                set_action(line, "edit").unwrap_or_else(|| line.to_string())
            } else {
                line.to_string()
            }
        })
        .collect();
    if todo.ends_with('\n') {
        out.push(String::new());
    }
    (out.join("\n"), rewords)
}

/// Whether the list has anything left to do: git treats a list with no
/// command as "cancel the rebase".
pub fn is_empty(todo: &str) -> bool {
    todo.lines().all(|line| {
        let line = line.trim();
        line.is_empty() || line.starts_with('#')
    })
}

/// A comment explaining the list, appended below git's own list. Git's own
/// help, which it puts in the list it hands the editor, still follows.
pub const HELP: &str = "\
# Edit the list above, then write the buffer to start the rebase.
# `:rebase-todo <action>` sets the action of the selected lines (pick,
# reword, edit, squash, fixup, drop), `:rebase-todo up` / `down` moves them.
# `reword` runs as `edit`: amend the message with the commit menu when the
# rebase stops, then continue it. Deleting every line cancels the rebase,
# as quitting without writing does.
";

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn paths_are_quoted_for_the_shell() {
        let path = PathBuf::from("/tmp/it's here/todo");
        assert_eq!(
            capture_editor(&path),
            r#"cp "$1" '/tmp/it'\''s here/todo' && : > "$1""#
        );
        assert_eq!(install_editor(&path), r"cp '/tmp/it'\''s here/todo'");
    }

    #[test]
    fn actions_are_read_and_replaced_in_either_form() {
        assert_eq!(action_of("pick 1a2b3c Subject"), Some("pick"));
        assert_eq!(action_of("f 1a2b3c fixup! Subject"), Some("fixup"));
        assert_eq!(action_of("# pick 1a2b3c"), None);
        assert_eq!(action_of("exec make"), None);
        assert_eq!(action_of(""), None);

        assert_eq!(
            set_action("pick 1a2b3c Subject", "squash").as_deref(),
            Some("squash 1a2b3c Subject")
        );
        assert_eq!(
            set_action("p 1a2b3c Subject", "d").as_deref(),
            Some("drop 1a2b3c Subject")
        );
        assert_eq!(set_action("exec make", "pick"), None);
        assert_eq!(set_action("pick 1a2b3c", "frobnicate"), None);
    }

    #[test]
    fn reword_runs_as_edit() {
        let (list, rewords) = prepare("pick a one\nreword b two\nr c three\n# reword d\n");
        assert_eq!(list, "pick a one\nedit b two\nedit c three\n# reword d\n");
        assert_eq!(rewords, 2);
    }

    #[test]
    fn a_list_of_comments_is_empty() {
        assert!(is_empty("# nothing\n\n# here\n"));
        assert!(!is_empty("# help\npick a one\n"));
    }
}
