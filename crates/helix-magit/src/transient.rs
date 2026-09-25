//! The transient-menu model.
//!
//! A transient is Magit's popup: a titled menu of groups, where each group
//! holds switches and options that accumulate into a command line, plus the
//! actions that run it. The model here is inert data — which key does what,
//! and which flags are currently on — so it can be built and tested without
//! an editor. `helix-term` renders it and feeds it keys.

/// What an action asks the editor to do.
///
/// Actions carry an identifier rather than a closure so that this crate stays
/// free of any dependency on the editor, and so menus stay comparable and
/// testable. `helix-term` maps each variant to behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MagitCommand {
    /// Open another transient, e.g. the main menu opening the commit menu.
    OpenMenu(MenuKind),

    Commit,
    CommitAmend,
    CommitExtend,
    CommitFixup,

    Push,
    PushToUpstream,
    PushElsewhere,
    PushRefspecs,

    Pull,
    Fetch,
    FetchAll,

    BranchCheckout,
    BranchCreate,
    BranchCreateAndCheckout,
    BranchDelete,
    BranchRename,
    BranchReset,
    BranchSpinoff,
    BranchSpinout,

    RebaseOntoUpstream,
    RebaseInteractive,
    RebaseOnto,
    RebaseAbort,
    RebaseContinue,
    RebaseSkip,

    /// The log of HEAD, of every reference, or of a revision asked for.
    /// The editor opens a log buffer for these; nothing is run.
    LogCurrent,
    LogAll,
    LogOther,
    /// The reflog of HEAD, or of a ref asked for.
    Reflog,
    ReflogOther,
    /// The log of the work-in-progress refs: the working tree's saves, or
    /// the index's.
    WipLog,
    WipIndexLog,

    ResetMixed,
    ResetSoft,
    ResetHard,
    ResetKeep,
    /// The working tree's files as a commit has them, HEAD and the index
    /// left alone: how a wip save is restored.
    ResetWorktree,

    StashBoth,
    StashIndex,
    StashWorktree,
    StashPop,
    StashApply,
    StashDrop,
    StashBranch,

    Merge,
    MergeSquash,
    MergeNoCommit,
    MergeContinue,
    MergeAbort,

    TagCreate,
    TagDelete,
    TagPush,

    CherryPick,
    CherryApply,
    CherryPickContinue,
    CherryPickSkip,
    CherryPickAbort,
    Revert,
    RevertNoCommit,
    RevertContinue,
    RevertSkip,
    RevertAbort,

    RemoteAdd,
    RemoteRename,
    RemoteRemove,
    RemoteSetUpstream,
    RemoteFetch,

    BisectStart,
    BisectGood,
    BisectBad,
    BisectSkip,
    BisectReset,

    WorktreeAdd,
    WorktreeRemove,
    WorktreePrune,
    SubmoduleAdd,
    SubmoduleUpdate,
    SubmoduleSync,
    SubmoduleFetch,

    PatchApplyMailbox,
    PatchApplyPlain,
    AmContinue,
    AmSkip,
    AmAbort,
    FormatPatch,

    SubtreeAdd,
    SubtreePull,
    SubtreePush,
    SubtreeSplit,

    NoteEdit,
    NoteAppend,
    NoteRemove,
    NotePrune,

    IgnoreShared,
    IgnorePrivate,

    BranchConfigDescription,
    BranchConfigUpstream,
    BranchConfigRebase,
    BranchConfigPushRemote,
    RemoteConfigUrl,
    RemoteConfigPushUrl,
    RemoteConfigFetch,

    /// Who contributed what over a range; shown, not just run.
    Shortlog,

    /// Resolving the conflicted file the menu was opened on.
    ConflictEdit,
    ConflictShowOurs,
    ConflictShowTheirs,
    ConflictShowBase,
    ConflictTakeOurs,
    ConflictTakeTheirs,
    ConflictWithBase,
    ConflictMarkResolved,
    /// Continue whichever operation stopped: merge, rebase, cherry-pick,
    /// revert or am.
    Continue,

    /// The file dispatch, for the file being edited: staging it, and its
    /// diff, log and blame (the last three are views the editor opens).
    FileStage,
    FileUnstage,
    FileDiff,
    FileLog,
    FileBlame,

    /// Start a repository: clone one, or make one in a directory.
    Clone,
    Init,
    /// A git command or a shell command typed in, run in the repository.
    RunGit,
    RunShell,
    /// Move the status buffer's cursor to a section.
    JumpTo(JumpTarget),
    /// Bring one of the open Git views to the front, or open it.
    SwitchTo(GitView),

    /// Apply the diff menu's arguments to the open diffs.
    ApplyDiffSettings,
    /// Diffs the editor opens: between two revisions, a revision against
    /// the working tree, or one commit.
    DiffRange,
    DiffWorktree,
    DiffCommit,

    /// Views the editor opens rather than commands it runs: every branch
    /// and tag against HEAD, the commits one branch has that another does
    /// not, and what the commands run so far printed.
    ShowRefs,
    ShowCherries,
    ShowProcess,

    /// Open or refresh the status buffer.
    Status,
    /// Refresh the status buffer.
    Refresh,
    /// Close the transient without running anything.
    Quit,
}

/// A section of the status buffer to jump to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JumpTarget {
    Unmerged,
    Untracked,
    Unstaged,
    Staged,
    Stashes,
    Unpulled,
    Unpushed,
    Recent,
    Worktrees,
    Submodules,
}

impl JumpTarget {
    pub const ALL: [(char, JumpTarget, &'static str); 10] = [
        ('m', JumpTarget::Unmerged, "Unmerged"),
        ('n', JumpTarget::Untracked, "Untracked"),
        ('u', JumpTarget::Unstaged, "Unstaged"),
        ('s', JumpTarget::Staged, "Staged"),
        ('z', JumpTarget::Stashes, "Stashes"),
        ('f', JumpTarget::Unpulled, "Unpulled"),
        ('p', JumpTarget::Unpushed, "Unpushed"),
        ('r', JumpTarget::Recent, "Recent commits"),
        ('w', JumpTarget::Worktrees, "Worktrees"),
        ('o', JumpTarget::Submodules, "Submodules"),
    ];
}

/// The fork's Git views, for switching between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GitView {
    Status,
    Log,
    Commit,
    Diff,
    Refs,
    Cherries,
    Blame,
}

impl GitView {
    pub const ALL: [(char, GitView, &'static str); 7] = [
        ('s', GitView::Status, "Status"),
        ('l', GitView::Log, "Log"),
        ('c', GitView::Commit, "Commit"),
        ('d', GitView::Diff, "Diff"),
        ('y', GitView::Refs, "Refs"),
        ('Y', GitView::Cherries, "Cherries"),
        ('b', GitView::Blame, "Blame"),
    ];
}

/// The transients this crate ships.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MenuKind {
    Main,
    Commit,
    Push,
    Pull,
    Branch,
    Rebase,
    Log,
    Reset,
    Stash,
    Merge,
    Tag,
    CherryPick,
    Revert,
    Remote,
    Bisect,
    Worktree,
    Submodule,
    Apply,
    FormatPatch,
    Subtree,
    Notes,
    Ignore,
    /// A branch's and a remote's own configuration, opened from the branch
    /// and remote menus.
    BranchConfig,
    RemoteConfig,
    /// A conflicted file, opened from the status buffer with `e`.
    Resolve,
    /// The file being edited, opened from the editor with `<space>M`.
    File,
    /// Which diff to show (`d`), and how to show diffs (`D`).
    Diff,
    DiffSettings,
    /// The status buffer's sections, to jump to (`'`).
    Jump,
    /// The open Git views, to switch between (`J`).
    Views,
    /// What `:magit` offers outside any repository: clone and init.
    Setup,
}

impl MenuKind {
    pub const ALL: [MenuKind; 31] = [
        MenuKind::Main,
        MenuKind::Commit,
        MenuKind::Push,
        MenuKind::Pull,
        MenuKind::Branch,
        MenuKind::Rebase,
        MenuKind::Log,
        MenuKind::Reset,
        MenuKind::Stash,
        MenuKind::Merge,
        MenuKind::Tag,
        MenuKind::CherryPick,
        MenuKind::Revert,
        MenuKind::Remote,
        MenuKind::Bisect,
        MenuKind::Worktree,
        MenuKind::Submodule,
        MenuKind::Apply,
        MenuKind::FormatPatch,
        MenuKind::Subtree,
        MenuKind::Notes,
        MenuKind::Ignore,
        MenuKind::BranchConfig,
        MenuKind::RemoteConfig,
        MenuKind::Resolve,
        MenuKind::File,
        MenuKind::Diff,
        MenuKind::DiffSettings,
        MenuKind::Jump,
        MenuKind::Views,
        MenuKind::Setup,
    ];

    /// The key that opens this menu, in the main menu and the status
    /// buffer alike (Magit's dispatch keys); `None` for a menu only another
    /// menu opens.
    pub fn key(self) -> Option<char> {
        Some(match self {
            MenuKind::Main => '?',
            MenuKind::Commit => 'c',
            MenuKind::Push => 'P',
            MenuKind::Pull => 'F',
            MenuKind::Branch => 'b',
            MenuKind::Rebase => 'r',
            MenuKind::Log => 'l',
            MenuKind::Reset => 'X',
            MenuKind::Stash => 'z',
            MenuKind::Merge => 'm',
            MenuKind::Tag => 't',
            MenuKind::CherryPick => 'A',
            MenuKind::Revert => 'V',
            MenuKind::Remote => 'M',
            MenuKind::Bisect => 'B',
            MenuKind::Worktree => '%',
            MenuKind::Submodule => 'o',
            MenuKind::Apply => 'w',
            MenuKind::FormatPatch => 'W',
            MenuKind::Subtree => 'O',
            MenuKind::Notes => 'T',
            MenuKind::Ignore => 'i',
            MenuKind::Diff => 'd',
            MenuKind::DiffSettings => 'D',
            MenuKind::Jump => '\'',
            MenuKind::Views => 'J',
            MenuKind::Setup => return None,
            MenuKind::BranchConfig
            | MenuKind::RemoteConfig
            | MenuKind::Resolve
            | MenuKind::File => return None,
        })
    }

    /// The menu `key` opens, if any.
    pub fn for_key(key: char) -> Option<MenuKind> {
        MenuKind::ALL
            .into_iter()
            .find(|kind| kind.key() == Some(key))
    }

    /// Builds the menu for this kind.
    pub fn menu(self) -> TransientMenu {
        match self {
            MenuKind::Main => main_menu(),
            MenuKind::Commit => commit_menu(),
            MenuKind::Push => push_menu(),
            MenuKind::Pull => pull_menu(),
            MenuKind::Branch => branch_menu(),
            MenuKind::Rebase => rebase_menu(),
            MenuKind::Log => log_menu(),
            MenuKind::Reset => reset_menu(),
            MenuKind::Stash => stash_menu(),
            MenuKind::Merge => merge_menu(),
            MenuKind::Tag => tag_menu(),
            MenuKind::CherryPick => cherry_pick_menu(),
            MenuKind::Revert => revert_menu(),
            MenuKind::Remote => remote_menu(),
            MenuKind::Bisect => bisect_menu(),
            MenuKind::Worktree => worktree_menu(),
            MenuKind::Submodule => submodule_menu(),
            MenuKind::Apply => apply_menu(),
            MenuKind::FormatPatch => format_patch_menu(),
            MenuKind::Subtree => subtree_menu(),
            MenuKind::Notes => notes_menu(),
            MenuKind::Ignore => ignore_menu(),
            MenuKind::BranchConfig => branch_config_menu(),
            MenuKind::RemoteConfig => remote_config_menu(),
            MenuKind::Resolve => resolve_menu(),
            MenuKind::File => file_menu(),
            MenuKind::Diff => diff_menu(),
            MenuKind::DiffSettings => diff_settings_menu(&crate::diff::DiffOptions::default()),
            MenuKind::Jump => jump_menu(),
            MenuKind::Views => views_menu(&GitView::ALL.map(|(_, view, _)| view)),
            MenuKind::Setup => setup_menu(),
        }
    }
}

/// A boolean flag, such as `--autostash`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransientSwitch {
    /// The key that toggles it.
    pub key: char,
    /// The flag passed to git when it is on.
    pub flag: String,
    /// What the flag does, shown beside it.
    pub description: String,
    pub enabled: bool,
}

impl TransientSwitch {
    pub fn new(key: char, flag: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            key,
            flag: flag.into(),
            description: description.into(),
            enabled: false,
        }
    }

    /// Starts the switch enabled.
    pub fn on(mut self) -> Self {
        self.enabled = true;
        self
    }
}

/// A flag that carries a value, such as `--strategy=ours`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransientOption {
    pub key: char,
    /// The flag's name including its trailing `=`, e.g. `--strategy=`.
    pub flag: String,
    pub description: String,
    /// The current value; `None` means the option is off.
    pub value: Option<String>,
}

impl TransientOption {
    pub fn new(key: char, flag: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            key,
            flag: flag.into(),
            description: description.into(),
            value: None,
        }
    }
}

/// A switch or an option: the two things that contribute arguments.
///
/// The roadmap calls this a `TransientArgument`; it is the sum of the two
/// concrete kinds Magit distinguishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransientArgument {
    Switch(TransientSwitch),
    Option(TransientOption),
}

impl TransientArgument {
    pub fn key(&self) -> char {
        match self {
            TransientArgument::Switch(switch) => switch.key,
            TransientArgument::Option(option) => option.key,
        }
    }

    pub fn description(&self) -> &str {
        match self {
            TransientArgument::Switch(switch) => &switch.description,
            TransientArgument::Option(option) => &option.description,
        }
    }

    /// Whether this argument currently contributes to the command line.
    pub fn is_active(&self) -> bool {
        match self {
            TransientArgument::Switch(switch) => switch.enabled,
            TransientArgument::Option(option) => option.value.is_some(),
        }
    }

    /// The argument as it would be passed to git, when active.
    pub fn to_arg(&self) -> Option<String> {
        match self {
            TransientArgument::Switch(switch) => switch.enabled.then(|| switch.flag.clone()),
            TransientArgument::Option(option) => option
                .value
                .as_ref()
                .map(|value| format!("{}{}", option.flag, value)),
        }
    }

    /// The flag as displayed, without any value.
    pub fn flag(&self) -> &str {
        match self {
            TransientArgument::Switch(switch) => &switch.flag,
            TransientArgument::Option(option) => &option.flag,
        }
    }

    /// Toggles a switch. Options need a value, so toggling only clears them.
    fn toggle(&mut self) {
        match self {
            TransientArgument::Switch(switch) => switch.enabled = !switch.enabled,
            TransientArgument::Option(option) => option.value = None,
        }
    }
}

impl From<TransientSwitch> for TransientArgument {
    fn from(switch: TransientSwitch) -> Self {
        TransientArgument::Switch(switch)
    }
}

impl From<TransientOption> for TransientArgument {
    fn from(option: TransientOption) -> Self {
        TransientArgument::Option(option)
    }
}

/// A key that runs something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransientAction {
    pub key: char,
    pub description: String,
    pub command: MagitCommand,
}

impl TransientAction {
    pub fn new(key: char, description: impl Into<String>, command: MagitCommand) -> Self {
        Self {
            key,
            description: description.into(),
            command,
        }
    }
}

/// A labelled column of the menu.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TransientGroup {
    pub label: String,
    pub arguments: Vec<TransientArgument>,
    pub actions: Vec<TransientAction>,
}

impl TransientGroup {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            arguments: Vec::new(),
            actions: Vec::new(),
        }
    }

    pub fn with_arguments(
        mut self,
        arguments: impl IntoIterator<Item = TransientArgument>,
    ) -> Self {
        self.arguments = arguments.into_iter().collect();
        self
    }

    pub fn with_actions(mut self, actions: impl IntoIterator<Item = TransientAction>) -> Self {
        self.actions = actions.into_iter().collect();
        self
    }

    /// Rows this group occupies: its label, then one row per entry.
    pub fn height(&self) -> usize {
        1 + self.arguments.len() + self.actions.len()
    }
}

/// What happened when a menu was given a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransientEvent {
    /// A switch or option was toggled; the menu stays open.
    Toggled,
    /// `-` was pressed: the next key names an argument.
    ArgumentPrefix,
    /// An action fired.
    Run(MagitCommand),
    /// No entry uses this key.
    Unhandled,
}

/// A transient menu: a title and the groups it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransientMenu {
    pub kind: MenuKind,
    pub title: String,
    pub groups: Vec<TransientGroup>,
    /// Set by `-`: the next key toggles an argument rather than running an
    /// action.
    pub argument_prefix: bool,
}

impl TransientMenu {
    pub fn new(kind: MenuKind, title: impl Into<String>) -> Self {
        Self {
            kind,
            title: title.into(),
            groups: Vec::new(),
            argument_prefix: false,
        }
    }

    pub fn with_groups(mut self, groups: impl IntoIterator<Item = TransientGroup>) -> Self {
        self.groups = groups.into_iter().collect();
        self
    }

    /// Feeds a key to the menu, toggling an argument or naming an action.
    ///
    /// Arguments are matched before actions, mirroring Magit: a menu may bind
    /// the same letter to a switch and, in another group, to an action.
    ///
    /// As in Magit, arguments are reached through `-`: `-a` toggles
    /// `--all` in the commit menu while `a` amends. Without the prefix the
    /// two share keys, and one of them could never be reached.
    pub fn handle_key(&mut self, key: char) -> TransientEvent {
        if std::mem::take(&mut self.argument_prefix) {
            for group in &mut self.groups {
                for argument in &mut group.arguments {
                    if argument.key() == key {
                        argument.toggle();
                        return TransientEvent::Toggled;
                    }
                }
            }
            return TransientEvent::Unhandled;
        }

        if key == '-' && self.groups.iter().any(|group| !group.arguments.is_empty()) {
            self.argument_prefix = true;
            return TransientEvent::ArgumentPrefix;
        }

        for group in &self.groups {
            for action in &group.actions {
                if action.key == key {
                    return TransientEvent::Run(action.command);
                }
            }
        }

        TransientEvent::Unhandled
    }

    /// The description of the option `key` names when it is off: turning it
    /// on needs a value, which the editor asks for.
    pub fn option_awaiting_value(&self, key: char) -> Option<&str> {
        self.groups
            .iter()
            .flat_map(|group| &group.arguments)
            .find_map(|argument| match argument {
                TransientArgument::Option(option)
                    if option.key == key && option.value.is_none() =>
                {
                    Some(option.description.as_str())
                }
                _ => None,
            })
    }

    /// Every active argument, in the order the menu lists them.
    ///
    /// This is what gets appended to the git command an action runs.
    pub fn args(&self) -> Vec<String> {
        self.groups
            .iter()
            .flat_map(|group| &group.arguments)
            .filter_map(TransientArgument::to_arg)
            .collect()
    }

    /// Sets an option's value, or clears it with `None`.
    ///
    /// Returns whether a matching option exists.
    pub fn set_option(&mut self, key: char, value: Option<String>) -> bool {
        for group in &mut self.groups {
            for argument in &mut group.arguments {
                if let TransientArgument::Option(option) = argument {
                    if option.key == key {
                        option.value = value;
                        return true;
                    }
                }
            }
        }
        false
    }
}

fn switch(key: char, flag: &str, description: &str) -> TransientArgument {
    TransientSwitch::new(key, flag, description).into()
}

fn option(key: char, flag: &str, description: &str) -> TransientArgument {
    TransientOption::new(key, flag, description).into()
}

/// The menu `:magit` opens: every other menu, by its dispatch key.
pub fn main_menu() -> TransientMenu {
    let open = |kind: MenuKind, label: &'static str| {
        TransientAction::new(
            kind.key().expect("a dispatch menu has a key"),
            label,
            MagitCommand::OpenMenu(kind),
        )
    };
    TransientMenu::new(MenuKind::Main, "Magit").with_groups([
        TransientGroup::new("Commits").with_actions([
            open(MenuKind::Commit, "Commit"),
            open(MenuKind::Merge, "Merge"),
            open(MenuKind::Rebase, "Rebase"),
            open(MenuKind::CherryPick, "Cherry-pick"),
            open(MenuKind::Revert, "Revert"),
            open(MenuKind::Reset, "Reset"),
            open(MenuKind::Stash, "Stash"),
            open(MenuKind::Tag, "Tag"),
            open(MenuKind::Notes, "Notes"),
        ]),
        TransientGroup::new("Branches and remotes").with_actions([
            open(MenuKind::Branch, "Branch"),
            open(MenuKind::Remote, "Remote"),
            open(MenuKind::Push, "Push"),
            open(MenuKind::Pull, "Pull"),
            TransientAction::new('f', "Fetch", MagitCommand::Fetch),
            open(MenuKind::Worktree, "Worktree"),
            open(MenuKind::Submodule, "Submodule"),
            open(MenuKind::Subtree, "Subtree"),
        ]),
        TransientGroup::new("Inspect").with_actions([
            open(MenuKind::Log, "Log"),
            open(MenuKind::Diff, "Diff"),
            open(MenuKind::DiffSettings, "Diff settings"),
            TransientAction::new('y', "Show refs", MagitCommand::ShowRefs),
            TransientAction::new('Y', "Cherries", MagitCommand::ShowCherries),
            open(MenuKind::Bisect, "Bisect"),
            open(MenuKind::Apply, "Apply patches"),
            open(MenuKind::FormatPatch, "Format patches"),
            open(MenuKind::Ignore, "Ignore"),
            TransientAction::new('$', "Process output", MagitCommand::ShowProcess),
        ]),
        TransientGroup::new("Repository").with_actions([
            TransientAction::new('C', "Clone", MagitCommand::Clone),
            TransientAction::new('I', "Init", MagitCommand::Init),
            TransientAction::new('Q', "Run a git command", MagitCommand::RunGit),
            TransientAction::new('!', "Run a shell command", MagitCommand::RunShell),
            open(MenuKind::Views, "Switch view"),
        ]),
        TransientGroup::new("Essential").with_actions([
            TransientAction::new('s', "Status", MagitCommand::Status),
            TransientAction::new('g', "Refresh", MagitCommand::Refresh),
            TransientAction::new('q', "Quit", MagitCommand::Quit),
        ]),
    ])
}

pub fn commit_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Commit, "Commit").with_groups([
        TransientGroup::new("Arguments").with_arguments([
            switch('a', "--all", "Stage all modified and deleted files"),
            switch('e', "--allow-empty", "Allow empty commit"),
            switch('v', "--verbose", "Show diff of changes to be committed"),
            switch('n', "--no-verify", "Disable hooks"),
            switch('s', "--signoff", "Add Signed-off-by line"),
            option('A', "--author=", "Override the author"),
            switch('R', "--reset-author", "Claim authorship and reset the date"),
            option('D', "--date=", "Override the date"),
            switch('G', "--gpg-sign", "Sign with the default key"),
            option('S', "--gpg-sign=", "Sign with key"),
        ]),
        TransientGroup::new("Create").with_actions([
            TransientAction::new('c', "Commit", MagitCommand::Commit),
            TransientAction::new('a', "Amend", MagitCommand::CommitAmend),
            TransientAction::new('e', "Extend", MagitCommand::CommitExtend),
            TransientAction::new('f', "Fixup", MagitCommand::CommitFixup),
        ]),
    ])
}

pub fn push_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Push, "Push").with_groups([
        TransientGroup::new("Arguments").with_arguments([
            switch('f', "--force-with-lease", "Force with lease"),
            switch('F', "--force", "Force"),
            switch('h', "--no-verify", "Disable hooks"),
            switch('u', "--set-upstream", "Set upstream"),
            switch('d', "--dry-run", "Dry run"),
            switch('t', "--tags", "Push tags too"),
        ]),
        TransientGroup::new("Push to").with_actions([
            TransientAction::new('p', "Upstream", MagitCommand::PushToUpstream),
            TransientAction::new('e', "Elsewhere", MagitCommand::PushElsewhere),
            TransientAction::new('P', "Push", MagitCommand::Push),
            TransientAction::new('r', "Explicit refspecs", MagitCommand::PushRefspecs),
            TransientAction::new('T', "All tags", MagitCommand::TagPush),
        ]),
    ])
}

pub fn pull_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Pull, "Pull").with_groups([
        TransientGroup::new("Arguments").with_arguments([
            switch('r', "--rebase", "Rebase local commits onto upstream"),
            switch('a', "--autostash", "Stash uncommitted changes first"),
            switch('f', "--ff-only", "Fast-forward only"),
        ]),
        TransientGroup::new("Pull from").with_actions([
            TransientAction::new('p', "Pull", MagitCommand::Pull),
            TransientAction::new('f', "Fetch", MagitCommand::Fetch),
            TransientAction::new('a', "Fetch all remotes", MagitCommand::FetchAll),
        ]),
    ])
}

pub fn branch_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Branch, "Branch").with_groups([
        TransientGroup::new("Arguments").with_arguments([switch(
            't',
            "--track",
            "Set up tracking",
        )]),
        TransientGroup::new("Checkout").with_actions([
            TransientAction::new('b', "Branch", MagitCommand::BranchCheckout),
            TransientAction::new('c', "New branch", MagitCommand::BranchCreateAndCheckout),
        ]),
        TransientGroup::new("Create").with_actions([
            TransientAction::new('n', "New branch", MagitCommand::BranchCreate),
            TransientAction::new('s', "Spin off", MagitCommand::BranchSpinoff),
            TransientAction::new('S', "Spin out", MagitCommand::BranchSpinout),
        ]),
        TransientGroup::new("Do").with_actions([
            TransientAction::new('m', "Rename", MagitCommand::BranchRename),
            TransientAction::new('X', "Reset", MagitCommand::BranchReset),
            TransientAction::new('x', "Delete", MagitCommand::BranchDelete),
            TransientAction::new(
                'C',
                "Configure…",
                MagitCommand::OpenMenu(MenuKind::BranchConfig),
            ),
        ]),
    ])
}

pub fn rebase_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Rebase, "Rebase").with_groups([
        TransientGroup::new("Arguments").with_arguments([
            switch('a', "--autostash", "Stash uncommitted changes first"),
            switch('i', "--interactive", "Interactive"),
            switch(
                'A',
                "--autosquash",
                "Move fixup! and squash! commits into place",
            ),
            switch('k', "--keep-empty", "Keep empty commits"),
            // `S`, so `s` stays free for skipping: an argument's key shadows
            // an action's.
            option('S', "--strategy=", "Merge strategy"),
        ]),
        TransientGroup::new("Rebase").with_actions([
            TransientAction::new('u', "Onto upstream", MagitCommand::RebaseOntoUpstream),
            TransientAction::new('r', "Interactively", MagitCommand::RebaseInteractive),
            TransientAction::new('o', "Onto a revision", MagitCommand::RebaseOnto),
        ]),
        TransientGroup::new("In progress").with_actions([
            TransientAction::new('c', "Continue", MagitCommand::RebaseContinue),
            TransientAction::new('s', "Skip", MagitCommand::RebaseSkip),
            TransientAction::new('z', "Abort", MagitCommand::RebaseAbort),
        ]),
    ])
}

pub fn stash_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Stash, "Stash").with_groups([
        TransientGroup::new("Arguments").with_arguments([
            switch('u', "--include-untracked", "Also untracked files"),
            switch('a', "--all", "Also untracked and ignored files"),
        ]),
        TransientGroup::new("Stash").with_actions([
            TransientAction::new('z', "Both", MagitCommand::StashBoth),
            TransientAction::new('i', "Index", MagitCommand::StashIndex),
            TransientAction::new('w', "Worktree", MagitCommand::StashWorktree),
        ]),
        TransientGroup::new("Use").with_actions([
            TransientAction::new('p', "Pop", MagitCommand::StashPop),
            TransientAction::new('a', "Apply", MagitCommand::StashApply),
            TransientAction::new('b', "Branch", MagitCommand::StashBranch),
            TransientAction::new('k', "Drop", MagitCommand::StashDrop),
        ]),
    ])
}

pub fn merge_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Merge, "Merge").with_groups([
        TransientGroup::new("Arguments").with_arguments([
            switch('f', "--ff-only", "Fast-forward only"),
            switch('n', "--no-ff", "No fast-forward"),
        ]),
        TransientGroup::new("Merge").with_actions([
            TransientAction::new('m', "Merge", MagitCommand::Merge),
            TransientAction::new('s', "Squash", MagitCommand::MergeSquash),
            TransientAction::new('n', "Without committing", MagitCommand::MergeNoCommit),
        ]),
        TransientGroup::new("In progress").with_actions([
            TransientAction::new('c', "Commit the merge", MagitCommand::MergeContinue),
            TransientAction::new('z', "Abort", MagitCommand::MergeAbort),
        ]),
    ])
}

pub fn tag_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Tag, "Tag").with_groups([
        TransientGroup::new("Arguments").with_arguments([
            option('m', "--message=", "Annotate with a message"),
            switch('f', "--force", "Replace an existing tag"),
        ]),
        TransientGroup::new("Tag").with_actions([
            TransientAction::new('t', "Create", MagitCommand::TagCreate),
            TransientAction::new('k', "Delete", MagitCommand::TagDelete),
            TransientAction::new('p', "Push tags", MagitCommand::TagPush),
        ]),
    ])
}

pub fn cherry_pick_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::CherryPick, "Cherry-pick").with_groups([
        TransientGroup::new("Arguments").with_arguments([
            switch('x', "-x", "Note the original commit"),
            switch('f', "--ff", "Fast-forward when possible"),
        ]),
        TransientGroup::new("Apply here").with_actions([
            TransientAction::new('A', "Pick", MagitCommand::CherryPick),
            TransientAction::new('a', "Apply", MagitCommand::CherryApply),
        ]),
        TransientGroup::new("In progress").with_actions([
            TransientAction::new('c', "Continue", MagitCommand::CherryPickContinue),
            TransientAction::new('s', "Skip", MagitCommand::CherryPickSkip),
            TransientAction::new('z', "Abort", MagitCommand::CherryPickAbort),
        ]),
    ])
}

pub fn revert_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Revert, "Revert").with_groups([
        TransientGroup::new("Revert").with_actions([
            TransientAction::new('V', "Revert", MagitCommand::Revert),
            TransientAction::new('v', "Without committing", MagitCommand::RevertNoCommit),
        ]),
        TransientGroup::new("In progress").with_actions([
            TransientAction::new('c', "Continue", MagitCommand::RevertContinue),
            TransientAction::new('s', "Skip", MagitCommand::RevertSkip),
            TransientAction::new('z', "Abort", MagitCommand::RevertAbort),
        ]),
    ])
}

pub fn remote_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Remote, "Remote").with_groups([
        TransientGroup::new("Remote").with_actions([
            TransientAction::new('a', "Add", MagitCommand::RemoteAdd),
            TransientAction::new('r', "Rename", MagitCommand::RemoteRename),
            TransientAction::new('k', "Remove", MagitCommand::RemoteRemove),
        ]),
        TransientGroup::new("Branch").with_actions([
            TransientAction::new('u', "Set upstream", MagitCommand::RemoteSetUpstream),
            TransientAction::new('f', "Fetch a remote", MagitCommand::RemoteFetch),
            TransientAction::new(
                'C',
                "Configure…",
                MagitCommand::OpenMenu(MenuKind::RemoteConfig),
            ),
        ]),
    ])
}

pub fn bisect_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Bisect, "Bisect").with_groups([
        TransientGroup::new("Bisect").with_actions([
            TransientAction::new('B', "Start", MagitCommand::BisectStart),
            TransientAction::new('r', "Reset", MagitCommand::BisectReset),
        ]),
        TransientGroup::new("This commit is").with_actions([
            TransientAction::new('g', "Good", MagitCommand::BisectGood),
            TransientAction::new('b', "Bad", MagitCommand::BisectBad),
            TransientAction::new('s', "Untestable (skip)", MagitCommand::BisectSkip),
        ]),
    ])
}

pub fn worktree_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Worktree, "Worktree").with_groups([TransientGroup::new(
        "Worktree",
    )
    .with_actions([
        TransientAction::new('a', "Add", MagitCommand::WorktreeAdd),
        TransientAction::new('k', "Remove", MagitCommand::WorktreeRemove),
        TransientAction::new('p', "Prune", MagitCommand::WorktreePrune),
    ])])
}

pub fn submodule_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Submodule, "Submodule").with_groups([TransientGroup::new(
        "Submodule",
    )
    .with_actions([
        TransientAction::new('a', "Add", MagitCommand::SubmoduleAdd),
        TransientAction::new(
            'u',
            "Update (init, recursive)",
            MagitCommand::SubmoduleUpdate,
        ),
        TransientAction::new('s', "Sync URLs", MagitCommand::SubmoduleSync),
        TransientAction::new('f', "Fetch in each", MagitCommand::SubmoduleFetch),
    ])])
}

pub fn apply_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Apply, "Apply patches").with_groups([
        TransientGroup::new("Arguments").with_arguments([
            switch('3', "--3way", "Fall back on a three-way merge"),
            switch('s', "--signoff", "Add Signed-off-by line"),
        ]),
        TransientGroup::new("Apply").with_actions([
            TransientAction::new(
                'w',
                "Mailbox or patches (am)",
                MagitCommand::PatchApplyMailbox,
            ),
            TransientAction::new('a', "Plain patch (apply)", MagitCommand::PatchApplyPlain),
        ]),
        TransientGroup::new("In progress").with_actions([
            TransientAction::new('c', "Continue", MagitCommand::AmContinue),
            TransientAction::new('s', "Skip", MagitCommand::AmSkip),
            TransientAction::new('z', "Abort", MagitCommand::AmAbort),
        ]),
    ])
}

pub fn format_patch_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::FormatPatch, "Format patches").with_groups([
        TransientGroup::new("Arguments").with_arguments([
            option('o', "--output-directory=", "Into directory"),
            switch('c', "--cover-letter", "With a cover letter"),
            switch('n', "--numbered", "Numbered [PATCH n/m]"),
        ]),
        TransientGroup::new("Format").with_actions([TransientAction::new(
            'c',
            "Create",
            MagitCommand::FormatPatch,
        )]),
    ])
}

pub fn subtree_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Subtree, "Subtree").with_groups([TransientGroup::new("Subtree")
        .with_actions([
            TransientAction::new('a', "Add", MagitCommand::SubtreeAdd),
            TransientAction::new('f', "Pull", MagitCommand::SubtreePull),
            TransientAction::new('P', "Push", MagitCommand::SubtreePush),
            TransientAction::new('s', "Split", MagitCommand::SubtreeSplit),
        ])])
}

pub fn notes_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Notes, "Notes").with_groups([TransientGroup::new("Notes")
        .with_actions([
            TransientAction::new('T', "Set", MagitCommand::NoteEdit),
            TransientAction::new('a', "Append", MagitCommand::NoteAppend),
            TransientAction::new('r', "Remove", MagitCommand::NoteRemove),
            TransientAction::new('p', "Prune", MagitCommand::NotePrune),
        ])])
}

pub fn ignore_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Ignore, "Ignore").with_groups([TransientGroup::new("Ignore")
        .with_actions([
            TransientAction::new('t', "For everyone (.gitignore)", MagitCommand::IgnoreShared),
            TransientAction::new(
                'p',
                "Privately (.git/info/exclude)",
                MagitCommand::IgnorePrivate,
            ),
        ])])
}

pub fn branch_config_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::BranchConfig, "Configure branch").with_groups([
        TransientGroup::new("branch.<name>.").with_actions([
            TransientAction::new('d', "description", MagitCommand::BranchConfigDescription),
            TransientAction::new(
                'u',
                "merge and remote (upstream)",
                MagitCommand::BranchConfigUpstream,
            ),
            TransientAction::new('r', "rebase", MagitCommand::BranchConfigRebase),
            TransientAction::new('p', "pushRemote", MagitCommand::BranchConfigPushRemote),
        ]),
    ])
}

pub fn remote_config_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::RemoteConfig, "Configure remote").with_groups([
        TransientGroup::new("remote.<name>.").with_actions([
            TransientAction::new('u', "url", MagitCommand::RemoteConfigUrl),
            TransientAction::new('U', "pushurl", MagitCommand::RemoteConfigPushUrl),
            TransientAction::new('f', "fetch (refspec)", MagitCommand::RemoteConfigFetch),
        ]),
    ])
}

pub fn resolve_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Resolve, "Resolve").with_groups([
        TransientGroup::new("Edit").with_actions([
            TransientAction::new('e', "Edit the file", MagitCommand::ConflictEdit),
            TransientAction::new(
                '3',
                "Rewrite with the base shown",
                MagitCommand::ConflictWithBase,
            ),
        ]),
        TransientGroup::new("Show").with_actions([
            TransientAction::new('O', "Ours", MagitCommand::ConflictShowOurs),
            TransientAction::new('B', "Base", MagitCommand::ConflictShowBase),
            TransientAction::new('T', "Theirs", MagitCommand::ConflictShowTheirs),
        ]),
        TransientGroup::new("Whole file").with_actions([
            TransientAction::new('o', "Take ours", MagitCommand::ConflictTakeOurs),
            TransientAction::new('t', "Take theirs", MagitCommand::ConflictTakeTheirs),
            TransientAction::new('s', "Mark resolved", MagitCommand::ConflictMarkResolved),
        ]),
        TransientGroup::new("Then").with_actions([TransientAction::new(
            'c',
            "Continue the operation",
            MagitCommand::Continue,
        )]),
    ])
}

/// Magit's file dispatch: what applies to the file being edited.
pub fn file_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::File, "File").with_groups([
        TransientGroup::new("Index").with_actions([
            TransientAction::new('s', "Stage", MagitCommand::FileStage),
            TransientAction::new('u', "Unstage", MagitCommand::FileUnstage),
            TransientAction::new('c', "Commit…", MagitCommand::OpenMenu(MenuKind::Commit)),
        ]),
        TransientGroup::new("Inspect").with_actions([
            TransientAction::new('d', "Diff", MagitCommand::FileDiff),
            TransientAction::new('l', "Log", MagitCommand::FileLog),
            TransientAction::new('b', "Blame", MagitCommand::FileBlame),
        ]),
        TransientGroup::new("Everything").with_actions([
            TransientAction::new('g', "Status", MagitCommand::Status),
            TransientAction::new('?', "Magit…", MagitCommand::OpenMenu(MenuKind::Main)),
        ]),
    ])
}

pub fn diff_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Diff, "Diff").with_groups([TransientGroup::new("Diff")
        .with_actions([
            TransientAction::new('r', "Between two revisions", MagitCommand::DiffRange),
            TransientAction::new(
                'w',
                "Working tree against a revision",
                MagitCommand::DiffWorktree,
            ),
            TransientAction::new('c', "A commit", MagitCommand::DiffCommit),
        ])])
}

/// The diff settings, showing `options` as they are now.
pub fn diff_settings_menu(options: &crate::diff::DiffOptions) -> TransientMenu {
    use crate::diff::Whitespace;
    let on = |switch: TransientSwitch, enabled: bool| -> TransientArgument {
        if enabled { switch.on() } else { switch }.into()
    };
    let valued = |key, flag: &str, description: &str, value: String| -> TransientArgument {
        let mut option = TransientOption::new(key, flag, description);
        option.value = Some(value);
        option.into()
    };
    TransientMenu::new(MenuKind::DiffSettings, "Diff settings").with_groups([
        TransientGroup::new("Arguments").with_arguments([
            valued(
                'U',
                "--unified=",
                "Context lines",
                options.context.to_string(),
            ),
            on(
                TransientSwitch::new('b', "--ignore-space-change", "Ignore changes in whitespace"),
                options.whitespace == Whitespace::IgnoreChange,
            ),
            on(
                TransientSwitch::new('w', "--ignore-all-space", "Ignore all whitespace"),
                options.whitespace == Whitespace::IgnoreAll,
            ),
            valued(
                'A',
                "--diff-algorithm=",
                "histogram, myers, minimal, patience",
                options.algorithm.name().to_string(),
            ),
            on(
                TransientSwitch::new('W', "--word-diff", "Mark the changed words"),
                options.word_diff,
            ),
            on(
                TransientSwitch::new('s', "--stat", "Summary: files and sizes only"),
                options.stat,
            ),
        ]),
        TransientGroup::new("Diff").with_actions([TransientAction::new(
            'g',
            "Apply to the open diffs",
            MagitCommand::ApplyDiffSettings,
        )]),
    ])
}

pub fn setup_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Setup, "No repository here").with_groups([
        TransientGroup::new("Start one").with_actions([
            TransientAction::new('C', "Clone", MagitCommand::Clone),
            TransientAction::new('I', "Init", MagitCommand::Init),
        ]),
        TransientGroup::new("Then").with_actions([TransientAction::new(
            'q',
            "Quit",
            MagitCommand::Quit,
        )]),
    ])
}

pub fn jump_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Jump, "Jump to").with_groups([TransientGroup::new("Section")
        .with_actions(JumpTarget::ALL.map(|(key, target, label)| {
            TransientAction::new(key, label, MagitCommand::JumpTo(target))
        }))])
}

/// The views to switch between: the ones in `open`, listed with their keys.
pub fn views_menu(open: &[GitView]) -> TransientMenu {
    let actions: Vec<TransientAction> = GitView::ALL
        .into_iter()
        .filter(|(_, view, _)| *view == GitView::Status || open.contains(view))
        .map(|(key, view, label)| TransientAction::new(key, label, MagitCommand::SwitchTo(view)))
        .collect();
    TransientMenu::new(MenuKind::Views, "Switch view")
        .with_groups([TransientGroup::new("Open").with_actions(actions)])
}

pub fn log_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Log, "Log").with_groups([
        TransientGroup::new("Limit").with_arguments([
            option('A', "--author=", "Author matches"),
            option('G', "--grep=", "Message matches"),
            option('F', crate::log::PATH_FLAG, "Touches file"),
        ]),
        TransientGroup::new("Log").with_actions([
            TransientAction::new('l', "Current", MagitCommand::LogCurrent),
            TransientAction::new('a', "All references", MagitCommand::LogAll),
            TransientAction::new('o', "Other", MagitCommand::LogOther),
            TransientAction::new('s', "Shortlog", MagitCommand::Shortlog),
        ]),
        TransientGroup::new("Reflog").with_actions([
            TransientAction::new('r', "HEAD's reflog", MagitCommand::Reflog),
            TransientAction::new('R', "Another ref's reflog", MagitCommand::ReflogOther),
            TransientAction::new('w', "Working tree saves (wip)", MagitCommand::WipLog),
            TransientAction::new('W', "Index saves (wip)", MagitCommand::WipIndexLog),
        ]),
    ])
}

pub fn reset_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Reset, "Reset").with_groups([TransientGroup::new(
        "Reset HEAD and",
    )
    .with_actions([
        TransientAction::new('m', "the index (mixed)", MagitCommand::ResetMixed),
        TransientAction::new('s', "nothing else (soft)", MagitCommand::ResetSoft),
        TransientAction::new('h', "index and worktree (hard)", MagitCommand::ResetHard),
        TransientAction::new('k', "keeping local changes (keep)", MagitCommand::ResetKeep),
        TransientAction::new(
            'w',
            "only the worktree's files",
            MagitCommand::ResetWorktree,
        ),
    ])])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// `-` then the key, as typed.
    fn toggle(menu: &mut TransientMenu, key: char) -> TransientEvent {
        assert_eq!(menu.handle_key('-'), TransientEvent::ArgumentPrefix);
        menu.handle_key(key)
    }

    #[test]
    fn an_argument_key_without_the_prefix_is_not_an_argument() {
        let mut menu = pull_menu();
        // `r` alone is not `--rebase`.
        let _ = menu.handle_key('r');
        assert!(menu.args().is_empty());
        // And the prefix lasts for one key only.
        assert_eq!(menu.handle_key('-'), TransientEvent::ArgumentPrefix);
        assert_eq!(menu.handle_key('Z'), TransientEvent::Unhandled);
        assert!(!menu.argument_prefix);
    }

    #[test]
    fn toggling_a_switch_adds_its_flag() {
        let mut menu = pull_menu();
        assert!(menu.args().is_empty());

        assert_eq!(toggle(&mut menu, 'r'), TransientEvent::Toggled);
        assert_eq!(menu.args(), ["--rebase"]);

        assert_eq!(toggle(&mut menu, 'a'), TransientEvent::Toggled);
        assert_eq!(menu.args(), ["--rebase", "--autostash"]);

        // Toggling again removes it.
        assert_eq!(toggle(&mut menu, 'r'), TransientEvent::Toggled);
        assert_eq!(menu.args(), ["--autostash"]);
    }

    #[test]
    fn an_action_key_reports_its_command() {
        let mut menu = commit_menu();
        // 'a' is both the --all switch and the Amend action: `-a` is the
        // switch, `a` the action, as in Magit.
        assert_eq!(toggle(&mut menu, 'a'), TransientEvent::Toggled);
        assert_eq!(menu.args(), ["--all"]);
        assert_eq!(
            menu.handle_key('a'),
            TransientEvent::Run(MagitCommand::CommitAmend)
        );
        assert_eq!(
            menu.handle_key('e'),
            TransientEvent::Run(MagitCommand::CommitExtend)
        );

        assert_eq!(
            menu.handle_key('c'),
            TransientEvent::Run(MagitCommand::Commit)
        );
        assert_eq!(
            menu.handle_key('f'),
            TransientEvent::Run(MagitCommand::CommitFixup)
        );
    }

    #[test]
    fn an_unbound_key_is_reported_as_unhandled() {
        let mut menu = push_menu();
        assert_eq!(menu.handle_key('Z'), TransientEvent::Unhandled);
        assert!(menu.args().is_empty());
    }

    #[test]
    fn options_contribute_only_once_given_a_value() {
        let mut menu = rebase_menu();
        assert!(menu.args().is_empty());

        assert!(menu.set_option('S', Some("ours".to_string())));
        assert_eq!(menu.args(), ["--strategy=ours"]);

        // Toggling an option clears it, since a value cannot be guessed.
        assert_eq!(toggle(&mut menu, 'S'), TransientEvent::Toggled);
        assert!(menu.args().is_empty());

        assert!(!menu.set_option('Z', Some("x".to_string())));
    }

    #[test]
    fn the_main_menu_opens_the_other_menus() {
        let mut menu = main_menu();
        assert_eq!(
            menu.handle_key('c'),
            TransientEvent::Run(MagitCommand::OpenMenu(MenuKind::Commit))
        );
        assert_eq!(
            menu.handle_key('P'),
            TransientEvent::Run(MagitCommand::OpenMenu(MenuKind::Push))
        );
        assert_eq!(
            menu.handle_key('q'),
            TransientEvent::Run(MagitCommand::Quit)
        );
    }

    #[test]
    fn every_menu_kind_builds_and_is_reachable() {
        for kind in MenuKind::ALL {
            let menu = kind.menu();
            assert_eq!(menu.kind, kind);
            assert!(!menu.title.is_empty());
            assert!(!menu.groups.is_empty(), "{kind:?} has no groups");
            assert!(
                menu.groups.iter().any(|group| !group.actions.is_empty()),
                "{kind:?} has no action to run"
            );
        }
    }

    /// A duplicate key inside one category would silently shadow an entry.
    #[test]
    fn keys_are_unique_within_arguments_and_within_actions() {
        for kind in MenuKind::ALL {
            let menu = kind.menu();

            let mut argument_keys = HashSet::new();
            for argument in menu.groups.iter().flat_map(|group| &group.arguments) {
                assert!(
                    argument_keys.insert(argument.key()),
                    "{kind:?} binds '{}' to two arguments",
                    argument.key()
                );
            }

            let mut action_keys = HashSet::new();
            for action in menu.groups.iter().flat_map(|group| &group.actions) {
                assert!(
                    action_keys.insert(action.key),
                    "{kind:?} binds '{}' to two actions",
                    action.key
                );
            }
        }
    }

    #[test]
    fn a_switch_can_start_enabled() {
        let menu = TransientMenu::new(MenuKind::Push, "Push").with_groups([TransientGroup::new(
            "Arguments",
        )
        .with_arguments([TransientSwitch::new('u', "--set-upstream", "Set upstream")
            .on()
            .into()])]);
        assert_eq!(menu.args(), ["--set-upstream"]);
    }

    #[test]
    fn group_height_counts_its_label_and_entries() {
        let group = TransientGroup::new("Arguments")
            .with_arguments([switch('a', "--all", "All")])
            .with_actions([TransientAction::new('c', "Commit", MagitCommand::Commit)]);
        assert_eq!(group.height(), 3);
    }
}
