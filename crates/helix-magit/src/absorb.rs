//! Absorb and autofixup: the staged changes, hunk by hunk, folded into the
//! unpushed commits they belong to.
//!
//! Both split the staged diff into hunks, find each one's commit, and make
//! one `fixup!` commit per commit found. They differ in how a hunk's commit
//! is found and in what happens next:
//!
//! - **Absorb** looks at the lines a hunk removes — or, for a hunk that only
//!   adds, at the lines on either side of it — and takes the commit that
//!   last changed all of them, when that is one commit. The hunks are as
//!   small as git makes them (`-U0`). The fixups are then squashed in at
//!   once, with an autosquash rebase.
//! - **Autofixup** looks at the whole hunk as the diff shows it, its context
//!   included, and takes the one unpushed commit among those that last
//!   changed those lines. The `fixup!` commits are left for review and a
//!   later autosquash (`r` with `-A`).
//!
//! Only commits not yet on any remote are candidates: folding a change into
//! a pushed commit would rewrite published history. A hunk that finds no
//! commit, or several, stays staged, as do new files and binary changes.
//!
//! The working tree is not touched until the rebase: each fixup commit is
//! built in the index from the original HEAD and the hunks so far, and the
//! index is put back at the end with whatever was not absorbed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::command::GitCommand;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Absorb,
    Autofixup,
}

/// What was done.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The commits fixed up, oldest first, abbreviated, with how many hunks
    /// went into each.
    pub fixups: Vec<(String, usize)>,
    /// Hunks left staged.
    pub left: usize,
}

impl Outcome {
    pub fn describe(&self, mode: Mode) -> String {
        let hunks: usize = self.fixups.iter().map(|(_, count)| count).sum();
        let into: Vec<&str> = self.fixups.iter().map(|(hash, _)| hash.as_str()).collect();
        let done = match mode {
            Mode::Absorb => format!(
                "Absorbed {hunks} hunk{} into {}",
                plural(hunks),
                into.join(", ")
            ),
            Mode::Autofixup => format!(
                "{} fixup! commit{} for {} ({hunks} hunk{})",
                into.len(),
                plural(into.len()),
                into.join(", "),
                plural(hunks)
            ),
        };
        if self.left == 0 {
            done
        } else {
            format!(
                "{done}; {} hunk{} left staged",
                self.left,
                plural(self.left)
            )
        }
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

/// One file of a diff: its header lines, and its hunks.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FilePatch {
    header: Vec<String>,
    /// The file's path before the change; `None` for a new file.
    old_path: Option<PathBuf>,
    binary: bool,
    hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Hunk {
    /// The whole hunk, its `@@` line included.
    lines: Vec<String>,
    old_start: usize,
    old_len: usize,
}

impl Hunk {
    /// The old side's line numbers (from 1) this hunk removes.
    fn removed(&self) -> Vec<usize> {
        let mut line = self.old_start;
        let mut removed = Vec::new();
        for text in &self.lines[1..] {
            match text.as_bytes().first() {
                Some(b'-') => {
                    removed.push(line);
                    line += 1;
                }
                Some(b' ') => line += 1,
                _ => {}
            }
        }
        removed
    }
}

fn parse_range(range: &str) -> Option<(usize, usize)> {
    match range.split_once(',') {
        Some((start, len)) => Some((start.parse().ok()?, len.parse().ok()?)),
        None => Some((range.parse().ok()?, 1)),
    }
}

/// Reads `git diff` output into files and hunks.
fn parse_diff(text: &str) -> Vec<FilePatch> {
    let mut files: Vec<FilePatch> = Vec::new();
    for line in text.lines() {
        if line.starts_with("diff --git ") {
            files.push(FilePatch {
                header: vec![line.to_string()],
                old_path: None,
                binary: false,
                hunks: Vec::new(),
            });
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if let Some(header) = line.strip_prefix("@@ -") {
            let old = header.split_whitespace().next().unwrap_or_default();
            let (old_start, old_len) = parse_range(old).unwrap_or((0, 0));
            file.hunks.push(Hunk {
                lines: vec![line.to_string()],
                old_start,
                old_len,
            });
        } else if let Some(hunk) = file.hunks.last_mut() {
            hunk.lines.push(line.to_string());
        } else {
            if let Some(path) = line.strip_prefix("--- a/") {
                file.old_path = Some(PathBuf::from(path));
            }
            if line.starts_with("Binary files ") || line == "GIT binary patch" {
                file.binary = true;
            }
            file.header.push(line.to_string());
        }
    }
    files
}

fn git(workdir: &Path, args: &[&str]) -> Result<String, String> {
    let output = GitCommand::new(workdir, args.iter().map(|arg| arg.to_string()).collect())
        .run()
        .map_err(|err| err.to_string())?;
    if output.success {
        Ok(output.stdout)
    } else {
        Err(output.summary())
    }
}

/// Which unpushed commit a hunk belongs to, if exactly one.
fn target(
    mode: Mode,
    hunk: &Hunk,
    blame: &[crate::blame::BlameLine],
    unpushed: &HashMap<String, usize>,
) -> Option<String> {
    let commit_of = |line: usize| blame.get(line.checked_sub(1)?).map(|l| l.hash.clone());
    let lines: Vec<usize> = match mode {
        Mode::Absorb => {
            let removed = hunk.removed();
            if removed.is_empty() {
                // Only added: after old line `old_start`. The lines on
                // either side, those that exist.
                [hunk.old_start, hunk.old_start + 1]
                    .into_iter()
                    .filter(|line| *line >= 1 && *line <= blame.len())
                    .collect()
            } else {
                removed
            }
        }
        Mode::Autofixup => {
            if hunk.old_len == 0 {
                [hunk.old_start, hunk.old_start + 1]
                    .into_iter()
                    .filter(|line| *line >= 1 && *line <= blame.len())
                    .collect()
            } else {
                (hunk.old_start..hunk.old_start + hunk.old_len).collect()
            }
        }
    };
    let commits: Vec<String> = lines.into_iter().filter_map(commit_of).collect();
    match mode {
        // Every line from one commit, and that commit unpushed.
        Mode::Absorb => {
            let first = commits.first()?;
            (commits.iter().all(|commit| commit == first) && unpushed.contains_key(first))
                .then(|| first.clone())
        }
        // Exactly one unpushed commit among them.
        Mode::Autofixup => {
            let mut candidates: Vec<&String> = commits
                .iter()
                .filter(|commit| unpushed.contains_key(*commit))
                .collect();
            candidates.sort();
            candidates.dedup();
            match candidates.as_slice() {
                [only] => Some((*only).clone()),
                _ => None,
            }
        }
    }
}

/// A patch of the given hunks, `chosen[file][hunk]`, with their files'
/// headers.
fn patch_of(files: &[FilePatch], chosen: &dyn Fn(usize, usize) -> bool) -> String {
    let mut patch = String::new();
    for (index, file) in files.iter().enumerate() {
        let hunks: Vec<&Hunk> = file
            .hunks
            .iter()
            .enumerate()
            .filter(|(hunk, _)| chosen(index, *hunk))
            .map(|(_, hunk)| hunk)
            .collect();
        if hunks.is_empty() {
            continue;
        }
        for line in &file.header {
            patch.push_str(line);
            patch.push('\n');
        }
        for hunk in hunks {
            for line in &hunk.lines {
                patch.push_str(line);
                patch.push('\n');
            }
        }
    }
    patch
}

/// Puts `patch` into the index, which is first reset to `tree`.
fn stage(workdir: &Path, tree: &str, file: &Path, patch: &str, zero: bool) -> Result<(), String> {
    git(workdir, &["read-tree", tree])?;
    if patch.is_empty() {
        return Ok(());
    }
    std::fs::write(file, patch).map_err(|err| err.to_string())?;
    let file = file.display().to_string();
    let mut args = vec!["apply", "--cached", "--whitespace=nowarn"];
    if zero {
        args.push("--unidiff-zero");
    }
    args.push(&file);
    git(workdir, &args).map(|_| ())
}

/// Absorbs or autofixes the staged changes. An error leaves the repository
/// as it was: HEAD, the index and the working tree.
pub fn run(workdir: &Path, mode: Mode) -> Result<Outcome, String> {
    let git_dir = crate::status::git_dir(workdir).ok_or("not in a git repository")?;
    if let Some(state) = crate::status::in_progress(&git_dir, &|_| None) {
        return Err(format!("{} is in progress", state.description));
    }
    let head = git(workdir, &["rev-parse", "--verify", "HEAD"])
        .map_err(|_| "there are no commits yet".to_string())?
        .trim()
        .to_string();

    // The unpushed commits, oldest first.
    let unpushed: HashMap<String, usize> = git(
        workdir,
        &["rev-list", "--reverse", "HEAD", "--not", "--remotes"],
    )?
    .lines()
    .enumerate()
    .map(|(index, hash)| (hash.to_string(), index))
    .collect();
    if unpushed.is_empty() {
        return Err("every commit is pushed: there is nothing to fold into".into());
    }

    let context = match mode {
        Mode::Absorb => "-U0",
        Mode::Autofixup => "-U3",
    };
    let diff = git(
        workdir,
        &[
            "diff",
            "--cached",
            "--no-color",
            "--no-ext-diff",
            "--no-renames",
            "--binary",
            context,
        ],
    )?;
    let files = parse_diff(&diff);
    if files.is_empty() {
        return Err("nothing is staged".into());
    }

    // Each hunk's commit.
    let mut targets: HashMap<(usize, usize), String> = HashMap::new();
    let mut left = 0;
    for (index, file) in files.iter().enumerate() {
        let blame = match (&file.old_path, file.binary) {
            (Some(path), false) => crate::blame::blame(workdir, path, Some("HEAD")).ok(),
            _ => None,
        };
        for (number, hunk) in file.hunks.iter().enumerate() {
            match blame
                .as_deref()
                .and_then(|blame| target(mode, hunk, blame, &unpushed))
            {
                Some(commit) => {
                    targets.insert((index, number), commit);
                }
                None => left += 1,
            }
        }
        if file.hunks.is_empty() {
            // A mode change, or a binary file: not absorbed.
            left += 1;
        }
    }
    let mut commits: Vec<String> = targets.values().cloned().collect();
    commits.sort_by_key(|commit| unpushed[commit]);
    commits.dedup();
    let Some(oldest) = commits.first().cloned() else {
        return Err(format!(
            "no staged hunk belongs to exactly one unpushed commit ({left} left staged)"
        ));
    };
    if mode == Mode::Absorb {
        let merges = git(
            workdir,
            &["rev-list", "--merges", &format!("{oldest}..HEAD")],
        )?;
        if !merges.trim().is_empty() {
            return Err("a merge lies after those commits, which the rebase would drop".into());
        }
    }

    // Everything staged, to put back what is not absorbed.
    let everything = git(
        workdir,
        &[
            "diff",
            "--cached",
            "--binary",
            "--no-color",
            "--no-ext-diff",
        ],
    )?;
    let patch_file = git_dir.join("helix-absorb.patch");
    let everything_file = git_dir.join("helix-absorb-staged.patch");
    std::fs::write(&everything_file, &everything).map_err(|err| err.to_string())?;
    let zero = mode == Mode::Absorb;

    // One fixup commit per target, each holding the hunks so far.
    let mut fixups = Vec::new();
    let mut done: Vec<String> = Vec::new();
    for commit in &commits {
        done.push(commit.clone());
        let patch = patch_of(&files, &|file, hunk| {
            targets
                .get(&(file, hunk))
                .is_some_and(|target| done.contains(target))
        });
        let made = stage(workdir, &head, &patch_file, &patch, zero).and_then(|_| {
            git(
                workdir,
                &[
                    "commit",
                    "--quiet",
                    "--no-verify",
                    &format!("--fixup={commit}"),
                ],
            )
        });
        if let Err(err) = made {
            // Back to where it started.
            let _ = git(workdir, &["reset", "--quiet", "--soft", &head]);
            let _ = stage(workdir, &head, &everything_file, &everything, false);
            let _ = std::fs::remove_file(&patch_file);
            let _ = std::fs::remove_file(&everything_file);
            return Err(err);
        }
        let count = targets.values().filter(|target| *target == commit).count();
        fixups.push((commit[..commit.len().min(7)].to_string(), count));
    }

    // The index as it was: against the new HEAD, only what was left.
    let restored = stage(workdir, &head, &everything_file, &everything, false);
    let _ = std::fs::remove_file(&patch_file);
    let _ = std::fs::remove_file(&everything_file);
    restored?;
    let outcome = Outcome { fixups, left };

    if mode == Mode::Absorb {
        squash(workdir, &git_dir, &oldest)?;
    }
    Ok(outcome)
}

/// The autosquash rebase that folds the fixups in, from `oldest`'s parent.
/// Uncommitted changes are set aside and put back, and what was staged is
/// staged again.
fn squash(workdir: &Path, git_dir: &Path, oldest: &str) -> Result<(), String> {
    let staged = git(
        workdir,
        &[
            "diff",
            "--cached",
            "--binary",
            "--no-color",
            "--no-ext-diff",
        ],
    )?;
    let base = crate::rebase::base_for(workdir, oldest);
    let output = GitCommand::new(
        workdir,
        [
            "rebase",
            "--interactive",
            "--autosquash",
            "--autostash",
            &base,
        ]
        .iter()
        .map(|arg| arg.to_string())
        .collect(),
    )
    // Take git's list as it is: the fixups already placed.
    .with_env("GIT_SEQUENCE_EDITOR", ":")
    .run()
    .map_err(|err| err.to_string())?;
    if !output.success {
        return Err(format!(
            "the fixup commits are made, but squashing them in stopped: {}",
            output.summary()
        ));
    }
    if !staged.trim().is_empty() {
        let file = git_dir.join("helix-absorb-left.patch");
        std::fs::write(&file, &staged).map_err(|err| err.to_string())?;
        let restaged = git(workdir, &["apply", "--cached", &file.display().to_string()]);
        let _ = std::fs::remove_file(&file);
        restaged.map_err(|err| {
            format!("absorbed, but what was left is back unstaged rather than staged: {err}")
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "\
diff --git a/f.txt b/f.txt
index 1111111..2222222 100644
--- a/f.txt
+++ b/f.txt
@@ -2 +2 @@
-two
+TWO
@@ -5,0 +6,2 @@ five
+six
+seven
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/new.txt
@@ -0,0 +1 @@
+new
";

    #[test]
    fn a_diff_is_read_into_files_and_hunks() {
        let files = parse_diff(DIFF);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].old_path.as_deref(), Some(Path::new("f.txt")));
        assert_eq!(files[0].hunks.len(), 2);
        assert_eq!(files[0].hunks[0].removed(), [2]);
        assert_eq!(
            (files[0].hunks[1].old_start, files[0].hunks[1].old_len),
            (5, 0)
        );
        assert!(files[0].hunks[1].removed().is_empty());
        assert_eq!(files[1].old_path, None);

        let only_second = patch_of(&files, &|file, hunk| file == 0 && hunk == 1);
        assert!(only_second.starts_with("diff --git a/f.txt b/f.txt\n"));
        assert!(only_second.contains("+six\n") && !only_second.contains("-two"));
    }

    fn blamed(commits: &[&str]) -> Vec<crate::blame::BlameLine> {
        commits
            .iter()
            .map(|hash| crate::blame::BlameLine {
                hash: hash.to_string(),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn a_hunk_goes_to_the_one_unpushed_commit_its_lines_come_from() {
        let files = parse_diff(DIFF);
        let (change, addition) = (&files[0].hunks[0], &files[0].hunks[1]);
        let unpushed: HashMap<String, usize> = [("b".to_string(), 0), ("c".to_string(), 1)].into();

        // Line 2 is b's; lines 5 and 6 are c's.
        let blame = blamed(&["a", "b", "a", "a", "c", "c"]);
        assert_eq!(
            target(Mode::Absorb, change, &blame, &unpushed).as_deref(),
            Some("b")
        );
        assert_eq!(
            target(Mode::Absorb, addition, &blame, &unpushed).as_deref(),
            Some("c")
        );

        // Pushed (`a`), or between two commits: nowhere.
        let blame = blamed(&["b", "a", "a", "a", "b", "c"]);
        assert_eq!(target(Mode::Absorb, change, &blame, &unpushed), None);
        assert_eq!(target(Mode::Absorb, addition, &blame, &unpushed), None);
    }
}
