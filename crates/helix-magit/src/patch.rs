//! Building and applying partial patches.
//!
//! Staging part of a change means answering two questions: what patch do the
//! selected lines make, and what does the file look like once it is applied.
//! Both are pure functions over [`FileDiff`], so the interesting logic is
//! testable without a repository.

use crate::diff::{DiffHunk, DiffLine, DiffLineKind, FileDiff, HunkHeader};

/// What part of a file's diff an operation covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// Every hunk of the file.
    File,
    /// One whole hunk, by its index in [`FileDiff::hunks`].
    Hunk(usize),
    /// Individual lines of one hunk, by their indices in [`DiffHunk::lines`].
    Lines { hunk: usize, lines: Vec<usize> },
}

impl Selection {
    /// Whether the line at `line_index` of hunk `hunk_index` is covered.
    pub fn covers(&self, hunk_index: usize, line_index: usize) -> bool {
        match self {
            Selection::File => true,
            Selection::Hunk(hunk) => *hunk == hunk_index,
            Selection::Lines { hunk, lines } => *hunk == hunk_index && lines.contains(&line_index),
        }
    }
}

/// Builds the patch made of only the selected changes.
///
/// Returns `None` when the selection covers no changed line, which is what
/// happens if the user asks to stage a context line.
///
/// The rewriting rule is the one Magit and `git add -p` use:
///
/// * a selected change stays as it is,
/// * an **unselected addition is dropped** — it is in neither the old file nor
///   the partially staged one,
/// * an **unselected deletion becomes context** — it is in the old file and
///   survives into the partially staged one.
///
/// Counts are then recomputed from the rewritten lines, and each hunk's
/// `new_start` is shifted by the net change of the hunks before it that this
/// patch also carries, because they are all applied to the same old file.
pub fn build_partial_patch(file: &FileDiff, selection: &Selection) -> Option<FileDiff> {
    let mut hunks = Vec::new();
    let mut offset: i64 = 0;

    for (hunk_index, hunk) in file.hunks.iter().enumerate() {
        let selects_a_change = hunk.lines.iter().enumerate().any(|(index, line)| {
            line.kind != DiffLineKind::Context && selection.covers(hunk_index, index)
        });
        if !selects_a_change {
            continue;
        }

        let mut lines: Vec<DiffLine> = Vec::with_capacity(hunk.lines.len());
        for (line_index, line) in hunk.lines.iter().enumerate() {
            let selected = selection.covers(hunk_index, line_index);
            match line.kind {
                DiffLineKind::Context => lines.push(line.clone()),
                DiffLineKind::Addition if selected => lines.push(line.clone()),
                DiffLineKind::Addition => continue,
                DiffLineKind::Deletion if selected => lines.push(line.clone()),
                DiffLineKind::Deletion => lines.push(DiffLine {
                    kind: DiffLineKind::Context,
                    ..line.clone()
                }),
            }
        }

        let old_count = lines
            .iter()
            .filter(|line| line.kind != DiffLineKind::Addition)
            .count() as u32;
        let new_count = lines
            .iter()
            .filter(|line| line.kind != DiffLineKind::Deletion)
            .count() as u32;

        let old_start = hunk.header.old_start;
        let new_start = (old_start as i64 + offset).max(0) as u32;
        offset += new_count as i64 - old_count as i64;

        renumber(&mut lines, old_start, new_start);

        hunks.push(DiffHunk {
            header: HunkHeader {
                old_start,
                old_count,
                new_start,
                new_count,
                section: hunk.header.section.clone(),
            },
            lines,
            folded: hunk.folded,
        });
    }

    if hunks.is_empty() {
        return None;
    }

    Some(FileDiff {
        hunks,
        ..file.clone()
    })
}

/// Rewrites each line's old/new numbers to match the rebuilt hunk.
fn renumber(lines: &mut [DiffLine], old_start: u32, new_start: u32) {
    let (mut old, mut new) = (old_start, new_start);
    for line in lines {
        match line.kind {
            DiffLineKind::Context => {
                line.old_line = Some(old);
                line.new_line = Some(new);
                old += 1;
                new += 1;
            }
            DiffLineKind::Addition => {
                line.old_line = None;
                line.new_line = Some(new);
                new += 1;
            }
            DiffLineKind::Deletion => {
                line.old_line = Some(old);
                line.new_line = None;
                old += 1;
            }
        }
    }
}

/// Why a patch could not be applied.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApplyError {
    #[error("hunk at line {line} does not match the file: expected {expected:?}, found {found:?}")]
    ContextMismatch {
        line: u32,
        expected: String,
        found: String,
    },
    #[error("hunk at line {0} starts before the previous hunk ended")]
    OverlappingHunks(u32),
    #[error("hunk at line {0} is past the end of the file")]
    PastEndOfFile(u32),
}

/// Applies `patch` to `content`, returning the resulting file.
///
/// With `reverse`, the patch is undone instead — additions are removed and
/// deletions restored — which is how unstaging works: the same patch, applied
/// backwards to the index.
///
/// Every context and removed line is checked against the file before it is
/// consumed. A patch built from a stale diff is rejected rather than silently
/// corrupting the file it is applied to.
pub fn apply_patch(content: &str, patch: &FileDiff, reverse: bool) -> Result<String, ApplyError> {
    let (source, ends_with_newline) = split_lines(content);
    let mut out: Vec<&str> = Vec::with_capacity(source.len());
    let mut cursor = 0usize;
    let mut last_no_newline = None;

    for hunk in &patch.hunks {
        let start_line = if reverse {
            hunk.header.new_start
        } else {
            hunk.header.old_start
        };
        // A hunk covering no source lines (a pure insertion into an empty
        // file) is anchored at 0 rather than 1.
        let start = start_line.saturating_sub(1) as usize;

        if start < cursor {
            return Err(ApplyError::OverlappingHunks(start_line));
        }
        if start > source.len() {
            return Err(ApplyError::PastEndOfFile(start_line));
        }

        out.extend_from_slice(&source[cursor..start]);
        cursor = start;

        for line in &hunk.lines {
            match effective_kind(line.kind, reverse) {
                DiffLineKind::Context | DiffLineKind::Deletion => {
                    let found = source
                        .get(cursor)
                        .copied()
                        .ok_or(ApplyError::PastEndOfFile(cursor as u32 + 1))?;
                    if found != line.content {
                        return Err(ApplyError::ContextMismatch {
                            line: cursor as u32 + 1,
                            expected: line.content.clone(),
                            found: found.to_string(),
                        });
                    }
                    cursor += 1;

                    if effective_kind(line.kind, reverse) == DiffLineKind::Context {
                        out.push(&line.content);
                        last_no_newline = Some(line.no_newline);
                    }
                }
                DiffLineKind::Addition => {
                    out.push(&line.content);
                    last_no_newline = Some(line.no_newline);
                }
            }
        }
    }

    let tail_is_untouched = cursor < source.len();
    out.extend_from_slice(&source[cursor..]);

    let mut result = out.join("\n");
    // The file keeps its own trailing newline unless the patch ended it, and
    // the patch only has a say when it reaches the end of the file.
    let trailing = match last_no_newline {
        Some(no_newline) if !tail_is_untouched => !no_newline,
        _ => ends_with_newline,
    };
    if trailing && !result.is_empty() {
        result.push('\n');
    }

    Ok(result)
}

/// A reversed patch swaps which side each changed line belongs to.
fn effective_kind(kind: DiffLineKind, reverse: bool) -> DiffLineKind {
    match (kind, reverse) {
        (DiffLineKind::Addition, true) => DiffLineKind::Deletion,
        (DiffLineKind::Deletion, true) => DiffLineKind::Addition,
        (kind, _) => kind,
    }
}

/// Splits into lines without their terminators, plus whether the last one had
/// a trailing newline.
fn split_lines(content: &str) -> (Vec<&str>, bool) {
    if content.is_empty() {
        return (Vec::new(), false);
    }
    match content.strip_suffix('\n') {
        Some(body) => (body.split('\n').collect(), true),
        None => (content.split('\n').collect(), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::FileStatus;
    use crate::repository::unified_diff;

    /// Builds a real diff between two texts, the way the status view does.
    fn diff(old: &str, new: &str) -> FileDiff {
        let body = unified_diff(old, new);
        let text = format!("diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n{body}");
        crate::parse_unified_diff(&text)
            .into_iter()
            .next()
            .expect("a diff for one file")
    }

    const OLD: &str = "one\ntwo\nthree\nfour\nfive\n";
    const NEW: &str = "one\nTWO\nthree\nfour\nFIVE\nsix\n";

    #[test]
    fn applying_the_whole_diff_reproduces_the_new_file() {
        let file = diff(OLD, NEW);
        assert_eq!(apply_patch(OLD, &file, false).unwrap(), NEW);
    }

    #[test]
    fn reversing_the_whole_diff_reproduces_the_old_file() {
        let file = diff(OLD, NEW);
        assert_eq!(apply_patch(NEW, &file, true).unwrap(), OLD);
    }

    #[test]
    fn selecting_the_whole_file_changes_nothing_about_the_patch() {
        let file = diff(OLD, NEW);
        let patch = build_partial_patch(&file, &Selection::File).unwrap();
        assert_eq!(apply_patch(OLD, &patch, false).unwrap(), NEW);
    }

    #[test]
    fn staging_one_hunk_leaves_the_other_alone() {
        // Edits must be more than two context lines apart to stay separate.
        let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\n";
        let new = "A\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nL\n";
        let file = diff(old, new);
        assert_eq!(file.hunks.len(), 2, "the edits are far enough apart");

        let patch = build_partial_patch(&file, &Selection::Hunk(0)).unwrap();
        let staged = apply_patch(old, &patch, false).unwrap();

        // Only the first edit landed.
        assert_eq!(staged, "A\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\n");
    }

    #[test]
    fn an_unselected_addition_is_dropped() {
        // Two additions in one hunk; stage only the first.
        let old = "a\nb\n";
        let new = "a\nX\nY\nb\n";
        let file = diff(old, new);

        let additions: Vec<usize> = file.hunks[0]
            .lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.kind == DiffLineKind::Addition)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(additions.len(), 2);

        let patch = build_partial_patch(
            &file,
            &Selection::Lines {
                hunk: 0,
                lines: vec![additions[0]],
            },
        )
        .unwrap();

        assert_eq!(apply_patch(old, &patch, false).unwrap(), "a\nX\nb\n");
    }

    #[test]
    fn an_unselected_deletion_becomes_context() {
        // Two deletions in one hunk; stage only the second.
        let old = "a\nX\nY\nb\n";
        let new = "a\nb\n";
        let file = diff(old, new);

        let deletions: Vec<usize> = file.hunks[0]
            .lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.kind == DiffLineKind::Deletion)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(deletions.len(), 2);

        let patch = build_partial_patch(
            &file,
            &Selection::Lines {
                hunk: 0,
                lines: vec![deletions[1]],
            },
        )
        .unwrap();

        // The unselected deletion survives as context, so only Y is removed.
        assert_eq!(apply_patch(old, &patch, false).unwrap(), "a\nX\nb\n");
    }

    #[test]
    fn the_rebuilt_header_counts_the_rewritten_lines() {
        let old = "a\nb\n";
        let new = "a\nX\nY\nb\n";
        let file = diff(old, new);
        let additions: Vec<usize> = file.hunks[0]
            .lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.kind == DiffLineKind::Addition)
            .map(|(index, _)| index)
            .collect();

        let patch = build_partial_patch(
            &file,
            &Selection::Lines {
                hunk: 0,
                lines: vec![additions[0]],
            },
        )
        .unwrap();

        let header = &patch.hunks[0].header;
        // Old side unchanged; new side gained exactly one line.
        assert_eq!(header.old_count + 1, header.new_count);
        assert_eq!(
            header.old_start, header.new_start,
            "first hunk is unshifted"
        );
    }

    #[test]
    fn later_hunks_are_shifted_by_the_earlier_ones_in_the_same_patch() {
        // The first hunk adds two lines, so the second hunk's new side starts
        // two lines further down in the partially staged file.
        let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nn\no\np\n";
        let new = "a\nX\nY\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nn\nZ\no\np\n";
        let file = diff(old, new);
        assert_eq!(file.hunks.len(), 2, "the edits are far apart");

        let patch = build_partial_patch(&file, &Selection::File).unwrap();
        assert_eq!(
            patch.hunks[1].header.new_start as i64 - patch.hunks[1].header.old_start as i64,
            2,
            "the second hunk is shifted by the first hunk's net gain"
        );
        assert_eq!(apply_patch(old, &patch, false).unwrap(), new);
    }

    #[test]
    fn staging_only_the_second_hunk_leaves_it_unshifted() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nn\no\np\n";
        let new = "a\nX\nY\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nn\nZ\no\np\n";
        let file = diff(old, new);

        let patch = build_partial_patch(&file, &Selection::Hunk(1)).unwrap();
        assert_eq!(
            patch.hunks[0].header.new_start, patch.hunks[0].header.old_start,
            "nothing precedes it in this patch"
        );
        assert_eq!(
            apply_patch(old, &patch, false).unwrap(),
            "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nn\nZ\no\np\n"
        );
    }

    #[test]
    fn selecting_only_context_produces_no_patch() {
        let file = diff(OLD, NEW);
        let context: Vec<usize> = file.hunks[0]
            .lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.kind == DiffLineKind::Context)
            .map(|(index, _)| index)
            .collect();

        assert!(build_partial_patch(
            &file,
            &Selection::Lines {
                hunk: 0,
                lines: context,
            }
        )
        .is_none());
    }

    #[test]
    fn a_partial_patch_can_be_unstaged_again() {
        // Stage one hunk, then reverse exactly that patch off the result.
        let file = diff(OLD, NEW);
        let patch = build_partial_patch(&file, &Selection::Hunk(0)).unwrap();

        let staged = apply_patch(OLD, &patch, false).unwrap();
        assert_eq!(apply_patch(&staged, &patch, true).unwrap(), OLD);
    }

    #[test]
    fn a_stale_patch_is_rejected_rather_than_corrupting_the_file() {
        let file = diff(OLD, NEW);
        // The file changed underneath: the context no longer matches.
        let drifted = "one\nCHANGED\nthree\nfour\nfive\n";

        let err = apply_patch(drifted, &file, false).unwrap_err();
        assert!(
            matches!(err, ApplyError::ContextMismatch { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn a_hunk_past_the_end_of_the_file_is_rejected() {
        let file = diff(OLD, NEW);
        let err = apply_patch("short\n", &file, false).unwrap_err();
        assert!(
            matches!(
                err,
                ApplyError::PastEndOfFile(_) | ApplyError::ContextMismatch { .. }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn a_file_created_from_nothing_applies() {
        let file = diff("", "hello\nworld\n");
        assert_eq!(apply_patch("", &file, false).unwrap(), "hello\nworld\n");
        assert_eq!(apply_patch("hello\nworld\n", &file, true).unwrap(), "");
    }

    #[test]
    fn a_missing_trailing_newline_survives_a_round_trip() {
        let old = "a\nb\n";
        let new = "a\nb";
        let file = diff(old, new);

        assert_eq!(apply_patch(old, &file, false).unwrap(), new);
        assert_eq!(apply_patch(new, &file, true).unwrap(), old);
    }

    #[test]
    fn selection_covers_matches_the_requested_scope() {
        assert!(Selection::File.covers(3, 7));
        assert!(Selection::Hunk(2).covers(2, 0));
        assert!(!Selection::Hunk(2).covers(3, 0));

        let lines = Selection::Lines {
            hunk: 1,
            lines: vec![4, 5],
        };
        assert!(lines.covers(1, 4));
        assert!(!lines.covers(1, 6));
        assert!(!lines.covers(0, 4));
    }

    #[test]
    fn the_patch_keeps_the_file_path_and_status() {
        let mut file = diff(OLD, NEW);
        file.status = FileStatus::Modified;
        let patch = build_partial_patch(&file, &Selection::Hunk(0)).unwrap();

        assert_eq!(patch.path, file.path);
        assert_eq!(patch.status, file.status);
    }
}
