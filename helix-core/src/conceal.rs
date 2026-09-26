//! Concealed ranges: text drawn as one other grapheme, such as Org's
//! `\alpha` drawn as `α`.
//!
//! A conceal is a fold that shows something: the [document
//! formatter](crate::doc_formatter) skips the range and draws its
//! replacement in its place, carrying the hidden characters so every position
//! past it keeps its index. Unlike folds, conceals never span a line break
//! and are not a property the user sets: whoever computes them recomputes
//! them when the text changes, and the view leaves out the ones on the
//! cursor's lines so the text being edited is the text in the document.

use std::ops::Range;

use crate::{Assoc, ChangeSet};

/// A range drawn as `replacement`, which is a single grapheme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conceal {
    pub start: usize,
    pub end: usize,
    pub replacement: String,
}

/// The conceals of one document, sorted and never overlapping.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Conceals {
    conceals: Vec<Conceal>,
}

impl Conceals {
    /// Builds the list from conceals in any order; empty ones, and any
    /// overlapping one already kept, are dropped.
    pub fn new(mut conceals: Vec<Conceal>) -> Self {
        conceals.sort_by_key(|conceal| conceal.start);
        let mut kept: Vec<Conceal> = Vec::with_capacity(conceals.len());
        for conceal in conceals {
            if conceal.start >= conceal.end || conceal.replacement.is_empty() {
                continue;
            }
            if kept.last().is_some_and(|last| last.end > conceal.start) {
                continue;
            }
            kept.push(conceal);
        }
        Self { conceals: kept }
    }

    pub fn is_empty(&self) -> bool {
        self.conceals.is_empty()
    }

    pub fn len(&self) -> usize {
        self.conceals.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Conceal> {
        self.conceals.iter()
    }

    pub fn clear(&mut self) {
        self.conceals.clear();
    }

    /// The conceal beginning exactly at `char_idx`.
    pub fn starting_at(&self, char_idx: usize) -> Option<&Conceal> {
        self.conceals
            .binary_search_by_key(&char_idx, |conceal| conceal.start)
            .ok()
            .map(|index| &self.conceals[index])
    }

    /// Moves the conceals with an edit. One whose text the edit touched is
    /// dropped: it would draw a replacement for text that is no longer
    /// there, until whoever computes them does so again.
    pub fn map(&mut self, changes: &ChangeSet) {
        if changes.is_empty() {
            return;
        }
        self.conceals.retain_mut(|conceal| {
            let start = changes.map_pos(conceal.start, Assoc::After);
            let end = changes.map_pos(conceal.end, Assoc::Before);
            if end < start || end - start != conceal.end - conceal.start {
                return false;
            }
            conceal.start = start;
            conceal.end = end;
            true
        });
    }
}

/// Whether `char_idx` lies in one of `revealed`, sorted ranges of text shown
/// as it is.
pub fn is_revealed(revealed: &[Range<usize>], char_idx: usize) -> bool {
    revealed.iter().any(|range| range.contains(&char_idx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Rope, Transaction};

    fn conceal(start: usize, end: usize, replacement: &str) -> Conceal {
        Conceal {
            start,
            end,
            replacement: replacement.to_string(),
        }
    }

    #[test]
    fn conceals_are_sorted_and_never_overlap() {
        let conceals = Conceals::new(vec![
            conceal(10, 16, "β"),
            conceal(0, 6, "α"),
            conceal(4, 8, "x"),
            conceal(20, 20, "y"),
        ]);
        let starts: Vec<usize> = conceals.iter().map(|c| c.start).collect();
        assert_eq!(starts, [0, 10]);
        assert_eq!(conceals.starting_at(10).unwrap().replacement, "β");
        assert!(conceals.starting_at(4).is_none());
    }

    #[test]
    fn an_edit_moves_conceals_and_drops_the_ones_it_touches() {
        let text = Rope::from("\\alpha and \\beta\n");
        let mut conceals = Conceals::new(vec![conceal(0, 6, "α"), conceal(11, 16, "β")]);
        // Typing before both moves them.
        let insert = Transaction::insert(&text, &crate::Selection::point(0), "x".into());
        conceals.map(insert.changes());
        assert_eq!(
            conceals.iter().map(|c| c.start).collect::<Vec<_>>(),
            [1, 12]
        );

        // Deleting inside `\beta` drops that one.
        let delete = Transaction::change(
            &Rope::from("x\\alpha and \\beta\n"),
            [(14, 15, None)].into_iter(),
        );
        conceals.map(delete.changes());
        assert_eq!(conceals.len(), 1);
        assert_eq!(conceals.iter().next().unwrap().replacement, "α");
    }
}
