//! Staging and unstaging against real repositories.
//!
//! The index is read back with `git` itself, so these assert what git sees,
//! not what this crate believes it wrote.

use std::fs;
use std::path::Path;
use std::process::Command;

use helix_magit::{Repository, Selection};

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The content of a path as git records it in the index.
fn index_content(dir: &Path, path: &str) -> String {
    git(dir, &["show", &format!(":{path}")]).unwrap_or_default()
}

fn fixture(initial: &str) -> Option<tempfile::TempDir> {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "--initial-branch=main"])?;
    git(dir.path(), &["config", "user.email", "t@e.invalid"])?;
    git(dir.path(), &["config", "user.name", "T"])?;

    fs::write(dir.path().join("f.txt"), initial).unwrap();
    git(dir.path(), &["add", "."])?;
    git(dir.path(), &["commit", "-m", "initial"])?;
    Some(dir)
}

macro_rules! fixture_or_skip {
    ($initial:expr) => {
        match fixture($initial) {
            Some(dir) => dir,
            None => {
                eprintln!("skipping: no usable git binary");
                return;
            }
        }
    };
}

const FAR_APART_OLD: &str = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\n";
const FAR_APART_NEW: &str = "A\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nL\n";

#[test]
fn staging_a_whole_file_matches_git_add() {
    let dir = fixture_or_skip!("one\ntwo\nthree\n");
    fs::write(dir.path().join("f.txt"), "one\nTWO\nthree\n").unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let diffs = repo.worktree_diff().unwrap();
    repo.stage(&diffs[0], &Selection::File).unwrap();

    assert_eq!(index_content(dir.path(), "f.txt"), "one\nTWO\nthree\n");
    // Nothing is left unstaged, exactly as after `git add`.
    assert_eq!(
        git(dir.path(), &["diff", "--name-only"]).unwrap().trim(),
        ""
    );
    assert_eq!(
        git(dir.path(), &["diff", "--cached", "--name-only"])
            .unwrap()
            .trim(),
        "f.txt"
    );
}

#[test]
fn staging_one_hunk_leaves_the_rest_unstaged() {
    let dir = fixture_or_skip!(FAR_APART_OLD);
    fs::write(dir.path().join("f.txt"), FAR_APART_NEW).unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let diffs = repo.worktree_diff().unwrap();
    assert_eq!(diffs[0].hunks.len(), 2);

    repo.stage(&diffs[0], &Selection::Hunk(0)).unwrap();

    // Only the first edit reached the index.
    assert_eq!(
        index_content(dir.path(), "f.txt"),
        "A\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\n"
    );
    // The second edit is still pending, and git agrees the file is both
    // staged and unstaged.
    assert_eq!(
        git(dir.path(), &["diff", "--name-only"]).unwrap().trim(),
        "f.txt"
    );
    assert_eq!(
        git(dir.path(), &["diff", "--cached", "--name-only"])
            .unwrap()
            .trim(),
        "f.txt"
    );
}

#[test]
fn staging_individual_lines_puts_only_those_lines_in_the_index() {
    let dir = fixture_or_skip!("a\nb\n");
    fs::write(dir.path().join("f.txt"), "a\nX\nY\nb\n").unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let diffs = repo.worktree_diff().unwrap();

    let additions: Vec<usize> = diffs[0].hunks[0]
        .lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.kind == helix_magit::DiffLineKind::Addition)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(additions.len(), 2);

    repo.stage(
        &diffs[0],
        &Selection::Lines {
            hunk: 0,
            lines: vec![additions[0]],
        },
    )
    .unwrap();

    assert_eq!(index_content(dir.path(), "f.txt"), "a\nX\nb\n");
}

#[test]
fn unstaging_reverses_what_was_staged() {
    let dir = fixture_or_skip!("one\ntwo\nthree\n");
    fs::write(dir.path().join("f.txt"), "one\nTWO\nthree\n").unwrap();
    git(dir.path(), &["add", "f.txt"]).unwrap();
    assert_eq!(index_content(dir.path(), "f.txt"), "one\nTWO\nthree\n");

    let repo = Repository::discover(dir.path()).unwrap();
    let staged = repo.staged_diff().unwrap();
    assert_eq!(staged.len(), 1, "the change is staged");

    repo.unstage(&staged[0], &Selection::File).unwrap();

    // The index is back to HEAD; the edit survives in the working tree.
    assert_eq!(index_content(dir.path(), "f.txt"), "one\ntwo\nthree\n");
    assert_eq!(
        fs::read_to_string(dir.path().join("f.txt")).unwrap(),
        "one\nTWO\nthree\n"
    );
    assert_eq!(
        git(dir.path(), &["diff", "--cached", "--name-only"])
            .unwrap()
            .trim(),
        ""
    );
}

#[test]
fn unstaging_one_hunk_keeps_the_other_staged() {
    let dir = fixture_or_skip!(FAR_APART_OLD);
    fs::write(dir.path().join("f.txt"), FAR_APART_NEW).unwrap();
    git(dir.path(), &["add", "f.txt"]).unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let staged = repo.staged_diff().unwrap();
    assert_eq!(staged[0].hunks.len(), 2);

    repo.unstage(&staged[0], &Selection::Hunk(0)).unwrap();

    // The first edit was pulled back out; the second is still staged.
    assert_eq!(
        index_content(dir.path(), "f.txt"),
        "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nL\n"
    );
}

#[test]
fn staging_then_unstaging_returns_the_index_to_head() {
    let dir = fixture_or_skip!(FAR_APART_OLD);
    fs::write(dir.path().join("f.txt"), FAR_APART_NEW).unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let diffs = repo.worktree_diff().unwrap();
    repo.stage(&diffs[0], &Selection::File).unwrap();
    assert_eq!(index_content(dir.path(), "f.txt"), FAR_APART_NEW);

    let staged = repo.staged_diff().unwrap();
    repo.unstage(&staged[0], &Selection::File).unwrap();

    assert_eq!(index_content(dir.path(), "f.txt"), FAR_APART_OLD);
}

#[test]
fn staging_an_untracked_file_adds_an_index_entry() {
    let dir = fixture_or_skip!("a\n");
    fs::write(dir.path().join("new.txt"), "fresh\nfile\n").unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let diffs = repo.worktree_diff().unwrap();
    let new_file = diffs
        .iter()
        .find(|diff| diff.path == Path::new("new.txt"))
        .expect("the untracked file is in the diff");

    repo.stage(new_file, &Selection::File).unwrap();

    assert_eq!(index_content(dir.path(), "new.txt"), "fresh\nfile\n");
    assert!(git(dir.path(), &["diff", "--cached", "--name-only"])
        .unwrap()
        .contains("new.txt"));
}

#[test]
fn staging_a_file_that_lost_its_trailing_newline_keeps_it_lost() {
    let dir = fixture_or_skip!("a\nb\n");
    fs::write(dir.path().join("f.txt"), "a\nb").unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let diffs = repo.worktree_diff().unwrap();
    repo.stage(&diffs[0], &Selection::File).unwrap();

    assert_eq!(index_content(dir.path(), "f.txt"), "a\nb");
    assert_eq!(
        git(dir.path(), &["diff", "--name-only"]).unwrap().trim(),
        "",
        "the worktree and index agree"
    );
}

#[test]
fn staging_a_stale_diff_is_refused_without_touching_the_index() {
    let dir = fixture_or_skip!("one\ntwo\nthree\n");
    fs::write(dir.path().join("f.txt"), "one\nTWO\nthree\n").unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let diffs = repo.worktree_diff().unwrap();

    // Someone rewrites the file — and so the index's side of the diff — from
    // under the captured diff.
    fs::write(dir.path().join("f.txt"), "completely\ndifferent\n").unwrap();
    git(dir.path(), &["add", "f.txt"]).unwrap();

    let before = index_content(dir.path(), "f.txt");
    assert!(repo.stage(&diffs[0], &Selection::File).is_err());
    assert_eq!(
        index_content(dir.path(), "f.txt"),
        before,
        "a refused apply leaves the index alone"
    );
}

#[test]
fn unstaged_changes_are_measured_against_the_index_not_head() {
    // A file with something already staged: the unstaged diff must describe
    // index-to-worktree, or the patch it produces will not apply to the index.
    let dir = fixture_or_skip!("one\ntwo\nthree\n");
    fs::write(dir.path().join("f.txt"), "ONE\ntwo\nthree\n").unwrap();
    git(dir.path(), &["add", "f.txt"]).unwrap();
    fs::write(dir.path().join("f.txt"), "ONE\ntwo\nTHREE\n").unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let diffs = repo.worktree_diff().unwrap();
    assert_eq!(diffs.len(), 1);

    // Only the second edit is unstaged; the first is already in the index.
    assert_eq!(diffs[0].stats(), (1, 1), "just the unstaged edit");
    let contents: Vec<&str> = diffs[0].hunks[0]
        .lines
        .iter()
        .map(|line| line.content.as_str())
        .collect();
    assert!(contents.contains(&"THREE"));
    assert!(
        !contents.contains(&"one"),
        "the staged edit is not shown again"
    );

    // And the patch it produces applies cleanly to the index.
    repo.stage(&diffs[0], &Selection::File).unwrap();
    assert_eq!(index_content(dir.path(), "f.txt"), "ONE\ntwo\nTHREE\n");
    assert_eq!(
        git(dir.path(), &["diff", "--name-only"]).unwrap().trim(),
        ""
    );
}

#[test]
fn staging_a_hunk_of_a_partly_staged_file_works() {
    // The case the status view hits: one edit already staged, two more hunks
    // pending, stage one of them.
    let dir = fixture_or_skip!(FAR_APART_OLD);
    fs::write(
        dir.path().join("f.txt"),
        "A\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\n",
    )
    .unwrap();
    git(dir.path(), &["add", "f.txt"]).unwrap();
    fs::write(dir.path().join("f.txt"), FAR_APART_NEW).unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let diffs = repo.worktree_diff().unwrap();
    repo.stage(&diffs[0], &Selection::Hunk(0)).unwrap();

    assert_eq!(index_content(dir.path(), "f.txt"), FAR_APART_NEW);
}

#[test]
fn staging_a_deleted_file_removes_it_from_the_index() {
    let dir = fixture_or_skip!("a\nb\n");
    fs::remove_file(dir.path().join("f.txt")).unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let diffs = repo.worktree_diff().unwrap();
    repo.stage(&diffs[0], &Selection::File).unwrap();

    // Exactly as after `git rm`: staged as a deletion, nothing unstaged.
    assert_eq!(
        git(dir.path(), &["status", "--porcelain"]).unwrap(),
        "D  f.txt\n"
    );
}

#[test]
fn unstaging_a_new_file_makes_it_untracked_again() {
    let dir = fixture_or_skip!("a\n");
    fs::write(dir.path().join("new.txt"), "fresh\n").unwrap();
    git(dir.path(), &["add", "new.txt"]).unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let staged = repo.staged_diff().unwrap();
    repo.unstage(&staged[0], &Selection::File).unwrap();

    assert_eq!(
        git(dir.path(), &["status", "--porcelain"]).unwrap(),
        "?? new.txt\n"
    );
}

#[test]
fn a_staged_deletion_is_listed_and_can_be_unstaged() {
    let dir = fixture_or_skip!("a\nb\n");
    git(dir.path(), &["rm", "-q", "f.txt"]).unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let staged = repo.staged_diff().unwrap();
    assert_eq!(staged.len(), 1);
    assert_eq!(staged[0].status, helix_magit::FileStatus::Deleted);

    repo.unstage(&staged[0], &Selection::File).unwrap();
    // Back in the index, still gone from the working tree.
    assert_eq!(
        git(dir.path(), &["status", "--porcelain"]).unwrap(),
        " D f.txt\n"
    );
}

#[test]
fn unstaging_a_pure_insertion_removes_exactly_those_lines() {
    let dir = fixture_or_skip!("a\nb\nc\nd\ne\nf\ng\nh\n");
    fs::write(dir.path().join("f.txt"), "a\nb\nc\nd\nNEW\ne\nf\ng\nh\n").unwrap();
    git(dir.path(), &["add", "f.txt"]).unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let staged = repo.staged_diff().unwrap();
    repo.unstage(&staged[0], &Selection::Hunk(0)).unwrap();

    assert_eq!(
        index_content(dir.path(), "f.txt"),
        "a\nb\nc\nd\ne\nf\ng\nh\n"
    );
}

#[test]
fn unstaging_one_line_of_a_hunk_keeps_the_rest_staged() {
    let dir = fixture_or_skip!("one\ntwo\n");
    fs::write(dir.path().join("f.txt"), "one\nTWO\nthree\n").unwrap();
    git(dir.path(), &["add", "f.txt"]).unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let staged = repo.staged_diff().unwrap();
    let added = staged[0].hunks[0]
        .lines
        .iter()
        .position(|line| line.content == "three")
        .unwrap();
    repo.unstage(
        &staged[0],
        &Selection::Lines {
            hunk: 0,
            lines: vec![added],
        },
    )
    .unwrap();

    assert_eq!(index_content(dir.path(), "f.txt"), "one\nTWO\n");
}

#[test]
fn a_diff_ignoring_whitespace_stages_only_what_it_shows() {
    use helix_magit::diff::{DiffOptions, Whitespace};

    let dir = fixture_or_skip!("fn a() {\n    one();\n    two();\n}\n");
    // A reindent and one real change.
    fs::write(
        dir.path().join("f.txt"),
        "fn a() {\n  one();\n    TWO();\n}\n",
    )
    .unwrap();

    let options = DiffOptions {
        whitespace: Whitespace::IgnoreChange,
        ..DiffOptions::default()
    };
    let repo = Repository::discover(dir.path())
        .unwrap()
        .with_diff_options(options);
    let diffs = repo.worktree_diff().unwrap();
    let changed: Vec<&str> = diffs[0].hunks[0]
        .changed_lines()
        .map(|line| line.content.as_str())
        .collect();
    assert_eq!(
        changed,
        ["    two();", "    TWO();"],
        "the reindent is context"
    );

    repo.stage(&diffs[0], &Selection::File).unwrap();
    // The index takes the real change and keeps its own indentation.
    assert_eq!(
        index_content(dir.path(), "f.txt"),
        "fn a() {\n    one();\n    TWO();\n}\n"
    );
}

#[test]
fn context_is_adjustable() {
    use helix_magit::diff::DiffOptions;

    let dir = fixture_or_skip!("a\nb\nc\nd\ne\nf\ng\n");
    fs::write(dir.path().join("f.txt"), "a\nb\nc\nD\ne\nf\ng\n").unwrap();
    let none = DiffOptions {
        context: 0,
        ..DiffOptions::default()
    };
    let repo = Repository::discover(dir.path())
        .unwrap()
        .with_diff_options(none);
    let diffs = repo.worktree_diff().unwrap();
    assert_eq!(diffs[0].hunks[0].lines.len(), 2);
    // Staged with no context at all, it still lands in the right place.
    repo.stage(&diffs[0], &Selection::File).unwrap();
    assert_eq!(index_content(dir.path(), "f.txt"), "a\nb\nc\nD\ne\nf\ng\n");
}
