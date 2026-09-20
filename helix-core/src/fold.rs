//! Folded ranges: text the document still holds and the display leaves out.
//!
//! A fold is a half-open character range that the [document
//! formatter](crate::doc_formatter) skips, drawing a marker where it starts.
//! Nothing is removed: the characters stay in the rope, so every position past
//! a fold keeps the index it always had and an edit inside a fold is still an
//! edit to the document.
//!
//! Folds are kept sorted and non-overlapping. That is not a tidiness rule but
//! what makes them cheap: the formatter asks "does a fold start here" once per
//! grapheme, and a sorted list answers by binary search.

use std::ops::Range;

use crate::{Assoc, ChangeSet};

/// A stretch of text the display leaves out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Fold {
    /// First hidden character.
    pub start: usize,
    /// One past the last hidden character.
    pub end: usize,
}

impl Fold {
    pub fn new(start: usize, end: usize) -> Self {
        Fold {
            start: start.min(end),
            end: start.max(end),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }

    /// Whether `char_idx` is hidden by this fold.
    ///
    /// The first character is not: a fold starts where its marker is drawn, and
    /// the cursor has to be able to sit there.
    pub fn hides(&self, char_idx: usize) -> bool {
        (self.start..self.end).contains(&char_idx)
    }

    pub fn range(&self) -> Range<usize> {
        self.start..self.end
    }
}

/// The folds of one document, sorted by where they start and never overlapping.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Folds {
    folds: Vec<Fold>,
}

impl Folds {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.folds.is_empty()
    }

    pub fn len(&self) -> usize {
        self.folds.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Fold> {
        self.folds.iter()
    }

    pub fn as_slice(&self) -> &[Fold] {
        &self.folds
    }

    pub fn clear(&mut self) {
        self.folds.clear();
    }

    /// Adds a fold, dropping any existing one it would overlap.
    ///
    /// Folding a subtree that already has a folded child is ordinary, and the
    /// outer fold hides the inner one's marker anyway; keeping both would mean
    /// the inner fold reappearing on its own when the outer one opens, which is
    /// not what Org does. An empty fold is not a fold and is ignored.
    pub fn insert(&mut self, fold: Fold) -> bool {
        if fold.is_empty() {
            return false;
        }

        self.folds
            .retain(|other| other.end <= fold.start || other.start >= fold.end);

        let at = self.folds.partition_point(|other| other.start < fold.start);
        self.folds.insert(at, fold);
        true
    }

    /// The fold that begins exactly at `char_idx`, if one does.
    pub fn starting_at(&self, char_idx: usize) -> Option<Fold> {
        self.folds
            .binary_search_by_key(&char_idx, |fold| fold.start)
            .ok()
            .map(|at| self.folds[at])
    }

    /// The fold covering `char_idx`, whether it starts there or hides it.
    pub fn at(&self, char_idx: usize) -> Option<Fold> {
        let at = self.folds.partition_point(|fold| fold.start <= char_idx);
        self.folds
            .get(at.checked_sub(1)?)
            .copied()
            .filter(|fold| char_idx < fold.end)
    }

    /// Whether `char_idx` is inside a fold rather than at its marker.
    pub fn hidden(&self, char_idx: usize) -> bool {
        self.at(char_idx).is_some_and(|fold| fold.hides(char_idx))
    }

    /// Removes the fold covering `char_idx`, returning it.
    pub fn remove_at(&mut self, char_idx: usize) -> Option<Fold> {
        let fold = self.at(char_idx)?;
        let at = self.folds.iter().position(|other| *other == fold)?;
        Some(self.folds.remove(at))
    }

    /// Moves every fold across an edit.
    ///
    /// A fold's start is associated with the character before it and its end
    /// with the character after, so text typed at either edge lands outside the
    /// fold rather than disappearing into it. A fold whose content an edit
    /// removed is dropped: there is nothing left to hide, and keeping a
    /// zero-width fold would leave a marker standing for nothing.
    pub fn map(&mut self, changes: &ChangeSet) {
        for fold in &mut self.folds {
            fold.start = changes.map_pos(fold.start, Assoc::Before);
            fold.end = changes.map_pos(fold.end, Assoc::After);
        }

        self.folds.retain(|fold| !fold.is_empty());
        self.folds.sort_unstable();
        self.folds.dedup();
    }
}

impl FromIterator<Fold> for Folds {
    fn from_iter<T: IntoIterator<Item = Fold>>(iter: T) -> Self {
        let mut folds = Folds::new();
        for fold in iter {
            folds.insert(fold);
        }
        folds
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Rope, Transaction};

    fn folds(ranges: &[(usize, usize)]) -> Folds {
        ranges
            .iter()
            .map(|&(start, end)| Fold::new(start, end))
            .collect()
    }

    fn ranges(folds: &Folds) -> Vec<(usize, usize)> {
        folds.iter().map(|fold| (fold.start, fold.end)).collect()
    }

    #[test]
    fn folds_are_kept_in_order_however_they_are_added() {
        assert_eq!(
            ranges(&folds(&[(20, 30), (0, 10), (12, 15)])),
            [(0, 10), (12, 15), (20, 30)]
        );
    }

    #[test]
    fn an_empty_fold_is_not_a_fold() {
        let mut folds = Folds::new();
        assert!(!folds.insert(Fold::new(4, 4)));
        assert!(folds.is_empty());
    }

    #[test]
    fn a_new_fold_replaces_the_ones_it_swallows() {
        let mut folds = folds(&[(5, 8), (10, 14)]);
        // Folding a subtree over its already-folded children.
        folds.insert(Fold::new(4, 20));

        assert_eq!(ranges(&folds), [(4, 20)]);
    }

    #[test]
    fn a_fold_starts_where_its_marker_is_and_hides_what_follows() {
        let fold = Fold::new(5, 15);

        assert!(!fold.hides(4));
        // The first character is where the marker is drawn, so the cursor can
        // sit on it; everything after it is hidden.
        assert!(fold.hides(5));
        assert!(fold.hides(14));
        assert!(!fold.hides(15));
    }

    #[test]
    fn a_fold_is_found_by_its_start_and_by_anything_it_hides() {
        let folds = folds(&[(5, 15), (20, 30)]);

        assert_eq!(folds.starting_at(5), Some(Fold::new(5, 15)));
        assert_eq!(folds.starting_at(9), None);
        assert_eq!(folds.at(9), Some(Fold::new(5, 15)));
        assert_eq!(folds.at(15), None);
        assert_eq!(folds.at(25), Some(Fold::new(20, 30)));
        assert!(folds.hidden(9));
        assert!(!folds.hidden(15));
    }

    #[test]
    fn removing_takes_the_fold_covering_the_position() {
        let mut folds = folds(&[(5, 15), (20, 30)]);

        assert_eq!(folds.remove_at(9), Some(Fold::new(5, 15)));
        assert_eq!(ranges(&folds), [(20, 30)]);
        assert_eq!(folds.remove_at(9), None);
    }

    #[test]
    fn an_edit_before_a_fold_carries_it_along() {
        let text = Rope::from("* One\nbody\nmore\n* Two\n");
        let mut folds = folds(&[(5, 15)]);
        // Two characters inserted at the very start.
        folds.map(Transaction::insert(&text, &crate::Selection::point(0), "xy".into()).changes());

        assert_eq!(ranges(&folds), [(7, 17)]);
    }

    #[test]
    fn an_edit_inside_a_fold_changes_what_it_hides() {
        let text = Rope::from("* One\nbody\nmore\n* Two\n");
        let mut folds = folds(&[(5, 15)]);
        folds.map(Transaction::insert(&text, &crate::Selection::point(8), "zzz".into()).changes());

        assert_eq!(ranges(&folds), [(5, 18)]);
    }

    #[test]
    fn text_typed_at_a_fold_edge_lands_outside_it() {
        let text = Rope::from("* One\nbody\nmore\n* Two\n");
        let mut folds = folds(&[(5, 15)]);
        // At the start: typing on the headline must not vanish into the fold.
        folds.map(Transaction::insert(&text, &crate::Selection::point(5), "!".into()).changes());

        assert_eq!(ranges(&folds), [(5, 16)]);
    }

    #[test]
    fn a_fold_whose_text_was_deleted_goes_with_it() {
        let text = Rope::from("* One\nbody\nmore\n* Two\n");
        let mut folds = folds(&[(5, 15)]);
        folds.map(Transaction::delete(&text, [(5, 15)].into_iter()).changes());

        assert!(folds.is_empty());
    }
}
