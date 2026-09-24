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

    Pull,
    Fetch,
    FetchAll,

    BranchCheckout,
    BranchCreate,
    BranchCreateAndCheckout,
    BranchDelete,

    RebaseOntoUpstream,
    RebaseInteractive,
    RebaseAbort,
    RebaseContinue,
    RebaseSkip,

    /// The log of HEAD, of every reference, or of a revision asked for.
    /// The editor opens a log buffer for these; nothing is run.
    LogCurrent,
    LogAll,
    LogOther,

    ResetMixed,
    ResetSoft,
    ResetHard,
    ResetKeep,

    /// Open or refresh the status buffer.
    Status,
    /// Refresh the status buffer.
    Refresh,
    /// Close the transient without running anything.
    Quit,
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
}

impl MenuKind {
    pub const ALL: [MenuKind; 8] = [
        MenuKind::Main,
        MenuKind::Commit,
        MenuKind::Push,
        MenuKind::Pull,
        MenuKind::Branch,
        MenuKind::Rebase,
        MenuKind::Log,
        MenuKind::Reset,
    ];

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

/// The menu `:magit` opens.
pub fn main_menu() -> TransientMenu {
    TransientMenu::new(MenuKind::Main, "Magit").with_groups([
        TransientGroup::new("Transient").with_actions([
            TransientAction::new('c', "Commit", MagitCommand::OpenMenu(MenuKind::Commit)),
            TransientAction::new('b', "Branch", MagitCommand::OpenMenu(MenuKind::Branch)),
            TransientAction::new('r', "Rebase", MagitCommand::OpenMenu(MenuKind::Rebase)),
            TransientAction::new('l', "Log", MagitCommand::OpenMenu(MenuKind::Log)),
            TransientAction::new('X', "Reset", MagitCommand::OpenMenu(MenuKind::Reset)),
        ]),
        TransientGroup::new("Remote").with_actions([
            TransientAction::new('P', "Push", MagitCommand::OpenMenu(MenuKind::Push)),
            TransientAction::new('F', "Pull", MagitCommand::OpenMenu(MenuKind::Pull)),
            TransientAction::new('f', "Fetch", MagitCommand::Fetch),
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
        ]),
        TransientGroup::new("Push to").with_actions([
            TransientAction::new('p', "Upstream", MagitCommand::PushToUpstream),
            TransientAction::new('e', "Elsewhere", MagitCommand::PushElsewhere),
            TransientAction::new('P', "Push", MagitCommand::Push),
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
            TransientAction::new('x', "Delete", MagitCommand::BranchDelete),
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
        ]),
        TransientGroup::new("In progress").with_actions([
            TransientAction::new('c', "Continue", MagitCommand::RebaseContinue),
            TransientAction::new('s', "Skip", MagitCommand::RebaseSkip),
            TransientAction::new('z', "Abort", MagitCommand::RebaseAbort),
        ]),
    ])
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
