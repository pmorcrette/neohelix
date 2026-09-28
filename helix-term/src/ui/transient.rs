//! The transient menu overlay, Magit's popup.
//!
//! Renders a [`TransientMenu`] as labelled columns across the bottom of the
//! screen and captures keys modally while it is open.

use std::path::PathBuf;

use helix_magit::transient::{MenuKind, TransientEvent, TransientGroup, TransientMenu};
use helix_magit::{AskKind, MagitCommand};

use crate::ui::diff_view::DiffView;
use helix_view::graphics::Rect;
use helix_view::input::{KeyCode, KeyEvent};
use helix_view::keyboard::KeyModifiers;
use tui::buffer::Buffer as Surface;
use tui::widgets::{Block, Widget};

use crate::compositor::{Callback, Component, Context, Event, EventResult};
use crate::ui::log_view::LogView;
use helix_magit::log::LogFilter;

// ── What the menus remember (Magit's transient values, history, levels) ──

use helix_magit::persist::{menu_name, TransientState};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// Saved values, option history and hidden actions, read from disk once.
static STATE: Mutex<Option<TransientState>> = Mutex::new(None);
/// Values set for this session only (`C-x s`), by menu.
static SESSION: Mutex<BTreeMap<String, Vec<String>>> = Mutex::new(BTreeMap::new());

/// Where the state is kept; none in tests, which must not read or write
/// the user's.
fn state_path() -> Option<PathBuf> {
    if cfg!(test) {
        None
    } else {
        Some(helix_loader::data_dir().join("magit-transient"))
    }
}

fn with_state<R>(f: impl FnOnce(&mut TransientState) -> R) -> R {
    let mut state = STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let state = state.get_or_insert_with(|| {
        state_path()
            .map(|path| TransientState::load(&path))
            .unwrap_or_default()
    });
    f(state)
}

/// Writes the state back; an error is for the status line.
fn persist() -> Result<(), String> {
    let Some(path) = state_path() else {
        return Ok(());
    };
    with_state(|state| state.save(&path)).map_err(|err| err.to_string())
}

/// A menu as it opens: with this session's values, else the saved ones,
/// and without its hidden actions. The diff settings show the view's own.
fn prepare(mut menu: TransientMenu) -> TransientMenu {
    let name = menu_name(menu.kind);
    if menu.kind != MenuKind::DiffSettings && menu.has_arguments() {
        let session = SESSION
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&name)
            .cloned();
        if let Some(args) = session.or_else(|| with_state(|state| state.values.get(&name).cloned()))
        {
            menu.set_args(&args);
        }
    }
    menu.hidden = with_state(|state| state.hidden_in(menu.kind));
    menu
}

/// The register an option's prompt recalls its history from.
const OPTION_HISTORY: char = '\u{F8F0}';

/// Columns narrower than this are not worth splitting into.
const MIN_COLUMN_WIDTH: u16 = 22;
/// Gap between two columns.
const COLUMN_GAP: u16 = 2;

/// A transient menu shown over the editor.
pub struct TransientOverlay {
    menu: TransientMenu,
    /// What the title shows after the menu name, typically the branch.
    context: String,
    /// The repository the menu acts on, so it can open the status buffer.
    workdir: PathBuf,
    /// What the menu was opened on — a commit, a branch, a stash, a file —
    /// which answers the first question it fits instead of asking it.
    target: Option<(String, AskKind)>,
    /// The cursor's line in the file the menu was opened from, where a
    /// blame starts.
    line: usize,
    /// `C-x` was pressed: the next key saves, sets or edits the levels.
    ctrl_x: bool,
    /// `C-x l`: keys hide or show actions instead of running them.
    editing_levels: bool,
}

impl TransientOverlay {
    pub const ID: &'static str = "magit-transient";

    pub fn new(menu: TransientMenu, context: impl Into<String>, workdir: PathBuf) -> Self {
        Self {
            menu: prepare(menu),
            context: context.into(),
            workdir,
            target: None,
            line: 0,
            ctrl_x: false,
            editing_levels: false,
        }
    }

    /// Where the file dispatch's blame puts its cursor.
    pub fn with_line(mut self, line: usize) -> Self {
        self.line = line;
        self
    }

    /// Makes the menu act on `commit` where an action needs one.
    pub fn with_commit(self, commit: String) -> Self {
        self.with_target(commit, AskKind::Revision)
    }

    /// Makes the menu act on `value` where an action asks for its kind.
    pub fn with_target(mut self, value: String, kind: AskKind) -> Self {
        self.target = Some((value, kind));
        self
    }

    /// The menu currently shown.
    pub fn menu(&self) -> &TransientMenu {
        &self.menu
    }

    /// Rows the widest group needs, plus the border.
    fn height(&self, width: u16) -> u16 {
        let columns = self.column_count(width);
        let rows_per_column = self
            .menu
            .groups
            .chunks(self.menu.groups.len().div_ceil(columns.max(1)))
            .map(|chunk| chunk.iter().map(TransientGroup::height).sum::<usize>())
            .max()
            .unwrap_or(0);

        // +2 for the border, +1 for the command-line preview.
        (rows_per_column as u16).saturating_add(3)
    }

    /// How many columns fit, never more than there are groups.
    fn column_count(&self, width: u16) -> usize {
        let fits = (width / (MIN_COLUMN_WIDTH + COLUMN_GAP)).max(1) as usize;
        fits.min(self.menu.groups.len().max(1))
    }

    /// The git command line the current arguments would produce.
    fn command_preview(&self) -> String {
        let args = self.menu.args();
        let base = match self.menu.kind {
            MenuKind::Main => return String::new(),
            MenuKind::Commit => "git commit",
            MenuKind::Push => "git push",
            MenuKind::Pull => "git pull",
            MenuKind::Branch => "git branch",
            MenuKind::Rebase => "git rebase",
            MenuKind::Log => "git log",
            MenuKind::Reset => "git reset",
            MenuKind::Stash => "git stash",
            MenuKind::Merge => "git merge",
            MenuKind::Tag => "git tag",
            MenuKind::CherryPick => "git cherry-pick",
            MenuKind::Revert => "git revert",
            MenuKind::Remote => "git remote",
            MenuKind::Bisect => "git bisect",
            MenuKind::Worktree => "git worktree",
            MenuKind::Submodule => "git submodule",
            MenuKind::Apply => "git am",
            MenuKind::FormatPatch => "git format-patch",
            MenuKind::Subtree => "git subtree",
            MenuKind::Notes => "git notes",
            MenuKind::Ignore => return String::new(),
            MenuKind::Sparse => "git sparse-checkout",
            MenuKind::Bundle => "git bundle",
            MenuKind::Clean => "git clean",
            MenuKind::BranchConfig | MenuKind::RemoteConfig => "git config",
            MenuKind::Resolve | MenuKind::File => return String::new(),
            MenuKind::Diff => "git diff",
            MenuKind::Ediff => return String::new(),
            MenuKind::Fetch => "git fetch",
            MenuKind::DiffSettings => "diff settings:",
            MenuKind::Jump | MenuKind::Views | MenuKind::Setup | MenuKind::Margin => {
                return String::new()
            }
        };

        if args.is_empty() {
            base.to_string()
        } else {
            format!("{base} {}", args.join(" "))
        }
    }
}

impl Component for TransientOverlay {
    fn render(&mut self, viewport: Rect, surface: &mut Surface, cx: &mut Context) {
        let height = self.height(viewport.width).min(viewport.height);
        // Dock to the bottom, above the statusline.
        let area = viewport.intersection(Rect::new(
            0,
            viewport.height.saturating_sub(height + 1),
            viewport.width,
            height,
        ));
        if area.height < 3 {
            return;
        }

        let popup_style = cx.editor.theme.get("ui.popup");
        let text_style = cx.editor.theme.get("ui.text");
        let key_style = cx.editor.theme.get("ui.text.focus");
        let label_style = cx.editor.theme.get("ui.text.directory");
        let active_style = cx.editor.theme.get("diagnostic.info");
        let muted_style = cx.editor.theme.get("ui.virtual");

        surface.clear_with(area, popup_style);

        let mut title = if self.context.is_empty() {
            self.menu.title.clone()
        } else {
            format!("{}: {}", self.menu.title, self.context)
        };
        if self.editing_levels {
            title.push_str(" — a key hides or shows its command; C-x l when done");
        }
        let block = Block::bordered().title(title).border_style(popup_style);
        let inner = block.inner(area);
        block.render(area, surface);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let columns = self.column_count(inner.width);
        let per_column = self.menu.groups.len().div_ceil(columns.max(1));
        let column_width = inner.width / columns as u16;

        for (index, chunk) in self.menu.groups.chunks(per_column.max(1)).enumerate() {
            let x = inner.x + index as u16 * column_width;
            let width = column_width.saturating_sub(COLUMN_GAP) as usize;
            if width == 0 {
                break;
            }
            let mut y = inner.y;

            for group in chunk {
                if y >= inner.y + inner.height {
                    break;
                }
                surface.set_string_truncated(
                    x,
                    y,
                    &group.label,
                    width,
                    |_| label_style,
                    true,
                    false,
                );
                y += 1;

                for argument in &group.arguments {
                    if y >= inner.y + inner.height {
                        break;
                    }
                    // `-a --autostash  Stash first`, with the flag highlighted
                    // while it is contributing to the command line.
                    let style = if argument.is_active() {
                        active_style
                    } else {
                        muted_style
                    };
                    // A fixed three-column key cell: nothing to elide.
                    surface.set_string_truncated(
                        x,
                        y,
                        &format!("-{} ", argument.key()),
                        3,
                        |_| key_style,
                        false,
                        false,
                    );
                    surface.set_string_truncated(
                        x + 3,
                        y,
                        &format!(
                            "{} {}",
                            argument.to_arg().as_deref().unwrap_or(argument.flag()),
                            argument.description()
                        ),
                        width.saturating_sub(3),
                        |_| style,
                        true,
                        false,
                    );
                    y += 1;
                }

                for action in &group.actions {
                    if y >= inner.y + inner.height {
                        break;
                    }
                    let hidden = self.menu.hidden.contains(&action.key);
                    if hidden && !self.editing_levels {
                        continue;
                    }
                    let (text_style, description) = if hidden {
                        (muted_style, format!("{} (hidden)", action.description))
                    } else {
                        (text_style, action.description.clone())
                    };
                    surface.set_string_truncated(
                        x,
                        y,
                        &format!(" {} ", action.key),
                        3,
                        |_| key_style,
                        false,
                        false,
                    );
                    surface.set_string_truncated(
                        x + 3,
                        y,
                        &description,
                        width.saturating_sub(3),
                        |_| text_style,
                        true,
                        false,
                    );
                    y += 1;
                }
            }
        }

        // Bottom row: what the current arguments would run.
        let preview = self.command_preview();
        if !preview.is_empty() && inner.height > 1 {
            surface.set_string_truncated(
                inner.x,
                inner.y + inner.height - 1,
                &preview,
                inner.width as usize,
                |_| muted_style,
                true,
                false,
            );
        }
    }

    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        let Event::Key(key) = event else {
            // The menu is modal: swallow everything, so a stray mouse event
            // does not reach the editor underneath.
            return EventResult::Consumed(None);
        };

        let close: Callback = Box::new(|compositor, _| {
            compositor.remove(TransientOverlay::ID);
        });

        if std::mem::take(&mut self.ctrl_x) {
            self.menu.argument_prefix = false;
            match (key.code, key.modifiers.contains(KeyModifiers::CONTROL)) {
                (KeyCode::Char('s'), true) => self.save_values(cx, true),
                (KeyCode::Char('s'), false) => self.save_values(cx, false),
                (KeyCode::Char('l'), _) => {
                    self.editing_levels = !self.editing_levels;
                    cx.editor.set_status(if self.editing_levels {
                        "Press a command's key to hide or show it; C-x l when done"
                    } else {
                        "Done hiding commands"
                    });
                }
                _ => cx.editor.set_status(
                    "C-x C-s saves the arguments, C-x s sets them for this session, \
                     C-x l hides or shows commands",
                ),
            }
            return EventResult::Consumed(None);
        }
        if key.code == KeyCode::Char('x') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.ctrl_x = true;
            return EventResult::Consumed(None);
        }
        if self.editing_levels {
            match key.code {
                KeyCode::Esc => self.editing_levels = false,
                KeyCode::Char(c) if self.menu.has_action(c) => {
                    let kind = self.menu.kind;
                    let hidden = with_state(|state| state.toggle_hidden(kind, c));
                    if hidden {
                        self.menu.hidden.insert(c);
                    } else {
                        self.menu.hidden.remove(&c);
                    }
                    if let Err(err) = persist() {
                        cx.editor.set_error(format!("could not save: {err}"));
                    }
                }
                _ => {}
            }
            return EventResult::Consumed(None);
        }

        match key {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => return EventResult::Consumed(Some(close)),
            // `-` then an option that is off: ask for its value.
            KeyEvent {
                code: KeyCode::Char(c),
                ..
            } if self.menu.argument_prefix && self.menu.option_awaiting_value(*c).is_some() => {
                self.menu.argument_prefix = false;
                let key = *c;
                let label = format!("{}: ", self.menu.option_awaiting_value(key).unwrap_or(""));
                let flag = self.menu.option_flag(key).unwrap_or_default().to_string();
                // Its history, newest first, for `M-p` / `M-n` and completion.
                let history =
                    with_state(|state| state.history.get(&flag).cloned().unwrap_or_default());
                if let Err(err) = cx.editor.registers.write(OPTION_HISTORY, history.clone()) {
                    cx.editor.set_error(err.to_string());
                }
                return EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.push(Box::new(option_prompt(label, key, flag, history)));
                })));
            }
            KeyEvent {
                code: KeyCode::Char(c),
                ..
            } => match self.menu.handle_key(*c) {
                TransientEvent::Toggled | TransientEvent::ArgumentPrefix => {
                    return EventResult::Consumed(None)
                }
                TransientEvent::Run(command) => return self.run(command, close),
                // An unbound key does nothing rather than leaking through to
                // the editor, which is what makes the menu modal.
                TransientEvent::Unhandled => return EventResult::Consumed(None),
            },
            _ => {}
        }

        EventResult::Consumed(None)
    }

    fn id(&self) -> Option<&'static str> {
        Some(Self::ID)
    }
}

impl TransientOverlay {
    /// `C-x C-s` saves the menu's arguments as its defaults, `C-x s` sets
    /// them for this session only.
    fn save_values(&mut self, cx: &mut Context, permanently: bool) {
        if !self.menu.has_arguments() || self.menu.kind == MenuKind::DiffSettings {
            return cx.editor.set_error("This menu has no arguments to keep");
        }
        let (name, args) = (menu_name(self.menu.kind), self.menu.args());
        SESSION
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(name.clone(), args.clone());
        if !permanently {
            return cx.editor.set_status("Arguments set for this session");
        }
        with_state(|state| state.values.insert(name, args));
        match persist() {
            Ok(()) => cx
                .editor
                .set_status("Arguments saved as this menu's defaults"),
            Err(err) => cx.editor.set_error(format!("could not save: {err}")),
        }
    }

    fn run(&mut self, command: MagitCommand, close: Callback) -> EventResult {
        match command {
            MagitCommand::OpenMenu(MenuKind::Views) => {
                let workdir = self.workdir.clone();
                EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.remove(TransientOverlay::ID);
                    let overlay = crate::magit::views_overlay(compositor, workdir);
                    compositor.push(Box::new(overlay));
                })))
            }
            MagitCommand::OpenMenu(kind) => {
                self.menu = prepare(kind.menu());
                self.editing_levels = false;
                EventResult::Consumed(None)
            }
            MagitCommand::Quit => EventResult::Consumed(Some(close)),
            MagitCommand::ShowRefs => {
                let workdir = self.workdir.clone();
                EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.remove(TransientOverlay::ID);
                    compositor.push(Box::new(DiffView::refs(&workdir)));
                })))
            }
            MagitCommand::ShowCherries => {
                let workdir = self.workdir.clone();
                EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.remove(TransientOverlay::ID);
                    compositor.push(Box::new(crate::ui::diff_view::cherries_prompt(workdir)));
                })))
            }
            MagitCommand::ConflictEdit
            | MagitCommand::ConflictShowOurs
            | MagitCommand::ConflictShowTheirs
            | MagitCommand::ConflictShowBase => {
                use helix_magit::conflict::Side;
                let Some((path, AskKind::Path)) = self.target.clone() else {
                    return EventResult::Consumed(Some(close));
                };
                let workdir = self.workdir.clone();
                EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                    compositor.remove(TransientOverlay::ID);
                    crate::magit::step_aside(compositor, cx.editor);
                    let path = std::path::PathBuf::from(path);
                    match command {
                        MagitCommand::ConflictEdit => {
                            crate::magit::edit_conflict(cx.editor, &workdir.join(&path))
                        }
                        command => {
                            // The file itself first, so the side opens beside it.
                            crate::magit::edit_conflict(cx.editor, &workdir.join(&path));
                            let side = match command {
                                MagitCommand::ConflictShowOurs => Side::Ours,
                                MagitCommand::ConflictShowTheirs => Side::Theirs,
                                _ => Side::Base,
                            };
                            crate::magit::show_conflict_side(cx.editor, &workdir, &path, side);
                        }
                    }
                })))
            }
            MagitCommand::RebaseEditTodo
            | MagitCommand::MergePreview
            | MagitCommand::StashList
            | MagitCommand::WorktreeVisit
            | MagitCommand::SubmoduleList => {
                let workdir = self.workdir.clone();
                EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                    compositor.remove(TransientOverlay::ID);
                    crate::magit::open_list(compositor, cx.editor, workdir, command);
                })))
            }
            MagitCommand::FileFind | MagitCommand::FileRename | MagitCommand::FileTrace => {
                let path = match self.target.clone() {
                    Some((path, AskKind::Path)) => Some(path),
                    _ => None,
                };
                let workdir = self.workdir.clone();
                let line = self.line;
                EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                    compositor.remove(TransientOverlay::ID);
                    match (command, path) {
                        (MagitCommand::FileFind, path) => {
                            crate::magit::find_file(compositor, workdir, path, line)
                        }
                        (MagitCommand::FileRename, Some(path)) => {
                            crate::magit::rename_file(compositor, cx.editor, workdir, path)
                        }
                        (_, Some(path)) => {
                            crate::magit::trace_lines(compositor, cx.editor, workdir, path, line)
                        }
                        (_, None) => cx.editor.set_error("Open the file menu on a file"),
                    }
                })))
            }
            MagitCommand::Ediff(kind) => {
                let target = self.target.clone();
                let workdir = self.workdir.clone();
                EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                    compositor.remove(TransientOverlay::ID);
                    crate::magit::ediff(compositor, cx.editor, workdir, kind, target);
                })))
            }
            MagitCommand::InsertRevision => {
                EventResult::Consumed(Some(Box::new(|compositor, cx| {
                    compositor.remove(TransientOverlay::ID);
                    if let Some(prompt) = crate::magit::revision_prompt(cx.editor) {
                        compositor.push(prompt);
                    }
                })))
            }
            MagitCommand::Mergetool => {
                let Some((path, AskKind::Path)) = self.target.clone() else {
                    return EventResult::Consumed(Some(close));
                };
                let workdir = self.workdir.clone();
                EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                    compositor.remove(TransientOverlay::ID);
                    crate::magit::mergetool(cx.editor, compositor, &workdir, &path);
                })))
            }
            MagitCommand::FileEditLineCommit => {
                let Some((path, AskKind::Path)) = self.target.clone() else {
                    return EventResult::Consumed(Some(close));
                };
                let workdir = self.workdir.clone();
                let line = self.line;
                EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                    compositor.remove(TransientOverlay::ID);
                    crate::magit::edit_line_commit(compositor, cx, workdir, &path, line);
                })))
            }
            MagitCommand::FileDiff | MagitCommand::FileLog | MagitCommand::FileBlame => {
                let Some((path, AskKind::Path)) = self.target.clone() else {
                    return EventResult::Consumed(Some(close));
                };
                let path = std::path::PathBuf::from(path);
                let workdir = self.workdir.clone();
                let line = self.line;
                EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                    compositor.remove(TransientOverlay::ID);
                    match command {
                        MagitCommand::FileDiff => match DiffView::file(&workdir, path) {
                            Ok(view) => compositor.push(Box::new(view)),
                            Err(err) => cx.editor.set_error(err.to_string()),
                        },
                        MagitCommand::FileLog => {
                            let filter = LogFilter {
                                path: Some(path),
                                follow: true,
                                ..LogFilter::default()
                            };
                            compositor.push(Box::new(LogView::new(workdir, filter)));
                        }
                        _ => compositor.push(Box::new(crate::ui::blame_view::BlameView::new(
                            workdir, path, None, line,
                        ))),
                    }
                })))
            }
            MagitCommand::ApplyDiffSettings => {
                let options = helix_magit::diff::DiffOptions::from_args(&self.menu.args());
                EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.remove(TransientOverlay::ID);
                    for id in [DiffView::COMMIT_ID, DiffView::RANGE_ID, DiffView::ID] {
                        if let Some(view) = compositor.find_id::<DiffView>(id) {
                            view.set_options(options.clone());
                        }
                    }
                })))
            }
            MagitCommand::DiffDwim
            | MagitCommand::DiffPaths
            | MagitCommand::DiffStash
            | MagitCommand::DiffToggleRange
            | MagitCommand::DiffFlip => {
                let (workdir, target) = (self.workdir.clone(), self.target.clone());
                EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                    compositor.remove(TransientOverlay::ID);
                    crate::magit::diff_action(compositor, cx.editor, workdir, command, target);
                })))
            }
            MagitCommand::DiffRange | MagitCommand::DiffWorktree | MagitCommand::DiffCommit => {
                let workdir = self.workdir.clone();
                let start = self
                    .target
                    .clone()
                    .filter(|(_, kind)| *kind != AskKind::Path)
                    .map(|(value, _)| value);
                EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                    compositor.remove(TransientOverlay::ID);
                    compositor.push(Box::new(crate::ui::diff_view::diff_prompt(
                        command, workdir, start, cx.editor,
                    )));
                })))
            }
            MagitCommand::Shortlog => {
                let workdir = self.workdir.clone();
                let args = self.menu.args();
                EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.remove(TransientOverlay::ID);
                    compositor.push(Box::new(crate::magit::shortlog_prompt(workdir, args)));
                })))
            }
            MagitCommand::ListRepositories => {
                EventResult::Consumed(Some(Box::new(|compositor, cx| {
                    compositor.remove(TransientOverlay::ID);
                    let config = cx.editor.config().magit.clone();
                    let roots = if config.repository_directories.is_empty() {
                        vec![helix_stdx::env::current_working_dir()]
                    } else {
                        config
                            .repository_directories
                            .iter()
                            .map(|dir| helix_stdx::path::expand_tilde(dir.as_path()).into_owned())
                            .collect()
                    };
                    compositor.remove(DiffView::REPOSITORIES_ID);
                    compositor.push(Box::new(DiffView::repositories(
                        roots,
                        config.repository_depth,
                    )));
                })))
            }
            MagitCommand::ShowProcess => EventResult::Consumed(Some(Box::new(|compositor, cx| {
                compositor.remove(TransientOverlay::ID);
                crate::magit::step_aside(compositor, cx.editor);
                crate::magit::show_process(cx.editor);
            }))),
            MagitCommand::LogCurrent | MagitCommand::LogAll => {
                let mut filter = LogFilter::from_args(&self.menu.args());
                filter.all = command == MagitCommand::LogAll;
                let workdir = self.workdir.clone();
                EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.remove(TransientOverlay::ID);
                    compositor.push(Box::new(LogView::new(workdir, filter)));
                })))
            }
            MagitCommand::Reflog => {
                let filter = LogFilter {
                    reflog: true,
                    ..LogFilter::default()
                };
                let workdir = self.workdir.clone();
                EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.remove(TransientOverlay::ID);
                    compositor.push(Box::new(LogView::new(workdir, filter)));
                })))
            }
            MagitCommand::WipLog | MagitCommand::WipIndexLog => {
                let refs = helix_magit::wip::refs(&self.workdir);
                let wip_ref = if command == MagitCommand::WipLog {
                    refs.worktree
                } else {
                    refs.index
                };
                let workdir = self.workdir.clone();
                EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                    compositor.remove(TransientOverlay::ID);
                    let exists = helix_magit::GitCommand::new(
                        &workdir,
                        vec![
                            "rev-parse".into(),
                            "--verify".into(),
                            "--quiet".into(),
                            wip_ref.clone(),
                        ],
                    )
                    .run()
                    .is_ok_and(|output| output.success);
                    if !exists {
                        cx.editor.set_error(if cx.editor.config().magit.wip {
                            "No wip save yet on this branch".to_string()
                        } else {
                            "No wip saves: they are off (editor.magit.wip = true turns them on)"
                                .to_string()
                        });
                        return;
                    }
                    let filter = LogFilter {
                        range: Some(wip_ref),
                        ..LogFilter::default()
                    };
                    compositor.push(Box::new(LogView::new(workdir, filter)));
                })))
            }
            MagitCommand::ReflogOther => {
                let filter = LogFilter {
                    reflog: true,
                    ..LogFilter::default()
                };
                let workdir = self.workdir.clone();
                EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.remove(TransientOverlay::ID);
                    compositor.push(Box::new(crate::ui::log_view::range_prompt(workdir, filter)));
                })))
            }
            MagitCommand::Margin(choice) => {
                EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                    compositor.remove(TransientOverlay::ID);
                    let message = crate::magit::set_margin(compositor, choice);
                    cx.editor.set_status(message);
                })))
            }
            MagitCommand::LogRelated
            | MagitCommand::LogLocalBranches
            | MagitCommand::LogBranches
            | MagitCommand::LogMatchingBranches
            | MagitCommand::LogMerged => {
                let filter = LogFilter::from_args(&self.menu.args());
                let (workdir, target) = (self.workdir.clone(), self.target.clone());
                EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                    compositor.remove(TransientOverlay::ID);
                    crate::magit::log_action(
                        compositor, cx.editor, workdir, command, filter, target,
                    );
                })))
            }
            MagitCommand::LogOther => {
                let filter = LogFilter::from_args(&self.menu.args());
                let workdir = self.workdir.clone();
                EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.remove(TransientOverlay::ID);
                    compositor.push(Box::new(crate::ui::log_view::range_prompt(workdir, filter)));
                })))
            }
            MagitCommand::RunGit | MagitCommand::RunShell => {
                let workdir = self.workdir.clone();
                let shell = command == MagitCommand::RunShell;
                EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.remove(TransientOverlay::ID);
                    compositor.push(Box::new(crate::magit::command_prompt(workdir, shell)));
                })))
            }
            MagitCommand::JumpTo(target) => {
                EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                    compositor.remove(TransientOverlay::ID);
                    match compositor.find_id::<DiffView>(DiffView::ID) {
                        Some(status) => {
                            if !status.jump_to(target) {
                                cx.editor.set_status("That section is empty");
                            }
                        }
                        None => cx.editor.set_error("Jumping needs the status buffer"),
                    }
                })))
            }
            MagitCommand::SwitchTo(view) => {
                let workdir = self.workdir.clone();
                EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                    compositor.remove(TransientOverlay::ID);
                    crate::magit::switch_to(compositor, cx.editor, view, &workdir);
                })))
            }
            // The status buffer replaces the menu rather than stacking on it.
            MagitCommand::Status | MagitCommand::Refresh => {
                let workdir = self.workdir.clone();
                EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                    compositor.remove(TransientOverlay::ID);
                    match crate::ui::diff_view::DiffView::new(&workdir) {
                        Ok(view) => compositor.push(Box::new(view)),
                        Err(err) => cx.editor.set_error(err.to_string()),
                    }
                })))
            }
            command => {
                let Some(mut plan) = helix_magit::resolve(command, &self.menu.args()) else {
                    return EventResult::Consumed(Some(close));
                };
                if let Some((value, kind)) = &self.target {
                    if plan.requirement == helix_magit::Requirement::TodoList {
                        if matches!(kind, AskKind::Revision | AskKind::Branch | AskKind::Tag) {
                            plan.args
                                .push(helix_magit::rebase::base_for(&self.workdir, value));
                        }
                    } else {
                        plan.preset(value, *kind);
                    }
                }
                // The directories there now, to edit rather than retype.
                if command == MagitCommand::SparseSet {
                    let current = helix_magit::status::sparse_directories(&self.workdir)
                        .filter(|directories| !directories.is_empty());
                    if let (Some(current), helix_magit::Requirement::Ask(asks)) =
                        (current, &mut plan.requirement)
                    {
                        if let Some(ask) = asks.first_mut() {
                            ask.preset = Some(current.join(" "));
                        }
                    }
                }
                // A repository is cloned or made where the editor is, not
                // inside the one the menu belongs to.
                let workdir = if matches!(command, MagitCommand::Clone | MagitCommand::Init) {
                    helix_stdx::env::current_working_dir()
                } else {
                    self.workdir.clone()
                };

                // The menu goes away first, so a confirmation or a prompt is
                // not stacked on top of it.
                EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                    compositor.remove(TransientOverlay::ID);
                    crate::magit::execute(compositor, cx, plan, workdir);
                })))
            }
        }
    }
}

/// Asks for an option's value and sets it in the menu, which stays open.
fn option_prompt(
    label: String,
    key: char,
    flag: String,
    history: Vec<String>,
) -> crate::ui::Prompt {
    let mut prompt = crate::ui::Prompt::new(
        label.into(),
        Some(OPTION_HISTORY),
        move |_, input| {
            history
                .iter()
                .filter(|value| value.contains(input))
                .map(|value| (0.., value.clone().into()))
                .collect()
        },
        move |cx, input, event| {
            if event != crate::ui::PromptEvent::Validate || input.trim().is_empty() {
                return;
            }
            let value = input.trim().to_string();
            with_state(|state| state.remember(&flag, &value));
            if let Err(err) = persist() {
                cx.editor
                    .set_error(format!("could not save the history: {err}"));
            }
            cx.jobs.callback(async move {
                Ok(crate::job::Callback::EditorCompositor(Box::new(
                    move |_: &mut helix_view::Editor,
                          compositor: &mut crate::compositor::Compositor| {
                        if let Some(overlay) =
                            compositor.find_id::<TransientOverlay>(TransientOverlay::ID)
                        {
                            overlay.menu.set_option(key, Some(value));
                        }
                    },
                )))
            });
        },
    );
    prompt.with_history_register(Some(OPTION_HISTORY));
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_magit::transient::{commit_menu, main_menu, pull_menu, tag_menu};

    fn overlay(menu: TransientMenu) -> TransientOverlay {
        TransientOverlay::new(menu, "main", PathBuf::from("/repo"))
    }

    #[test]
    fn the_preview_shows_the_command_the_arguments_build() {
        let mut overlay = overlay(pull_menu());
        assert_eq!(overlay.command_preview(), "git pull");

        for key in ['-', 'r', '-', 'a'] {
            overlay.menu.handle_key(key);
        }
        assert_eq!(overlay.command_preview(), "git pull --rebase --autostash");

        overlay.menu.handle_key('-');
        overlay.menu.handle_key('r');
        assert_eq!(overlay.command_preview(), "git pull --autostash");
    }

    #[test]
    fn the_main_menu_has_no_command_to_preview() {
        // It only opens other menus, so there is no git invocation to show.
        assert!(overlay(main_menu()).command_preview().is_empty());
    }

    #[test]
    fn columns_never_outnumber_groups() {
        let overlay = overlay(commit_menu());
        let groups = overlay.menu.groups.len();

        // A very wide terminal still shows one column per group.
        assert_eq!(overlay.column_count(500), groups);
        // A narrow one collapses to a single column rather than to zero.
        assert_eq!(overlay.column_count(10), 1);
        assert_eq!(overlay.column_count(0), 1);
    }

    #[test]
    fn height_covers_the_tallest_column_plus_chrome() {
        let overlay = overlay(commit_menu());

        // Laid out in one column per group, the tallest group decides.
        let tallest = overlay
            .menu
            .groups
            .iter()
            .map(TransientGroup::height)
            .max()
            .unwrap();
        assert_eq!(overlay.height(500), tallest as u16 + 3);

        // Squeezed into one column, every group stacks.
        let total: usize = overlay.menu.groups.iter().map(TransientGroup::height).sum();
        assert_eq!(overlay.height(10), total as u16 + 3);
    }

    #[test]
    fn every_action_a_menu_offers_resolves_or_opens_another_menu() {
        // An action that neither runs a command nor opens a menu would be a
        // key that silently does nothing.
        for kind in MenuKind::ALL {
            let menu = kind.menu();
            for action in menu.groups.iter().flat_map(|group| &group.actions) {
                let handled = matches!(
                    action.command,
                    MagitCommand::OpenMenu(_)
                        | MagitCommand::Status
                        | MagitCommand::Refresh
                        | MagitCommand::Quit
                        | MagitCommand::LogCurrent
                        | MagitCommand::LogAll
                        | MagitCommand::LogOther
                        | MagitCommand::Reflog
                        | MagitCommand::ReflogOther
                        | MagitCommand::WipLog
                        | MagitCommand::WipIndexLog
                        | MagitCommand::ShowRefs
                        | MagitCommand::ShowCherries
                        | MagitCommand::ShowProcess
                        | MagitCommand::ListRepositories
                        | MagitCommand::Shortlog
                        | MagitCommand::ConflictEdit
                        | MagitCommand::ConflictShowOurs
                        | MagitCommand::ConflictShowTheirs
                        | MagitCommand::ConflictShowBase
                        | MagitCommand::FileDiff
                        | MagitCommand::FileLog
                        | MagitCommand::FileBlame
                        | MagitCommand::FileEditLineCommit
                        | MagitCommand::FileFind
                        | MagitCommand::FileRename
                        | MagitCommand::FileTrace
                        | MagitCommand::RebaseEditTodo
                        | MagitCommand::MergePreview
                        | MagitCommand::StashList
                        | MagitCommand::WorktreeVisit
                        | MagitCommand::SubmoduleList
                        | MagitCommand::InsertRevision
                        | MagitCommand::Mergetool
                        | MagitCommand::ApplyDiffSettings
                        | MagitCommand::RunGit
                        | MagitCommand::RunShell
                        | MagitCommand::JumpTo(_)
                        | MagitCommand::Ediff(_)
                        | MagitCommand::SwitchTo(_)
                        | MagitCommand::DiffRange
                        | MagitCommand::DiffWorktree
                        | MagitCommand::DiffCommit
                        | MagitCommand::DiffDwim
                        | MagitCommand::DiffPaths
                        | MagitCommand::DiffStash
                        | MagitCommand::DiffToggleRange
                        | MagitCommand::DiffFlip
                        | MagitCommand::Margin(_)
                        | MagitCommand::LogRelated
                        | MagitCommand::LogLocalBranches
                        | MagitCommand::LogBranches
                        | MagitCommand::LogMatchingBranches
                        | MagitCommand::LogMerged
                ) || helix_magit::resolve(action.command, &[]).is_some();
                assert!(handled, "{kind:?} binds '{}' to nothing", action.key);
            }
        }
    }

    #[test]
    fn session_values_and_hidden_commands_shape_the_menu_as_it_opens() {
        // The tag menu is this test's alone: the state is the process's.
        let mut menu = tag_menu();
        menu.handle_key('-');
        menu.handle_key('a');
        SESSION
            .lock()
            .unwrap()
            .insert(menu_name(MenuKind::Tag), menu.args());
        with_state(|state| state.toggle_hidden(MenuKind::Tag, 'k'));

        let overlay = overlay(tag_menu());
        assert_eq!(overlay.menu.args(), ["--annotate"]);
        assert!(overlay.menu.hidden.contains(&'k'));
        let mut menu = overlay.menu.clone();
        assert_eq!(menu.handle_key('k'), TransientEvent::Unhandled);
        assert!(matches!(menu.handle_key('t'), TransientEvent::Run(_)));

        // The diff settings keep the view's own values.
        SESSION
            .lock()
            .unwrap()
            .insert(menu_name(MenuKind::DiffSettings), vec!["--stat".into()]);
        let settings = prepare(helix_magit::transient::diff_settings_menu(
            &helix_magit::diff::DiffOptions::default(),
        ));
        assert!(!settings.args().contains(&"--stat".to_string()));
    }
}
