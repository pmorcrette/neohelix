//! Versions of a file, for the Ediff views.

use std::fs;
use std::path::Path;
use std::process::Command;

use helix_magit::ediff::{self, Dwim, Version};

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "T")
        .env("GIT_AUTHOR_EMAIL", "t@e.invalid")
        .env("GIT_COMMITTER_NAME", "T")
        .env("GIT_COMMITTER_EMAIL", "t@e.invalid")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn repo() -> Option<tempfile::TempDir> {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "--initial-branch=main"])?;
    fs::write(dir.path().join("f.txt"), "one\n").unwrap();
    git(dir.path(), &["add", "."])?;
    git(dir.path(), &["commit", "-m", "one"])?;
    Some(dir)
}

macro_rules! repo_or_skip {
    () => {
        match repo() {
            Some(dir) => dir,
            None => return,
        }
    };
}

#[test]
fn each_version_is_read_from_where_git_keeps_it() {
    let dir = repo_or_skip!();
    let work = dir.path();
    fs::write(work.join("f.txt"), "two\n").unwrap();
    git(work, &["add", "f.txt"]).unwrap();
    fs::write(work.join("f.txt"), "three\n").unwrap();

    let path = Path::new("f.txt");
    assert_eq!(
        ediff::content(work, &Version::Head, path).as_deref(),
        Some("one\n")
    );
    assert_eq!(
        ediff::content(work, &Version::Index, path).as_deref(),
        Some("two\n")
    );
    assert_eq!(
        ediff::content(work, &Version::Worktree, path).as_deref(),
        Some("three\n")
    );
    assert_eq!(
        ediff::content(work, &Version::Head, Path::new("new.txt")),
        None
    );

    // Written where a buffer opens it, named by the version.
    let file = ediff::materialize(work, &Version::Rev("HEAD~0".into()), path, "one\n").unwrap();
    assert!(
        file.ends_with("helix/ediff/HEAD_0/f.txt"),
        "{}",
        file.display()
    );
    assert_eq!(fs::read_to_string(file).unwrap(), "one\n");

    assert_eq!(ediff::dwim(work, "f.txt"), Some(Dwim::Unstaged));
    fs::write(work.join("f.txt"), "two\n").unwrap();
    assert_eq!(ediff::dwim(work, "f.txt"), Some(Dwim::Staged));
}

#[test]
fn an_edited_index_version_is_staged_with_its_mode() {
    let dir = repo_or_skip!();
    let work = dir.path();
    fs::write(work.join("f.txt"), "one\ntwo\nthree\n").unwrap();

    ediff::write_index(work, Path::new("f.txt"), "one\ntwo\n").unwrap();
    assert_eq!(git(work, &["show", ":f.txt"]).unwrap(), "one\ntwo\n");
    assert!(git(work, &["ls-files", "-s", "f.txt"])
        .unwrap()
        .starts_with("100644"));
    // The working tree is untouched.
    assert_eq!(
        fs::read_to_string(work.join("f.txt")).unwrap(),
        "one\ntwo\nthree\n"
    );

    // A file the index did not have yet is added.
    fs::write(work.join("new.txt"), "n\n").unwrap();
    ediff::write_index(work, Path::new("new.txt"), "n\n").unwrap();
    assert_eq!(git(work, &["show", ":new.txt"]).unwrap(), "n\n");
}

#[test]
fn a_conflict_offers_its_three_stages_and_dwim_resolves_it() {
    let dir = repo_or_skip!();
    let work = dir.path();
    git(work, &["checkout", "-b", "other"]).unwrap();
    fs::write(work.join("f.txt"), "theirs\n").unwrap();
    git(work, &["commit", "-am", "theirs"]).unwrap();
    git(work, &["checkout", "main"]).unwrap();
    fs::write(work.join("f.txt"), "ours\n").unwrap();
    git(work, &["commit", "-am", "ours"]).unwrap();
    assert!(
        git(work, &["merge", "other"]).is_none(),
        "the merge conflicts"
    );

    let path = Path::new("f.txt");
    assert_eq!(
        ediff::content(work, &Version::Stage(1), path).as_deref(),
        Some("one\n")
    );
    assert_eq!(
        ediff::content(work, &Version::Stage(2), path).as_deref(),
        Some("ours\n")
    );
    assert_eq!(
        ediff::content(work, &Version::Stage(3), path).as_deref(),
        Some("theirs\n")
    );
    assert_eq!(ediff::unmerged_paths(work), ["f.txt"]);
    assert_eq!(ediff::dwim(work, "f.txt"), Some(Dwim::Resolve));
}

#[test]
fn a_files_history_follows_it_through_a_rename() {
    let dir = repo_or_skip!();
    let work = dir.path();
    fs::write(work.join("f.txt"), "two\n").unwrap();
    git(work, &["commit", "-am", "two"]).unwrap();
    git(work, &["mv", "f.txt", "g.txt"]).unwrap();
    git(work, &["commit", "-m", "rename"]).unwrap();
    fs::write(work.join("other.txt"), "x\n").unwrap();
    git(work, &["add", "other.txt"]).unwrap();
    git(work, &["commit", "-m", "unrelated"]).unwrap();

    let history = ediff::file_history(work, Path::new("g.txt"));
    let names: Vec<_> = history
        .iter()
        .map(|(_, path)| path.display().to_string())
        .collect();
    assert_eq!(names, ["g.txt", "f.txt", "f.txt"]);

    // HEAD did not change g.txt: the rename did, last.
    let last = ediff::last_change(work, "HEAD", Path::new("g.txt")).unwrap();
    assert_eq!(last, history[0].0);
    assert_eq!(ediff::commit_hash(work, "HEAD~1").unwrap(), history[0].0);
    assert_eq!(ediff::files_at(work, "HEAD"), ["g.txt", "other.txt"]);

    // The oldest version, read through its old name.
    let (oldest, name) = history.last().unwrap();
    assert_eq!(
        ediff::content(work, &Version::Rev(oldest.clone()), name).as_deref(),
        Some("one\n")
    );
    let file = ediff::materialize_blob(work, "abc1234", Path::new("g.txt"), "x").unwrap();
    assert!(file.ends_with("helix/blob/abc1234/g.txt"));
}

#[test]
fn tracing_lines_is_log_minus_l_without_a_pathspec() {
    let filter = helix_magit::log::LogFilter {
        path: Some("src/lib.rs".into()),
        lines: Some("10,20".into()),
        ..Default::default()
    };
    let args = filter.args();
    assert!(args.contains(&"-L10,20:src/lib.rs".to_string()), "{args:?}");
    assert!(args.contains(&"-s".to_string()));
    assert!(!args.contains(&"--".to_string()), "{args:?}");
}
