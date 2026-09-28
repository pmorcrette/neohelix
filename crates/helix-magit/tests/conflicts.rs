//! Resolving conflicts, against real repositories.

use std::fs;
use std::path::Path;
use std::process::Command;

use helix_magit::command::run_plan;
use helix_magit::conflict::{regions, stage_content, Side};
use helix_magit::transient::MagitCommand;
use helix_magit::{resolve, Repository};

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_EDITOR", "true")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// main and topic both change f.txt's middle line; topic deletes gone.txt,
/// which main changes. The merge of topic into main stops on both.
fn conflicted() -> Option<tempfile::TempDir> {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path();
    git(work, &["init", "-q", "--initial-branch=main"])?;
    git(work, &["config", "user.email", "t@e.invalid"])?;
    git(work, &["config", "user.name", "T"])?;
    fs::write(work.join("f.txt"), "one\ntwo\nthree\n").unwrap();
    fs::write(work.join("gone.txt"), "old\n").unwrap();
    git(work, &["add", "."])?;
    git(work, &["commit", "-q", "-m", "base"])?;
    git(work, &["checkout", "-q", "-b", "topic"])?;
    fs::write(work.join("f.txt"), "one\nTOPIC\nthree\n").unwrap();
    git(work, &["rm", "-q", "gone.txt"])?;
    git(work, &["commit", "-q", "-am", "topic"])?;
    git(work, &["checkout", "-q", "main"])?;
    fs::write(work.join("f.txt"), "one\nMAIN\nthree\n").unwrap();
    fs::write(work.join("gone.txt"), "changed\n").unwrap();
    git(work, &["commit", "-q", "-am", "main"])?;
    // Stops on the conflicts.
    if git(work, &["merge", "topic"]).is_some() {
        return None;
    }
    Some(dir)
}

fn run(work: &Path, command: MagitCommand, answers: &[&str]) -> helix_magit::GitOutput {
    let plan = resolve(command, &[]).unwrap();
    let answers: Vec<String> = answers.iter().map(|a| a.to_string()).collect();
    run_plan(work, &plan.answered(&answers)).unwrap()
}

fn unmerged(work: &Path) -> Vec<String> {
    Repository::discover(work)
        .unwrap()
        .unmerged()
        .unwrap()
        .into_iter()
        .map(|path| format!("{} {}", path.path.display(), path.state))
        .collect()
}

#[test]
fn the_sides_and_the_regions_are_read() {
    let Some(dir) = conflicted() else { return };
    let work = dir.path();
    let path = Path::new("f.txt");
    assert_eq!(
        stage_content(work, path, Side::Base).unwrap(),
        "one\ntwo\nthree\n"
    );
    assert_eq!(
        stage_content(work, path, Side::Ours).unwrap(),
        "one\nMAIN\nthree\n"
    );
    assert_eq!(
        stage_content(work, path, Side::Theirs).unwrap(),
        "one\nTOPIC\nthree\n"
    );
    assert_eq!(
        stage_content(work, Path::new("gone.txt"), Side::Theirs),
        None
    );

    let found = regions(&fs::read_to_string(work.join("f.txt")).unwrap());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].ours, ["MAIN"]);
    assert_eq!(found[0].theirs, ["TOPIC"]);
    assert_eq!(found[0].base, None);

    // Rewritten with the base shown, the base is there too.
    assert!(run(work, MagitCommand::ConflictWithBase, &["f.txt"]).success);
    let found = regions(&fs::read_to_string(work.join("f.txt")).unwrap());
    assert_eq!(found[0].base.as_deref(), Some(&["two".to_string()][..]));
}

#[test]
fn a_file_with_markers_left_is_not_marked_resolved() {
    let Some(dir) = conflicted() else { return };
    let work = dir.path();
    let output = run(work, MagitCommand::ConflictMarkResolved, &["f.txt"]);
    assert!(!output.success);
    assert_eq!(
        output.summary(),
        "f.txt still has a conflict marker on line 2"
    );
    assert_eq!(unmerged(work).len(), 2);

    fs::write(work.join("f.txt"), "one\nMAIN and TOPIC\nthree\n").unwrap();
    assert!(run(work, MagitCommand::ConflictMarkResolved, &["f.txt"]).success);
    assert_eq!(unmerged(work), ["gone.txt deleted by them"]);
}

#[test]
fn a_whole_side_resolves_even_a_deletion_and_the_merge_continues() {
    let Some(dir) = conflicted() else { return };
    let work = dir.path();

    let output = run(work, MagitCommand::ConflictTakeTheirs, &["f.txt"]);
    assert!(output.success, "{output:?}");
    assert_eq!(
        fs::read_to_string(work.join("f.txt")).unwrap(),
        "one\nTOPIC\nthree\n"
    );

    // Their side deleted gone.txt: taking it is the deletion.
    let output = run(work, MagitCommand::ConflictTakeTheirs, &["gone.txt"]);
    assert!(output.success, "{output:?}");
    assert!(!work.join("gone.txt").exists());
    assert!(unmerged(work).is_empty());

    let output = run(work, MagitCommand::Continue, &[]);
    assert!(output.success, "{output:?}");
    assert_eq!(
        git(work, &["log", "-1", "--format=%s"]).unwrap().trim(),
        "Merge branch 'topic'"
    );
    assert_eq!(git(work, &["status", "--porcelain"]).unwrap(), "");
}

#[test]
fn ours_keeps_a_file_their_side_deleted() {
    let Some(dir) = conflicted() else { return };
    let work = dir.path();
    assert!(run(work, MagitCommand::ConflictTakeOurs, &["gone.txt"]).success);
    assert_eq!(
        fs::read_to_string(work.join("gone.txt")).unwrap(),
        "changed\n"
    );
    assert_eq!(unmerged(work), ["f.txt both modified"]);
}

#[test]
fn continuing_with_nothing_in_progress_says_so() {
    let Some(dir) = conflicted() else { return };
    let work = dir.path();
    git(work, &["merge", "--abort"]).unwrap();
    let output = run(work, MagitCommand::Continue, &[]);
    assert!(!output.success);
    assert_eq!(output.summary(), "nothing is in progress");
}
