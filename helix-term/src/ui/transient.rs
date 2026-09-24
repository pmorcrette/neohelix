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
use tui::buffer::Buffer as Surface;
use tui::widgets::{Block, Widget};

use crate::compositor::{Callback, Component, Context, Event, EventResult};
use crate::ui::log_view::LogView;
use helix_magit::log::LogFilter;

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
}

impl TransientOverlay {
    pub const ID: &'static str = "magit-transient";

    pub fn new(menu: TransientMenu, context: impl Into<String>, workdir: PathBuf) -> Self {
        Self {
            menu,
            context: context.into(),
            workdir,
            target: None,
        }
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
            MenuKind::BranchConfig | MenuKind::RemoteConfig => "git config",
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

        let title = if self.context.is_empty() {
            self.menu.title.clone()
        } else {
            format!("{}: {}", self.menu.title, self.context)
        };
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
                        &action.description,
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

    fn handle_event(&mut self, event: &Event, _cx: &mut Context) -> EventResult {
        let Event::Key(key) = event else {
            // The menu is modal: swallow everything, so a stray mouse event
            // does not reach the editor underneath.
            return EventResult::Consumed(None);
        };

        let close: Callback = Box::new(|compositor, _| {
            compositor.remove(TransientOverlay::ID);
        });

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
                return EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.push(Box::new(option_prompt(label, key)));
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
    fn run(&mut self, command: MagitCommand, close: Callback) -> EventResult {
        match command {
            MagitCommand::OpenMenu(kind) => {
                self.menu = kind.menu();
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
            MagitCommand::Shortlog => {
                let workdir = self.workdir.clone();
                let args = self.menu.args();
                EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.remove(TransientOverlay::ID);
                    compositor.push(Box::new(crate::magit::shortlog_prompt(workdir, args)));
                })))
            }
            MagitCommand::ShowProcess => EventResult::Consumed(Some(Box::new(|compositor, cx| {
                compositor.remove(TransientOverlay::ID);
                crate::magit::close_views(compositor);
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
            MagitCommand::LogOther => {
                let filter = LogFilter::from_args(&self.menu.args());
                let workdir = self.workdir.clone();
                EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.remove(TransientOverlay::ID);
                    compositor.push(Box::new(crate::ui::log_view::range_prompt(workdir, filter)));
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
                let workdir = self.workdir.clone();

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
fn option_prompt(label: String, key: char) -> crate::ui::Prompt {
    crate::ui::Prompt::new(
        label.into(),
        None,
        |_, _| Vec::new(),
        move |cx, input, event| {
            if event != crate::ui::PromptEvent::Validate || input.trim().is_empty() {
                return;
            }
            let value = input.trim().to_string();
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
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_magit::transient::{commit_menu, main_menu, pull_menu};

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
                        | MagitCommand::ShowRefs
                        | MagitCommand::ShowCherries
                        | MagitCommand::ShowProcess
                        | MagitCommand::Shortlog
                ) || helix_magit::resolve(action.command, &[]).is_some();
                assert!(handled, "{kind:?} binds '{}' to nothing", action.key);
            }
        }
    }
}
