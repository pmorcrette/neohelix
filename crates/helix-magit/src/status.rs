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
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Commit {
    /// The abbreviated hash.
    pub hash: String,
    pub subject: String,
    /// For the margin: who wrote it, and when — in seconds since the
    /// epoch, and as a date.
    pub author: String,
    pub time: i64,
    pub date: String,
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
    /// Where `git push` would go, shown only when that is not the upstream:
    /// the push-remote's branch of the same name, when one is configured.
    pub push: Option<Tracked>,
    pub in_progress: Option<InProgress>,
    /// Commits the upstream has that HEAD does not.
    pub unpulled: Vec<Commit>,
    /// Commits HEAD has that the upstream does not.
    pub unpushed: Vec<Commit>,
    /// The same two against the push branch, when it is not the upstream.
    pub push_unpulled: Vec<Commit>,
    pub push_unpushed: Vec<Commit>,
    /// The last few commits, shown when there is nothing unpushed to show
    /// instead.
    pub recent: Vec<Commit>,
    pub stashes: Vec<Stash>,
    /// Every worktree but this one.
    pub worktrees: Vec<Worktree>,
    pub submodules: Vec<Submodule>,
    /// With a sparse checkout, the directories in the working tree (none:
    /// the top-level files only); `None` when everything is checked out.
    pub sparse: Option<Vec<String>>,
    /// The last tag HEAD contains, and the first tag that contains HEAD.
    pub tag: Option<NearTag>,
    pub next_tag: Option<NearTag>,
    /// The steps of the rebase, `am`, cherry-pick or revert under way,
    /// newest first: still to do, the current one, done, and where a
    /// rebase started.
    pub sequence: Vec<Step>,
    /// What each step of a bisect found, in order.
    pub bisect_log: Vec<BisectEntry>,
    /// Files git is told to ignore changes to (`update-index
    /// --assume-unchanged`), and files left out of the working tree
    /// (`--skip-worktree`; not listed under a sparse checkout, which
    /// leaves out whole directories that way).
    pub assumed: Vec<String>,
    pub skipped: Vec<String>,
    /// The keyword comments of the repository, for magit-todos' section.
    /// [`read`] leaves them empty: the caller knows the keywords.
    pub todos: Todos,
}

/// The keyword comments listed, and whether there were more.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Todos {
    pub items: Vec<crate::todos::Todo>,
    pub more: bool,
}

/// A tag, and how many commits lie between it and HEAD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NearTag {
    pub name: String,
    pub distance: usize,
}

/// Where a step of an operation under way stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Todo,
    /// The step the operation stopped at.
    Current,
    Done,
    /// The commit a rebase starts from.
    Onto,
}

/// A step of a rebase, `am`, cherry-pick or revert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub state: StepState,
    /// `pick`, `fixup`, `exec`, … as the todo list names it.
    pub action: String,
    /// The commit, abbreviated; empty for a step that is not about one
    /// (`exec`, `break`, a patch being applied).
    pub hash: String,
    pub subject: String,
}

/// A line of `git bisect log`: a commit and what it was found to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BisectEntry {
    /// `good`, `bad`, `skip`, or the custom terms; `first bad commit` for
    /// the answer.
    pub verdict: String,
    pub hash: String,
    pub subject: String,
}

/// Another working tree of the same repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// Its branch, or `None` when detached.
    pub branch: Option<String>,
    pub head: String,
}

/// A submodule, as `git submodule status` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submodule {
    pub path: String,
    pub hash: String,
    /// "not initialized", "out of date" (checked out at another commit
    /// than recorded), "conflict", or "" when it is as recorded.
    pub state: &'static str,
}

/// Reads `git worktree list --porcelain`.
pub fn parse_worktrees(text: &str) -> Vec<Worktree> {
    text.split("\n\n")
        .filter_map(|block| {
            let mut path = None;
            let mut head = String::new();
            let mut branch = None;
            for line in block.lines() {
                if let Some(value) = line.strip_prefix("worktree ") {
                    path = Some(PathBuf::from(value));
                } else if let Some(value) = line.strip_prefix("HEAD ") {
                    head = value.chars().take(7).collect();
                } else if let Some(value) = line.strip_prefix("branch ") {
                    branch = Some(value.trim_start_matches("refs/heads/").to_string());
                }
            }
            Some(Worktree {
                path: path?,
                branch,
                head,
            })
        })
        .collect()
}

/// Reads `git submodule status`.
pub fn parse_submodules(text: &str) -> Vec<Submodule> {
    text.lines()
        .filter_map(|line| {
            let state = match line.chars().next()? {
                '-' => "not initialized",
                '+' => "out of date",
                'U' => "conflict",
                _ => "",
            };
            let mut words = line[1..].split_whitespace();
            let hash: String = words.next()?.chars().take(7).collect();
            let path = words.next()?.to_string();
            Some(Submodule { path, hash, state })
        })
        .collect()
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
const COMMIT_FORMAT: &str = "--format=%h%x00%s%x00%an%x00%at%x00%as";

/// Reads `git log` output in [`COMMIT_FORMAT`]: hash, subject, author, time
/// and date, the last three optional.
pub fn parse_commits(text: &str) -> Vec<Commit> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\0');
            let hash = fields.next()?.to_string();
            let subject = fields.next()?.to_string();
            Some(Commit {
                hash,
                subject,
                author: fields.next().unwrap_or_default().to_string(),
                time: fields.next().and_then(|t| t.parse().ok()).unwrap_or(0),
                date: fields.next().unwrap_or_default().to_string(),
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

/// The subject of the commit `rev` names.
pub fn commit_subject(workdir: &Path, rev: &str) -> Option<String> {
    log(workdir, rev, 1).pop().map(|commit| commit.subject)
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
    // Magit's push branch: the push-remote's branch of the same name, even
    // before it exists; without a push-remote, where git would push.
    let push = branch
        .as_ref()
        .and_then(|branch| match push_remote(workdir, branch) {
            Some(remote) => {
                let name = format!("{remote}/{branch}");
                Some(
                    tracked(workdir, &format!("refs/remotes/{name}"))
                        .unwrap_or(Tracked { name, commit: None }),
                )
            }
            None => tracked(workdir, "@{push}"),
        })
        .filter(|push| upstream.as_ref().map(|up| &up.name) != Some(&push.name));
    let (push_unpulled, push_unpushed) = match &push {
        Some(Tracked {
            name,
            commit: Some(_),
        }) => (
            log(
                workdir,
                &format!("HEAD..refs/remotes/{name}"),
                DIVERGENCE_LIMIT,
            ),
            log(
                workdir,
                &format!("refs/remotes/{name}..HEAD"),
                DIVERGENCE_LIMIT,
            ),
        ),
        _ => (Vec::new(), Vec::new()),
    };

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

    let in_progress = git_dir(workdir)
        .and_then(|git_dir| in_progress(&git_dir, &|rev| log(workdir, rev, 1).pop()));

    let this = std::fs::canonicalize(workdir).unwrap_or_else(|_| workdir.to_path_buf());
    let worktrees = git(workdir, &["worktree", "list", "--porcelain"])
        .map(|text| parse_worktrees(&text))
        .unwrap_or_default()
        .into_iter()
        .filter(|tree| std::fs::canonicalize(&tree.path).unwrap_or(tree.path.clone()) != this)
        .collect();
    // Only asked when there are submodules to report on: without a
    // `.gitmodules`, `git submodule` has nothing to say.
    let submodules = if workdir.join(".gitmodules").exists() {
        git(workdir, &["submodule", "status"])
            .map(|text| parse_submodules(&text))
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let sparse = sparse_directories(workdir);
    let (tag, next_tag) = if head.is_some() {
        near_tags(workdir)
    } else {
        (None, None)
    };
    let sequence = git_dir(workdir)
        .map(|git_dir| sequence(workdir, &git_dir))
        .unwrap_or_default();
    let bisect_log = match &in_progress {
        Some(InProgress {
            operation: Operation::Bisect,
            ..
        }) => git(workdir, &["bisect", "log"])
            .map(|text| parse_bisect_log(&text))
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let (assumed, skipped) = git(workdir, &["ls-files", "-v"])
        .map(|text| parse_index_flags(&text, sparse.is_some()))
        .unwrap_or_default();

    Overview {
        branch,
        head,
        upstream,
        push,
        in_progress,
        unpulled,
        unpushed,
        push_unpulled,
        push_unpushed,
        recent,
        stashes,
        worktrees,
        submodules,
        sparse,
        tag,
        next_tag,
        sequence,
        bisect_log,
        assumed,
        skipped,
        todos: Todos::default(),
    }
}

/// The last tag HEAD contains and the first one containing HEAD, each with
/// its distance; the second is left out when it is the first (HEAD tagged).
fn near_tags(workdir: &Path) -> (Option<NearTag>, Option<NearTag>) {
    let count = |range: String| -> usize {
        git(workdir, &["rev-list", "--count", &range])
            .and_then(|count| count.trim().parse().ok())
            .unwrap_or(0)
    };
    let tag = git(workdir, &["describe", "--tags", "--abbrev=0", "HEAD"])
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .map(|name| NearTag {
            distance: count(format!("{name}..HEAD")),
            name,
        });
    // `v1.2~3`, or `v1.2^2~1` through a merge: the tag is the name before.
    let next_tag = git(
        workdir,
        &["describe", "--tags", "--contains", "--match=*", "HEAD"],
    )
    .map(|name| {
        name.trim()
            .split(['~', '^'])
            .next()
            .unwrap_or("")
            .to_string()
    })
    .filter(|name| !name.is_empty())
    .filter(|name| tag.as_ref().is_none_or(|tag| &tag.name != name))
    .map(|name| NearTag {
        distance: count(format!("HEAD..{name}")),
        name,
    });
    (tag, next_tag)
}

/// Reads a todo list: its steps as `(action, hash, subject)`, comments
/// left out. A step that names no commit (`exec`, `break`, `label`, …)
/// has no hash and keeps the rest of its line as its subject.
pub fn parse_todo(text: &str) -> Vec<(String, String, String)> {
    const TAKES_COMMIT: [&str; 12] = [
        "pick", "p", "reword", "r", "edit", "e", "squash", "s", "fixup", "f", "drop", "d",
    ];
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let mut words = line.splitn(2, char::is_whitespace);
            let action = words.next().unwrap_or("").to_string();
            let mut rest = words.next().unwrap_or("").trim_start();
            if !TAKES_COMMIT.contains(&action.as_str()) {
                return (action, String::new(), rest.to_string());
            }
            // `fixup -C <commit>`: the option is not the commit.
            if rest.starts_with('-') {
                rest = rest
                    .split_once(char::is_whitespace)
                    .map_or("", |(_, after)| after.trim_start());
            }
            let (hash, subject) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            let subject = subject.trim_start();
            let subject = subject.strip_prefix("# ").unwrap_or(subject);
            (
                action,
                short_hash(hash).to_string(),
                subject.trim().to_string(),
            )
        })
        .collect()
}

/// The steps of the operation under way, newest first; empty when none, or
/// for a single cherry-pick or revert, whose header line says it all.
fn sequence(workdir: &Path, git_dir: &Path) -> Vec<Step> {
    let step = |state, (action, hash, subject): (String, String, String)| Step {
        state,
        action,
        hash,
        subject,
    };
    let mut steps = Vec::new();

    let merge_dir = git_dir.join("rebase-merge");
    if merge_dir.is_dir() {
        let todo = std::fs::read_to_string(merge_dir.join("git-rebase-todo")).unwrap_or_default();
        let done = std::fs::read_to_string(merge_dir.join("done")).unwrap_or_default();
        steps.extend(
            parse_todo(&todo)
                .into_iter()
                .rev()
                .map(|entry| step(StepState::Todo, entry)),
        );
        // The last step done is the one the rebase stopped at.
        let mut done = parse_todo(&done);
        if let Some(current) = done.pop() {
            steps.push(step(StepState::Current, current));
        }
        steps.extend(
            done.into_iter()
                .rev()
                .map(|entry| step(StepState::Done, entry)),
        );
        if let Some(onto) = read_trimmed(&merge_dir.join("onto")) {
            let subject = log(workdir, &onto, 1)
                .pop()
                .map(|commit| commit.subject)
                .unwrap_or_default();
            steps.push(Step {
                state: StepState::Onto,
                action: "onto".into(),
                hash: short_hash(&onto).to_string(),
                subject,
            });
        }
        return steps;
    }

    let apply_dir = git_dir.join("rebase-apply");
    if apply_dir.is_dir() {
        let number = |name: &str| -> usize {
            read_trimmed(&apply_dir.join(name))
                .and_then(|n| n.parse().ok())
                .unwrap_or(0)
        };
        let (next, last) = (number("next"), number("last"));
        for patch in (1..=last).rev() {
            let text =
                std::fs::read_to_string(apply_dir.join(format!("{patch:04}"))).unwrap_or_default();
            let subject = text
                .lines()
                .find_map(|line| line.strip_prefix("Subject: "))
                .map(|subject| {
                    // `[PATCH 2/5] Subject`: the bracket is mail's, not the commit's.
                    match subject.strip_prefix('[') {
                        Some(rest) => rest.split_once("] ").map_or(subject, |(_, s)| s),
                        None => subject,
                    }
                })
                .unwrap_or("")
                .to_string();
            let state = match patch.cmp(&next) {
                std::cmp::Ordering::Less => StepState::Done,
                std::cmp::Ordering::Equal => StepState::Current,
                std::cmp::Ordering::Greater => StepState::Todo,
            };
            steps.push(Step {
                state,
                action: format!("patch {patch}"),
                hash: String::new(),
                subject,
            });
        }
        return steps;
    }

    let sequencer = git_dir.join("sequencer");
    if sequencer.join("todo").is_file() {
        let todo = std::fs::read_to_string(sequencer.join("todo")).unwrap_or_default();
        let mut todo = parse_todo(&todo);
        // The step git stopped at stays first in its list until committed.
        let stopped =
            git_dir.join("CHERRY_PICK_HEAD").exists() || git_dir.join("REVERT_HEAD").exists();
        let current = (stopped && !todo.is_empty()).then(|| todo.remove(0));
        steps.extend(
            todo.into_iter()
                .rev()
                .map(|entry| step(StepState::Todo, entry)),
        );
        steps.extend(current.map(|entry| step(StepState::Current, entry)));
        if let Some(head) = read_trimmed(&sequencer.join("head")) {
            steps.extend(
                log(workdir, &format!("{head}..HEAD"), DIVERGENCE_LIMIT)
                    .into_iter()
                    .map(|commit| Step {
                        state: StepState::Done,
                        action: "done".into(),
                        hash: commit.hash,
                        subject: commit.subject,
                    }),
            );
        }
    }
    steps
}

/// Reads `git bisect log`: the comment lines git writes for each verdict,
/// `# good: [<hash>] <subject>`.
pub fn parse_bisect_log(text: &str) -> Vec<BisectEntry> {
    text.lines()
        .filter_map(|line| {
            let line = line.strip_prefix("# ")?;
            let (verdict, rest) = line.split_once(": [")?;
            let (hash, subject) = rest.split_once("] ")?;
            Some(BisectEntry {
                verdict: verdict.to_string(),
                hash: short_hash(hash).to_string(),
                subject: subject.to_string(),
            })
        })
        .collect()
}

/// Reads `git ls-files -v`: the assume-unchanged files (a lowercase tag)
/// and the skip-worktree ones (`S`), the latter only when `sparse` is off.
pub fn parse_index_flags(text: &str, sparse: bool) -> (Vec<String>, Vec<String>) {
    let mut assumed = Vec::new();
    let mut skipped = Vec::new();
    for line in text.lines() {
        let Some((tag, path)) = line.split_once(' ') else {
            continue;
        };
        if tag.chars().all(|c| c.is_ascii_lowercase()) {
            assumed.push(path.to_string());
        }
        if !sparse && tag.eq_ignore_ascii_case("s") {
            skipped.push(path.to_string());
        }
    }
    (assumed, skipped)
}

/// The directories a sparse checkout includes, or `None` without one. In
/// the older non-cone mode, these are the patterns instead.
pub fn sparse_directories(workdir: &Path) -> Option<Vec<String>> {
    let enabled = git(workdir, &["config", "--type=bool", "core.sparseCheckout"])?;
    if enabled.trim() != "true" {
        return None;
    }
    let listed = git(workdir, &["sparse-checkout", "list"]).unwrap_or_default();
    Some(
        listed
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// The checked-out branch, `None` when HEAD is detached.
pub fn current_branch(workdir: &Path) -> Option<String> {
    git(workdir, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

/// Where `branch` is pushed to by Magit's `P p`: its own `pushRemote`, or
/// the repository's `remote.pushDefault`.
pub fn push_remote(workdir: &Path, branch: &str) -> Option<String> {
    let config = |key: &str| {
        git(workdir, &["config", "--get", key])
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    config(&format!("branch.{branch}.pushRemote")).or_else(|| config("remote.pushDefault"))
}

/// The repository's git directory — `.git`, or elsewhere for a linked
/// worktree or a submodule.
pub fn git_dir(workdir: &Path) -> Option<PathBuf> {
    git(workdir, &["rev-parse", "--absolute-git-dir"])
        .map(|dir| PathBuf::from(dir.trim()))
        .filter(|dir| !dir.as_os_str().is_empty())
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
            description: format!(
                "Bisecting from {}, now at {}",
                short_ref(&start),
                named("HEAD")
            ),
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todo_lists_bisect_logs_and_index_flags_are_read() {
        let todo = "pick 1234567890 First\n\
                    fixup -C abcdef0123 # Second\n\
                    exec make test\n\
                    # a comment\n\
                    \n\
                    break\n";
        assert_eq!(
            parse_todo(todo),
            [
                ("pick".into(), "1234567".into(), "First".into()),
                ("fixup".into(), "abcdef0".into(), "Second".into()),
                ("exec".into(), String::new(), "make test".into()),
                ("break".into(), String::new(), String::new()),
            ]
        );

        let log = "git bisect start\n\
                   # bad: [1234567890abcdef] Broke it\n\
                   git bisect bad 1234567890abcdef\n\
                   # first bad commit: [1234567890abcdef] Broke it\n";
        let entries = parse_bisect_log(log);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].verdict, "bad");
        assert_eq!(entries[0].hash, "1234567");
        assert_eq!(entries[1].verdict, "first bad commit");
        assert_eq!(entries[1].subject, "Broke it");

        let flags = "H tracked.txt\nh assumed.txt\nS skipped.txt\ns both.txt\n";
        assert_eq!(
            parse_index_flags(flags, false),
            (
                vec!["assumed.txt".to_string(), "both.txt".to_string()],
                vec!["skipped.txt".to_string(), "both.txt".to_string()]
            )
        );
        assert!(parse_index_flags(flags, true).1.is_empty());
    }

    #[test]
    fn log_and_stash_output_is_read() {
        assert_eq!(
            parse_commits("abc1234\0First\ndef5678\0Second: with colon\0Ann\x0042\x001970-01-01\n"),
            [
                Commit {
                    hash: "abc1234".into(),
                    subject: "First".into(),
                    ..Commit::default()
                },
                Commit {
                    hash: "def5678".into(),
                    subject: "Second: with colon".into(),
                    author: "Ann".into(),
                    time: 42,
                    date: "1970-01-01".into(),
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
                ..Commit::default()
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
