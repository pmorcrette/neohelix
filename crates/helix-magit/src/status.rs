//! Everything the status buffer shows besides the diffs.
//!
//! The diffs come from `gix` through [`crate::Repository`]; this is the rest
//! of Magit's status buffer — where HEAD is, how it stands against its
//! upstream, the stashes, and whether a merge, rebase or cherry-pick is under
//! way. It is read with the `git` binary, the same one the transient commands
//! run, so what the buffer says is what those commands will act on.

use std::path::{Path, PathBuf};

use crate::command::GitCommand;

/// How many commits the recent-commits section shows, as Magit's
/// `magit-log-section-commit-count` does.
pub const RECENT_COUNT: usize = 10;

/// How many commits an unpushed or unpulled section lists at most. A branch
/// thousands of commits away from its upstream says so well enough with the
/// first hundred.
pub const DIVERGENCE_LIMIT: usize = 100;

/// A commit as a status section lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// The abbreviated hash.
    pub hash: String,
    pub subject: String,
}

/// A stash entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stash {
    /// `stash@{0}`, which is what `git stash show` takes.
    pub name: String,
    /// `WIP on main: abc1234 subject`, or the message it was saved with.
    pub subject: String,
}

/// A branch HEAD is compared with: its upstream, or where it pushes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tracked {
    /// `origin/main`.
    pub name: String,
    /// Its tip, when it exists locally.
    pub commit: Option<Commit>,
}

/// The kind of multi-step operation git is in the middle of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Merge,
    Rebase,
    /// `git am`, which also uses `rebase-apply/`.
    Am,
    CherryPick,
    Revert,
    Bisect,
}

impl Operation {
    /// The git command that continues or aborts it.
    pub fn command(self) -> &'static str {
        match self {
            Operation::Merge => "merge",
            Operation::Rebase => "rebase",
            Operation::Am => "am",
            Operation::CherryPick => "cherry-pick",
            Operation::Revert => "revert",
            Operation::Bisect => "bisect",
        }
    }

    /// How to get out of it, for the line under the description.
    pub fn hint(self) -> &'static str {
        match self {
            Operation::Merge => "git merge --continue, or git merge --abort",
            Operation::Rebase => "git rebase --continue, --skip or --abort",
            Operation::Am => "git am --continue, --skip or --abort",
            Operation::CherryPick => "git cherry-pick --continue, --skip or --abort",
            Operation::Revert => "git revert --continue, --skip or --abort",
            Operation::Bisect => "git bisect good, bad, or reset",
        }
    }
}

/// An operation under way, and what it is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InProgress {
    pub operation: Operation,
    /// "Rebasing feature onto 1a2b3c4 (2/5)".
    pub description: String,
}

/// The status buffer's picture of the repository, diffs aside.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overview {
    /// The checked-out branch; `None` when HEAD is detached.
    pub branch: Option<String>,
    /// HEAD's commit; `None` before the first commit.
    pub head: Option<Commit>,
    pub upstream: Option<Tracked>,
    /// Where `git push` would go, shown only when that is not the upstream.
    pub push: Option<Tracked>,
    pub in_progress: Option<InProgress>,
    /// Commits the upstream has that HEAD does not.
    pub unpulled: Vec<Commit>,
    /// Commits HEAD has that the upstream does not.
    pub unpushed: Vec<Commit>,
    /// The last few commits, shown when there is nothing unpushed to show
    /// instead.
    pub recent: Vec<Commit>,
    pub stashes: Vec<Stash>,
}

/// Runs git and returns its output, or `None` when it fails: every question
/// asked here has "there is none" as a legitimate answer, which is exactly
/// how git reports a missing upstream or an empty stash list.
fn git(workdir: &Path, args: &[&str]) -> Option<String> {
    let output = GitCommand::new(workdir, args.iter().map(|arg| arg.to_string()).collect())
        .run()
        .ok()?;
    output.success.then_some(output.stdout)
}

/// The format [`parse_commits`] reads: hash and subject, NUL-separated.
const COMMIT_FORMAT: &str = "--format=%h%x00%s";

/// Reads `git log --format=%h%x00%s` output.
pub fn parse_commits(text: &str) -> Vec<Commit> {
    text.lines()
        .filter_map(|line| {
            let (hash, subject) = line.split_once('\0')?;
            Some(Commit {
                hash: hash.to_string(),
                subject: subject.to_string(),
            })
        })
        .collect()
}

/// Reads `git stash list --format=%gd%x00%s` output.
pub fn parse_stashes(text: &str) -> Vec<Stash> {
    text.lines()
        .filter_map(|line| {
            let (name, subject) = line.split_once('\0')?;
            Some(Stash {
                name: name.to_string(),
                subject: subject.to_string(),
            })
        })
        .collect()
}

fn log(workdir: &Path, range: &str, limit: usize) -> Vec<Commit> {
    let limit = format!("-n{limit}");
    git(workdir, &["log", &limit, COMMIT_FORMAT, range, "--"])
        .map(|text| parse_commits(&text))
        .unwrap_or_default()
}

fn tracked(workdir: &Path, rev: &str) -> Option<Tracked> {
    let name = git(
        workdir,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", rev],
    )?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some(Tracked {
        name: name.to_string(),
        commit: log(workdir, rev, 1).into_iter().next(),
    })
}

/// Reads the whole overview. Nothing in it is an error: a repository with
/// no commits, no upstream and no stashes simply has none of those.
pub fn read(workdir: &Path) -> Overview {
    let branch = git(workdir, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty());
    let head = log(workdir, "HEAD", 1).into_iter().next();

    let upstream = branch
        .as_ref()
        .and_then(|_| tracked(workdir, "@{upstream}"));
    let push = branch
        .as_ref()
        .and_then(|_| tracked(workdir, "@{push}"))
        .filter(|push| upstream.as_ref().map(|up| &up.name) != Some(&push.name));

    let (unpulled, unpushed) = match &upstream {
        Some(_) => (
            log(workdir, "HEAD..@{upstream}", DIVERGENCE_LIMIT),
            log(workdir, "@{upstream}..HEAD", DIVERGENCE_LIMIT),
        ),
        None => (Vec::new(), Vec::new()),
    };
    let recent = if unpushed.is_empty() && head.is_some() {
        log(workdir, "HEAD", RECENT_COUNT)
    } else {
        Vec::new()
    };

    let stashes = git(workdir, &["stash", "list", "--format=%gd%x00%s"])
        .map(|text| parse_stashes(&text))
        .unwrap_or_default();

    let in_progress = git(workdir, &["rev-parse", "--absolute-git-dir"])
        .map(|dir| PathBuf::from(dir.trim()))
        .and_then(|git_dir| in_progress(&git_dir, &|rev| log(workdir, rev, 1).pop()));

    Overview {
        branch,
        head,
        upstream,
        push,
        in_progress,
        unpulled,
        unpushed,
        recent,
        stashes,
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// `refs/heads/feature` → `feature`; anything else as it is.
fn short_ref(name: &str) -> &str {
    name.strip_prefix("refs/heads/").unwrap_or(name)
}

fn short_hash(hash: &str) -> &str {
    &hash[..hash.len().min(7)]
}

/// Which operation the state files in `git_dir` say is under way.
///
/// `describe` looks a commit up, for naming the one being picked or
/// reverted; it is a parameter so the file layout can be tested without a
/// repository behind it. The checks go from the most specific state to the
/// least: a cherry-pick stopped inside a rebase is reported as the rebase,
/// which is what `--continue` has to be run for.
pub fn in_progress(
    git_dir: &Path,
    describe: &dyn Fn(&str) -> Option<Commit>,
) -> Option<InProgress> {
    let step = |dir: &Path, done: &str, total: &str| -> String {
        match (
            read_trimmed(&dir.join(done)),
            read_trimmed(&dir.join(total)),
        ) {
            (Some(done), Some(total)) => format!(" ({done}/{total})"),
            _ => String::new(),
        }
    };
    let named = |rev: &str| -> String {
        match describe(rev) {
            Some(commit) => format!("{} {}", commit.hash, commit.subject),
            None => short_hash(rev).to_string(),
        }
    };

    let merge_dir = git_dir.join("rebase-merge");
    if merge_dir.is_dir() {
        let head = read_trimmed(&merge_dir.join("head-name")).unwrap_or_default();
        let onto = read_trimmed(&merge_dir.join("onto")).unwrap_or_default();
        // No "interactive" distinction: git's merge backend writes the
        // `interactive` marker for a plain `git rebase` too.
        let mut description = format!(
            "Rebasing {} onto {}{}",
            short_ref(&head),
            short_hash(&onto),
            step(&merge_dir, "msgnum", "end")
        );
        if let Some(stopped) = read_trimmed(&merge_dir.join("stopped-sha")) {
            description.push_str(&format!(", stopped at {}", named(&stopped)));
        }
        return Some(InProgress {
            operation: Operation::Rebase,
            description,
        });
    }

    let apply_dir = git_dir.join("rebase-apply");
    if apply_dir.is_dir() {
        let progress = step(&apply_dir, "next", "last");
        if apply_dir.join("applying").exists() {
            return Some(InProgress {
                operation: Operation::Am,
                description: format!("Applying patches{progress}"),
            });
        }
        let head = read_trimmed(&apply_dir.join("head-name")).unwrap_or_default();
        let onto = read_trimmed(&apply_dir.join("onto")).unwrap_or_default();
        return Some(InProgress {
            operation: Operation::Rebase,
            description: format!(
                "Rebasing {} onto {}{progress}",
                short_ref(&head),
                short_hash(&onto)
            ),
        });
    }

    if let Some(merge_head) = read_trimmed(&git_dir.join("MERGE_HEAD")) {
        // MERGE_HEAD lists one commit per merged head; an octopus has several.
        let heads: Vec<String> = merge_head
            .lines()
            .map(|hash| short_hash(hash.trim()).to_string())
            .collect();
        let message = read_trimmed(&git_dir.join("MERGE_MSG"))
            .and_then(|msg| msg.lines().next().map(str::to_string));
        let description = match message {
            Some(message) => format!("Merging {}: {message}", heads.join(", ")),
            None => format!("Merging {}", heads.join(", ")),
        };
        return Some(InProgress {
            operation: Operation::Merge,
            description,
        });
    }

    if let Some(pick) = read_trimmed(&git_dir.join("CHERRY_PICK_HEAD")) {
        return Some(InProgress {
            operation: Operation::CherryPick,
            description: format!("Cherry-picking {}", named(&pick)),
        });
    }

    if let Some(revert) = read_trimmed(&git_dir.join("REVERT_HEAD")) {
        return Some(InProgress {
            operation: Operation::Revert,
            description: format!("Reverting {}", named(&revert)),
        });
    }

    if git_dir.join("BISECT_LOG").exists() {
        let start = read_trimmed(&git_dir.join("BISECT_START")).unwrap_or_default();
        return Some(InProgress {
            operation: Operation::Bisect,
            description: format!("Bisecting, started from {}", short_ref(&start)),
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_and_stash_output_is_read() {
        assert_eq!(
            parse_commits("abc1234\0First\ndef5678\0Second: with colon\n"),
            [
                Commit {
                    hash: "abc1234".into(),
                    subject: "First".into()
                },
                Commit {
                    hash: "def5678".into(),
                    subject: "Second: with colon".into()
                }
            ]
        );
        assert_eq!(
            parse_stashes("stash@{0}\0WIP on main: abc1234 x\n"),
            [Stash {
                name: "stash@{0}".into(),
                subject: "WIP on main: abc1234 x".into()
            }]
        );
    }

    fn no_commit(_: &str) -> Option<Commit> {
        None
    }

    #[test]
    fn nothing_under_way_is_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(in_progress(dir.path(), &no_commit), None);
    }

    #[test]
    fn a_rebase_names_its_branch_base_step_and_stop() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("rebase-merge");
        std::fs::create_dir(&state).unwrap();
        std::fs::write(state.join("head-name"), "refs/heads/feature\n").unwrap();
        std::fs::write(state.join("onto"), "1a2b3c4d5e6f\n").unwrap();
        std::fs::write(state.join("msgnum"), "2\n").unwrap();
        std::fs::write(state.join("end"), "5\n").unwrap();
        std::fs::write(state.join("interactive"), "").unwrap();
        std::fs::write(state.join("stopped-sha"), "9f8e7d6c5b4a\n").unwrap();

        let found = in_progress(dir.path(), &|rev| {
            Some(Commit {
                hash: short_hash(rev).to_string(),
                subject: "Add things".into(),
            })
        })
        .unwrap();
        assert_eq!(found.operation, Operation::Rebase);
        assert_eq!(
            found.description,
            "Rebasing feature onto 1a2b3c4 (2/5), stopped at 9f8e7d6 Add things"
        );
    }

    #[test]
    fn a_merge_names_what_is_merged() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("MERGE_HEAD"), "0123456789abcdef\n").unwrap();
        std::fs::write(
            dir.path().join("MERGE_MSG"),
            "Merge branch 'topic'\n\n# Conflicts:\n",
        )
        .unwrap();
        let found = in_progress(dir.path(), &no_commit).unwrap();
        assert_eq!(found.operation, Operation::Merge);
        assert_eq!(found.description, "Merging 0123456: Merge branch 'topic'");
    }

    #[test]
    fn am_and_rebase_share_a_directory_but_not_a_name() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("rebase-apply");
        std::fs::create_dir(&state).unwrap();
        std::fs::write(state.join("next"), "1\n").unwrap();
        std::fs::write(state.join("last"), "3\n").unwrap();
        std::fs::write(state.join("applying"), "").unwrap();
        let found = in_progress(dir.path(), &no_commit).unwrap();
        assert_eq!(found.operation, Operation::Am);
        assert_eq!(found.description, "Applying patches (1/3)");
    }

    #[test]
    fn a_cherry_pick_without_a_known_commit_falls_back_to_its_hash() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("CHERRY_PICK_HEAD"), "fedcba9876543210\n").unwrap();
        let found = in_progress(dir.path(), &no_commit).unwrap();
        assert_eq!(found.operation, Operation::CherryPick);
        assert_eq!(found.description, "Cherry-picking fedcba9");
    }
}
