//! Copy mode: moving through the scrollback with the keyboard, selecting,
//! searching and copying, on Alacritty's own vi mode.
//!
//! Everything here works on the emulator's grid, and none of it reaches the
//! program running in the terminal: while copy mode is on, keys move a
//! cursor of their own over the text.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Boundary, Column, Direction, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
pub use alacritty_terminal::term::search::Match;
use alacritty_terminal::term::search::RegexSearch;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vi_mode::ViMotion;

pub use alacritty_terminal::vi_mode::ViMotion as Motion;

/// How a selection extends as the cursor moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selecting {
    /// From one character to another.
    Characters,
    /// Whole lines.
    Lines,
    /// A rectangle.
    Block,
}

pub fn is_active<T>(term: &Term<T>) -> bool {
    term.mode().contains(TermMode::VI)
}

/// Turns copy mode on, the cursor starting where the terminal's is.
pub fn enter<T: EventListener>(term: &mut Term<T>) {
    if !is_active(term) {
        term.toggle_vi_mode();
    }
}

/// Turns copy mode off, dropping the selection and returning to the
/// bottom, where the program's output is.
pub fn leave<T: EventListener>(term: &mut Term<T>) {
    term.selection = None;
    if is_active(term) {
        term.toggle_vi_mode();
    }
    term.scroll_display(Scroll::Bottom);
}

/// Moves the copy-mode cursor, `count` times.
pub fn motion<T: EventListener>(term: &mut Term<T>, motion: ViMotion, count: usize) {
    for _ in 0..count.max(1) {
        term.vi_motion(motion);
    }
}

/// To the first line of the scrollback, or the last line of the screen.
pub fn to_edge<T: EventListener>(term: &mut Term<T>, top: bool) {
    let line = if top {
        term.topmost_line()
    } else {
        term.bottommost_line()
    };
    term.vi_goto_point(Point::new(line, Column(0)));
}

/// Starts selecting from the cursor, or stops when a selection of that kind
/// is already being made; another kind switches to it.
pub fn toggle_selection<T>(term: &mut Term<T>, kind: Selecting) {
    let ty = match kind {
        Selecting::Characters => SelectionType::Simple,
        Selecting::Lines => SelectionType::Lines,
        Selecting::Block => SelectionType::Block,
    };
    match &mut term.selection {
        Some(selection) if selection.ty == ty => term.selection = None,
        Some(selection) => selection.ty = ty,
        None => {
            let point = term.vi_mode_cursor.point;
            term.selection = Some(Selection::new(ty, point, Side::Left));
        }
    }
    // The selection's end follows the cursor as it moves; set it now too.
    let point = term.vi_mode_cursor.point;
    if let Some(selection) = &mut term.selection {
        selection.update(point, Side::Right);
    }
}

/// The selected text, if anything is selected.
pub fn selected_text<T>(term: &Term<T>) -> Option<String> {
    term.selection_to_string().filter(|text| !text.is_empty())
}

/// Searches for `pattern` from just past the cursor, forwards or backwards,
/// wrapping around the scrollback, and moves the cursor to the match.
pub fn search<T: EventListener>(
    term: &mut Term<T>,
    pattern: &str,
    forward: bool,
) -> Result<Option<Match>, String> {
    let mut regex = RegexSearch::new(pattern).map_err(|err| err.to_string())?;
    let cursor = term.vi_mode_cursor.point;
    let direction = if forward {
        Direction::Right
    } else {
        Direction::Left
    };
    // Past either end of the scrollback is the other end.
    let step = |point: Point| {
        if forward {
            point.add(term, Boundary::None, 1)
        } else {
            point.sub(term, Boundary::None, 1)
        }
    };
    let mut found = term.search_next(&mut regex, step(cursor), direction, Side::Left, None);
    // A match the cursor is already in is the one being left: search again
    // from beyond it. Once only, so a single match is still found.
    if let Some(current) = found.clone() {
        if current.contains(&cursor) {
            let beyond = if forward {
                step(*current.end())
            } else {
                step(*current.start())
            };
            found = term
                .search_next(&mut regex, beyond, direction, Side::Left, None)
                .or(Some(current));
        }
    }
    if let Some(found) = &found {
        term.vi_goto_point(*found.start());
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::TermSize;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::index::Line;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

    /// A 20×3 terminal whose output was `lines`, most of it now scrolled off.
    fn term_with(lines: &[&str]) -> Term<VoidListener> {
        let mut term = Term::new(Config::default(), &TermSize::new(20, 3), VoidListener);
        let mut processor: Processor<StdSyncHandler> = Processor::new();
        let text = lines.join("\r\n");
        processor.advance(&mut term, text.as_bytes());
        term
    }

    #[test]
    fn the_scrollback_is_reached_and_left() {
        let mut term = term_with(&["one", "two", "three", "four", "five"]);
        enter(&mut term);
        assert!(is_active(&term));
        to_edge(&mut term, true);
        assert_eq!(term.vi_mode_cursor.point.line, Line(-2));
        assert_eq!(term.grid().display_offset(), 2);

        leave(&mut term);
        assert!(!is_active(&term));
        assert_eq!(term.grid().display_offset(), 0);
    }

    #[test]
    fn a_selection_follows_the_cursor_and_is_copied() {
        let mut term = term_with(&["alpha beta", "gamma", "delta"]);
        enter(&mut term);
        to_edge(&mut term, true);
        toggle_selection(&mut term, Selecting::Characters);
        motion(&mut term, Motion::SemanticRightEnd, 1);
        assert_eq!(selected_text(&term).as_deref(), Some("alpha"));

        // Whole lines, from the same start.
        toggle_selection(&mut term, Selecting::Lines);
        motion(&mut term, Motion::Down, 1);
        assert_eq!(selected_text(&term).as_deref(), Some("alpha beta\ngamma\n"));

        // The same kind again stops selecting.
        toggle_selection(&mut term, Selecting::Lines);
        assert_eq!(selected_text(&term), None);
    }

    #[test]
    fn a_search_moves_the_cursor_to_the_match_and_wraps() {
        let mut term = term_with(&["error: one", "fine", "error: two", "last"]);
        enter(&mut term);
        to_edge(&mut term, true);
        let first = search(&mut term, "error", true).unwrap().unwrap();
        // Not the one under the cursor: the next.
        assert_eq!(first.start().line, Line(1));
        assert_eq!(term.vi_mode_cursor.point, *first.start());

        let wrapped = search(&mut term, "error", true).unwrap().unwrap();
        assert_eq!(wrapped.start().line, Line(-1));
        let back = search(&mut term, "error", false).unwrap().unwrap();
        assert_eq!(back.start().line, Line(1));

        assert_eq!(search(&mut term, "absent", true).unwrap(), None);
        assert!(search(&mut term, "(", true).is_err());
    }
}
