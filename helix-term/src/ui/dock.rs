//! Which of the fork's views are docked this frame, and how a docked view
//! shares the keys and the mouse with the documents; see
//! [`helix_view::dock`].

use helix_view::dock::Occupant;
use helix_view::input::{KeyCode, MouseEventKind};
use helix_view::Editor;

use crate::compositor::{Compositor, Event, EventResult};
use crate::ui::{
    blame_view::BlameView, diff_view::DiffView, log_view::LogView, roam::RoamPanel,
    terminal::TerminalView,
};

/// The dock name Magit's views share: the status, the log and the blame
/// take turns in one pane rather than each taking a side.
pub const MAGIT: &str = "magit";

/// Tells the dock which dockable views are open and where each goes, before
/// the frame is drawn.
pub fn update(compositor: &Compositor, editor: &mut Editor) {
    let config = editor.config();
    let open = |type_name: &str| compositor.has_component(type_name);
    let magit_open = open(std::any::type_name::<DiffView>())
        || open(std::any::type_name::<LogView>())
        || open(std::any::type_name::<BlameView>());
    // In order of precedence for a side both want.
    let dockable = [
        (MAGIT, magit_open, config.dock.magit, true),
        (
            TerminalView::ID,
            open(std::any::type_name::<TerminalView>()),
            config.dock.terminal,
            true,
        ),
        (
            RoamPanel::ID,
            open(std::any::type_name::<RoamPanel>()),
            config.dock.backlinks,
            false,
        ),
    ];
    let wanted: Vec<Occupant> = dockable
        .into_iter()
        .filter(|(_, open, _, _)| *open)
        .map(|(id, _, placement, takes_keys)| Occupant {
            id,
            placement,
            takes_keys,
        })
        .collect();
    drop(config);
    editor.dock.set_occupants(&wanted);
}

/// Decides whether `event` is for the pane `id` when it is docked:
/// `Some(result)` when it is not, after moving the focus as a click says,
/// and `None` when the pane should handle it. A click on the pane focuses
/// it and a click elsewhere gives the keys back to the documents; while the
/// documents have them, keys and pastes fall through to them. With
/// `escape_leaves`, `Esc` also gives them back.
pub fn route(
    id: &'static str,
    event: &Event,
    editor: &mut Editor,
    escape_leaves: bool,
) -> Option<EventResult> {
    let area = editor.dock.area_of(id)?;
    match event {
        Event::Mouse(mouse) => {
            let inside = mouse.column >= area.x
                && mouse.column < area.right()
                && mouse.row >= area.y
                && mouse.row < area.bottom();
            let click = matches!(mouse.kind, MouseEventKind::Down(_));
            if !inside {
                if click && editor.dock.focused() == Some(id) {
                    editor.dock.focus(None);
                }
                return Some(EventResult::Ignored(None));
            }
            if click {
                editor.dock.focus(Some(id));
            }
            None
        }
        _ if editor.dock.focused() != Some(id) => Some(EventResult::Ignored(None)),
        Event::Key(key) if escape_leaves && key.code == KeyCode::Esc => {
            editor.dock.focus(None);
            Some(EventResult::Consumed(None))
        }
        _ => None,
    }
}
