//! The gix layer, against real repositories on disk.

use std::fs;
use std::path::Path;
use std::process::Command;

use helix_magit::diff::FileStatus;
use helix_magit::Repository;

/// Builds a repository with one committed file, using git itself so the
/// fixture does not depend on the code under test.
fn fixture() -> Option<tempfile::TempDir> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();

    if !git(path, &["init", "--initial-branch=main"]) {
        // No usable git binary in this environment.
        return None;
    }
    git(path, &["config", "user.email", "test@example.invalid"]);
    git(path, &["config", "user.name", "Test"]);

    fs::write(path.join("tracked.txt"), "one\ntwo\nthree\n").unwrap();
    git(path, &["add", "."]);
    git(path, &["commit", "-m", "initial"]);

    Some(dir)
}

fn git(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

macro_rules! repo_or_skip {
    () => {
        match fixture() {
            Some(dir) => dir,
            None => {
                eprintln!("skipping: no usable git binary");
                return;
            }
        }
    };
}

#[test]
fn reports_the_checked_out_branch() {
    let dir = repo_or_skip!();
    let repo = Repository::discover(dir.path()).unwrap();

    assert_eq!(repo.head_branch().as_deref(), Some("main"));
    assert_eq!(repo.head_description(), "main");
    assert_eq!(
        repo.workdir().canonicalize().unwrap(),
        dir.path().canonicalize().unwrap()
    );
}

#[test]
fn a_clean_tree_has_no_changes() {
    let dir = repo_or_skip!();
    let repo = Repository::discover(dir.path()).unwrap();

    assert!(repo.worktree_status().unwrap().is_empty());
    assert!(repo.worktree_diff().unwrap().is_empty());
}

#[test]
fn a_modified_file_produces_hunks() {
    let dir = repo_or_skip!();
    fs::write(dir.path().join("tracked.txt"), "one\nTWO\nthree\n").unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let status = repo.worktree_status().unwrap();
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].path, Path::new("tracked.txt"));
    assert_eq!(status[0].status, FileStatus::Modified);
    assert!(!status[0].untracked);

    let diffs = repo.worktree_diff().unwrap();
    assert_eq!(diffs.len(), 1);
    assert_eq!(diffs[0].path, Path::new("tracked.txt"));
    assert_eq!(diffs[0].stats(), (1, 1), "one line replaced");

    let lines = &diffs[0].hunks[0].lines;
    assert!(lines
        .iter()
        .any(|line| line.content == "TWO" && line.new_line == Some(2)));
    assert!(lines
        .iter()
        .any(|line| line.content == "two" && line.old_line == Some(2)));
}

#[test]
fn an_untracked_file_is_reported_as_added() {
    let dir = repo_or_skip!();
    fs::write(dir.path().join("new.txt"), "fresh\n").unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let status = repo.worktree_status().unwrap();

    let entry = status
        .iter()
        .find(|entry| entry.path == Path::new("new.txt"))
        .expect("untracked file should be listed");
    assert_eq!(entry.status, FileStatus::Added);
    assert!(entry.untracked);

    let diffs = repo.worktree_diff().unwrap();
    let diff = diffs
        .iter()
        .find(|diff| diff.path == Path::new("new.txt"))
        .expect("untracked file should have a diff");
    assert_eq!(diff.status, FileStatus::Added);
    assert_eq!(diff.stats(), (1, 0));
}

#[test]
fn a_deleted_file_is_reported_as_deleted() {
    let dir = repo_or_skip!();
    fs::remove_file(dir.path().join("tracked.txt")).unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let status = repo.worktree_status().unwrap();

    assert_eq!(status.len(), 1);
    assert_eq!(status[0].status, FileStatus::Deleted);

    let diffs = repo.worktree_diff().unwrap();
    assert_eq!(diffs[0].stats(), (0, 3), "every line removed");
}

#[test]
fn a_binary_file_is_flagged_without_hunks() {
    let dir = repo_or_skip!();
    fs::write(dir.path().join("blob.bin"), [0x00, 0x01, 0x02, 0x00]).unwrap();

    let repo = Repository::discover(dir.path()).unwrap();
    let diffs = repo.worktree_diff().unwrap();

    let diff = diffs
        .iter()
        .find(|diff| diff.path == Path::new("blob.bin"))
        .expect("binary file should still be listed");
    assert!(diff.binary);
    assert!(diff.hunks.is_empty());
}

#[test]
fn discovering_outside_a_repository_fails() {
    let dir = tempfile::tempdir().unwrap();
    assert!(Repository::discover(dir.path()).is_err());
}
