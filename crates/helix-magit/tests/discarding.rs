//! Discarding, reversing and bulk staging against real repositories.

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

fn read(dir: &Path, path: &str) -> String {
    fs::read_to_string(dir.join(path)).unwrap_or_default()
}

fn status(dir: &Path) -> String {
    git(dir, &["status", "--porcelain"]).unwrap()
}

const OLD: &str = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\n";
const NEW: &str = "A\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nL\n";

#[test]
fn discarding_one_unstaged_hunk_keeps_the_other() {
    let dir = fixture_or_skip!(OLD);
    fs::write(dir.path().join("f.txt"), NEW).unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let diffs = repo.worktree_diff().unwrap();
    assert_eq!(diffs[0].hunks.len(), 2);
    repo.discard(&diffs[0], &Selection::Hunk(0), false).unwrap();

    assert_eq!(
        read(dir.path(), "f.txt"),
        "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nL\n"
    );
    // The index was never touched.
    assert_eq!(status(dir.path()), " M f.txt\n");
}

#[test]
fn discarding_a_line_reverts_only_that_line() {
    let dir = fixture_or_skip!("one\ntwo\n");
    fs::write(dir.path().join("f.txt"), "one\nTWO\nthree\n").unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let diffs = repo.worktree_diff().unwrap();
    let hunk = &diffs[0].hunks[0];
    let added = hunk
        .lines
        .iter()
        .position(|line| line.content == "three")
        .unwrap();
    repo.discard(
        &diffs[0],
        &Selection::Lines {
            hunk: 0,
            lines: vec![added],
        },
        false,
    )
    .unwrap();

    assert_eq!(read(dir.path(), "f.txt"), "one\nTWO\n");
}

#[test]
fn discarding_an_unstaged_deletion_brings_the_file_back() {
    let dir = fixture_or_skip!(OLD);
    fs::remove_file(dir.path().join("f.txt")).unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let diffs = repo.worktree_diff().unwrap();
    repo.discard(&diffs[0], &Selection::File, false).unwrap();

    assert_eq!(read(dir.path(), "f.txt"), OLD);
    assert_eq!(status(dir.path()), "");
}

#[test]
fn discarding_a_staged_change_removes_it_from_index_and_worktree() {
    let dir = fixture_or_skip!(OLD);
    fs::write(dir.path().join("f.txt"), NEW).unwrap();
    git(dir.path(), &["add", "f.txt"]).unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let staged = repo.staged_diff().unwrap();
    repo.discard(&staged[0], &Selection::Hunk(1), true).unwrap();

    let expected = "A\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\n";
    assert_eq!(read(dir.path(), "f.txt"), expected);
    assert_eq!(git(dir.path(), &["show", ":f.txt"]).unwrap(), expected);
}

#[test]
fn discarding_a_staged_new_file_deletes_it() {
    let dir = fixture_or_skip!(OLD);
    fs::write(dir.path().join("new.txt"), "fresh\n").unwrap();
    git(dir.path(), &["add", "new.txt"]).unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let staged = repo.staged_diff().unwrap();
    repo.discard(&staged[0], &Selection::File, true).unwrap();

    assert!(!dir.path().join("new.txt").exists());
    assert_eq!(status(dir.path()), "");
}

#[test]
fn a_staged_discard_is_refused_when_the_worktree_moved_on() {
    let dir = fixture_or_skip!("one\ntwo\n");
    fs::write(dir.path().join("f.txt"), "one\nTWO\n").unwrap();
    git(dir.path(), &["add", "f.txt"]).unwrap();
    fs::write(dir.path().join("f.txt"), "one\nTwo, again\n").unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let staged = repo.staged_diff().unwrap();
    assert!(repo.discard(&staged[0], &Selection::File, true).is_err());

    // Neither side was touched.
    assert_eq!(read(dir.path(), "f.txt"), "one\nTwo, again\n");
    assert_eq!(git(dir.path(), &["show", ":f.txt"]).unwrap(), "one\nTWO\n");
}

#[test]
fn reversing_a_staged_hunk_changes_only_the_worktree() {
    let dir = fixture_or_skip!(OLD);
    fs::write(dir.path().join("f.txt"), NEW).unwrap();
    git(dir.path(), &["add", "f.txt"]).unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let staged = repo.staged_diff().unwrap();
    repo.reverse(&staged[0], &Selection::Hunk(0)).unwrap();

    assert_eq!(
        read(dir.path(), "f.txt"),
        "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nL\n"
    );
    assert_eq!(git(dir.path(), &["show", ":f.txt"]).unwrap(), NEW);
}

#[test]
fn untracked_files_are_deleted() {
    let dir = fixture_or_skip!(OLD);
    fs::write(dir.path().join("junk.txt"), "x\n").unwrap();
    let repo = Repository::discover(dir.path()).unwrap();
    repo.discard_untracked(Path::new("junk.txt")).unwrap();
    assert_eq!(status(dir.path()), "");
}

#[test]
fn everything_is_staged_and_unstaged_at_once() {
    let dir = fixture_or_skip!(OLD);
    fs::write(dir.path().join("f.txt"), NEW).unwrap();
    fs::write(dir.path().join("untracked.txt"), "x\n").unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    repo.stage_all().unwrap();
    // Tracked changes only, as `S` does in Magit.
    assert_eq!(status(dir.path()), "M  f.txt\n?? untracked.txt\n");

    repo.unstage_all().unwrap();
    assert_eq!(status(dir.path()), " M f.txt\n?? untracked.txt\n");
}

#[test]
fn unstaging_everything_works_before_the_first_commit() {
    let dir = tempfile::tempdir().unwrap();
    if git(dir.path(), &["init", "--initial-branch=main"]).is_none() {
        return;
    }
    fs::write(dir.path().join("a.txt"), "a\n").unwrap();
    git(dir.path(), &["add", "a.txt"]).unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    repo.unstage_all().unwrap();
    assert_eq!(status(dir.path()), "?? a.txt\n");
}

#[test]
fn several_files_are_staged_and_unstaged_by_path() {
    let dir = fixture_or_skip!(OLD);
    fs::write(dir.path().join("f.txt"), NEW).unwrap();
    fs::write(dir.path().join("new.txt"), "x\n").unwrap();
    fs::write(dir.path().join("other.bin"), [0u8, 1, 2, 255]).unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let paths: Vec<std::path::PathBuf> = ["f.txt", "new.txt", "other.bin"]
        .iter()
        .map(std::path::PathBuf::from)
        .collect();
    repo.stage_paths(&paths).unwrap();
    assert_eq!(status(dir.path()), "M  f.txt\nA  new.txt\nA  other.bin\n");

    repo.unstage_paths(&paths[..2]).unwrap();
    assert_eq!(status(dir.path()), " M f.txt\nA  other.bin\n?? new.txt\n");
}

#[test]
fn staging_a_deleted_file_by_path_stages_the_deletion() {
    let dir = fixture_or_skip!(OLD);
    fs::remove_file(dir.path().join("f.txt")).unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    repo.stage_paths(&[std::path::PathBuf::from("f.txt")])
        .unwrap();
    assert_eq!(status(dir.path()), "D  f.txt\n");
}

#[test]
fn two_hunks_are_staged_together_as_one_patch() {
    let dir = fixture_or_skip!(OLD);
    let three = "A\nb\nc\nd\ne\nF\ng\nh\ni\nj\nk\nL\n";
    fs::write(dir.path().join("f.txt"), three).unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let options = helix_magit::diff::DiffOptions {
        context: 1,
        ..Default::default()
    };
    let repo = repo.with_diff_options(options);
    let diffs = repo.worktree_diff().unwrap();
    assert_eq!(diffs[0].hunks.len(), 3);
    repo.stage(&diffs[0], &Selection::Hunks(vec![1, 2]))
        .unwrap();

    let staged = git(dir.path(), &["show", ":f.txt"]).unwrap();
    assert_eq!(staged, "a\nb\nc\nd\ne\nF\ng\nh\ni\nj\nk\nL\n");
}
