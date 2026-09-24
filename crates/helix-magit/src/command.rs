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
    /// Values have to be supplied, asked in order. They fill the plan's
    /// `{0}`, `{1}`… placeholders, or are appended when it has none.
    Ask(Vec<Ask>),
    /// An interactive rebase: its todo-list has to be edited first, and
    /// where to rebase from may still have to be asked.
    TodoList,
}

/// What a value asked for is, which decides how it is completed and
/// whether the thing under the cursor can supply it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AskKind {
    Branch,
    Remote,
    /// Any commit-ish: a hash, a branch, a tag, a stash.
    Revision,
    Tag,
    Stash,
    Path,
    /// A new name or a URL: nothing to complete from.
    Text,
    /// Free text passed as `--message=…`, the one kind of answer that may
    /// start with a dash.
    Message,
}

/// One value an action asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub kind: AskKind,
    pub label: &'static str,
    /// Whether an empty answer is allowed; it then contributes nothing.
    pub optional: bool,
    /// Already answered, by what the menu was opened on.
    pub preset: Option<String>,
    /// Whether a preset is only a suggestion, shown for editing rather than
    /// taken as the answer: the file an ignore pattern starts from.
    pub suggested: bool,
    /// Whether the answer is several arguments, split at spaces: refspecs.
    pub words: bool,
}

impl Ask {
    pub fn required(kind: AskKind, label: &'static str) -> Self {
        Self {
            kind,
            label,
            optional: false,
            preset: None,
            suggested: false,
            words: false,
        }
    }

    /// Makes the answer several arguments, split at spaces.
    pub fn words(mut self) -> Self {
        self.words = true;
        self
    }

    /// Makes a preset a suggestion to edit rather than the answer.
    pub fn suggested(mut self) -> Self {
        self.suggested = true;
        self
    }

    pub fn optional(kind: AskKind, label: &'static str) -> Self {
        Self {
            optional: true,
            ..Self::required(kind, label)
        }
    }

    /// Why `answer` cannot be used, if it cannot. Anything but a message
    /// lands on git's command line as an argument of its own, where a
    /// leading dash would make it an option.
    pub fn refuse(&self, answer: &str) -> Option<String> {
        if answer.is_empty() && !self.optional {
            Some(format!("{} is required", self.label))
        } else if answer.starts_with('-') && self.kind != AskKind::Message {
            Some(format!("`{answer}` would be taken for an option"))
        } else {
            None
        }
    }

    /// Whether a value of kind `given` answers this: a revision takes any
    /// commit-ish, anything else only its own kind.
    pub fn takes(&self, given: AskKind) -> bool {
        self.kind == given
            || (self.kind == AskKind::Revision
                && matches!(
                    given,
                    AskKind::Branch | AskKind::Tag | AskKind::Stash | AskKind::Revision
                ))
    }
}

/// Work a plan does that is not one git command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Special {
    /// Stash the working tree's changes and leave the index alone.
    StashWorktree,
    /// Add the answer to `.gitignore` (shared) or `.git/info/exclude`.
    Ignore { private: bool },
    /// Move the unpushed commits to a new branch, which is checked out
    /// (`spinoff`) or not (`spinout`), and reset the current one to its
    /// upstream.
    Spinoff { checkout: bool },
    /// Resolve a conflicted path with one side's whole file — or with its
    /// deletion, when that side deleted it.
    TakeSide { theirs: bool },
    /// Add a conflicted path as resolved, refusing while a conflict marker
    /// is left in it.
    MarkResolved,
    /// `--continue` for whichever operation stopped.
    Continue,
    /// A shell command line, `args[0]`, run by `sh -c` in the repository.
    Shell,
}

/// What an action would do, before anything is run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Arguments to `git`, without the program itself.
    pub args: Vec<String>,
    /// Further git commands run after `args`, each only if the one before
    /// succeeded.
    pub then: Vec<Vec<String>>,
    /// What is still missing.
    pub requirement: Requirement,
    /// Whether this can destroy work the user cannot get back.
    pub destructive: bool,
    /// A short description, for the confirmation prompt and the status line.
    pub summary: String,
    /// Set when the plan is carried out by [`run_plan`] rather than as git
    /// command lines.
    pub special: Option<Special>,
}

impl Plan {
    pub fn new(args: impl IntoIterator<Item = impl Into<String>>, summary: &str) -> Self {
        Self {
            args: args.into_iter().map(Into::into).collect(),
            then: Vec::new(),
            requirement: Requirement::None,
            destructive: false,
            summary: summary.to_string(),
            special: None,
        }
    }

    fn asking(self, asks: impl IntoIterator<Item = Ask>) -> Self {
        self.requiring(Requirement::Ask(asks.into_iter().collect()))
    }

    fn special(mut self, special: Special) -> Self {
        self.special = Some(special);
        self
    }

    /// Answers the first question `given` can answer, as when a menu is
    /// opened on a commit. Returns whether one was answered.
    pub fn preset(&mut self, value: &str, given: AskKind) -> bool {
        if let Requirement::Ask(asks) = &mut self.requirement {
            if let Some(ask) = asks
                .iter_mut()
                .find(|ask| ask.preset.is_none() && ask.takes(given))
            {
                ask.preset = Some(value.to_string());
                return true;
            }
        }
        false
    }

    /// The plan with its questions answered, one answer per question.
    ///
    /// `{N}` in an argument is replaced by answer N; with no placeholder
    /// anywhere, the answers are appended instead. An empty answer drops a
    /// placeholder-only argument, and a `--flag={N}` one with it.
    pub fn answered(&self, answers: &[String]) -> Plan {
        let placeholders = self
            .args
            .iter()
            .chain(self.then.iter().flatten())
            .any(|arg| arg.contains("{0}"));
        let words: Vec<bool> = match &self.requirement {
            Requirement::Ask(asks) => asks.iter().map(|ask| ask.words).collect(),
            _ => Vec::new(),
        };
        let split = |index: usize, answer: &String| -> Vec<String> {
            if words.get(index).copied().unwrap_or(false) {
                answer.split_whitespace().map(str::to_string).collect()
            } else {
                vec![answer.clone()]
            }
        };
        let fill = |args: &[String]| -> Vec<String> {
            args.iter()
                .flat_map(|arg| {
                    // A placeholder alone, for an answer of several words.
                    if let Some(index) = arg
                        .strip_prefix('{')
                        .and_then(|rest| rest.strip_suffix('}'))
                        .and_then(|index| index.parse::<usize>().ok())
                    {
                        if words.get(index).copied().unwrap_or(false) {
                            return answers
                                .get(index)
                                .map(|a| split(index, a))
                                .unwrap_or_default();
                        }
                    }
                    let mut out = arg.clone();
                    let mut emptied = false;
                    for (index, answer) in answers.iter().enumerate() {
                        let token = format!("{{{index}}}");
                        if out.contains(&token) {
                            emptied |= answer.is_empty();
                            out = out.replace(&token, answer);
                        }
                    }
                    (!(emptied && (out.is_empty() || out.ends_with('='))))
                        .then_some(out)
                        .into_iter()
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        let mut plan = self.clone();
        plan.requirement = Requirement::None;
        plan.args = fill(&self.args);
        plan.then = self.then.iter().map(|args| fill(args)).collect();
        if !placeholders {
            plan.args.extend(
                answers
                    .iter()
                    .enumerate()
                    .filter(|(_, answer)| !answer.is_empty())
                    .flat_map(|(index, answer)| split(index, answer)),
            );
        }
        plan
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
        if self.special == Some(Special::Shell) {
            return format!("$ {}", self.args.join(" "));
        }
        if self.special == Some(Special::Continue) {
            return "git … --continue".to_string();
        }
        if self.special.is_some() {
            return format!("{} ({})", self.summary, self.args.join(" "));
        }
        std::iter::once(&self.args)
            .chain(&self.then)
            .map(|args| {
                let words: Vec<String> = args.iter().map(|arg| quote_for_display(arg)).collect();
                format!("git {}", words.join(" "))
            })
            .collect::<Vec<_>>()
            .join(" && ")
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
        | MagitCommand::Quit
        | MagitCommand::LogCurrent
        | MagitCommand::LogAll
        | MagitCommand::LogOther
        | MagitCommand::Reflog
        | MagitCommand::ReflogOther
        | MagitCommand::ShowRefs
        | MagitCommand::ShowCherries
        | MagitCommand::ShowProcess
        | MagitCommand::Shortlog
        | MagitCommand::ConflictEdit
        | MagitCommand::ConflictShowOurs
        | MagitCommand::ConflictShowTheirs
        | MagitCommand::ConflictShowBase
        | MagitCommand::FileDiff
        | MagitCommand::FileLog
        | MagitCommand::FileBlame
        | MagitCommand::RunGit
        | MagitCommand::RunShell
        | MagitCommand::JumpTo(_)
        | MagitCommand::SwitchTo(_)
        | MagitCommand::ApplyDiffSettings
        | MagitCommand::DiffRange
        | MagitCommand::DiffWorktree
        | MagitCommand::DiffCommit => return None,

        // Run from the editor's directory rather than a repository's.
        MagitCommand::Clone => Plan::new(["clone", "{0}", "{1}"], "Clone").asking([
            Ask::required(AskKind::Text, "Clone from (URL or path)"),
            Ask::optional(
                AskKind::Path,
                "Into directory (empty for the repository's name)",
            ),
        ]),
        MagitCommand::Init => Plan::new(["init", "{0}"], "Init").asking([Ask::optional(
            AskKind::Path,
            "Make a repository in (empty for here)",
        )]),

        MagitCommand::FileStage => Plan::new(["add", "--", "{0}"], "Stage the file")
            .asking([Ask::required(AskKind::Path, "Stage file")]),
        MagitCommand::FileUnstage => {
            Plan::new(["reset", "--quiet", "--", "{0}"], "Unstage the file")
                .asking([Ask::required(AskKind::Path, "Unstage file")])
        }

        // ── Conflicts ──
        MagitCommand::ConflictTakeOurs => Plan::new(["{0}"], "Resolve with our side")
            .asking([Ask::required(AskKind::Path, "Conflicted file")])
            .special(Special::TakeSide { theirs: false })
            .destructive(),
        MagitCommand::ConflictTakeTheirs => Plan::new(["{0}"], "Resolve with their side")
            .asking([Ask::required(AskKind::Path, "Conflicted file")])
            .special(Special::TakeSide { theirs: true })
            .destructive(),
        MagitCommand::ConflictMarkResolved => Plan::new(["{0}"], "Mark resolved")
            .asking([Ask::required(AskKind::Path, "Conflicted file")])
            .special(Special::MarkResolved),
        // Rewrites the file from the index, so edits made to it are lost.
        MagitCommand::ConflictWithBase => Plan::new(
            ["checkout", "--conflict=diff3", "--", "{0}"],
            "Rewrite the conflicts with the base shown, discarding edits to the file",
        )
        .asking([Ask::required(AskKind::Path, "Conflicted file")])
        .destructive(),
        MagitCommand::Continue => {
            Plan::new(Vec::<String>::new(), "Continue").special(Special::Continue)
        }

        // ── Configuration ──
        MagitCommand::BranchConfigDescription => Plan::new(
            ["config", "branch.{0}.description", "{1}"],
            "Describe a branch",
        )
        .asking([
            Ask::required(AskKind::Branch, "Branch"),
            Ask::required(AskKind::Message, "Description"),
        ]),
        // `git branch --set-upstream-to` writes both `merge` and `remote`.
        MagitCommand::BranchConfigUpstream => Plan::new(
            ["branch", "--set-upstream-to={1}", "{0}"],
            "Set a branch's upstream",
        )
        .asking([
            Ask::required(AskKind::Branch, "Branch"),
            Ask::required(AskKind::Branch, "Upstream (remote/branch)"),
        ]),
        MagitCommand::BranchConfigRebase => Plan::new(
            ["config", "branch.{0}.rebase", "{1}"],
            "Set whether pull rebases",
        )
        .asking([
            Ask::required(AskKind::Branch, "Branch"),
            Ask::required(
                AskKind::Text,
                "Rebase on pull (true, false, interactive, merges)",
            ),
        ]),
        MagitCommand::BranchConfigPushRemote => Plan::new(
            ["config", "branch.{0}.pushRemote", "{1}"],
            "Set where a branch pushes",
        )
        .asking([
            Ask::required(AskKind::Branch, "Branch"),
            Ask::required(AskKind::Remote, "Push to remote"),
        ]),
        MagitCommand::RemoteConfigUrl => {
            Plan::new(["remote", "set-url", "{0}", "{1}"], "Set a remote's URL").asking([
                Ask::required(AskKind::Remote, "Remote"),
                Ask::required(AskKind::Text, "URL"),
            ])
        }
        MagitCommand::RemoteConfigPushUrl => Plan::new(
            ["remote", "set-url", "--push", "{0}", "{1}"],
            "Set a remote's push URL",
        )
        .asking([
            Ask::required(AskKind::Remote, "Remote"),
            Ask::required(AskKind::Text, "Push URL"),
        ]),
        MagitCommand::RemoteConfigFetch => Plan::new(
            ["config", "remote.{0}.fetch", "{1}"],
            "Set a remote's fetch refspec",
        )
        .asking([
            Ask::required(AskKind::Remote, "Remote"),
            Ask::required(
                AskKind::Text,
                "Refspec (+refs/heads/*:refs/remotes/<remote>/*)",
            ),
        ]),

        // Where to reset to is asked, unless the menu was opened on a
        // commit. Hard throws away uncommitted work; keep refuses to.
        MagitCommand::ResetMixed => Plan::new(["reset", "--mixed"], "Reset HEAD and the index")
            .asking([Ask::required(AskKind::Revision, "Reset to")]),
        MagitCommand::ResetSoft => Plan::new(["reset", "--soft"], "Reset HEAD")
            .asking([Ask::required(AskKind::Revision, "Reset to")]),
        MagitCommand::ResetHard => Plan::new(
            ["reset", "--hard"],
            "Reset HEAD, index and worktree, discarding uncommitted changes",
        )
        .asking([Ask::required(AskKind::Revision, "Reset to")])
        .destructive(),
        MagitCommand::ResetKeep => {
            Plan::new(["reset", "--keep"], "Reset HEAD, keeping local changes")
                .asking([Ask::required(AskKind::Revision, "Reset to")])
        }

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
        MagitCommand::Push | MagitCommand::PushToUpstream => {
            let plan = Plan::new(with(["push"], args), "Push");
            if forced {
                plan.destructive()
            } else {
                plan
            }
        }
        // The current branch, under its own name, to a remote picked from
        // the configured ones.
        MagitCommand::PushElsewhere => {
            let mut push = with(["push"], args);
            push.extend(["{0}".to_string(), "HEAD".to_string()]);
            let plan = Plan::new(push, "Push elsewhere")
                .asking([Ask::required(AskKind::Remote, "Push to remote")]);
            if forced {
                plan.destructive()
            } else {
                plan
            }
        }

        MagitCommand::PushRefspecs => {
            let mut push = with(["push"], args);
            push.extend(["{0}".to_string(), "{1}".to_string()]);
            let plan = Plan::new(push, "Push refspecs").asking([
                Ask::required(AskKind::Remote, "Push to remote"),
                Ask::required(AskKind::Text, "Refspecs (e.g. HEAD:refs/for/main)").words(),
            ]);
            if forced {
                plan.destructive()
            } else {
                plan
            }
        }

        MagitCommand::Pull => Plan::new(with(["pull"], args), "Pull"),
        MagitCommand::Fetch => Plan::new(with(["fetch"], args), "Fetch"),
        MagitCommand::FetchAll => Plan::new(with(["fetch", "--all"], args), "Fetch all remotes"),

        // Any revision, a branch or not; one the menu was opened on is
        // offered for editing rather than checked out at once.
        MagitCommand::BranchCheckout => Plan::new(with(["checkout"], args), "Check out")
            .asking([Ask::required(AskKind::Revision, "Check out").suggested()]),
        MagitCommand::BranchCreate => {
            Plan::new(with(["branch"], args), "Create a branch").asking([
                Ask::required(AskKind::Text, "New branch"),
                Ask::optional(AskKind::Revision, "Starting at (empty for HEAD)"),
            ])
        }
        MagitCommand::BranchCreateAndCheckout => Plan::new(
            with(["checkout", "-b"], args),
            "Create and check out a branch",
        )
        .asking([
            Ask::required(AskKind::Text, "New branch"),
            Ask::optional(AskKind::Revision, "Starting at (empty for HEAD)"),
        ]),
        // Deleting a branch can lose commits that nothing else points at.
        MagitCommand::BranchDelete => {
            Plan::new(with(["branch", "--delete"], args), "Delete a branch")
                .asking([Ask::required(AskKind::Branch, "Delete branch")])
                .destructive()
        }
        MagitCommand::BranchRename => {
            Plan::new(["branch", "--move", "{0}", "{1}"], "Rename a branch").asking([
                Ask::required(AskKind::Branch, "Rename branch"),
                Ask::required(AskKind::Text, "To"),
            ])
        }
        // Points a branch elsewhere; the commits only it had are lost.
        MagitCommand::BranchReset => {
            Plan::new(["branch", "--force", "{0}", "{1}"], "Reset a branch")
                .asking([
                    Ask::required(AskKind::Branch, "Reset branch"),
                    Ask::required(AskKind::Revision, "To"),
                ])
                .destructive()
        }
        MagitCommand::BranchSpinoff => Plan::new(["{0}"], "Spin off the unpushed commits")
            .asking([Ask::required(AskKind::Text, "New branch")])
            .special(Special::Spinoff { checkout: true }),
        MagitCommand::BranchSpinout => Plan::new(["{0}"], "Spin out the unpushed commits")
            .asking([Ask::required(AskKind::Text, "New branch")])
            .special(Special::Spinoff { checkout: false }),

        // ── Stash ──
        MagitCommand::StashBoth => stash_push(args, &[], "Stash index and worktree"),
        MagitCommand::StashIndex => stash_push(args, &["--staged"], "Stash the index"),
        MagitCommand::StashWorktree => {
            stash_push(args, &[], "Stash the worktree").special(Special::StashWorktree)
        }
        MagitCommand::StashPop => {
            Plan::new(["stash", "pop"], "Pop a stash").asking([Ask::optional(
                AskKind::Stash,
                "Pop stash (empty for the latest)",
            )])
        }
        MagitCommand::StashApply => {
            Plan::new(["stash", "apply"], "Apply a stash").asking([Ask::optional(
                AskKind::Stash,
                "Apply stash (empty for the latest)",
            )])
        }
        MagitCommand::StashDrop => Plan::new(["stash", "drop"], "Drop a stash")
            .asking([Ask::optional(
                AskKind::Stash,
                "Drop stash (empty for the latest)",
            )])
            .destructive(),
        MagitCommand::StashBranch => {
            Plan::new(["stash", "branch", "{0}", "{1}"], "Branch from a stash").asking([
                Ask::required(AskKind::Text, "New branch"),
                Ask::optional(AskKind::Stash, "From stash (empty for the latest)"),
            ])
        }

        // ── Merge ──
        MagitCommand::Merge => Plan::new(with(["merge"], args), "Merge")
            .asking([Ask::required(AskKind::Revision, "Merge")]),
        MagitCommand::MergeSquash => Plan::new(with(["merge", "--squash"], args), "Squash-merge")
            .asking([Ask::required(AskKind::Revision, "Squash-merge")]),
        MagitCommand::MergeNoCommit => Plan::new(
            with(["merge", "--no-commit", "--no-ff"], args),
            "Merge without committing",
        )
        .asking([Ask::required(AskKind::Revision, "Merge")]),
        MagitCommand::MergeContinue => Plan::new(["merge", "--continue"], "Commit the merge"),
        MagitCommand::MergeAbort => Plan::new(
            ["merge", "--abort"],
            "Abort the merge, discarding its changes",
        )
        .destructive(),

        // ── Tag ──
        MagitCommand::TagCreate => {
            let mut tag = with(["tag"], args);
            tag.extend(["{0}".to_string(), "{1}".to_string()]);
            Plan::new(tag, "Create a tag").asking([
                Ask::required(AskKind::Text, "Tag name"),
                Ask::optional(AskKind::Revision, "At (empty for HEAD)"),
            ])
        }
        MagitCommand::TagDelete => Plan::new(["tag", "--delete"], "Delete a tag")
            .asking([Ask::required(AskKind::Tag, "Delete tag")])
            .destructive(),
        MagitCommand::TagPush => {
            Plan::new(["push", "--tags"], "Push tags").asking([Ask::optional(
                AskKind::Remote,
                "To remote (empty for the default)",
            )])
        }

        // ── Cherry-pick and revert ──
        MagitCommand::CherryPick => Plan::new(with(["cherry-pick"], args), "Cherry-pick")
            .asking([Ask::required(AskKind::Revision, "Cherry-pick")]),
        MagitCommand::CherryApply => Plan::new(
            with(["cherry-pick", "--no-commit"], args),
            "Apply a commit without committing",
        )
        .asking([Ask::required(AskKind::Revision, "Apply")]),
        MagitCommand::CherryPickContinue => {
            Plan::new(["cherry-pick", "--continue"], "Continue the cherry-pick")
        }
        MagitCommand::CherryPickSkip => {
            Plan::new(["cherry-pick", "--skip"], "Skip this commit").destructive()
        }
        MagitCommand::CherryPickAbort => {
            Plan::new(["cherry-pick", "--abort"], "Abort the cherry-pick").destructive()
        }
        MagitCommand::Revert => Plan::new(with(["revert", "--no-edit"], args), "Revert")
            .asking([Ask::required(AskKind::Revision, "Revert")]),
        MagitCommand::RevertNoCommit => Plan::new(
            with(["revert", "--no-commit"], args),
            "Revert without committing",
        )
        .asking([Ask::required(AskKind::Revision, "Revert")]),
        MagitCommand::RevertContinue => Plan::new(["revert", "--continue"], "Continue the revert"),
        MagitCommand::RevertSkip => {
            Plan::new(["revert", "--skip"], "Skip this commit").destructive()
        }
        MagitCommand::RevertAbort => {
            Plan::new(["revert", "--abort"], "Abort the revert").destructive()
        }

        // ── Remotes ──
        MagitCommand::RemoteAdd => Plan::new(["remote", "add", "{0}", "{1}"], "Add a remote")
            .asking([
                Ask::required(AskKind::Text, "Remote name"),
                Ask::required(AskKind::Text, "URL"),
            ]),
        MagitCommand::RemoteRename => {
            Plan::new(["remote", "rename", "{0}", "{1}"], "Rename a remote").asking([
                Ask::required(AskKind::Remote, "Rename remote"),
                Ask::required(AskKind::Text, "To"),
            ])
        }
        MagitCommand::RemoteRemove => Plan::new(["remote", "remove"], "Remove a remote")
            .asking([Ask::required(AskKind::Remote, "Remove remote")])
            .destructive(),
        MagitCommand::RemoteSetUpstream => {
            Plan::new(["branch", "--set-upstream-to={0}"], "Set the upstream")
                .asking([Ask::required(AskKind::Branch, "Upstream (remote/branch)")])
        }
        MagitCommand::RemoteFetch => Plan::new(with(["fetch"], args), "Fetch a remote")
            .asking([Ask::required(AskKind::Remote, "Fetch remote")]),

        // ── Bisect ──
        MagitCommand::BisectStart => {
            Plan::new(["bisect", "start", "{0}", "{1}"], "Start bisecting").asking([
                Ask::required(AskKind::Revision, "Bad revision"),
                Ask::required(AskKind::Revision, "Good revision"),
            ])
        }
        MagitCommand::BisectGood => Plan::new(["bisect", "good"], "Mark good"),
        MagitCommand::BisectBad => Plan::new(["bisect", "bad"], "Mark bad"),
        MagitCommand::BisectSkip => Plan::new(["bisect", "skip"], "Skip this commit"),
        MagitCommand::BisectReset => Plan::new(["bisect", "reset"], "End the bisect"),

        // ── Worktrees and submodules ──
        MagitCommand::WorktreeAdd => Plan::new(["worktree", "add", "{0}", "{1}"], "Add a worktree")
            .asking([
                Ask::required(AskKind::Path, "New worktree at"),
                Ask::optional(AskKind::Revision, "Checking out (empty for a new branch)"),
            ]),
        MagitCommand::WorktreeRemove => Plan::new(["worktree", "remove"], "Remove a worktree")
            .asking([Ask::required(AskKind::Path, "Remove worktree")])
            .destructive(),
        MagitCommand::WorktreePrune => Plan::new(["worktree", "prune"], "Prune worktrees"),
        MagitCommand::SubmoduleAdd => {
            Plan::new(["submodule", "add", "{0}", "{1}"], "Add a submodule").asking([
                Ask::required(AskKind::Text, "Submodule URL"),
                Ask::optional(AskKind::Path, "At path (empty for its name)"),
            ])
        }
        MagitCommand::SubmoduleUpdate => Plan::new(
            ["submodule", "update", "--init", "--recursive"],
            "Update submodules",
        ),
        MagitCommand::SubmoduleSync => {
            Plan::new(["submodule", "sync", "--recursive"], "Sync submodule URLs")
        }
        MagitCommand::SubmoduleFetch => Plan::new(
            ["submodule", "foreach", "--recursive", "git fetch"],
            "Fetch in every submodule",
        ),

        // ── Patches ──
        MagitCommand::PatchApplyMailbox => Plan::new(with(["am"], args), "Apply patches")
            .asking([Ask::required(AskKind::Path, "Patch or mailbox")]),
        MagitCommand::PatchApplyPlain => Plan::new(with(["apply"], args), "Apply a patch")
            .asking([Ask::required(AskKind::Path, "Patch")]),
        MagitCommand::AmContinue => Plan::new(["am", "--continue"], "Continue applying"),
        MagitCommand::AmSkip => Plan::new(["am", "--skip"], "Skip this patch").destructive(),
        MagitCommand::AmAbort => Plan::new(["am", "--abort"], "Abort applying").destructive(),
        MagitCommand::FormatPatch => Plan::new(with(["format-patch"], args), "Format patches")
            .asking([Ask::required(
                AskKind::Revision,
                "Commits (e.g. origin/main..)",
            )]),

        // ── Subtrees ──
        MagitCommand::SubtreeAdd => subtree("add", "Add a subtree"),
        MagitCommand::SubtreePull => subtree("pull", "Pull into a subtree"),
        MagitCommand::SubtreePush => subtree("push", "Push a subtree"),
        MagitCommand::SubtreeSplit => Plan::new(
            ["subtree", "split", "--prefix={0}", "--branch={1}"],
            "Split a subtree into a branch",
        )
        .asking([
            Ask::required(AskKind::Path, "Subtree prefix"),
            Ask::required(AskKind::Text, "Into branch"),
        ]),

        // ── Notes ──
        MagitCommand::NoteEdit => Plan::new(
            ["notes", "add", "--force", "--message={0}", "{1}"],
            "Set a note",
        )
        .asking([
            Ask::required(AskKind::Message, "Note"),
            Ask::optional(AskKind::Revision, "On commit (empty for HEAD)"),
        ]),
        MagitCommand::NoteAppend => Plan::new(
            ["notes", "append", "--message={0}", "{1}"],
            "Append to a note",
        )
        .asking([
            Ask::required(AskKind::Message, "Append"),
            Ask::optional(AskKind::Revision, "On commit (empty for HEAD)"),
        ]),
        MagitCommand::NoteRemove => Plan::new(["notes", "remove"], "Remove a note")
            .asking([Ask::optional(
                AskKind::Revision,
                "From commit (empty for HEAD)",
            )])
            .destructive(),
        MagitCommand::NotePrune => Plan::new(["notes", "prune"], "Prune notes of lost commits"),

        // ── Ignoring ──
        MagitCommand::IgnoreShared => Plan::new(["{0}"], "Ignore in .gitignore")
            .asking([Ask::required(AskKind::Path, "Ignore (pattern)").suggested()])
            .special(Special::Ignore { private: false }),
        MagitCommand::IgnorePrivate => Plan::new(["{0}"], "Ignore in .git/info/exclude")
            .asking([Ask::required(AskKind::Path, "Ignore (pattern)").suggested()])
            .special(Special::Ignore { private: true }),

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
        // `git rebase --onto new old`: the commits after `old` replayed on
        // `new`. The todo-list flow asks only for a base, so this one is
        // never interactive.
        MagitCommand::RebaseOnto => {
            let mut rebase = with(
                ["rebase"],
                &args
                    .iter()
                    .filter(|arg| *arg != "--interactive")
                    .cloned()
                    .collect::<Vec<_>>(),
            );
            rebase.extend(["--onto", "{0}", "{1}"].map(str::to_string));
            Plan::new(rebase, "Rebase onto a revision")
                .asking([
                    Ask::required(AskKind::Revision, "Onto"),
                    Ask::required(
                        AskKind::Revision,
                        "The commits after (usually the upstream)",
                    ),
                ])
                .destructive()
        }
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

/// `git stash push`, with the menu's arguments and a message.
fn stash_push(args: &[String], extra: &[&'static str], summary: &str) -> Plan {
    let mut push = with(["stash", "push"], args);
    push.extend(extra.iter().map(|arg| arg.to_string()));
    push.push("--message={0}".into());
    Plan::new(push, summary).asking([Ask::optional(AskKind::Message, "Stash message")])
}

/// `git subtree add|pull|push --prefix=… <repository> <ref>`.
fn subtree(action: &'static str, summary: &str) -> Plan {
    Plan::new(["subtree", action, "--prefix={0}", "{1}", "{2}"], summary).asking([
        Ask::required(AskKind::Path, "Subtree prefix"),
        Ask::required(AskKind::Text, "Repository (URL or remote)"),
        Ask::required(AskKind::Text, "Ref"),
    ])
}

/// An argument as it would have to be typed: quoted when it has spaces or
/// quotes in it, so a shown command line reads back the way it ran.
fn quote_for_display(arg: &str) -> String {
    if arg.is_empty() {
        "''".to_string()
    } else if arg
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '\'' | '"' | '\\'))
    {
        format!("'{}'", arg.replace('\'', r"'\''"))
    } else {
        arg.to_string()
    }
}

/// Splits a command line typed in into arguments, as a shell would for
/// the simple cases: words, `'…'` taken literally, `"…"` with `\"` and `\\`
/// escapes, and `\` escaping the next character outside quotes.
pub fn split_args(line: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => current.push(c),
                        None => return Err("an unclosed ' quote".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\')) => current.push(c),
                            Some(c) => {
                                current.push('\\');
                                current.push(c);
                            }
                            None => return Err("an unclosed \" quote".into()),
                        },
                        Some(c) => current.push(c),
                        None => return Err("an unclosed \" quote".into()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(c) = chars.next() {
                    current.push(c);
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    args.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                current.push(c);
            }
        }
    }
    if in_word {
        args.push(current);
    }
    Ok(args)
}

/// A plan for a git command typed in: its words after `git`.
pub fn typed_git(line: &str) -> Result<Plan, String> {
    let mut args = split_args(line)?;
    if args.first().map(String::as_str) == Some("git") {
        args.remove(0);
    }
    if args.is_empty() {
        return Err("no git command given".into());
    }
    let summary = format!("git {}", args.join(" "));
    Ok(Plan::new(args, &summary))
}

/// A plan for a shell command typed in.
pub fn typed_shell(line: &str) -> Result<Plan, String> {
    if line.trim().is_empty() {
        return Err("no command given".into());
    }
    Ok(Plan::new([line.trim().to_string()], "Shell command").special(Special::Shell))
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
        // A failure's reason may be on either stream and not first: a merge
        // says "Auto-merging" before "CONFLICT".
        if !self.success {
            let reason = self
                .stdout
                .lines()
                .chain(self.stderr.lines())
                .map(str::trim)
                .find(|line| {
                    line.starts_with("CONFLICT")
                        || line.starts_with("error:")
                        || line.starts_with("fatal:")
                });
            if let Some(reason) = reason {
                return reason.to_string();
            }
        }
        let source = if self.stderr.trim().is_empty() {
            &self.stdout
        } else {
            &self.stderr
        };
        // Progress is drawn with carriage returns and erase-line escapes:
        // what a terminal would end up showing is the text after the last
        // return, without the escapes.
        // git's `hint:` lines advise; they are not what happened.
        source
            .lines()
            .map(|line| line.rsplit('\r').next().unwrap_or(line))
            .map(|line| line.replace("\x1b[K", ""))
            .map(|line| line.trim().to_string())
            .find(|line| !line.is_empty() && !line.starts_with("hint:"))
            .or_else(|| {
                // Nothing but hints: then the other stream says it.
                let other = if std::ptr::eq(source, &self.stderr) {
                    &self.stdout
                } else {
                    &self.stderr
                };
                other
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty() && !line.starts_with("hint:"))
                    .map(str::to_string)
            })
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

/// Carries a plan out: its command lines one after the other, stopping at
/// the first that fails, or its [`Special`] work. What every step printed is
/// kept, so a failure shows what led up to it.
///
/// This blocks; callers run it off the editor's thread.
pub fn run_plan(workdir: &Path, plan: &Plan) -> std::io::Result<GitOutput> {
    let mut log = Transcript::default();
    match plan.special {
        None => {
            for args in std::iter::once(&plan.args).chain(&plan.then) {
                if !log.run(workdir, args)? {
                    break;
                }
            }
        }
        Some(Special::StashWorktree) => stash_worktree(workdir, &plan.args, &mut log)?,
        Some(Special::Ignore { private }) => {
            let pattern = plan.args.first().map(String::as_str).unwrap_or("");
            return ignore(workdir, pattern, private);
        }
        Some(Special::Spinoff { checkout }) => {
            let new = plan.args.first().cloned().unwrap_or_default();
            spinoff(workdir, &new, checkout, &mut log)?;
        }
        Some(Special::TakeSide { theirs }) => {
            let path = plan.args.first().cloned().unwrap_or_default();
            take_side(workdir, Path::new(&path), theirs, &mut log)?;
        }
        Some(Special::MarkResolved) => {
            let path = plan.args.first().cloned().unwrap_or_default();
            mark_resolved(workdir, Path::new(&path), &mut log)?;
        }
        Some(Special::Shell) => {
            let line = plan.args.first().cloned().unwrap_or_default();
            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg(&line)
                .current_dir(workdir)
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("GIT_EDITOR", "true")
                .env("GIT_PAGER", "cat")
                .env("PAGER", "cat")
                .stdin(std::process::Stdio::null())
                .output()?;
            return Ok(GitOutput {
                success: output.status.success(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Some(Special::Continue) => {
            let operation = crate::status::git_dir(workdir)
                .and_then(|dir| crate::status::in_progress(&dir, &|_| None))
                .map(|state| state.operation);
            match operation {
                None => log.fail("nothing is in progress"),
                Some(crate::status::Operation::Bisect) => {
                    log.fail("a bisect goes on with good, bad or skip (B)")
                }
                Some(operation) => {
                    log.run(workdir, &args_of(&[operation.command(), "--continue"]))?;
                }
            }
        }
    }
    Ok(log.output())
}

/// The outputs of several commands, as one.
#[derive(Default)]
struct Transcript {
    success: bool,
    stdout: String,
    stderr: String,
    ran: bool,
    /// What a successful multi-step operation did, said once rather than
    /// through whatever its first step printed.
    note: Option<String>,
}

impl Transcript {
    /// Runs one command; returns whether it succeeded.
    fn run(&mut self, workdir: &Path, args: &[String]) -> std::io::Result<bool> {
        let output = GitCommand::new(workdir, args.to_vec()).run()?;
        self.stdout.push_str(&output.stdout);
        self.stderr.push_str(&output.stderr);
        self.success = output.success;
        self.ran = true;
        Ok(output.success)
    }

    fn fail(&mut self, message: &str) {
        self.stderr.push_str(message);
        self.stderr.push('\n');
        self.success = false;
        self.ran = true;
    }

    fn output(self) -> GitOutput {
        let success = self.success || !self.ran;
        match self.note {
            Some(note) if success => GitOutput {
                success,
                stdout: format!("{note}\n{}{}", self.stdout, self.stderr),
                stderr: String::new(),
            },
            _ => GitOutput {
                success,
                stdout: self.stdout,
                stderr: self.stderr,
            },
        }
    }
}

fn args_of(list: &[&str]) -> Vec<String> {
    list.iter().map(|arg| arg.to_string()).collect()
}

fn stash_count(workdir: &Path) -> std::io::Result<usize> {
    let output = GitCommand::new(workdir, args_of(&["stash", "list"])).run()?;
    Ok(output.stdout.lines().count())
}

/// Stashes the working tree's changes, leaving the index as it is.
///
/// git has no switch for it, so the index is set aside in a stash of its
/// own first, the rest is stashed, and the index is put back from the first
/// stash. With nothing staged it is a plain stash. The count of stashes is
/// checked at each step, so a step that saved nothing never makes a later
/// one pop the wrong stash.
fn stash_worktree(workdir: &Path, push: &[String], log: &mut Transcript) -> std::io::Result<()> {
    let staged = !GitCommand::new(workdir, args_of(&["diff", "--cached", "--quiet"]))
        .run()?
        .success;
    if !staged {
        log.run(workdir, push)?;
        return Ok(());
    }

    let before = stash_count(workdir)?;
    let aside = args_of(&[
        "stash",
        "push",
        "--staged",
        "--message=helix: index set aside",
    ]);
    if !log.run(workdir, &aside)? {
        return Ok(());
    }
    if stash_count(workdir)? != before + 1 {
        log.fail("could not set the index aside");
        return Ok(());
    }
    if !log.run(workdir, push)? || stash_count(workdir)? != before + 2 {
        // Nothing else to stash, or it failed: give the index back.
        let saved = log.success;
        log.run(workdir, &args_of(&["stash", "pop", "--index"]))?;
        if !saved {
            log.fail("the worktree could not be stashed; the index was put back");
        }
        return Ok(());
    }
    if log.run(workdir, &args_of(&["stash", "pop", "--index", "stash@{1}"]))? {
        log.note = Some("Stashed the worktree; the index is as it was".into());
    }
    Ok(())
}

/// Adds `pattern` to the top-level `.gitignore`, or to the private
/// `info/exclude` in the git directory.
fn ignore(workdir: &Path, pattern: &str, private: bool) -> std::io::Result<GitOutput> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Ok(GitOutput {
            success: false,
            stdout: String::new(),
            stderr: "nothing to ignore".into(),
        });
    }
    let file = if private {
        let dir = GitCommand::new(workdir, args_of(&["rev-parse", "--absolute-git-dir"])).run()?;
        PathBuf::from(dir.stdout.trim())
            .join("info")
            .join("exclude")
    } else {
        workdir.join(".gitignore")
    };
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut content = std::fs::read_to_string(&file).unwrap_or_default();
    if content.lines().any(|line| line.trim() == pattern) {
        return Ok(GitOutput {
            success: true,
            stdout: format!("{pattern} is already ignored in {}", file.display()),
            stderr: String::new(),
        });
    }
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(pattern);
    content.push('\n');
    std::fs::write(&file, content)?;
    Ok(GitOutput {
        success: true,
        stdout: format!("Ignoring {pattern} in {}", file.display()),
        stderr: String::new(),
    })
}

/// Resolves a conflicted path with one side's whole version.
fn take_side(
    workdir: &Path,
    path: &Path,
    theirs: bool,
    log: &mut Transcript,
) -> std::io::Result<()> {
    use crate::conflict::{stage_content, Side};
    let side = if theirs { Side::Theirs } else { Side::Ours };
    let spec = path.display().to_string();
    match stage_content(workdir, path, side) {
        Some(content) => {
            std::fs::write(workdir.join(path), content)?;
            if log.run(workdir, &args_of(&["add", "--", &spec]))? {
                log.note = Some(format!(
                    "{spec}: resolved with {} version",
                    if theirs { "their" } else { "our" }
                ));
            }
        }
        // That side deleted it: resolving with it is the deletion.
        None => {
            if log.run(
                workdir,
                &args_of(&["rm", "--quiet", "--force", "--", &spec]),
            )? {
                log.note = Some(format!(
                    "{spec}: resolved as deleted, as {} side has it",
                    if theirs { "their" } else { "our" }
                ));
            }
        }
    }
    Ok(())
}

/// Adds a resolved path, unless a conflict marker is still in it; a path
/// that is gone is resolved as deleted.
fn mark_resolved(workdir: &Path, path: &Path, log: &mut Transcript) -> std::io::Result<()> {
    let spec = path.display().to_string();
    match std::fs::read_to_string(workdir.join(path)) {
        Ok(text) => {
            if let Some(line) = crate::conflict::first_marker(&text) {
                log.fail(&format!(
                    "{spec} still has a conflict marker on line {}",
                    line + 1
                ));
                return Ok(());
            }
            if log.run(workdir, &args_of(&["add", "--", &spec]))? {
                log.note = Some(format!("{spec}: resolved"));
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if log.run(
                workdir,
                &args_of(&["rm", "--quiet", "--cached", "--", &spec]),
            )? {
                log.note = Some(format!("{spec}: resolved as deleted"));
            }
        }
        // Not text: nothing to check for markers, the user decides.
        Err(_) => {
            if log.run(workdir, &args_of(&["add", "--", &spec]))? {
                log.note = Some(format!("{spec}: resolved"));
            }
        }
    }
    Ok(())
}

/// Moves the commits the upstream does not have to a new branch.
fn spinoff(workdir: &Path, new: &str, checkout: bool, log: &mut Transcript) -> std::io::Result<()> {
    let branch = GitCommand::new(
        workdir,
        args_of(&["symbolic-ref", "--quiet", "--short", "HEAD"]),
    )
    .run()?;
    let upstream = GitCommand::new(
        workdir,
        args_of(&[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ]),
    )
    .run()?;
    if !branch.success {
        log.fail("HEAD is detached: there is no branch to spin off from");
        return Ok(());
    }
    if !upstream.success {
        log.fail("the branch has no upstream to reset to");
        return Ok(());
    }
    let (branch, upstream) = (branch.stdout.trim(), upstream.stdout.trim());
    let done = if checkout {
        log.run(workdir, &args_of(&["checkout", "-b", new]))?
            && log.run(workdir, &args_of(&["branch", "--force", branch, upstream]))?
    } else {
        log.run(workdir, &args_of(&["branch", new]))?
            && log.run(workdir, &args_of(&["reset", "--keep", upstream]))?
    };
    if done {
        log.note = Some(format!(
            "The unpushed commits are on {new}; {branch} is back at {upstream}"
        ));
    }
    Ok(())
}

/// git's comment character: lines starting with it are not part of a message.
const COMMENT: char = '#';

/// The message a commit buffer starts with.
///
/// Mirrors what git itself writes into `COMMIT_EDITMSG`: the existing message
/// when amending, then a comment block explaining the rules and showing what
/// is staged. The comments are stripped again by [`strip_comments`].
pub fn commit_template(working_directory: &Path, amend: bool) -> String {
    commit_template_with(working_directory, amend, false)
}

/// git's scissors line: everything below it is not part of the message.
pub const SCISSORS: &str = "# ------------------------ >8 ------------------------";

/// [`commit_template`], with `verbose` adding the diff being committed below
/// a scissors line, as `git commit --verbose` does in its own editor. The
/// message is supplied with `-F`, so no editor of git's runs and the switch
/// itself would do nothing; the fork writes the diff in instead.
pub fn commit_template_with(working_directory: &Path, amend: bool, verbose: bool) -> String {
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

    if verbose {
        // Amending commits everything since HEAD's parent, not just what is
        // staged on top of HEAD.
        let parent_exists = amend
            && GitCommand::new(
                working_directory,
                vec![
                    "rev-parse".into(),
                    "--verify".into(),
                    "--quiet".into(),
                    "HEAD^".into(),
                ],
            )
            .run()
            .is_ok_and(|output| output.success);
        let mut args: Vec<String> = vec!["diff".into(), "--cached".into(), "--no-color".into()];
        if parent_exists {
            args.push("HEAD^".into());
        }
        if let Ok(diff) = GitCommand::new(working_directory, args).run() {
            if diff.success {
                template.push_str(SCISSORS);
                template.push_str(
                    "\n# Do not modify or remove the line above.\n\
                     # Everything below it will be ignored.\n",
                );
                template.push_str(&diff.stdout);
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
        // Below the scissors is the diff `--verbose` showed.
        .take_while(|line| *line != SCISSORS)
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

    fn asks(plan: &Plan) -> Vec<AskKind> {
        match &plan.requirement {
            Requirement::Ask(asks) => asks.iter().map(|ask| ask.kind).collect(),
            _ => Vec::new(),
        }
    }

    #[test]
    fn the_actions_needing_a_name_or_a_remote_say_so() {
        assert_eq!(
            asks(&plan(MagitCommand::BranchCheckout, &[])),
            [AskKind::Revision]
        );
        assert_eq!(
            asks(&plan(MagitCommand::BranchDelete, &[])),
            [AskKind::Branch]
        );
        assert_eq!(
            asks(&plan(MagitCommand::BranchCreate, &[])),
            [AskKind::Text, AskKind::Revision]
        );
        assert_eq!(
            asks(&plan(MagitCommand::PushElsewhere, &[])),
            [AskKind::Remote]
        );
    }

    #[test]
    fn typed_command_lines_are_split_as_a_shell_would() {
        assert_eq!(
            split_args(r#"commit -m "two words" --author='A B <a@b>' x\ y"#).unwrap(),
            ["commit", "-m", "two words", "--author=A B <a@b>", "x y"]
        );
        assert_eq!(
            split_args(r#"say "a \"quote\"""#).unwrap(),
            ["say", r#"a "quote""#]
        );
        assert_eq!(split_args("  ").unwrap(), Vec::<String>::new());
        assert_eq!(split_args(r#"-m ''"#).unwrap(), ["-m", ""]);
        assert!(split_args("'open").is_err());

        let tag = typed_git(r#"tag -a v9 -m "nine nine""#).unwrap();
        assert_eq!(tag.command_line(), "git tag -a v9 -m 'nine nine'");
        let plan = typed_git("git log --oneline -3").unwrap();
        assert_eq!(plan.args, ["log", "--oneline", "-3"]);
        assert!(typed_git("git").is_err());
        let shell = typed_shell("ls | wc -l").unwrap();
        assert_eq!(shell.command_line(), "$ ls | wc -l");
    }

    #[test]
    fn an_answer_that_would_be_an_option_is_refused() {
        let tag = Ask::required(AskKind::Text, "Tag name");
        assert!(tag.refuse("-v1").is_some());
        assert!(tag.refuse("").is_some());
        assert_eq!(tag.refuse("v1"), None);
        assert_eq!(
            Ask::required(AskKind::Message, "Note").refuse("-- see below"),
            None
        );
        assert_eq!(Ask::optional(AskKind::Stash, "Stash").refuse(""), None);
    }

    #[test]
    fn a_refspec_answer_is_several_arguments_and_onto_takes_two_revisions() {
        let push = plan(MagitCommand::PushRefspecs, &["--tags"])
            .answered(&["origin".into(), "HEAD:refs/for/main  v1".into()]);
        assert_eq!(
            push.args,
            ["push", "--tags", "origin", "HEAD:refs/for/main", "v1"]
        );

        let onto = plan(MagitCommand::RebaseOnto, &["--interactive", "--autostash"])
            .answered(&["main".into(), "old-base".into()]);
        assert_eq!(
            onto.args,
            ["rebase", "--autostash", "--onto", "main", "old-base"]
        );
        assert!(onto.destructive);
    }

    #[test]
    fn answers_fill_placeholders_or_are_appended() {
        // No placeholder: appended, empty optional answers dropped.
        let create =
            plan(MagitCommand::BranchCreate, &[]).answered(&["topic".into(), String::new()]);
        assert_eq!(create.args, ["branch", "topic"]);
        assert_eq!(create.requirement, Requirement::None);

        // Placeholders, and a `--flag={N}` left empty goes with its answer.
        let stash =
            plan(MagitCommand::StashBoth, &["--include-untracked"]).answered(&[String::new()]);
        assert_eq!(stash.args, ["stash", "push", "--include-untracked"]);
        let stash = plan(MagitCommand::StashBoth, &[]).answered(&["wip".into()]);
        assert_eq!(stash.args, ["stash", "push", "--message=wip"]);

        let push =
            plan(MagitCommand::PushElsewhere, &["--set-upstream"]).answered(&["fork".into()]);
        assert_eq!(push.args, ["push", "--set-upstream", "fork", "HEAD"]);
        assert_eq!(push.command_line(), "git push --set-upstream fork HEAD");
    }

    #[test]
    fn what_the_menu_was_opened_on_answers_the_question_it_fits() {
        let mut reset = plan(MagitCommand::ResetHard, &[]);
        // A branch is a revision.
        assert!(reset.preset("main", AskKind::Branch));
        let Requirement::Ask(asks) = &reset.requirement else {
            panic!("{:?}", reset.requirement)
        };
        assert_eq!(asks[0].preset.as_deref(), Some("main"));

        // A commit is not a remote.
        let mut push = plan(MagitCommand::PushElsewhere, &[]);
        assert!(!push.preset("abc123", AskKind::Revision));

        // The second question of a kind waits for the first.
        let mut bisect = plan(MagitCommand::BisectStart, &[]);
        assert!(bisect.preset("abc123", AskKind::Revision));
        assert!(bisect.preset("def456", AskKind::Revision));
        assert!(!bisect.preset("0000000", AskKind::Revision));
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
    fn hints_are_not_the_summary() {
        let output = GitOutput {
            success: true,
            stdout: "Initialized empty Git repository in /x/.git/\n".into(),
            stderr: "hint: Using 'master' as the name\nhint: more\n".into(),
        };
        assert_eq!(
            output.summary(),
            "Initialized empty Git repository in /x/.git/"
        );
    }

    #[test]
    fn a_failure_is_summed_up_by_its_reason() {
        let output = GitOutput {
            success: false,
            stdout: "Auto-merging f\nCONFLICT (content): Merge conflict in f\n".into(),
            stderr: String::new(),
        };
        assert_eq!(output.summary(), "CONFLICT (content): Merge conflict in f");
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
        // The diff below the scissors is never part of the message, even
        // its lines that do not start with '#'.
        let verbose = format!("msg\n{SCISSORS}\n# Everything below…\ndiff --git a/x b/x\n+added\n");
        assert_eq!(strip_comments(&verbose), "msg");
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
