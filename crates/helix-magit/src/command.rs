//! Turning a transient action into a git command, and running it.
//!
//! Reading a repository and writing its index are done with `gix`, but the
//! operations here go through the `git` binary. Two reasons decide it:
//!
//! * gitoxide does not implement push at all — `Direction::Push` exists only
//!   to resolve which remote and branch a push *would* use.
//! * Everything in this group is expected to honour things `gix` does not do:
//!   hooks (`pre-commit`, `commit-msg`, `pre-push`), commit signing, credential
//!   helpers, the SSH agent, and `url.*.insteadOf`. A commit that silently
//!   skips the user's hooks is a bug, not a simplification.
//!
//! The subprocess has no terminal, so anything that would prompt — an editor,
//! a password — would hang forever. [`GitCommand`] closes both doors, turning
//! what would be a hang into an error the user can read.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::transient::MagitCommand;

/// What an action needs before it can run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Requirement {
    /// Ready to run.
    None,
    /// A commit message has to be composed first.
    CommitMessage {
        /// Whether to seed the buffer from HEAD's message.
        amend: bool,
    },
    /// A branch name has to be supplied.
    BranchName,
    /// A remote has to be chosen.
    Remote,
    /// An interactive rebase: its todo-list has to be edited first, and
    /// where to rebase from may still have to be asked.
    TodoList,
}

/// What an action would do, before anything is run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Arguments to `git`, without the program itself.
    pub args: Vec<String>,
    /// What is still missing.
    pub requirement: Requirement,
    /// Whether this can destroy work the user cannot get back.
    pub destructive: bool,
    /// A short description, for the confirmation prompt and the status line.
    pub summary: String,
}

impl Plan {
    fn new(args: impl IntoIterator<Item = impl Into<String>>, summary: &str) -> Self {
        Self {
            args: args.into_iter().map(Into::into).collect(),
            requirement: Requirement::None,
            destructive: false,
            summary: summary.to_string(),
        }
    }

    fn requiring(mut self, requirement: Requirement) -> Self {
        self.requirement = requirement;
        self
    }

    fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }

    /// The command line as it would be typed, for display.
    pub fn command_line(&self) -> String {
        format!("git {}", self.args.join(" "))
    }
}

/// Resolves a menu action and its accumulated arguments into a plan.
///
/// Returns `None` for an action that runs nothing itself, such as opening
/// another menu.
pub fn resolve(command: MagitCommand, args: &[String]) -> Option<Plan> {
    // A `--force` push can destroy commits on the remote, whatever else the
    // menu asked for.
    let forced = args
        .iter()
        .any(|arg| arg == "--force" || arg == "--force-with-lease");

    let plan = match command {
        MagitCommand::OpenMenu(_)
        | MagitCommand::Status
        | MagitCommand::Refresh
        | MagitCommand::Quit => return None,

        MagitCommand::Commit => Plan::new(with(["commit"], args), "Commit")
            .requiring(Requirement::CommitMessage { amend: false }),
        MagitCommand::CommitAmend => Plan::new(with(["commit", "--amend"], args), "Amend")
            .requiring(Requirement::CommitMessage { amend: true }),
        // Extend adds the staged changes to HEAD without touching its message.
        MagitCommand::CommitExtend => Plan::new(
            with(["commit", "--amend", "--no-edit"], args),
            "Extend the last commit",
        ),
        MagitCommand::CommitFixup => Plan::new(
            with(["commit", "--fixup=HEAD"], args),
            "Fixup the last commit",
        ),

        // git resolves the upstream itself, so both of these are a bare push.
        // Telling them apart needs a remote picker, which `PushElsewhere`
        // waits on too.
        MagitCommand::Push | MagitCommand::PushToUpstream => {
            let plan = Plan::new(with(["push"], args), "Push");
            if forced {
                plan.destructive()
            } else {
                plan
            }
        }
        MagitCommand::PushElsewhere => {
            Plan::new(with(["push"], args), "Push elsewhere").requiring(Requirement::Remote)
        }

        MagitCommand::Pull => Plan::new(with(["pull"], args), "Pull"),
        MagitCommand::Fetch => Plan::new(with(["fetch"], args), "Fetch"),
        MagitCommand::FetchAll => Plan::new(with(["fetch", "--all"], args), "Fetch all remotes"),

        MagitCommand::BranchCheckout => Plan::new(with(["checkout"], args), "Check out a branch")
            .requiring(Requirement::BranchName),
        MagitCommand::BranchCreate => {
            Plan::new(with(["branch"], args), "Create a branch").requiring(Requirement::BranchName)
        }
        MagitCommand::BranchCreateAndCheckout => Plan::new(
            with(["checkout", "-b"], args),
            "Create and check out a branch",
        )
        .requiring(Requirement::BranchName),
        // Deleting a branch can lose commits that nothing else points at.
        MagitCommand::BranchDelete => {
            Plan::new(with(["branch", "--delete"], args), "Delete a branch")
                .requiring(Requirement::BranchName)
                .destructive()
        }

        // With the `--interactive` switch on, this is the interactive
        // rebase, which has to go through the todo-list rather than run
        // with an editor that accepts it unchanged.
        MagitCommand::RebaseOntoUpstream if args.iter().any(|arg| arg == "--interactive") => {
            return resolve(MagitCommand::RebaseInteractive, args);
        }
        MagitCommand::RebaseOntoUpstream => {
            Plan::new(with(["rebase"], args), "Rebase onto upstream").destructive()
        }
        // The edited todo-list is the confirmation: nothing is rewritten
        // until it is written, and quitting it cancels.
        MagitCommand::RebaseInteractive => Plan::new(
            with(
                ["rebase", "--interactive"],
                &args
                    .iter()
                    .filter(|arg| *arg != "--interactive")
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
            "Rebase interactively",
        )
        .requiring(Requirement::TodoList),
        MagitCommand::RebaseSkip => {
            Plan::new(["rebase", "--skip"], "Skip this commit").destructive()
        }
        MagitCommand::RebaseContinue => Plan::new(["rebase", "--continue"], "Continue the rebase"),
        // Aborting throws away everything the rebase has replayed so far.
        MagitCommand::RebaseAbort => {
            Plan::new(["rebase", "--abort"], "Abort the rebase").destructive()
        }
    };

    Some(plan)
}

/// Appends the menu's arguments to a fixed prefix.
fn with(prefix: impl IntoIterator<Item = &'static str>, args: &[String]) -> Vec<String> {
    prefix
        .into_iter()
        .map(ToString::to_string)
        .chain(args.iter().cloned())
        .collect()
}

/// What a finished command left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl GitOutput {
    /// The most useful line to show, preferring git's own diagnosis.
    pub fn summary(&self) -> String {
        let source = if self.stderr.trim().is_empty() {
            &self.stdout
        } else {
            &self.stderr
        };
        // Progress is drawn with carriage returns and erase-line escapes:
        // what a terminal would end up showing is the text after the last
        // return, without the escapes.
        source
            .lines()
            .map(|line| line.rsplit('\r').next().unwrap_or(line))
            .map(|line| line.replace("\x1b[K", ""))
            .map(|line| line.trim().to_string())
            .find(|line| !line.is_empty())
            .unwrap_or_else(|| if self.success { "done" } else { "failed" }.to_string())
    }
}

/// A `git` invocation, built so it can never wait for a human.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommand {
    pub args: Vec<String>,
    pub working_directory: PathBuf,
    /// Set after the defaults, so it can override them.
    pub env: Vec<(String, String)>,
}

impl GitCommand {
    pub fn new(working_directory: impl Into<PathBuf>, args: Vec<String>) -> Self {
        Self {
            args,
            working_directory: working_directory.into(),
            env: Vec::new(),
        }
    }

    /// Sets an environment variable for the process, over the defaults —
    /// how the interactive rebase supplies its own sequence editor.
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Builds the process, with the environment that keeps it non-interactive.
    ///
    /// Without these, a command that wants an editor or a password blocks on a
    /// terminal that is not there, and the editor waits on it forever.
    pub fn build(&self) -> Command {
        let mut command = Command::new("git");
        command
            .args(&self.args)
            .current_dir(&self.working_directory)
            // Never open an editor: a message is supplied with `-F` or not at
            // all. `true` exits successfully, keeping whatever message exists.
            .env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true")
            // Fail fast rather than hang when a credential is missing. Helpers
            // and the SSH agent still work; only the terminal prompt is off.
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "")
            .env("SSH_ASKPASS", "")
            // Output is parsed and shown, so it must not be paged or coloured.
            .env("GIT_PAGER", "cat")
            .env("NO_COLOR", "1")
            .stdin(std::process::Stdio::null());
        for (key, value) in &self.env {
            command.env(key, value);
        }
        command
    }

    /// Runs the command, waiting for it to finish.
    ///
    /// This blocks; callers run it off the editor's thread.
    pub fn run(&self) -> std::io::Result<GitOutput> {
        let output = self.build().output()?;
        Ok(GitOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// git's comment character: lines starting with it are not part of a message.
const COMMENT: char = '#';

/// The message a commit buffer starts with.
///
/// Mirrors what git itself writes into `COMMIT_EDITMSG`: the existing message
/// when amending, then a comment block explaining the rules and showing what
/// is staged. The comments are stripped again by [`strip_comments`].
pub fn commit_template(working_directory: &Path, amend: bool) -> String {
    let mut template = String::new();

    if amend {
        if let Some(message) = head_message(working_directory) {
            template.push_str(&message);
            template.push('\n');
        }
    }

    template.push_str(
        "\n\
         # Write the commit message above, then write and close this buffer.\n\
         # Lines starting with '#' are ignored, and an empty message aborts\n\
         # the commit, leaving the index untouched.\n",
    );

    if let Ok(status) = GitCommand::new(
        working_directory,
        vec!["status".into(), "--short".into(), "--branch".into()],
    )
    .run()
    {
        if status.success {
            template.push_str("#\n");
            for line in status.stdout.lines() {
                template.push_str("# ");
                template.push_str(line);
                template.push('\n');
            }
        }
    }

    template
}

/// Removes the comment lines, leaving the message git would use.
///
/// A message that is only comments and blank lines is empty, which is how git
/// itself decides that a commit was aborted.
pub fn strip_comments(text: &str) -> String {
    let body: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim_start().starts_with(COMMENT))
        .collect();

    body.join("\n").trim().to_string()
}

/// The message HEAD was committed with, for seeding an amend.
pub fn head_message(working_directory: &Path) -> Option<String> {
    let output = GitCommand::new(
        working_directory,
        vec!["log".into(), "-1".into(), "--pretty=%B".into()],
    )
    .run()
    .ok()?;

    output.success.then(|| output.stdout.trim_end().to_string())
}

/// Whether HEAD is also on the branch's upstream, i.e. already pushed.
///
/// Amending such a commit rewrites history someone else may have, so it is
/// worth a confirmation.
pub fn head_is_pushed(working_directory: &Path) -> bool {
    let output = GitCommand::new(
        working_directory,
        vec![
            "branch".into(),
            "--remotes".into(),
            "--contains".into(),
            "HEAD".into(),
        ],
    )
    .run();

    matches!(output, Ok(output) if output.success && !output.stdout.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transient::{pull_menu, push_menu, MenuKind};

    fn plan(command: MagitCommand, args: &[&str]) -> Plan {
        let args: Vec<String> = args.iter().map(ToString::to_string).collect();
        resolve(command, &args).expect("this action runs something")
    }

    #[test]
    fn an_action_that_runs_nothing_has_no_plan() {
        for command in [
            MagitCommand::OpenMenu(MenuKind::Commit),
            MagitCommand::Status,
            MagitCommand::Refresh,
            MagitCommand::Quit,
        ] {
            assert!(resolve(command, &[]).is_none(), "{command:?}");
        }
    }

    #[test]
    fn the_menus_arguments_are_appended_to_the_subcommand() {
        let pull = plan(MagitCommand::Pull, &["--rebase", "--autostash"]);
        assert_eq!(pull.args, ["pull", "--rebase", "--autostash"]);
        assert_eq!(pull.command_line(), "git pull --rebase --autostash");
    }

    #[test]
    fn a_menus_switches_reach_the_plan_unchanged() {
        // The arguments a real menu produces, end to end.
        let mut menu = pull_menu();
        for key in ['-', 'r', '-', 'a'] {
            menu.handle_key(key);
        }

        let from_menu = resolve(MagitCommand::Pull, &menu.args()).unwrap();
        assert_eq!(from_menu.command_line(), "git pull --rebase --autostash");
    }

    #[test]
    fn commit_waits_for_a_message() {
        let commit = plan(MagitCommand::Commit, &["--signoff"]);
        assert_eq!(commit.args, ["commit", "--signoff"]);
        assert_eq!(
            commit.requirement,
            Requirement::CommitMessage { amend: false }
        );

        // Amend seeds the buffer from HEAD instead of starting empty.
        let amend = plan(MagitCommand::CommitAmend, &[]);
        assert_eq!(
            amend.requirement,
            Requirement::CommitMessage { amend: true }
        );
        assert_eq!(amend.args, ["commit", "--amend"]);
    }

    #[test]
    fn extend_and_fixup_need_no_message() {
        let extend = plan(MagitCommand::CommitExtend, &[]);
        assert_eq!(extend.args, ["commit", "--amend", "--no-edit"]);
        assert_eq!(extend.requirement, Requirement::None);

        let fixup = plan(MagitCommand::CommitFixup, &[]);
        assert_eq!(fixup.args, ["commit", "--fixup=HEAD"]);
        assert_eq!(fixup.requirement, Requirement::None);
    }

    #[test]
    fn a_forced_push_is_flagged_as_destructive() {
        assert!(!plan(MagitCommand::Push, &[]).destructive);
        assert!(plan(MagitCommand::Push, &["--force"]).destructive);
        // `--force-with-lease` is safer but can still discard remote commits.
        assert!(plan(MagitCommand::Push, &["--force-with-lease"]).destructive);

        // And it comes out of the real menu the same way.
        let mut menu = push_menu();
        menu.handle_key('-');
        menu.handle_key('F');
        let from_menu = resolve(MagitCommand::Push, &menu.args()).unwrap();
        assert!(from_menu.destructive, "args were {:?}", menu.args());
    }

    #[test]
    fn the_actions_that_can_lose_work_are_flagged() {
        assert!(plan(MagitCommand::BranchDelete, &[]).destructive);
        assert!(plan(MagitCommand::RebaseAbort, &[]).destructive);
        assert!(plan(MagitCommand::RebaseOntoUpstream, &[]).destructive);

        // And the ones that cannot are not, or every action would prompt.
        assert!(!plan(MagitCommand::Fetch, &[]).destructive);
        assert!(!plan(MagitCommand::Pull, &[]).destructive);
        assert!(!plan(MagitCommand::Commit, &[]).destructive);
    }

    #[test]
    fn the_actions_needing_a_name_or_a_remote_say_so() {
        for command in [
            MagitCommand::BranchCheckout,
            MagitCommand::BranchCreate,
            MagitCommand::BranchCreateAndCheckout,
            MagitCommand::BranchDelete,
        ] {
            assert_eq!(
                plan(command, &[]).requirement,
                Requirement::BranchName,
                "{command:?}"
            );
        }

        assert_eq!(
            plan(MagitCommand::PushElsewhere, &[]).requirement,
            Requirement::Remote
        );
    }

    #[test]
    fn rebase_continue_and_abort_ignore_the_menus_arguments() {
        // `--autostash` has no meaning once a rebase is under way, and git
        // rejects it there.
        let continued = plan(MagitCommand::RebaseContinue, &["--autostash"]);
        assert_eq!(continued.args, ["rebase", "--continue"]);

        let aborted = plan(MagitCommand::RebaseAbort, &["--interactive"]);
        assert_eq!(aborted.args, ["rebase", "--abort"]);
    }

    #[test]
    fn an_interactive_rebase_goes_through_its_todo_list() {
        let interactive = plan(MagitCommand::RebaseInteractive, &["--autosquash"]);
        assert_eq!(
            interactive.args,
            ["rebase", "--interactive", "--autosquash"]
        );
        assert_eq!(interactive.requirement, Requirement::TodoList);
        // The list is the confirmation.
        assert!(!interactive.destructive);

        // The switch on the plain rebase makes it the interactive one, once.
        let switched = plan(MagitCommand::RebaseOntoUpstream, &["--interactive"]);
        assert_eq!(switched.args, ["rebase", "--interactive"]);
        assert_eq!(switched.requirement, Requirement::TodoList);
    }

    #[test]
    fn a_summary_shows_what_progress_output_ends_on() {
        let output = GitOutput {
            success: true,
            stdout: String::new(),
            stderr: "Rebasing (1/2)\rRebasing (2/2)\r\x1b[KSuccessfully rebased.\n".into(),
        };
        assert_eq!(output.summary(), "Successfully rebased.");
    }

    #[test]
    fn the_process_can_never_wait_for_a_human() {
        let command = GitCommand::new("/tmp", vec!["status".into()]);
        let built = command.build();
        let env: std::collections::HashMap<_, _> = built
            .get_envs()
            .filter_map(|(key, value)| Some((key.to_string_lossy().into_owned(), value?)))
            .collect();

        // An editor or a sequence editor would block on a terminal that is
        // not there.
        assert_eq!(env.get("GIT_EDITOR").unwrap().to_string_lossy(), "true");
        assert_eq!(
            env.get("GIT_SEQUENCE_EDITOR").unwrap().to_string_lossy(),
            "true"
        );
        // So would a credential prompt.
        assert_eq!(
            env.get("GIT_TERMINAL_PROMPT").unwrap().to_string_lossy(),
            "0"
        );
        // And a pager would never exit.
        assert_eq!(env.get("GIT_PAGER").unwrap().to_string_lossy(), "cat");
    }

    #[test]
    fn comments_are_stripped_the_way_git_strips_them() {
        let text = "a message\n\n# a comment\n  # an indented comment\nmore text\n";
        assert_eq!(strip_comments(text), "a message\n\nmore text");

        // Only comments and blank lines means the commit was aborted.
        assert!(strip_comments("# just\n# comments\n\n").is_empty());
        assert!(strip_comments("").is_empty());
        assert!(strip_comments("   \n\n").is_empty());

        // A `#` inside a line is not a comment marker.
        assert_eq!(strip_comments("fix issue #12\n"), "fix issue #12");
    }

    #[test]
    fn the_summary_prefers_gits_own_diagnosis() {
        let failed = GitOutput {
            success: false,
            stdout: "something on stdout\n".into(),
            stderr: "\nfatal: not a git repository\nmore detail\n".into(),
        };
        assert_eq!(failed.summary(), "fatal: not a git repository");

        // With nothing on stderr, stdout is the next best thing.
        let quiet = GitOutput {
            success: true,
            stdout: "Everything up-to-date\n".into(),
            stderr: String::new(),
        };
        assert_eq!(quiet.summary(), "Everything up-to-date");

        // And a command that said nothing at all still reads sensibly.
        let silent = GitOutput {
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        };
        assert_eq!(silent.summary(), "done");
    }
}
