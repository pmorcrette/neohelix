//! The status overview, read from real repositories.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use helix_magit::status::{self, Operation};

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

fn commit(work: &Path, file: &str, content: &str, message: &str) -> Option<()> {
    fs::write(work.join(file), content).unwrap();
    git(work, &["add", file])?;
    git(work, &["commit", "-q", "-m", message])?;
    Some(())
}

/// `work` tracks `origin/main`; `other` is a second clone that pushes to it.
fn fixture() -> Option<(tempfile::TempDir, PathBuf, PathBuf)> {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    let other = dir.path().join("other");
    let remote = dir.path().join("remote.git");

    git(dir.path(), &["init", "-q", "--bare", remote.to_str()?])?;
    git(
        dir.path(),
        &["init", "-q", "--initial-branch=main", work.to_str()?],
    )?;
    for (key, value) in [("user.email", "t@e.invalid"), ("user.name", "T")] {
        git(&work, &["config", key, value])?;
    }
    git(&work, &["remote", "add", "origin", remote.to_str()?])?;
    commit(&work, "f.txt", "one\n", "initial")?;
    git(&work, &["push", "-q", "-u", "origin", "main"])?;

    git(
        dir.path(),
        &[
            "clone",
            "-q",
            "-b",
            "main",
            remote.to_str()?,
            other.to_str()?,
        ],
    )?;
    for (key, value) in [("user.email", "t@e.invalid"), ("user.name", "T")] {
        git(&other, &["config", key, value])?;
    }
    Some((dir, work, other))
}

macro_rules! fixture_or_skip {
    () => {
        match fixture() {
            Some(fixture) => fixture,
            None => {
                eprintln!("skipping: no usable git binary");
                return;
            }
        }
    };
}

#[test]
fn a_clean_branch_shows_its_head_upstream_and_recent_commits() {
    let (_dir, work, _other) = fixture_or_skip!();
    let overview = status::read(&work);

    assert_eq!(overview.branch.as_deref(), Some("main"));
    assert_eq!(overview.head.as_ref().unwrap().subject, "initial");
    let upstream = overview.upstream.unwrap();
    assert_eq!(upstream.name, "origin/main");
    assert_eq!(upstream.commit.unwrap().subject, "initial");
    // Pushing goes to the upstream, so there is no separate push line.
    assert_eq!(overview.push, None);
    assert!(overview.unpushed.is_empty() && overview.unpulled.is_empty());
    // Nothing unpushed: the recent commits stand in.
    assert_eq!(overview.recent.len(), 1);
    assert_eq!(overview.in_progress, None);
}

#[test]
fn divergence_from_the_upstream_is_listed_both_ways() {
    let (_dir, work, other) = fixture_or_skip!();
    commit(&other, "g.txt", "theirs\n", "their change").unwrap();
    git(&other, &["push", "-q"]).unwrap();
    commit(&work, "h.txt", "mine\n", "my change").unwrap();
    git(&work, &["fetch", "-q"]).unwrap();

    let overview = status::read(&work);
    let subjects = |commits: &[status::Commit]| -> Vec<String> {
        commits.iter().map(|c| c.subject.clone()).collect()
    };
    assert_eq!(subjects(&overview.unpushed), ["my change"]);
    assert_eq!(subjects(&overview.unpulled), ["their change"]);
    // Unpushed commits replace the recent ones.
    assert!(overview.recent.is_empty());
}

#[test]
fn stashes_are_listed_newest_first() {
    let (_dir, work, _other) = fixture_or_skip!();
    fs::write(work.join("f.txt"), "two\n").unwrap();
    git(&work, &["stash", "push", "-q", "-m", "first"]).unwrap();
    fs::write(work.join("f.txt"), "three\n").unwrap();
    git(&work, &["stash", "push", "-q", "-m", "second"]).unwrap();

    let overview = status::read(&work);
    let names: Vec<_> = overview
        .stashes
        .iter()
        .map(|s| (s.name.as_str(), s.subject.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("stash@{0}", "On main: second"),
            ("stash@{1}", "On main: first")
        ]
    );
}

#[test]
fn a_conflicted_merge_and_cherry_pick_are_reported() {
    let (_dir, work, _other) = fixture_or_skip!();
    git(&work, &["checkout", "-q", "-b", "topic"]).unwrap();
    commit(&work, "f.txt", "topic\n", "topic edit").unwrap();
    git(&work, &["checkout", "-q", "main"]).unwrap();
    commit(&work, "f.txt", "main\n", "main edit").unwrap();

    // Conflicts, so the merge stops half-way.
    assert!(git(&work, &["merge", "topic"]).is_none());
    let merge = status::read(&work).in_progress.unwrap();
    assert_eq!(merge.operation, Operation::Merge);
    assert!(
        merge.description.contains("Merge branch 'topic'"),
        "{}",
        merge.description
    );
    git(&work, &["merge", "--abort"]).unwrap();

    assert!(git(&work, &["cherry-pick", "topic"]).is_none());
    let pick = status::read(&work).in_progress.unwrap();
    assert_eq!(pick.operation, Operation::CherryPick);
    assert!(
        pick.description.ends_with("topic edit"),
        "{}",
        pick.description
    );
    git(&work, &["cherry-pick", "--abort"]).unwrap();

    assert_eq!(status::read(&work).in_progress, None);
}

#[test]
fn a_detached_head_or_an_empty_repository_is_not_an_error() {
    let (_dir, work, _other) = fixture_or_skip!();
    git(&work, &["checkout", "-q", "--detach"]).unwrap();
    let overview = status::read(&work);
    assert_eq!(overview.branch, None);
    assert_eq!(overview.upstream, None);
    assert!(overview.head.is_some());

    let empty = tempfile::tempdir().unwrap();
    git(empty.path(), &["init", "-q"]).unwrap();
    let overview = status::read(empty.path());
    assert_eq!(overview.head, None);
    assert!(overview.recent.is_empty());
}

#[test]
fn conflicted_paths_are_unmerged_rather_than_staged() {
    let (_dir, work, _other) = fixture_or_skip!();
    git(&work, &["checkout", "-q", "-b", "topic"]).unwrap();
    commit(&work, "f.txt", "topic\n", "topic edit").unwrap();
    commit(&work, "clean.txt", "clean\n", "clean add").unwrap();
    git(&work, &["checkout", "-q", "main"]).unwrap();
    commit(&work, "f.txt", "main\n", "main edit").unwrap();
    assert!(git(&work, &["merge", "--no-commit", "topic"]).is_none());

    let repository = helix_magit::Repository::discover(&work).unwrap();
    let unmerged = repository.unmerged().unwrap();
    assert_eq!(unmerged.len(), 1);
    assert_eq!(unmerged[0].path, Path::new("f.txt"));
    assert_eq!(unmerged[0].state, "both modified");

    // The merge's clean part is staged; the conflicted file is not listed
    // there once per stage.
    let staged: Vec<PathBuf> = repository
        .staged_diff()
        .unwrap()
        .into_iter()
        .map(|file| file.path)
        .collect();
    assert_eq!(staged, [PathBuf::from("clean.txt")]);
}

#[test]
fn other_worktrees_are_listed_and_this_one_is_not() {
    let (dir, work, _other) = fixture_or_skip!();
    let linked = dir.path().join("linked");
    git(
        &work,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            linked.to_str().unwrap(),
        ],
    )
    .unwrap();

    let overview = status::read(&work);
    assert_eq!(overview.worktrees.len(), 1);
    assert_eq!(overview.worktrees[0].branch.as_deref(), Some("side"));
    assert!(overview.submodules.is_empty());
}

#[test]
fn refs_and_cherries_are_read() {
    let (_dir, work, _other) = fixture_or_skip!();
    commit(&work, "h.txt", "mine\n", "my change").unwrap();
    git(&work, &["tag", "v1"]).unwrap();

    let refs = helix_magit::refs::read_refs(&work);
    let main = refs.iter().find(|info| info.name == "main").unwrap();
    assert!(main.is_head);
    assert_eq!(main.upstream.as_deref(), Some("origin/main"));
    let origin = refs.iter().find(|info| info.name == "origin/main").unwrap();
    assert_eq!(origin.relation(), "1 behind");
    assert!(refs.iter().any(|info| info.name == "v1"));

    let cherries = helix_magit::refs::cherries(&work, "origin/main", None).unwrap();
    assert_eq!(cherries.len(), 1);
    assert_eq!(cherries[0].subject, "my change");
    assert!(!cherries[0].equivalent);

    assert_eq!(
        helix_magit::refs::names(&work, helix_magit::AskKind::Remote),
        ["origin"]
    );
}

#[test]
fn a_sparse_checkout_lists_its_directories() {
    let (_dir, work, _other) = fixture_or_skip!();
    assert_eq!(status::read(&work).sparse, None);

    fs::create_dir_all(work.join("docs")).unwrap();
    fs::create_dir_all(work.join("src")).unwrap();
    commit(&work, "docs/a.md", "a\n", "docs").unwrap();
    commit(&work, "src/b.rs", "b\n", "src").unwrap();
    git(&work, &["sparse-checkout", "set", "--cone"]).unwrap();
    assert_eq!(status::read(&work).sparse, Some(Vec::new()));
    assert!(!work.join("docs/a.md").exists());

    git(&work, &["sparse-checkout", "add", "docs"]).unwrap();
    assert_eq!(status::read(&work).sparse, Some(vec!["docs".to_string()]));
    assert!(work.join("docs/a.md").exists() && !work.join("src/b.rs").exists());

    git(&work, &["sparse-checkout", "disable"]).unwrap();
    assert_eq!(status::read(&work).sparse, None);
}

#[test]
fn tags_around_head_are_found_with_their_distances() {
    let Some((_dir, work, _other)) = fixture() else {
        return;
    };
    git(&work, &["tag", "v1"]).unwrap();
    commit(&work, "f.txt", "two\n", "two").unwrap();
    commit(&work, "f.txt", "three\n", "three").unwrap();
    git(&work, &["tag", "-a", "-m", "v2", "v2"]).unwrap();
    git(&work, &["checkout", "-q", "HEAD~1"]).unwrap();

    let overview = status::read(&work);
    let tag = overview.tag.unwrap();
    assert_eq!((tag.name.as_str(), tag.distance), ("v1", 1));
    let next = overview.next_tag.unwrap();
    assert_eq!((next.name.as_str(), next.distance), ("v2", 1));

    // HEAD tagged: that tag, and no next one.
    git(&work, &["checkout", "-q", "v2"]).unwrap();
    let overview = status::read(&work);
    assert_eq!(overview.tag.unwrap().distance, 0);
    assert_eq!(overview.next_tag, None);
}

#[test]
fn a_stopped_rebase_lists_its_steps_newest_first() {
    use helix_magit::status::StepState;
    let Some((_dir, work, _other)) = fixture() else {
        return;
    };
    for n in 1..=3 {
        commit(&work, &format!("{n}.txt"), "x\n", &format!("c{n}")).unwrap();
    }
    // Stop at the second of the three.
    let out = Command::new("git")
        .current_dir(&work)
        .args(["rebase", "-q", "-i", "HEAD~3"])
        .env("GIT_SEQUENCE_EDITOR", "sed -i '2s/^pick/edit/'")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");

    let overview = status::read(&work);
    let steps: Vec<(StepState, &str, &str)> = overview
        .sequence
        .iter()
        .map(|step| (step.state, step.action.as_str(), step.subject.as_str()))
        .collect();
    assert_eq!(
        steps,
        [
            (StepState::Todo, "pick", "c3"),
            (StepState::Current, "edit", "c2"),
            (StepState::Done, "pick", "c1"),
            (StepState::Onto, "onto", "initial"),
        ]
    );
    assert!(overview.sequence[0].hash.len() >= 7);
    git(&work, &["rebase", "--abort"]).unwrap();
    assert!(status::read(&work).sequence.is_empty());
}

#[test]
fn a_bisect_log_and_flagged_files_are_listed() {
    let Some((_dir, work, _other)) = fixture() else {
        return;
    };
    for n in 1..=4 {
        commit(&work, "f.txt", &format!("{n}\n"), &format!("c{n}")).unwrap();
    }
    git(&work, &["bisect", "start", "HEAD", "HEAD~4"]).unwrap();
    git(&work, &["bisect", "good"]).unwrap();
    let overview = status::read(&work);
    let verdicts: Vec<&str> = overview
        .bisect_log
        .iter()
        .map(|entry| entry.verdict.as_str())
        .collect();
    assert_eq!(verdicts, ["bad", "good", "good"]);
    assert_eq!(overview.bisect_log[0].subject, "c4");
    git(&work, &["bisect", "reset"]).unwrap();
    assert!(status::read(&work).bisect_log.is_empty());

    commit(&work, "g.txt", "g\n", "g").unwrap();
    git(&work, &["update-index", "--assume-unchanged", "f.txt"]).unwrap();
    git(&work, &["update-index", "--skip-worktree", "g.txt"]).unwrap();
    let overview = status::read(&work);
    assert_eq!(overview.assumed, ["f.txt"]);
    assert_eq!(overview.skipped, ["g.txt"]);
}
