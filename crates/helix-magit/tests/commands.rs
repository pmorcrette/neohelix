//! Running the resolved commands against real repositories.

use std::fs;
use std::path::Path;
use std::process::Command;

use helix_magit::command::{head_is_pushed, head_message, GitCommand};
use helix_magit::transient::MagitCommand;
use helix_magit::{resolve, Requirement};

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

/// A repository with one commit and a bare remote to push to.
fn fixture() -> Option<(tempfile::TempDir, std::path::PathBuf)> {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    let remote = dir.path().join("remote.git");
    fs::create_dir_all(&work).unwrap();

    Command::new("git")
        .args(["init", "--bare", remote.to_str()?])
        .output()
        .ok()?;
    git(
        dir.path(),
        &["init", "--initial-branch=main", work.to_str()?],
    )?;
    git(&work, &["config", "user.email", "t@e.invalid"])?;
    git(&work, &["config", "user.name", "T"])?;
    git(&work, &["remote", "add", "origin", remote.to_str()?])?;

    fs::write(work.join("f.txt"), "one\n").unwrap();
    git(&work, &["add", "."])?;
    git(&work, &["commit", "-m", "initial"])?;
    git(&work, &["push", "-u", "origin", "main"])?;

    Some((dir, work))
}

macro_rules! fixture_or_skip {
    () => {
        match fixture() {
            Some(pair) => pair,
            None => {
                eprintln!("skipping: no usable git binary");
                return;
            }
        }
    };
}

/// Runs a resolved plan, as the editor would.
fn run(work: &Path, command: MagitCommand, args: &[&str]) -> helix_magit::GitOutput {
    let args: Vec<String> = args.iter().map(ToString::to_string).collect();
    let plan = resolve(command, &args).expect("this action runs something");
    assert_eq!(
        plan.requirement,
        Requirement::None,
        "this plan still needs input"
    );
    GitCommand::new(work, plan.args).run().unwrap()
}

#[test]
fn a_commit_is_created_from_a_message_file() {
    let (_dir, work) = fixture_or_skip!();
    fs::write(work.join("f.txt"), "two\n").unwrap();
    git(&work, &["add", "f.txt"]).unwrap();

    // The editor writes the message to a file and passes it with `-F`, which
    // is what keeps the subprocess from wanting an editor.
    let message = work.join(".git").join("COMMIT_EDITMSG_HELIX");
    fs::write(&message, "a new commit\n\nwith a body\n").unwrap();

    let plan = resolve(MagitCommand::Commit, &[]).unwrap();
    let mut args = plan.args;
    args.push("-F".into());
    args.push(message.to_string_lossy().into_owned());

    let output = GitCommand::new(&work, args).run().unwrap();
    assert!(output.success, "{}", output.summary());

    assert_eq!(head_message(&work).unwrap(), "a new commit\n\nwith a body");
}

#[test]
fn extend_folds_the_staged_changes_into_head() {
    let (_dir, work) = fixture_or_skip!();
    let before = git(&work, &["rev-list", "--count", "HEAD"]).unwrap();

    fs::write(work.join("f.txt"), "extended\n").unwrap();
    git(&work, &["add", "f.txt"]).unwrap();

    let output = run(&work, MagitCommand::CommitExtend, &[]);
    assert!(output.success, "{}", output.summary());

    // The message is untouched and no commit was added.
    assert_eq!(head_message(&work).unwrap(), "initial");
    assert_eq!(
        git(&work, &["rev-list", "--count", "HEAD"]).unwrap(),
        before
    );
    assert_eq!(git(&work, &["show", "HEAD:f.txt"]).unwrap(), "extended\n");
}

#[test]
fn a_push_reaches_the_remote() {
    let (_dir, work) = fixture_or_skip!();
    fs::write(work.join("f.txt"), "pushed\n").unwrap();
    git(&work, &["add", "f.txt"]).unwrap();
    git(&work, &["commit", "-m", "second"]).unwrap();

    let output = run(&work, MagitCommand::Push, &[]);
    assert!(output.success, "{}", output.summary());

    // The remote now has the commit, which is what `head_is_pushed` reads.
    assert!(head_is_pushed(&work));
    let local = git(&work, &["rev-parse", "HEAD"]).unwrap();
    let remote = git(&work, &["rev-parse", "origin/main"]).unwrap();
    assert_eq!(local, remote);
}

#[test]
fn an_unpushed_commit_is_recognised_as_such() {
    let (_dir, work) = fixture_or_skip!();
    assert!(head_is_pushed(&work), "the fixture pushed its first commit");

    fs::write(work.join("f.txt"), "local only\n").unwrap();
    git(&work, &["add", "f.txt"]).unwrap();
    git(&work, &["commit", "-m", "local"]).unwrap();

    assert!(
        !head_is_pushed(&work),
        "a commit that was never pushed must not look pushed"
    );
}

#[test]
fn a_fetch_updates_the_remote_tracking_branch() {
    let (_dir, work) = fixture_or_skip!();

    // Another clone pushes something the first one has not seen.
    let other = work.parent().unwrap().join("other");
    let remote = work.parent().unwrap().join("remote.git");
    // The bare repo's HEAD may still point at an unborn default branch, so
    // the branch to work on is named explicitly.
    let cloned = Command::new("git")
        .args([
            "clone",
            "-b",
            "main",
            remote.to_str().unwrap(),
            other.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        cloned.status.success(),
        "clone failed: {}",
        String::from_utf8_lossy(&cloned.stderr)
    );
    git(&other, &["config", "user.email", "t@e.invalid"]).unwrap();
    git(&other, &["config", "user.name", "T"]).unwrap();
    fs::write(other.join("f.txt"), "from elsewhere\n").unwrap();
    git(&other, &["add", "f.txt"]).unwrap();
    git(&other, &["commit", "-m", "elsewhere"]).unwrap();
    git(&other, &["push"]).unwrap();

    let output = run(&work, MagitCommand::Fetch, &[]);
    assert!(output.success, "{}", output.summary());

    assert_eq!(
        git(&work, &["show", "origin/main:f.txt"]).unwrap(),
        "from elsewhere\n"
    );
}

#[test]
fn a_failing_command_reports_gits_own_message() {
    let (_dir, work) = fixture_or_skip!();
    // Nothing is staged, so the commit is refused.
    let plan = resolve(MagitCommand::CommitFixup, &[]).unwrap();
    let output = GitCommand::new(&work, plan.args).run().unwrap();

    assert!(!output.success);
    assert!(
        !output.summary().is_empty(),
        "the failure should carry a message"
    );
}

#[test]
fn a_command_that_would_want_an_editor_does_not_hang() {
    let (_dir, work) = fixture_or_skip!();
    fs::write(work.join("f.txt"), "amended\n").unwrap();
    git(&work, &["add", "f.txt"]).unwrap();

    // `commit --amend` without `--no-edit` or `-F` opens an editor. With the
    // environment this crate sets, it must finish instead of blocking.
    let started = std::time::Instant::now();
    let output = GitCommand::new(&work, vec!["commit".into(), "--amend".into()])
        .run()
        .unwrap();

    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "the command blocked on an editor"
    );
    assert!(output.success, "{}", output.summary());
    // `GIT_EDITOR=true` accepts the existing message unchanged.
    assert_eq!(head_message(&work).unwrap(), "initial");
}

#[test]
fn a_rebase_abort_outside_a_rebase_fails_without_hanging() {
    let (_dir, work) = fixture_or_skip!();
    let output = run(&work, MagitCommand::RebaseAbort, &[]);
    assert!(!output.success);
    assert!(!output.summary().is_empty());
}

fn commit_file(work: &Path, file: &str, content: &str, message: &str) {
    fs::write(work.join(file), content).unwrap();
    git(work, &["add", file]).unwrap();
    git(work, &["commit", "-q", "-m", message]).unwrap();
}

fn run_plan(work: &Path, plan: &helix_magit::Plan) -> helix_magit::GitOutput {
    helix_magit::command::run_plan(work, plan).unwrap()
}

#[test]
fn abort_ends_whatever_is_in_progress() {
    let (_dir, work) = fixture_or_skip!();
    let abort = resolve(MagitCommand::Abort, &[]).unwrap();
    assert!(abort.destructive);
    assert!(!run_plan(&work, &abort).success, "nothing to abort yet");

    git(&work, &["checkout", "-q", "-b", "other"]).unwrap();
    commit_file(&work, "f.txt", "theirs\n", "theirs");
    git(&work, &["checkout", "-q", "main"]).unwrap();
    commit_file(&work, "f.txt", "ours\n", "ours");
    assert!(
        git(&work, &["merge", "other"]).is_none(),
        "the merge conflicts"
    );
    assert!(work.join(".git/MERGE_HEAD").exists());

    let output = run_plan(&work, &abort);
    assert!(output.success, "{output:?}");
    assert!(!work.join(".git/MERGE_HEAD").exists());
    assert_eq!(fs::read_to_string(work.join("f.txt")).unwrap(), "ours\n");
}

#[test]
fn clean_names_what_it_would_remove_before_removing_it() {
    let (_dir, work) = fixture_or_skip!();
    commit_file(&work, ".gitignore", "build/\n", "ignore build");
    fs::write(work.join("u.txt"), "untracked\n").unwrap();
    fs::create_dir_all(work.join("build")).unwrap();
    fs::write(work.join("build/out"), "ignored\n").unwrap();

    let untracked = resolve(MagitCommand::CleanUntracked, &[]).unwrap();
    let ignored = resolve(MagitCommand::CleanIgnored, &[]).unwrap();
    let all = resolve(MagitCommand::CleanAll, &[]).unwrap();
    let preview =
        |plan: &helix_magit::Plan| helix_magit::command::clean_preview(&work, &plan.args).unwrap();
    assert_eq!(preview(&untracked), ["u.txt"]);
    assert_eq!(preview(&ignored), ["build/"]);
    assert_eq!(preview(&all), ["build/", "u.txt"]);
    // A preview removes nothing.
    assert!(work.join("u.txt").exists() && work.join("build/out").exists());

    assert!(run_plan(&work, &untracked).success);
    assert!(!work.join("u.txt").exists() && work.join("build/out").exists());
    assert!(run_plan(&work, &ignored).success);
    assert!(!work.join("build").exists());
    assert!(preview(&all).is_empty());
}

#[test]
fn editing_a_commit_stops_the_rebase_there_unless_it_is_pushed() {
    let (_dir, work) = fixture_or_skip!();
    commit_file(&work, "f.txt", "two\n", "second");
    let second = git(&work, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string();
    commit_file(&work, "f.txt", "three\n", "third");

    let edit = |rev: &str| {
        let mut plan = helix_magit::Plan::new([rev.to_string()], "Edit");
        plan.special = Some(helix_magit::Special::EditCommit);
        run_plan(&work, &plan)
    };

    // `initial` is on origin/main.
    let refused = edit("HEAD~2");
    assert!(!refused.success);
    assert!(refused.summary().contains("already pushed"), "{refused:?}");

    let output = edit(&second);
    assert!(output.success, "{output:?}");
    assert!(output.summary().starts_with("Stopped at"), "{output:?}");
    assert_eq!(
        git(&work, &["rev-parse", "HEAD"]).unwrap().trim(),
        second,
        "the rebase stopped at the commit"
    );
    assert!(work.join(".git/rebase-merge").exists());
    git(&work, &["rebase", "--abort"]).unwrap();
}

#[test]
fn reshelving_gives_the_commits_dates_a_minute_apart() {
    let (_dir, work) = fixture_or_skip!();
    commit_file(&work, "f.txt", "two\n", "second");
    commit_file(&work, "f.txt", "three\n", "third");

    let plan = resolve(MagitCommand::Reshelve, &[])
        .unwrap()
        // Relative to HEAD, which moves while the dates are rewritten.
        .answered(&["HEAD~2".into(), "2020-01-02 03:04:00 +0000".into()]);
    assert!(plan.destructive);
    let output = run_plan(&work, &plan);
    assert!(output.success, "{output:?}");

    let dates = git(&work, &["log", "--format=%s %at %ct", "origin/main..HEAD"]).unwrap();
    assert_eq!(
        dates.lines().collect::<Vec<_>>(),
        [
            "third 1577934300 1577934300",
            "second 1577934240 1577934240"
        ]
    );
    // What is pushed is refused.
    let pushed = resolve(MagitCommand::Reshelve, &[])
        .unwrap()
        .answered(&["HEAD~3".into(), "2020-01-02".into()]);
    assert!(!run_plan(&work, &pushed).success);
}

/// `initial` pushed; then `a` (f.txt's lines), `b` (g.txt) and `h` unpushed.
/// Staged: a change to a line of `a`'s, one of `b`'s and one of the pushed
/// commit's; unstaged: a change to another file.
fn absorb_fixture(work: &Path) {
    commit_file(work, "f.txt", "one\nA1\nA2\nA3\n", "a");
    commit_file(work, "g.txt", "B1\nB2\n", "b");
    commit_file(work, "h.txt", "h\n", "h");
    fs::write(work.join("f.txt"), "ONE\nA1\nA2 fixed\nA3\n").unwrap();
    fs::write(work.join("g.txt"), "B1\nB2\nB3\n").unwrap();
    git(work, &["add", "f.txt", "g.txt"]).unwrap();
    fs::write(work.join("h.txt"), "h, unstaged\n").unwrap();
}

fn absorb_plan(command: MagitCommand) -> helix_magit::Plan {
    let plan = resolve(command, &[]).unwrap();
    assert!(plan.destructive, "history is rewritten: confirmed first");
    plan
}

#[test]
fn absorb_folds_each_hunk_into_its_commit_and_leaves_the_rest() {
    let (_dir, work) = fixture_or_skip!();
    absorb_fixture(&work);
    let output = run_plan(&work, &absorb_plan(MagitCommand::CommitAbsorb));
    assert!(output.success, "{output:?}");
    assert!(output.summary().contains("Absorbed 2 hunks"), "{output:?}");
    assert!(
        output.summary().contains("1 hunk left staged"),
        "{output:?}"
    );

    // No fixup! commit left: squashed into a and b.
    let subjects = git(&work, &["log", "--format=%s", "origin/main..HEAD"]).unwrap();
    assert_eq!(subjects.lines().collect::<Vec<_>>(), ["h", "b", "a"]);
    assert_eq!(
        git(&work, &["show", "HEAD~2:f.txt"]).unwrap(),
        "one\nA1\nA2 fixed\nA3\n"
    );
    assert_eq!(
        git(&work, &["show", "HEAD~1:g.txt"]).unwrap(),
        "B1\nB2\nB3\n"
    );
    // The pushed commit's line is still staged; the unstaged change is
    // still in the working tree, unstaged.
    assert_eq!(
        git(&work, &["diff", "--cached", "--name-only"]).unwrap(),
        "f.txt\n"
    );
    assert!(git(&work, &["diff", "--cached"]).unwrap().contains("+ONE"));
    assert_eq!(git(&work, &["diff", "--name-only"]).unwrap(), "h.txt\n");
    assert_eq!(
        fs::read_to_string(work.join("f.txt")).unwrap(),
        "ONE\nA1\nA2 fixed\nA3\n"
    );
}

#[test]
fn autofixup_makes_the_fixup_commits_and_stops_there() {
    let (_dir, work) = fixture_or_skip!();
    absorb_fixture(&work);
    let output = run_plan(&work, &absorb_plan(MagitCommand::CommitAutofixup));
    assert!(output.success, "{output:?}");

    let subjects = git(&work, &["log", "--format=%s", "origin/main..HEAD"]).unwrap();
    let subjects: Vec<&str> = subjects.lines().collect();
    assert_eq!(subjects[..2], ["fixup! b", "fixup! a"]);
    // With its context, f.txt's change is one hunk, touching the pushed
    // line and a's: a is the one unpushed commit among them.
    assert!(git(&work, &["diff", "--cached"]).unwrap().is_empty());
    assert_eq!(git(&work, &["diff", "--name-only"]).unwrap(), "h.txt\n");
}

#[test]
fn absorb_refuses_when_nothing_belongs_to_an_unpushed_commit() {
    let (_dir, work) = fixture_or_skip!();
    fs::write(work.join("f.txt"), "changed\n").unwrap();
    git(&work, &["add", "f.txt"]).unwrap();
    let head = git(&work, &["rev-parse", "HEAD"]).unwrap();
    let output = run_plan(&work, &absorb_plan(MagitCommand::CommitAbsorb));
    assert!(!output.success);
    assert!(
        output.summary().contains("every commit is pushed"),
        "{output:?}"
    );
    assert_eq!(git(&work, &["rev-parse", "HEAD"]).unwrap(), head);
    assert_eq!(
        git(&work, &["diff", "--cached", "--name-only"]).unwrap(),
        "f.txt\n"
    );
}

/// Commits the staged changes with `message`, as the editor does once the
/// message buffer is written: the plan's arguments, then `-F`.
fn commit_with(work: &Path, plan: &helix_magit::Plan, message: &str) -> helix_magit::GitOutput {
    let file = work.join(".git").join("COMMIT_EDITMSG_HELIX");
    fs::write(&file, message).unwrap();
    let mut args = plan.args.clone();
    args.push("-F".into());
    args.push(file.to_string_lossy().into_owned());
    GitCommand::new(work, args).run().unwrap()
}

/// Three unpushed commits on top of the pushed one: a, b and c.
fn three_commits(work: &Path) {
    for (name, subject) in [("a.txt", "Add a"), ("b.txt", "Add b"), ("c.txt", "Add c")] {
        fs::write(work.join(name), format!("{name}\n")).unwrap();
        git(work, &["add", name]).unwrap();
        git(work, &["commit", "-m", subject]).unwrap();
    }
}

fn subjects(work: &Path) -> String {
    git(work, &["log", "--format=%s", "origin/main..HEAD"]).unwrap()
}

#[test]
fn reword_changes_heads_message_and_leaves_the_index() {
    let (_dir, work) = fixture_or_skip!();
    three_commits(&work);
    fs::write(work.join("staged.txt"), "x\n").unwrap();
    git(&work, &["add", "staged.txt"]).unwrap();

    let plan = resolve(MagitCommand::CommitReword, &[]).unwrap();
    let output = commit_with(&work, &plan, "Add c, reworded\n");
    assert!(output.success, "{}", output.summary());

    assert_eq!(subjects(&work), "Add c, reworded\nAdd b\nAdd a\n");
    // What was staged is still staged, not committed.
    assert_eq!(
        git(&work, &["status", "--porcelain"]).unwrap(),
        "A  staged.txt\n"
    );
    assert!(git(&work, &["show", "HEAD:staged.txt"]).is_none());
}

#[test]
fn an_instant_fixup_lands_in_its_commit() {
    let (_dir, work) = fixture_or_skip!();
    three_commits(&work);
    fs::write(work.join("a.txt"), "a.txt fixed\n").unwrap();
    git(&work, &["add", "a.txt"]).unwrap();
    // Something else staged and unstaged survives the rebase as it was.
    fs::write(work.join("c.txt"), "c.txt unstaged\n").unwrap();

    let plan = resolve(MagitCommand::CommitInstantFixup, &[])
        .unwrap()
        .answered(&["HEAD~2".into()]);
    let output = helix_magit::command::run_plan(&work, &plan).unwrap();
    assert!(output.success, "{}", output.summary());

    assert_eq!(subjects(&work), "Add c\nAdd b\nAdd a\n");
    let a = git(&work, &["log", "--format=%h", "-1", "--grep=Add a"]).unwrap();
    assert_eq!(
        git(&work, &["show", &format!("{}:a.txt", a.trim())]).unwrap(),
        "a.txt fixed\n"
    );
    assert_eq!(
        git(&work, &["status", "--porcelain"]).unwrap(),
        " M c.txt\n"
    );
}

#[test]
fn an_instant_fixup_of_a_pushed_commit_is_refused() {
    let (_dir, work) = fixture_or_skip!();
    fs::write(work.join("f.txt"), "changed\n").unwrap();
    git(&work, &["add", "f.txt"]).unwrap();
    let before = git(&work, &["rev-parse", "HEAD"]).unwrap();

    let plan = resolve(MagitCommand::CommitInstantFixup, &[])
        .unwrap()
        .answered(&["HEAD".into()]);
    let output = helix_magit::command::run_plan(&work, &plan).unwrap();
    assert!(!output.success);
    assert!(
        output.summary().contains("already pushed"),
        "{}",
        output.summary()
    );
    // Refused before anything was committed.
    assert_eq!(git(&work, &["rev-parse", "HEAD"]).unwrap(), before);
}

#[test]
fn alter_and_revise_replace_a_commits_message_when_squashed_in() {
    let (_dir, work) = fixture_or_skip!();
    three_commits(&work);

    // Revise: the message alone, as the buffer offers it, edited.
    let plan = resolve(MagitCommand::CommitRevise, &[])
        .unwrap()
        .answered(&["HEAD~1".into()]);
    let Requirement::CommitMessage { seed } = &plan.requirement else {
        panic!("revise composes a message");
    };
    let template = helix_magit::command::commit_template_for(&work, seed, false);
    assert!(
        template.starts_with("amend! Add b\n\nAdd b\n"),
        "{template}"
    );
    let edited = template.replacen("\n\nAdd b\n", "\n\nAdd b, revised\n", 1);
    let message = helix_magit::command::strip_comments(&edited);
    let output = commit_with(&work, &plan, &message);
    assert!(output.success, "{}", output.summary());

    helix_magit::absorb::fold_in(&work, "HEAD~2").unwrap();
    assert_eq!(subjects(&work), "Add c\nAdd b, revised\nAdd a\n");
}

#[test]
fn an_instant_squash_adds_its_words_to_the_commit() {
    let (_dir, work) = fixture_or_skip!();
    three_commits(&work);
    fs::write(work.join("b.txt"), "b.txt more\n").unwrap();
    git(&work, &["add", "b.txt"]).unwrap();

    let plan = resolve(MagitCommand::CommitInstantSquash, &[])
        .unwrap()
        .answered(&["HEAD~1".into()]);
    assert_eq!(plan.fold_into.as_deref(), Some("HEAD~1"));
    // The editor resolves the target before committing, since the new
    // commit moves what HEAD~1 means.
    let target = git(&work, &["rev-parse", "HEAD~1"]).unwrap();
    let output = commit_with(&work, &plan, "And more of b\n");
    assert!(output.success, "{}", output.summary());
    helix_magit::absorb::fold_in(&work, target.trim()).unwrap();

    assert_eq!(subjects(&work), "Add c\nAdd b\nAdd a\n");
    let b = git(&work, &["log", "--format=%B", "-1", "--grep=Add b"]).unwrap();
    assert_eq!(b.trim_end(), "Add b\n\nAnd more of b");
}
