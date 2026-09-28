//! Multi-step and special plans, against real repositories.

use std::fs;
use std::path::Path;
use std::process::Command;

use helix_magit::command::run_plan;
use helix_magit::resolve;
use helix_magit::transient::MagitCommand;

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

fn fixture() -> Option<tempfile::TempDir> {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path();
    git(work, &["init", "-q", "--initial-branch=main"])?;
    git(work, &["config", "user.email", "t@e.invalid"])?;
    git(work, &["config", "user.name", "T"])?;
    fs::write(work.join("a.txt"), "a\n").unwrap();
    fs::write(work.join("b.txt"), "b\n").unwrap();
    git(work, &["add", "."])?;
    git(work, &["commit", "-q", "-m", "initial"])?;
    Some(dir)
}

fn run(work: &Path, command: MagitCommand, answers: &[&str]) -> helix_magit::GitOutput {
    let plan = resolve(command, &[]).unwrap();
    let answers: Vec<String> = answers.iter().map(|a| a.to_string()).collect();
    run_plan(work, &plan.answered(&answers)).unwrap()
}

#[test]
fn stashing_the_worktree_leaves_the_index_alone() {
    let Some(dir) = fixture() else { return };
    let work = dir.path();
    fs::write(work.join("a.txt"), "staged\n").unwrap();
    git(work, &["add", "a.txt"]).unwrap();
    fs::write(work.join("b.txt"), "unstaged\n").unwrap();

    let output = run(work, MagitCommand::StashWorktree, &["wip"]);
    assert!(output.success, "{output:?}");

    // The staged change is still staged; the unstaged one is in the stash.
    assert_eq!(git(work, &["status", "--porcelain"]).unwrap(), "M  a.txt\n");
    let list = git(work, &["stash", "list"]).unwrap();
    assert_eq!(list.lines().count(), 1, "{list}");
    assert!(list.contains("wip"));
    assert_eq!(
        git(work, &["stash", "show", "--name-only", "stash@{0}"]).unwrap(),
        "b.txt\n"
    );
}

#[test]
fn stashing_the_worktree_with_nothing_staged_is_a_plain_stash() {
    let Some(dir) = fixture() else { return };
    let work = dir.path();
    fs::write(work.join("b.txt"), "unstaged\n").unwrap();
    // An older stash must not be touched.
    fs::write(work.join("a.txt"), "older\n").unwrap();
    git(work, &["stash", "push", "-q", "a.txt"]).unwrap();

    let output = run(work, MagitCommand::StashWorktree, &[""]);
    assert!(output.success, "{output:?}");
    assert_eq!(git(work, &["status", "--porcelain"]).unwrap(), "");
    assert_eq!(git(work, &["stash", "list"]).unwrap().lines().count(), 2);
}

#[test]
fn stashing_only_the_index() {
    let Some(dir) = fixture() else { return };
    let work = dir.path();
    fs::write(work.join("a.txt"), "staged\n").unwrap();
    git(work, &["add", "a.txt"]).unwrap();
    fs::write(work.join("b.txt"), "unstaged\n").unwrap();

    assert!(run(work, MagitCommand::StashIndex, &[""]).success);
    assert_eq!(git(work, &["status", "--porcelain"]).unwrap(), " M b.txt\n");
}

#[test]
fn ignoring_appends_once_to_the_right_file() {
    let Some(dir) = fixture() else { return };
    let work = dir.path();
    fs::write(work.join("junk.log"), "x\n").unwrap();
    fs::write(work.join("secret.txt"), "x\n").unwrap();

    assert!(run(work, MagitCommand::IgnoreShared, &["*.log"]).success);
    assert!(run(work, MagitCommand::IgnoreShared, &["*.log"]).success);
    assert_eq!(
        fs::read_to_string(work.join(".gitignore")).unwrap(),
        "*.log\n"
    );

    assert!(run(work, MagitCommand::IgnorePrivate, &["secret.txt"]).success);
    let exclude = fs::read_to_string(work.join(".git/info/exclude")).unwrap();
    assert!(exclude.ends_with("secret.txt\n"), "{exclude}");

    assert_eq!(
        git(work, &["status", "--porcelain"]).unwrap(),
        "?? .gitignore\n"
    );
}

#[test]
fn spinning_off_moves_unpushed_commits_to_a_new_branch() {
    let Some(dir) = fixture() else { return };
    let work = dir.path();
    let remote = dir.path().join("remote.git");
    git(work, &["init", "-q", "--bare", remote.to_str().unwrap()]).unwrap();
    git(work, &["remote", "add", "origin", remote.to_str().unwrap()]).unwrap();
    git(work, &["push", "-q", "-u", "origin", "main"]).unwrap();
    fs::write(work.join("a.txt"), "local\n").unwrap();
    git(work, &["commit", "-q", "-am", "unpushed"]).unwrap();

    let output = run(work, MagitCommand::BranchSpinoff, &["feature"]);
    assert!(output.success, "{output:?}");
    assert_eq!(
        git(work, &["symbolic-ref", "--short", "HEAD"])
            .unwrap()
            .trim(),
        "feature"
    );
    assert_eq!(
        git(work, &["rev-parse", "main"]).unwrap(),
        git(work, &["rev-parse", "origin/main"]).unwrap()
    );
    assert_eq!(
        git(work, &["log", "-1", "--format=%s", "feature"])
            .unwrap()
            .trim(),
        "unpushed"
    );
}

#[test]
fn spinning_out_stays_on_the_branch() {
    let Some(dir) = fixture() else { return };
    let work = dir.path();
    let remote = dir.path().join("remote.git");
    git(work, &["init", "-q", "--bare", remote.to_str().unwrap()]).unwrap();
    git(work, &["remote", "add", "origin", remote.to_str().unwrap()]).unwrap();
    git(work, &["push", "-q", "-u", "origin", "main"]).unwrap();
    fs::write(work.join("a.txt"), "local\n").unwrap();
    git(work, &["commit", "-q", "-am", "unpushed"]).unwrap();

    assert!(run(work, MagitCommand::BranchSpinout, &["feature"]).success);
    assert_eq!(
        git(work, &["symbolic-ref", "--short", "HEAD"])
            .unwrap()
            .trim(),
        "main"
    );
    assert_eq!(
        git(work, &["log", "-1", "--format=%s", "feature"])
            .unwrap()
            .trim(),
        "unpushed"
    );
    assert_eq!(
        git(work, &["log", "-1", "--format=%s"]).unwrap().trim(),
        "initial"
    );
}

#[test]
fn a_spinoff_without_an_upstream_changes_nothing() {
    let Some(dir) = fixture() else { return };
    let work = dir.path();
    let output = run(work, MagitCommand::BranchSpinoff, &["feature"]);
    assert!(!output.success);
    assert!(output.summary().contains("upstream"), "{output:?}");
    assert!(git(work, &["rev-parse", "--verify", "-q", "feature"]).is_none());
}

#[test]
fn a_tag_is_created_where_asked_and_a_remote_added_and_renamed() {
    let Some(dir) = fixture() else { return };
    let work = dir.path();
    assert!(run(work, MagitCommand::TagCreate, &["v1", ""]).success);
    assert_eq!(git(work, &["tag"]).unwrap(), "v1\n");

    assert!(
        run(
            work,
            MagitCommand::RemoteAdd,
            &["up", "https://example.invalid/x.git"]
        )
        .success
    );
    assert!(run(work, MagitCommand::RemoteRename, &["up", "upstream"]).success);
    assert_eq!(git(work, &["remote"]).unwrap(), "upstream\n");
}

#[test]
fn a_multi_step_operation_says_what_it_did() {
    let Some(dir) = fixture() else { return };
    let work = dir.path();
    fs::write(work.join("a.txt"), "staged\n").unwrap();
    git(work, &["add", "a.txt"]).unwrap();
    fs::write(work.join("b.txt"), "unstaged\n").unwrap();
    let output = run(work, MagitCommand::StashWorktree, &[""]);
    assert_eq!(
        output.summary(),
        "Stashed the worktree; the index is as it was"
    );
}
