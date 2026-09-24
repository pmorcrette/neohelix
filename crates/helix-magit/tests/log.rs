//! The log and commit reading, against real repositories.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use helix_magit::diff::FileStatus;
use helix_magit::log::{read_log, show, LogFilter};
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

fn commit(dir: &Path, author: &str, file: &str, content: &str, message: &str) -> Option<()> {
    fs::write(dir.join(file), content).unwrap();
    git(dir, &["add", file])?;
    git(
        dir,
        &[
            "-c",
            &format!("user.name={author}"),
            "commit",
            "-q",
            "-m",
            message,
        ],
    )?;
    Some(())
}

/// main: c1 (Ann) → c2 (Bob, b.txt) → merge of topic (t1, Ann); tag v1 on c1.
fn fixture() -> Option<tempfile::TempDir> {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path();
    git(work, &["init", "-q", "--initial-branch=main"])?;
    git(work, &["config", "user.email", "t@e.invalid"])?;
    git(work, &["config", "user.name", "T"])?;
    commit(work, "Ann", "a.txt", "one\n", "First commit")?;
    git(work, &["tag", "v1"])?;
    git(work, &["checkout", "-q", "-b", "topic"])?;
    commit(work, "Ann", "t.txt", "topic\n", "Topic work")?;
    git(work, &["checkout", "-q", "main"])?;
    commit(work, "Bob", "b.txt", "bee\n", "Fix the bee")?;
    git(
        work,
        &["merge", "-q", "--no-ff", "-m", "Merge topic", "topic"],
    )?;
    Some(dir)
}

fn subjects(entries: &[helix_magit::log::LogEntry]) -> Vec<&str> {
    entries
        .iter()
        .filter(|entry| entry.hash.is_some())
        .map(|entry| entry.subject.as_str())
        .collect()
}

#[test]
fn the_log_has_the_graph_refs_and_every_commit() {
    let Some(dir) = fixture() else { return };
    let entries = read_log(dir.path(), &LogFilter::default()).unwrap();

    // Commits made within the same second have no defined order between
    // branches; the merge comes first and the root last all the same.
    let mut listed = subjects(&entries);
    assert_eq!(listed.first(), Some(&"Merge topic"));
    assert_eq!(listed.last(), Some(&"First commit"));
    listed.sort();
    assert_eq!(
        listed,
        ["First commit", "Fix the bee", "Merge topic", "Topic work"]
    );
    assert!(entries[0].refs.iter().any(|name| name == "HEAD -> main"));
    let first = entries
        .iter()
        .find(|e| e.subject == "First commit")
        .unwrap();
    assert!(first.refs.iter().any(|name| name == "tag: v1"));
    assert_eq!(first.author, "Ann");
    // A merge draws graph-only lines between commits.
    assert!(entries.iter().any(|entry| entry.hash.is_none()));
}

#[test]
fn filters_narrow_the_log() {
    let Some(dir) = fixture() else { return };
    let work = dir.path();
    let by = |filter: LogFilter| -> Vec<String> {
        subjects(&read_log(work, &filter).unwrap())
            .into_iter()
            .map(str::to_string)
            .collect()
    };

    let author = LogFilter {
        author: Some("bob".into()),
        ..LogFilter::default()
    };
    assert_eq!(by(author), ["Fix the bee"]);

    let grep = LogFilter {
        grep: Some("TOPIC".into()),
        ..LogFilter::default()
    };
    assert_eq!(by(grep), ["Merge topic", "Topic work"]);

    let path = LogFilter {
        path: Some(PathBuf::from("t.txt")),
        ..LogFilter::default()
    };
    assert_eq!(by(path), ["Topic work"]);

    let range = LogFilter {
        range: Some("v1..HEAD".into()),
        ..LogFilter::default()
    };
    let mut ranged = by(range);
    ranged.sort();
    assert_eq!(ranged, ["Fix the bee", "Merge topic", "Topic work"]);

    let bad = LogFilter {
        range: Some("no-such-rev".into()),
        ..LogFilter::default()
    };
    assert!(read_log(work, &bad).is_err());
}

#[test]
fn a_commit_is_read_with_its_message_and_diff() {
    let Some(dir) = fixture() else { return };
    let work = dir.path();

    let bee = show(work, "HEAD~1").unwrap();
    assert_eq!(bee.message, "Fix the bee");
    assert!(bee.author.starts_with("Bob <"));
    assert_eq!(bee.files.len(), 1);
    assert_eq!(bee.files[0].path, PathBuf::from("b.txt"));
    assert_eq!(bee.files[0].status, FileStatus::Added);

    // A merge shows what it brought in against its first parent.
    let merge = show(work, "HEAD").unwrap();
    assert_eq!(merge.message, "Merge topic");
    assert_eq!(merge.files.len(), 1);
    assert_eq!(merge.files[0].path, PathBuf::from("t.txt"));

    assert!(show(work, "--output=x").is_err());
}

#[test]
fn a_commit_s_change_is_applied_to_and_reversed_out_of_the_worktree() {
    let Some(dir) = fixture() else { return };
    let work = dir.path();
    let repository = Repository::discover(work).unwrap();

    // Reversing "Fix the bee" deletes b.txt from the working tree only.
    let bee = show(work, "HEAD~1").unwrap();
    repository
        .apply_to_worktree(&bee.files[0], &Selection::File, true)
        .unwrap();
    assert!(!work.join("b.txt").exists());
    assert_eq!(git(work, &["status", "--porcelain"]).unwrap(), " D b.txt\n");

    // And applying it puts it back.
    repository
        .apply_to_worktree(&bee.files[0], &Selection::File, false)
        .unwrap();
    assert_eq!(fs::read_to_string(work.join("b.txt")).unwrap(), "bee\n");
    assert_eq!(git(work, &["status", "--porcelain"]).unwrap(), "");
}
