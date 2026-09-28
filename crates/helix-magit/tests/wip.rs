//! Work-in-progress refs, against real repositories.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use helix_magit::wip;

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
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn fixture() -> Option<tempfile::TempDir> {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path();
    git(work, &["init", "-q", "--initial-branch=main"])?;
    git(work, &["config", "user.email", "t@e.invalid"])?;
    git(work, &["config", "user.name", "T"])?;
    fs::write(work.join("a.txt"), "one\n").unwrap();
    fs::write(work.join("b.txt"), "bee\n").unwrap();
    git(work, &["add", "."])?;
    git(work, &["commit", "-q", "-m", "base"])?;
    Some(dir)
}

#[test]
fn a_save_records_the_index_and_the_worktree_and_touches_neither() {
    let Some(dir) = fixture() else { return };
    let work = dir.path();
    fs::write(work.join("a.txt"), "staged\n").unwrap();
    git(work, &["add", "a.txt"]).unwrap();
    fs::write(work.join("b.txt"), "unstaged\n").unwrap();
    let status_before = git(work, &["status", "--porcelain"]).unwrap();

    let saved = wip::save(work, "save", None).unwrap();
    assert!(saved.index.is_some() && saved.worktree.is_some());

    let refs = wip::refs(work);
    assert_eq!(refs.worktree, "refs/wip/wtree/refs/heads/main");
    assert_eq!(
        git(work, &["show", &format!("{}:a.txt", refs.index)]).unwrap(),
        "staged"
    );
    assert_eq!(
        git(work, &["show", &format!("{}:b.txt", refs.index)]).unwrap(),
        "bee",
        "the index's own b.txt"
    );
    assert_eq!(
        git(work, &["show", &format!("{}:b.txt", refs.worktree)]).unwrap(),
        "unstaged"
    );
    // Nothing the user sees moved.
    assert_eq!(
        git(work, &["status", "--porcelain"]).unwrap(),
        status_before
    );
    assert_eq!(git(work, &["log", "-1", "--format=%s"]).unwrap(), "base");
}

#[test]
fn saves_chain_skip_repeats_and_restart_when_head_moves() {
    let Some(dir) = fixture() else { return };
    let work = dir.path();
    let refs = wip::refs(work);

    fs::write(work.join("a.txt"), "v1\n").unwrap();
    let first = wip::save(work, "one", None).unwrap().worktree.unwrap();
    // Nothing new: nothing added.
    assert_eq!(wip::save(work, "again", None).unwrap().worktree, None);

    fs::write(work.join("a.txt"), "v2\n").unwrap();
    let second = wip::save(work, "two", None).unwrap().worktree.unwrap();
    assert_eq!(
        git(work, &["rev-parse", &format!("{second}^")]).unwrap(),
        first,
        "on top of the previous save"
    );

    // A commit moves HEAD past the chain: the next save starts from HEAD.
    git(work, &["commit", "-q", "-am", "v2"]).unwrap();
    fs::write(work.join("a.txt"), "v3\n").unwrap();
    let third = wip::save(work, "three", None).unwrap().worktree.unwrap();
    assert_eq!(
        git(work, &["rev-parse", &format!("{third}^")]).unwrap(),
        git(work, &["rev-parse", "HEAD"]).unwrap()
    );
    assert_eq!(git(work, &["rev-parse", &refs.worktree]).unwrap(), third);
}

#[test]
fn a_save_after_writing_one_file_records_only_that_file() {
    let Some(dir) = fixture() else { return };
    let work = dir.path();
    fs::write(work.join("a.txt"), "written\n").unwrap();
    fs::write(work.join("b.txt"), "also changed\n").unwrap();
    wip::save(work, "after writing a.txt", Some(&[PathBuf::from("a.txt")])).unwrap();
    let refs = wip::refs(work);
    assert_eq!(
        git(work, &["show", &format!("{}:a.txt", refs.worktree)]).unwrap(),
        "written"
    );
    assert_eq!(
        git(work, &["show", &format!("{}:b.txt", refs.worktree)]).unwrap(),
        "bee"
    );
}

#[test]
fn nothing_is_saved_before_the_first_commit() {
    let dir = tempfile::tempdir().unwrap();
    if git(dir.path(), &["init", "-q"]).is_none() {
        return;
    }
    fs::write(dir.path().join("a.txt"), "x\n").unwrap();
    assert_eq!(
        wip::save(dir.path(), "save", None).unwrap(),
        wip::Saved::default()
    );
}
