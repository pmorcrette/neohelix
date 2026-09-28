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

#[test]
fn a_file_s_log_follows_it_through_a_rename_and_blame_names_each_line() {
    let Some(dir) = fixture() else { return };
    let work = dir.path();
    git(work, &["mv", "b.txt", "bee.txt"]).unwrap();
    git(work, &["commit", "-q", "-m", "Rename the bee"]).unwrap();
    fs::write(work.join("bee.txt"), "bee\nbuzz\n").unwrap();
    git(work, &["commit", "-q", "-am", "Buzz"]).unwrap();

    let filter = LogFilter {
        path: Some(PathBuf::from("bee.txt")),
        follow: true,
        ..LogFilter::default()
    };
    assert_eq!(
        subjects(&read_log(work, &filter).unwrap()),
        ["Buzz", "Rename the bee", "Fix the bee"]
    );

    let lines = helix_magit::blame::blame(work, Path::new("bee.txt"), None).unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].summary, "Fix the bee");
    assert_eq!(lines[0].author, "Bob");
    assert_eq!(lines[0].filename, "b.txt", "blame follows the rename");
    assert_eq!(lines[1].summary, "Buzz");
    assert_eq!(lines[1].content, "buzz");

    // An edit not committed yet is blamed on nobody.
    fs::write(work.join("bee.txt"), "bee\nbuzz\nhum\n").unwrap();
    let lines = helix_magit::blame::blame(work, Path::new("bee.txt"), None).unwrap();
    assert!(lines[2].is_uncommitted());

    // As of an earlier revision.
    let lines = helix_magit::blame::blame(work, Path::new("b.txt"), Some("HEAD~2")).unwrap();
    assert_eq!(lines.len(), 1);
}

#[test]
fn a_commit_and_a_range_are_diffed_with_the_options_asked_for() {
    use helix_magit::diff::{DiffOptions, Whitespace};
    use helix_magit::log::{diff_range, show_with};

    let Some(dir) = fixture() else { return };
    let work = dir.path();
    fs::write(
        work.join("a.txt"),
        "one\n  two\nthree\nfour\nfive\nsix\nseven\n",
    )
    .unwrap();
    git(work, &["commit", "-q", "-am", "Lines"]).unwrap();
    // Reindent only.
    fs::write(
        work.join("a.txt"),
        "one\ntwo\nthree\nfour\nfive\nsix\nseven\n",
    )
    .unwrap();
    git(work, &["commit", "-q", "-am", "Reindent"]).unwrap();

    let plain = show_with(work, "HEAD", &DiffOptions::default()).unwrap();
    assert_eq!(plain.files.len(), 1);
    let ignoring = DiffOptions {
        whitespace: Whitespace::IgnoreAll,
        ..DiffOptions::default()
    };
    // Nothing but whitespace changed: nothing to show.
    let blank = show_with(work, "HEAD", &ignoring).unwrap();
    assert!(
        blank.files.iter().all(|file| file.hunks.is_empty()),
        "{:?}",
        blank.files
    );

    let wide = DiffOptions {
        context: 10,
        ..DiffOptions::default()
    };
    let hunk = &show_with(work, "HEAD", &wide).unwrap().files[0].hunks[0];
    assert_eq!(
        hunk.lines.len(),
        8,
        "all seven lines, the changed one twice"
    );

    // Between two revisions, and against the working tree.
    let range = diff_range(work, "HEAD~2", Some("HEAD"), &DiffOptions::default()).unwrap();
    assert_eq!(range.len(), 1);
    fs::write(work.join("new.txt"), "x\n").unwrap();
    git(work, &["add", "new.txt"]).unwrap();
    let against_tree = diff_range(work, "HEAD", None, &DiffOptions::default()).unwrap();
    assert_eq!(against_tree[0].path, PathBuf::from("new.txt"));
    assert!(diff_range(work, "--output=x", None, &DiffOptions::default()).is_err());
}

#[test]
fn the_reflog_lists_where_head_has_been() {
    let Some(dir) = fixture() else { return };
    let work = dir.path();
    // A hard reset away from the merge: the commit is only in the reflog.
    git(work, &["reset", "-q", "--hard", "HEAD~1"]).unwrap();

    let filter = LogFilter {
        reflog: true,
        ..LogFilter::default()
    };
    let entries = read_log(work, &filter).unwrap();
    assert!(
        entries.iter().all(|entry| entry.hash.is_some()),
        "no graph lines"
    );
    assert_eq!(entries[0].refs, ["HEAD@{0}"]);
    assert!(
        entries[0].subject.starts_with("reset: moving to"),
        "{:?}",
        entries[0]
    );
    assert_eq!(entries[1].refs, ["HEAD@{1}"]);
    assert!(
        entries[1].subject.starts_with("merge topic"),
        "{:?}",
        entries[1]
    );
    assert_eq!(filter.describe(), "reflog of HEAD");

    let topic = LogFilter {
        reflog: true,
        range: Some("topic".into()),
        ..LogFilter::default()
    };
    let entries = read_log(work, &topic).unwrap();
    assert_eq!(entries[0].refs, ["topic@{0}"]);
}

#[test]
fn the_new_filters_and_starting_points_narrow_the_log() {
    let Some(dir) = fixture() else { return };
    let work = dir.path();
    let by = |filter: LogFilter| -> Vec<String> {
        subjects(&read_log(work, &filter).unwrap())
            .into_iter()
            .map(str::to_string)
            .collect()
    };
    let from = |args: &[&str]| {
        LogFilter::from_args(&args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>())
    };

    let mut no_merges = by(from(&["--no-merges"]));
    no_merges.sort();
    assert_eq!(no_merges, ["First commit", "Fix the bee", "Topic work"]);
    assert_eq!(by(from(&["-Sbee"])), ["Fix the bee"]);
    assert_eq!(by(from(&["-Gtop.c"])), ["Topic work"]);
    assert_eq!(by(from(&["--until=1970-01-02"])), Vec::<String>::new());

    // Oldest first, without a graph: every line is a commit.
    let reversed = read_log(work, &from(&["--reverse"])).unwrap();
    assert_eq!(subjects(&reversed).first(), Some(&"First commit"));
    assert!(reversed.iter().all(|entry| entry.hash.is_some()));
    let plain = read_log(work, &from(&["--no-graph", "--no-decorate"])).unwrap();
    assert!(plain
        .iter()
        .all(|entry| entry.graph.is_empty() && entry.refs.is_empty()));

    // `-L`: the file after the last colon, the lines before it.
    let traced = from(&["-L1,1:b.txt"]);
    assert_eq!(traced.lines.as_deref(), Some("1,1"));
    assert_eq!(traced.path, Some(PathBuf::from("b.txt")));
    assert_eq!(by(traced), ["Fix the bee"]);

    // Another branch as a starting point, from main.
    git(work, &["branch", "side", "v1"]).unwrap();
    git(work, &["checkout", "-q", "side"]).unwrap();
    commit(work, "Ann", "s.txt", "s\n", "Side work").unwrap();
    git(work, &["checkout", "-q", "main"]).unwrap();
    let branches = LogFilter {
        revs: vec!["HEAD".into(), "--branches".into()],
        ..LogFilter::default()
    };
    assert!(by(branches).contains(&"Side work".to_string()));

    // The merge that brought a topic commit into main.
    let topic = git(work, &["rev-parse", "topic"]).unwrap();
    let merge = helix_magit::log::merged_by(work, topic.trim(), "main").unwrap();
    assert_eq!(merge, git(work, &["rev-parse", "main"]).unwrap().trim());
    assert!(helix_magit::log::merged_by(work, "side", "main")
        .unwrap_err()
        .contains("does not contain"));
    let bee = git(work, &["rev-parse", "main^1"]).unwrap();
    assert!(helix_magit::log::merged_by(work, bee.trim(), "main")
        .unwrap_err()
        .contains("without a merge"));
}

#[test]
fn blame_follows_moved_lines_and_blames_in_reverse() {
    use helix_magit::blame::{blame_with, BlameOptions};
    let Some(dir) = fixture() else { return };
    let work = dir.path();
    let block = "fn a_moved_function_with_a_long_name() {\n    call_something_long_enough();\n}\n";
    // More lines stay than move, so the diff moves the block.
    let rest: String = (1..=5)
        .map(|n| format!("// the rest, line {n}\n"))
        .collect();
    commit(work, "Ann", "m.rs", &format!("{block}{rest}"), "Write m").unwrap();
    let first = git(work, &["rev-parse", "--short=7", "HEAD"]).unwrap();
    commit(
        work,
        "Bob",
        "m.rs",
        &format!("{rest}{block}"),
        "Move the block",
    )
    .unwrap();
    let moved = git(work, &["rev-parse", "--short=7", "HEAD"]).unwrap();
    commit(work, "Bob", "m.rs", &rest, "Drop the block").unwrap();

    let commits = |options: &BlameOptions, rev: Option<&str>| -> Vec<String> {
        blame_with(work, std::path::Path::new("m.rs"), rev, options)
            .unwrap()
            .iter()
            .map(|line| line.short().to_string())
            .collect()
    };
    let plain = commits(&BlameOptions::default(), Some("HEAD~1"));
    assert_eq!(plain[5], moved.trim(), "moved lines belong to the move");
    let with_moves = BlameOptions {
        moves: true,
        ..BlameOptions::default()
    };
    assert_eq!(
        commits(&with_moves, Some("HEAD~1"))[5],
        first.trim(),
        "with -M, to the commit that wrote them"
    );

    // Reverse: since the first version, each line's last commit in its
    // place. The move took the block from there; the rest is still at HEAD.
    let reverse = BlameOptions {
        reverse_from: Some(first.trim().to_string()),
        ..BlameOptions::default()
    };
    let lines = commits(&reverse, None);
    let head = git(work, &["rev-parse", "--short=7", "HEAD"]).unwrap();
    assert_eq!(
        lines[0],
        first.trim(),
        "the block left its place in the move"
    );
    assert_eq!(lines[3], head.trim(), "the rest is still there");
    assert_eq!(with_moves.describe(), "-M");
    let empty = BlameOptions {
        reverse_from: Some("HEAD".into()),
        ..BlameOptions::default()
    };
    let err = blame_with(work, std::path::Path::new("m.rs"), None, &empty).unwrap_err();
    assert!(err.contains("older commit"), "{err}");
}
