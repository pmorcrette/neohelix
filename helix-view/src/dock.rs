//! Docked panes: the fork's own views given a side of the screen, so that
//! the documents make room for them instead of being covered.
//!
//! The view tree is not told about panes; it is only given a smaller area.
//! Each frame the application says which pane occupies which side, the
//! editor view lays the sides out before resizing the tree, and each pane
//! then draws in the area it was given. A pane that does not fit falls back
//! to drawing over the documents, as it would undocked.

use serde::{Deserialize, Serialize};

use crate::graphics::Rect;

/// Where a view goes, from `[editor.dock]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Placement {
    Right,
    Bottom,
    /// Not docked: the view covers the documents, as before docking.
    #[default]
    None,
}

/// The sides of the screen a pane can take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Right,
    Bottom,
}

/// Narrowest the documents may become for a right pane to be given room.
const MIN_DOCUMENT_WIDTH: u16 = 30;
/// Lowest the documents may become for a bottom pane to be given room.
const MIN_DOCUMENT_HEIGHT: u16 = 6;
/// Smallest a pane is worth drawing docked.
const MIN_PANE_WIDTH: u16 = 20;
const MIN_PANE_HEIGHT: u16 = 4;

/// A view that wants a dock this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Occupant {
    /// The pane's name: a component id, or one name for a group of views
    /// that share a side, such as Magit's.
    pub id: &'static str,
    pub placement: Placement,
    /// Whether it takes keys at all; a pane that only shows something, like
    /// the backlinks, never has the focus.
    pub takes_keys: bool,
}

/// Which pane is on which side, where it is, and whether it has the keys.
#[derive(Debug, Default)]
pub struct Dock {
    right: Option<Occupant>,
    bottom: Option<Occupant>,
    right_area: Option<Rect>,
    bottom_area: Option<Rect>,
    /// The pane that has the keyboard; `None` while the documents have it.
    focus: Option<&'static str>,
    /// The panes open last frame, to tell which one just opened.
    previous: Vec<&'static str>,
}

impl Dock {
    /// Sets the panes for this frame, from the views open and where each is
    /// configured to go; the first to ask for a side gets it. A pane that
    /// just opened and takes keys gets the focus; one no longer open loses
    /// it.
    pub fn set_occupants(&mut self, wanted: &[Occupant]) {
        self.right = None;
        self.bottom = None;
        for occupant in wanted {
            match occupant.placement {
                Placement::Right if self.right.is_none() => self.right = Some(*occupant),
                Placement::Bottom if self.bottom.is_none() => self.bottom = Some(*occupant),
                _ => {}
            }
        }
        for occupant in [self.right, self.bottom].into_iter().flatten() {
            if occupant.takes_keys && !self.previous.contains(&occupant.id) {
                self.focus = Some(occupant.id);
            }
        }
        self.previous = wanted.iter().map(|occupant| occupant.id).collect();
        if self
            .focus
            .is_some_and(|focused| self.side_of(focused).is_none())
        {
            self.focus = None;
        }
    }

    /// Moves the focus to the next docked pane that takes keys, and from the
    /// last back to the documents; returns the pane that has it now.
    pub fn cycle_focus(&mut self) -> Option<&'static str> {
        let panes: Vec<&'static str> = [
            self.right_area.and(self.right),
            self.bottom_area.and(self.bottom),
        ]
        .into_iter()
        .flatten()
        .filter(|occupant| occupant.takes_keys)
        .map(|occupant| occupant.id)
        .collect();
        self.focus = match self.focus {
            None => panes.first().copied(),
            Some(current) => panes
                .iter()
                .position(|id| *id == current)
                .and_then(|index| panes.get(index + 1).copied()),
        };
        self.focus
    }

    /// Lays the sides out in `area`, the space the documents would have had,
    /// and returns what is left for them. The right pane takes the full
    /// height; the bottom one spans what the documents keep. `right_size`
    /// and `bottom_size` are percentages.
    pub fn layout(&mut self, area: Rect, right_size: u16, bottom_size: u16) -> Rect {
        self.right_area = None;
        self.bottom_area = None;
        let mut documents = area;

        if self.right.is_some() {
            let width = (area.width as u32 * right_size.min(90) as u32 / 100) as u16;
            if width >= MIN_PANE_WIDTH && area.width.saturating_sub(width) >= MIN_DOCUMENT_WIDTH {
                self.right_area = Some(Rect::new(area.right() - width, area.y, width, area.height));
                documents.width -= width;
            }
        }
        if self.bottom.is_some() {
            let height = (documents.height as u32 * bottom_size.min(90) as u32 / 100) as u16;
            if height >= MIN_PANE_HEIGHT
                && documents.height.saturating_sub(height) >= MIN_DOCUMENT_HEIGHT
            {
                self.bottom_area = Some(Rect::new(
                    documents.x,
                    documents.bottom() - height,
                    documents.width,
                    height,
                ));
                documents.height -= height;
            }
        }
        documents
    }

    /// The side `id` occupies this frame, if any.
    pub fn side_of(&self, id: &str) -> Option<Side> {
        if self.right.is_some_and(|occupant| occupant.id == id) {
            Some(Side::Right)
        } else if self.bottom.is_some_and(|occupant| occupant.id == id) {
            Some(Side::Bottom)
        } else {
            None
        }
    }

    /// Where `id` draws, when it is docked and was given room.
    pub fn area_of(&self, id: &str) -> Option<Rect> {
        match self.side_of(id)? {
            Side::Right => self.right_area,
            Side::Bottom => self.bottom_area,
        }
    }

    /// Whether `id` is drawn in a dock this frame.
    pub fn is_docked(&self, id: &str) -> bool {
        self.area_of(id).is_some()
    }

    /// Gives `id` the keyboard, or gives it back to the documents.
    pub fn focus(&mut self, id: Option<&'static str>) {
        self.focus = id;
    }

    pub fn focused(&self) -> Option<&'static str> {
        self.focus
    }

    /// Whether `id` should take keys: it has the focus, or it is not docked
    /// and so covers the documents anyway.
    pub fn has_keys(&self, id: &str) -> bool {
        self.focus == Some(id) || !self.is_docked(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &'static str, placement: Placement) -> Occupant {
        Occupant {
            id,
            placement,
            takes_keys: true,
        }
    }

    #[test]
    fn panes_take_their_side_and_the_documents_the_rest() {
        let mut dock = Dock::default();
        dock.set_occupants(&[
            pane("terminal", Placement::Bottom),
            pane("roam", Placement::Right),
        ]);
        let documents = dock.layout(Rect::new(0, 1, 100, 40), 30, 25);
        assert_eq!(dock.area_of("roam"), Some(Rect::new(70, 1, 30, 40)));
        assert_eq!(dock.area_of("terminal"), Some(Rect::new(0, 31, 70, 10)));
        assert_eq!(documents, Rect::new(0, 1, 70, 30));
    }

    #[test]
    fn a_pane_that_does_not_fit_is_not_docked() {
        let mut dock = Dock::default();
        dock.set_occupants(&[
            pane("roam", Placement::Right),
            pane("terminal", Placement::Bottom),
        ]);
        // 40 columns: a 12-column pane is too narrow, and a wider one would
        // squeeze the documents under 30.
        let area = Rect::new(0, 0, 40, 8);
        assert_eq!(dock.layout(area, 30, 50), area);
        assert!(!dock.is_docked("roam"));
        assert!(!dock.is_docked("terminal"));
        // Not docked, a view keeps the keys as it did before docking.
        assert!(dock.has_keys("terminal"));
    }

    #[test]
    fn a_side_goes_to_the_first_asking_and_focus_follows_the_panes() {
        let mut dock = Dock::default();
        dock.set_occupants(&[pane("a", Placement::Right), pane("b", Placement::Right)]);
        assert_eq!(dock.side_of("a"), Some(Side::Right));
        assert_eq!(dock.side_of("b"), None);
        // Just opened: it has the keys.
        assert_eq!(dock.focused(), Some("a"));

        dock.focus(None);
        dock.layout(Rect::new(0, 0, 120, 40), 40, 40);
        assert!(!dock.has_keys("a"));
        // Still open: it does not take them back by itself.
        dock.set_occupants(&[pane("a", Placement::Right)]);
        assert_eq!(dock.focused(), None);

        dock.focus(Some("a"));
        dock.set_occupants(&[]);
        assert_eq!(dock.focused(), None);
    }

    #[test]
    fn the_focus_cycles_through_the_panes_that_take_keys() {
        let mut dock = Dock::default();
        dock.set_occupants(&[
            Occupant {
                id: "roam",
                placement: Placement::Right,
                takes_keys: false,
            },
            pane("terminal", Placement::Bottom),
        ]);
        dock.layout(Rect::new(0, 0, 120, 40), 30, 40);
        dock.focus(None);
        assert_eq!(dock.cycle_focus(), Some("terminal"));
        assert_eq!(dock.cycle_focus(), None);

        dock.set_occupants(&[
            pane("magit", Placement::Right),
            pane("terminal", Placement::Bottom),
        ]);
        dock.layout(Rect::new(0, 0, 120, 40), 30, 40);
        dock.focus(None);
        assert_eq!(dock.cycle_focus(), Some("magit"));
        assert_eq!(dock.cycle_focus(), Some("terminal"));
        assert_eq!(dock.cycle_focus(), None);
    }
}
