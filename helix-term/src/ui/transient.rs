//! The transient menu overlay, Magit's popup.
//!
//! Renders a [`TransientMenu`] as labelled columns across the bottom of the
//! screen and captures keys modally while it is open.

use helix_magit::transient::{MenuKind, TransientEvent, TransientGroup, TransientMenu};
use helix_magit::MagitCommand;
use helix_view::graphics::Rect;
use helix_view::input::{KeyCode, KeyEvent};
use tui::buffer::Buffer as Surface;
use tui::widgets::{Block, Widget};

use crate::compositor::{Callback, Component, Context, Event, EventResult};

/// Columns narrower than this are not worth splitting into.
const MIN_COLUMN_WIDTH: u16 = 22;
/// Gap between two columns.
const COLUMN_GAP: u16 = 2;

/// A transient menu shown over the editor.
pub struct TransientOverlay {
    menu: TransientMenu,
    /// What the title shows after the menu name, typically the branch.
    context: String,
}

impl TransientOverlay {
    pub const ID: &'static str = "magit-transient";

    pub fn new(menu: TransientMenu, context: impl Into<String>) -> Self {
        Self {
            menu,
            context: context.into(),
        }
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
                        &format!(" {} ", argument.key()),
                        3,
                        |_| key_style,
                        false,
                        false,
                    );
                    surface.set_string_truncated(
                        x + 3,
                        y,
                        &format!("{} {}", argument.flag(), argument.description()),
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

    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
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
            KeyEvent {
                code: KeyCode::Char(c),
                ..
            } => match self.menu.handle_key(*c) {
                TransientEvent::Toggled => return EventResult::Consumed(None),
                TransientEvent::Run(command) => return self.run(command, cx, close),
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
    fn run(&mut self, command: MagitCommand, cx: &mut Context, close: Callback) -> EventResult {
        match command {
            MagitCommand::OpenMenu(kind) => {
                self.menu = kind.menu();
                EventResult::Consumed(None)
            }
            MagitCommand::Quit => EventResult::Consumed(Some(close)),
            // Running git is the next sprint's work. Reporting the resolved
            // command line makes the menu verifiable now, and is honest about
            // not having touched the repository.
            command => {
                cx.editor.set_status(format!(
                    "{} (not executed yet): {}",
                    describe(command),
                    self.command_preview()
                ));
                EventResult::Consumed(Some(close))
            }
        }
    }
}

fn describe(command: MagitCommand) -> &'static str {
    match command {
        MagitCommand::Commit => "commit",
        MagitCommand::CommitAmend => "commit --amend",
        MagitCommand::CommitExtend => "commit --amend --no-edit",
        MagitCommand::CommitFixup => "commit --fixup",
        MagitCommand::Push => "push",
        MagitCommand::PushToUpstream => "push upstream",
        MagitCommand::PushElsewhere => "push elsewhere",
        MagitCommand::Pull => "pull",
        MagitCommand::Fetch => "fetch",
        MagitCommand::FetchAll => "fetch --all",
        MagitCommand::BranchCheckout => "checkout",
        MagitCommand::BranchCreate => "branch",
        MagitCommand::BranchCreateAndCheckout => "checkout -b",
        MagitCommand::BranchDelete => "branch -d",
        MagitCommand::RebaseOntoUpstream => "rebase upstream",
        MagitCommand::RebaseInteractive => "rebase --interactive",
        MagitCommand::RebaseAbort => "rebase --abort",
        MagitCommand::RebaseContinue => "rebase --continue",
        MagitCommand::OpenMenu(_) | MagitCommand::Refresh | MagitCommand::Quit => "magit",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_magit::transient::{commit_menu, main_menu, pull_menu};

    fn overlay(menu: TransientMenu) -> TransientOverlay {
        TransientOverlay::new(menu, "main")
    }

    #[test]
    fn the_preview_shows_the_command_the_arguments_build() {
        let mut overlay = overlay(pull_menu());
        assert_eq!(overlay.command_preview(), "git pull");

        overlay.menu.handle_key('r');
        overlay.menu.handle_key('a');
        assert_eq!(overlay.command_preview(), "git pull --rebase --autostash");

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
    fn every_command_has_a_description() {
        // `describe` feeds the status line; an empty one would be a silent
        // action.
        for command in [
            MagitCommand::Commit,
            MagitCommand::CommitAmend,
            MagitCommand::Push,
            MagitCommand::Pull,
            MagitCommand::Fetch,
            MagitCommand::BranchCreate,
            MagitCommand::RebaseInteractive,
            MagitCommand::Refresh,
        ] {
            assert!(!describe(command).is_empty(), "{command:?} has no label");
        }
    }
}
