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

use crate::syntax::Loader;
use crate::{Assoc, ChangeSet, RopeSlice, Syntax};

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

    /// The fold the cursor's line owns: one covering `char_idx`, or one whose
    /// marker sits on the same line.
    ///
    /// A fold is asked for from the headline above it, which is *before* the
    /// first hidden character, so it has to be findable from there. Matching
    /// only the exact position would let a fold be closed from a line and
    /// never opened from it again.
    pub fn on_line(&self, text: RopeSlice, char_idx: usize) -> Option<Fold> {
        if let Some(fold) = self.at(char_idx) {
            return Some(fold);
        }

        let line = text.char_to_line(char_idx.min(text.len_chars()));
        let from = text.line_to_char(line);
        let to = text.line_to_char((line + 1).min(text.len_lines()));

        self.folds
            .iter()
            .copied()
            .find(|fold| (from..to).contains(&fold.start))
    }

    /// Removes the fold the cursor's line owns, returning it.
    pub fn remove_on_line(&mut self, text: RopeSlice, char_idx: usize) -> Option<Fold> {
        let fold = self.on_line(text, char_idx)?;
        let at = self.folds.iter().position(|other| *other == fold)?;
        Some(self.folds.remove(at))
    }

    /// Removes the fold covering `char_idx`, returning it.
    pub fn remove_at(&mut self, char_idx: usize) -> Option<Fold> {
        let fold = self.at(char_idx)?;
        let at = self.folds.iter().position(|other| *other == fold)?;
        Some(self.folds.remove(at))
    }

    /// Opens the fold hiding `char_idx`, if one does, and returns it.
    ///
    /// A position on a fold's marker is not hidden — that is where folding
    /// leaves the cursor — so only a position strictly inside opens anything.
    pub fn reveal(&mut self, char_idx: usize) -> Option<Fold> {
        self.at(char_idx).filter(|fold| char_idx > fold.start)?;
        self.remove_at(char_idx)
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

/// Turns a syntax node's byte range into the fold that hides it.
///
/// The node's first line stays visible and the newline that ends the node
/// stays out of the fold. Both matter: a folded section whose headline
/// vanished would be unopenable, and a fold that swallowed its final newline
/// would pull the line after it up onto the marker.
fn fold_for(text: RopeSlice, start_byte: usize, end_byte: usize) -> Option<Fold> {
    let last = text.len_chars();
    let start = text.byte_to_char(start_byte.min(text.len_bytes()));
    let end = text.byte_to_char(end_byte.min(text.len_bytes())).min(last);

    let first_line = text.char_to_line(start);
    if first_line + 1 >= text.len_lines() {
        return None;
    }
    // The newline ending the first line, which is where the marker goes.
    let from = text.line_to_char(first_line + 1) - 1;

    let to = if end > from && text.char(end - 1) == '\n' {
        end - 1
    } else {
        end
    };

    (from < to).then(|| Fold::new(from, to))
}

/// The text of `range` without what `folds` hide: what the screen shows,
/// markers aside.
///
/// A fold hides from the newline ending its first line to the newline
/// ending its last, so cutting the hidden spans out leaves whole lines — a
/// folded outline copies as the outline it looks like.
pub fn visible_text(text: RopeSlice, range: Range<usize>, folds: &Folds) -> String {
    let mut out = String::new();
    let mut at = range.start;
    for fold in folds.iter() {
        if fold.end <= at || fold.start >= range.end {
            continue;
        }
        if fold.start > at {
            out.extend(text.slice(at..fold.start).chunks());
        }
        at = at.max(fold.end);
    }
    if at < range.end {
        out.extend(text.slice(at..range.end).chunks());
    }
    out
}

/// The outermost of `folds`, dropping every fold another one contains.
///
/// What "fold everything" means is folding the file to its top level; folding
/// it to its leaves would hide almost nothing, since a leaf's marker sits
/// inside its parent anyway.
pub fn outermost(folds: impl IntoIterator<Item = Fold>) -> Vec<Fold> {
    let mut sorted: Vec<Fold> = folds.into_iter().collect();
    sorted.sort_unstable_by_key(|fold| (fold.start, std::cmp::Reverse(fold.end)));

    let mut kept: Vec<Fold> = Vec::new();
    for fold in sorted {
        if kept.last().is_none_or(|outer| fold.start >= outer.end) {
            kept.push(fold);
        }
    }
    kept
}

/// Every range the language's `folds.scm` marks, outermost first.
///
/// A language with no `folds.scm`, or one whose grammar is not built, simply
/// has nothing to fold; that is not an error and not worth a message.
pub fn foldable(text: RopeSlice, syntax: &Syntax, loader: &Loader) -> Vec<Fold> {
    let layer = syntax.layer(syntax.root_layer());
    let Some(query) = loader.fold_query(layer.language) else {
        return Vec::new();
    };
    let root = syntax.tree().root_node();
    let Some(nodes) = query.capture_nodes("fold", &root, text) else {
        return Vec::new();
    };

    let mut folds: Vec<Fold> = nodes
        .flat_map(|node| {
            let range = node.byte_range();
            fold_for(text, range.start, range.end)
        })
        .collect();

    folds.sort_unstable();
    folds.dedup();
    folds
}

/// The smallest foldable range whose marker would sit at or after `char_idx`'s
/// line, and which covers the cursor.
///
/// Smallest rather than outermost, so folding twice on a headline folds the
/// section and not the file.
pub fn foldable_at(
    text: RopeSlice,
    syntax: &Syntax,
    loader: &Loader,
    char_idx: usize,
) -> Option<Fold> {
    let line = text.char_to_line(char_idx.min(text.len_chars()));

    foldable(text, syntax, loader)
        .into_iter()
        .filter(|fold| {
            // The cursor is inside the range, counting the line the marker
            // would sit on: folding is asked for from the headline, which is
            // just before the first hidden character.
            text.char_to_line(fold.start) <= line && line < text.char_to_line(fold.end) + 1
        })
        .min_by_key(|fold| fold.end - fold.start)
}

/// How much of a foldable range is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cycle {
    /// Everything below its first line is hidden.
    Folded,
    /// The ranges directly inside it show their first lines; the rest is hidden.
    Children,
    /// Nothing inside it is hidden.
    Open,
}

/// How much of a whole buffer is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    /// Only the outermost first lines.
    Overview,
    /// Every first line, and no bodies.
    Contents,
    /// Everything.
    ShowAll,
}

/// The foldable ranges directly inside `parent`.
///
/// Direct means contained by `parent` and by nothing else `parent` contains,
/// which is what makes a list of ranges into a tree without building one.
pub fn direct_children(all: &[Fold], parent: Fold) -> Vec<Fold> {
    let inside: Vec<Fold> = all
        .iter()
        .copied()
        .filter(|fold| *fold != parent && fold.start >= parent.start && fold.end <= parent.end)
        .collect();

    inside
        .iter()
        .copied()
        .filter(|fold| {
            !inside
                .iter()
                .any(|other| other != fold && other.start <= fold.start && fold.end <= other.end)
        })
        .collect()
}

/// The text between a range's first line and the first range inside it.
///
/// This is the body: an Org headline's paragraphs, everything written before
/// the first subsection. Showing "just the children" has to hide it, or the
/// entry claims to be showing only its children while showing its prose.
fn body(text: RopeSlice, parent: Fold, first_child: Fold) -> Option<Fold> {
    let head = text.line_to_char(text.char_to_line(first_child.start));
    (head > parent.start + 1).then(|| Fold::new(parent.start, head - 1))
}

/// The folds that leave `parent` showing its children and nothing else.
///
/// A range with nothing inside it has no children to show, so it simply
/// closes: that is why a leaf headline has two states rather than three.
pub fn children_folds(text: RopeSlice, all: &[Fold], parent: Fold) -> Vec<Fold> {
    let children = direct_children(all, parent);
    let Some(first) = children.first().copied() else {
        return vec![parent];
    };

    body(text, parent, first)
        .into_iter()
        .chain(children)
        .collect()
}

/// Which of the three states the current folds put `parent` in.
///
/// Derived rather than remembered: a stored cycle position goes stale the
/// moment an edit or another command changes the folds under it.
pub fn cycle_state(folds: &Folds, text: RopeSlice, all: &[Fold], parent: Fold) -> Cycle {
    if folds.iter().any(|fold| *fold == parent) {
        return Cycle::Folded;
    }

    let wanted = children_folds(text, all, parent);
    if wanted
        .iter()
        .all(|fold| folds.iter().any(|other| other == fold))
    {
        return Cycle::Children;
    }

    Cycle::Open
}

/// The folds that hide every body while leaving every first line showing.
pub fn contents_folds(text: RopeSlice, all: &[Fold]) -> Vec<Fold> {
    all.iter()
        .filter_map(|parent| match direct_children(all, *parent).first() {
            // Nothing inside it, so the whole range is body.
            None => Some(*parent),
            Some(first) => body(text, *parent, *first),
        })
        .collect()
}

/// Which of the three whole-buffer states the current folds amount to.
pub fn visibility(folds: &Folds, all: &[Fold]) -> Visibility {
    if folds.is_empty() {
        return Visibility::ShowAll;
    }

    let overview = outermost(all.iter().copied());
    if folds.as_slice() == overview.as_slice() {
        return Visibility::Overview;
    }

    Visibility::Contents
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
    fn visible_text_leaves_out_what_is_folded() {
        let text = Rope::from("* A\nbody a\n* B\nbody b\n* C\n");
        // Fold A's and B's bodies: from each headline's newline to the
        // newline ending the body.
        let folded = folds(&[(3, 10), (14, 21)]);
        let all = 0..text.len_chars();
        assert_eq!(
            visible_text(text.slice(..), all, &folded),
            "* A\n* B\n* C\n"
        );
        // A range starting inside a fold starts where the fold ends.
        assert_eq!(visible_text(text.slice(..), 5..17, &folded), "\n* B");
        assert_eq!(visible_text(text.slice(..), 0..3, &Folds::new()), "* A");
    }

    #[test]
    fn revealing_opens_only_a_fold_that_hides_the_position() {
        let mut folded = folds(&[(10, 20), (30, 40)]);
        assert_eq!(folded.reveal(10), None, "the marker is not hidden");
        assert_eq!(folded.reveal(25), None);
        assert_eq!(folded.reveal(15), Some(Fold::new(10, 20)));
        assert_eq!(ranges(&folded), [(30, 40)]);
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
    fn a_fold_is_found_from_the_line_it_was_asked_for() {
        let text = Rope::from("* One\nbody\nmore\n* Two\n");
        let folds = folds(&[(5, 15)]);

        // The cursor sits on the headline, before the first hidden character.
        // Matching only the exact position would let a fold be closed from a
        // line and never opened from it again.
        assert_eq!(folds.at(0), None);
        assert_eq!(folds.on_line(text.slice(..), 0), Some(Fold::new(5, 15)));
        assert_eq!(folds.on_line(text.slice(..), 3), Some(Fold::new(5, 15)));
        assert_eq!(folds.on_line(text.slice(..), 16), None);
    }

    #[test]
    fn removing_from_the_headline_opens_the_fold_below_it() {
        let text = Rope::from("* One\nbody\nmore\n* Two\n");
        let mut folds = folds(&[(5, 15)]);

        assert_eq!(
            folds.remove_on_line(text.slice(..), 0),
            Some(Fold::new(5, 15))
        );
        assert!(folds.is_empty());
    }

    #[test]
    fn folding_everything_means_the_top_level_not_the_leaves() {
        // A section and the subsection inside it: keeping both would fold the
        // file to its leaves, and the inner marker is hidden by the outer fold
        // anyway.
        let all = [Fold::new(5, 40), Fold::new(20, 35), Fold::new(50, 60)];

        assert_eq!(outermost(all), [Fold::new(5, 40), Fold::new(50, 60)]);
    }

    /// `* One` / body / `** Two` / body / `* Three`, with the folds a
    /// `folds.scm` would give for the two sections and the subsection.
    fn outline() -> (Rope, Vec<Fold>) {
        let text = Rope::from("* One\nbody\nmore\n** Two\ndeep\n* Three\n");
        // "* One" ends at char 5 and its section runs to the newline at 27,
        // just before "* Three"; "** Two" ends at 22 and ends in the same
        // place.
        let one = Fold::new(5, 27);
        let two = Fold::new(22, 27);
        (text, vec![one, two])
    }

    #[test]
    fn a_range_inside_another_is_its_child_and_not_its_grandparent_s() {
        let all = [Fold::new(0, 100), Fold::new(10, 60), Fold::new(20, 30)];

        assert_eq!(
            direct_children(&all, Fold::new(0, 100)),
            [Fold::new(10, 60)]
        );
        assert_eq!(
            direct_children(&all, Fold::new(10, 60)),
            [Fold::new(20, 30)]
        );
        assert!(direct_children(&all, Fold::new(20, 30)).is_empty());
    }

    #[test]
    fn showing_the_children_hides_the_body_with_them() {
        let (text, all) = outline();
        let folds = children_folds(text.slice(..), &all, all[0]);

        // The body between the headline and the first subsection, and the
        // subsection itself. Showing the prose while claiming to show only
        // the children is the mistake this guards.
        assert_eq!(folds, [Fold::new(5, 15), Fold::new(22, 27)]);
    }

    #[test]
    fn a_range_with_nothing_inside_it_has_two_states_not_three() {
        let (text, all) = outline();

        // A leaf has no children to show, so asking for them closes it.
        assert_eq!(children_folds(text.slice(..), &all, all[1]), [all[1]]);
    }

    #[test]
    fn the_cycle_state_is_read_back_from_the_folds() {
        let (text, all) = outline();
        let parent = all[0];

        let open = Folds::new();
        assert_eq!(
            cycle_state(&open, text.slice(..), &all, parent),
            Cycle::Open
        );

        let closed: Folds = [parent].into_iter().collect();
        assert_eq!(
            cycle_state(&closed, text.slice(..), &all, parent),
            Cycle::Folded
        );

        let children: Folds = children_folds(text.slice(..), &all, parent)
            .into_iter()
            .collect();
        assert_eq!(
            cycle_state(&children, text.slice(..), &all, parent),
            Cycle::Children
        );
    }

    #[test]
    fn the_contents_view_keeps_every_first_line_and_no_body() {
        let (text, all) = outline();

        // The outer section loses its body up to the subsection; the
        // subsection has no children, so all of it goes.
        assert_eq!(
            contents_folds(text.slice(..), &all),
            [Fold::new(5, 15), Fold::new(22, 27)]
        );
    }

    #[test]
    fn the_buffer_state_is_read_back_too() {
        let (text, all) = outline();

        assert_eq!(visibility(&Folds::new(), &all), Visibility::ShowAll);

        let overview: Folds = outermost(all.iter().copied()).into_iter().collect();
        assert_eq!(visibility(&overview, &all), Visibility::Overview);

        let contents: Folds = contents_folds(text.slice(..), &all).into_iter().collect();
        assert_eq!(visibility(&contents, &all), Visibility::Contents);
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
