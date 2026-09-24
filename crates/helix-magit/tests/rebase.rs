//! The two-run interactive rebase, against real repositories.

use std::fs;
use std::path::Path;
use std::process::Command;

use helix_magit::rebase;

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
    git(dir.path(), &["init", "--initial-branch=main"])?;
    git(dir.path(), &["config", "user.email", "t@e.invalid"])?;
    git(dir.path(), &["config", "user.name", "T"])?;
    for i in 1..=3 {
        fs::write(dir.path().join(format!("f{i}")), format!("{i}\n")).unwrap();
        git(dir.path(), &["add", "."])?;
        git(dir.path(), &["commit", "-m", &format!("c{i}")])?;
    }
    // A fixup for c2, which --autosquash moves under it.
    fs::write(dir.path().join("f2"), "2\nmore\n").unwrap();
    git(dir.path(), &["commit", "-a", "--fixup", "HEAD~1"])?;
    Some(dir)
}

fn subjects(dir: &Path) -> Vec<String> {
    git(dir, &["log", "--format=%s"])
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

fn args(extra: &[&str]) -> Vec<String> {
    ["rebase", "--interactive"]
        .iter()
        .chain(extra)
        .map(|arg| arg.to_string())
        .collect()
}

#[test]
fn the_first_run_captures_git_s_list_and_changes_nothing() {
    let Some(dir) = fixture() else {
        return;
    };
    let work = dir.path();
    let todo = work.join(".git").join("captured-todo");
    let before = subjects(work);
    let head = rebase::head(work);

    let output = rebase::capture_command(work, args(&["--autosquash", "HEAD~3"]), &todo)
        .run()
        .unwrap();
    assert!(!output.success);
    assert!(rebase::captured(&output.stderr), "{}", output.stderr);

    // git's own list, shaped by --autosquash.
    let list: Vec<String> = fs::read_to_string(&todo)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(|line| {
            let mut words = line.split_whitespace();
            let action = words.next().unwrap().to_string();
            let subject: Vec<&str> = words.skip(1).collect();
            format!("{action} {}", subject.join(" "))
        })
        .collect();
    assert_eq!(list, ["pick c2", "fixup fixup! c2", "pick c3"]);

    // Nothing happened to the repository.
    assert_eq!(subjects(work), before);
    assert_eq!(rebase::head(work), head);
    assert!(!work.join(".git/rebase-merge").exists());
}

#[test]
fn the_second_run_follows_the_edited_list() {
    let Some(dir) = fixture() else {
        return;
    };
    let work = dir.path();
    let todo = work.join(".git").join("captured-todo");
    rebase::capture_command(work, args(&["--autosquash", "HEAD~3"]), &todo)
        .run()
        .unwrap();

    // Swap c3 above c2 and drop nothing; the fixup stays under c2.
    let list = fs::read_to_string(&todo).unwrap();
    let mut lines: Vec<&str> = list
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .collect();
    let c3 = lines.pop().unwrap();
    lines.insert(0, c3);
    let edited = work.join(".git").join("edited-todo");
    fs::write(&edited, lines.join("\n") + "\n").unwrap();

    let output = rebase::install_command(work, args(&["--autosquash", "HEAD~3"]), &edited)
        .run()
        .unwrap();
    assert!(output.success, "{}", output.stderr);
    assert_eq!(subjects(work), ["c2", "c3", "c1"]);
    assert_eq!(fs::read_to_string(work.join("f2")).unwrap(), "2\nmore\n");
}
