//! Branches, remotes and tags: for completing what a menu asks for, for the
//! refs view, and for the cherries view.

use std::path::Path;

use crate::command::{AskKind, GitCommand};

fn git(workdir: &Path, args: &[&str]) -> Option<String> {
    let output = GitCommand::new(workdir, args.iter().map(|arg| arg.to_string()).collect())
        .run()
        .ok()?;
    output.success.then_some(output.stdout)
}

fn lines(text: Option<String>) -> Vec<String> {
    text.unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// The names a question of `kind` can be answered with, for completion.
pub fn names(workdir: &Path, kind: AskKind) -> Vec<String> {
    let refs = |patterns: &[&str]| {
        let mut args = vec!["for-each-ref", "--format=%(refname:short)"];
        args.extend(patterns);
        lines(git(workdir, &args))
    };
    match kind {
        AskKind::Branch => refs(&["refs/heads", "refs/remotes"]),
        AskKind::Tag => refs(&["refs/tags"]),
        AskKind::Remote => lines(git(workdir, &["remote"])),
        AskKind::Stash => lines(git(workdir, &["stash", "list", "--format=%gd"])),
        AskKind::Revision => {
            let mut names = refs(&["refs/heads", "refs/remotes", "refs/tags"]);
            names.insert(0, "HEAD".to_string());
            names
        }
        AskKind::Path | AskKind::Text | AskKind::Message => Vec::new(),
    }
}

/// What kind of reference a [`RefInfo`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Local,
    Remote,
    Tag,
}

/// A reference, and where it stands against HEAD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefInfo {
    pub kind: RefKind,
    /// `main`, `origin/main`, `v1.0`.
    pub name: String,
    pub hash: String,
    pub subject: String,
    /// A local branch's upstream.
    pub upstream: Option<String>,
    /// Whether it is the checked-out branch.
    pub is_head: bool,
    /// Commits HEAD has that it lacks, and commits it has that HEAD lacks.
    pub behind_ahead: Option<(usize, usize)>,
}

impl RefInfo {
    /// `↑2 ↓1` against HEAD, or nothing when they are the same commit.
    pub fn relation(&self) -> String {
        match self.behind_ahead {
            Some((0, 0)) | None => String::new(),
            Some((behind, ahead)) => {
                let mut parts = Vec::new();
                if ahead > 0 {
                    parts.push(format!("{ahead} ahead"));
                }
                if behind > 0 {
                    parts.push(format!("{behind} behind"));
                }
                parts.join(", ")
            }
        }
    }
}

/// Reads `git for-each-ref` with [`REF_FORMAT`].
pub fn parse_refs(text: &str) -> Vec<RefInfo> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(5, '\0');
            let full = fields.next()?;
            let hash = fields.next()?.to_string();
            let upstream = fields.next()?.to_string();
            let head = fields.next()?;
            let subject = fields.next().unwrap_or_default().to_string();
            let (kind, name) = if let Some(name) = full.strip_prefix("refs/heads/") {
                (RefKind::Local, name)
            } else if let Some(name) = full.strip_prefix("refs/remotes/") {
                (RefKind::Remote, name)
            } else if let Some(name) = full.strip_prefix("refs/tags/") {
                (RefKind::Tag, name)
            } else {
                return None;
            };
            // `origin/HEAD` is an alias, not a branch.
            if kind == RefKind::Remote && name.ends_with("/HEAD") {
                return None;
            }
            Some(RefInfo {
                kind,
                name: name.to_string(),
                hash,
                subject,
                upstream: (!upstream.is_empty()).then_some(upstream),
                is_head: head == "*",
                behind_ahead: None,
            })
        })
        .collect()
}

const REF_FORMAT: &str =
    "--format=%(refname)%00%(objectname:short)%00%(upstream:short)%00%(HEAD)%00%(contents:subject)";

/// How many references get their distance to HEAD counted; each is a
/// `git rev-list` of its own.
pub const COUNTED_REFS: usize = 100;

/// Every branch, remote branch and tag, with how far each is from HEAD.
pub fn read_refs(workdir: &Path) -> Vec<RefInfo> {
    let mut refs = parse_refs(
        &git(
            workdir,
            &[
                "for-each-ref",
                REF_FORMAT,
                "refs/heads",
                "refs/remotes",
                "refs/tags",
            ],
        )
        .unwrap_or_default(),
    );
    for info in refs.iter_mut().take(COUNTED_REFS) {
        let range = format!("HEAD...{}", info.hash);
        info.behind_ahead = git(workdir, &["rev-list", "--left-right", "--count", &range])
            .and_then(|text| {
                let mut counts = text.split_whitespace().map(|n| n.parse().ok());
                Some((counts.next()??, counts.next()??))
            });
    }
    refs
}

/// A commit `git cherry` lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cherry {
    /// Whether an equivalent change is already upstream (`-` in git's
    /// output), so only the commit itself is missing there.
    pub equivalent: bool,
    pub hash: String,
    pub subject: String,
}

/// Reads `git cherry -v --abbrev`.
pub fn parse_cherries(text: &str) -> Vec<Cherry> {
    text.lines()
        .filter_map(|line| {
            let (mark, rest) = line.split_once(' ')?;
            let (hash, subject) = rest.split_once(' ').unwrap_or((rest, ""));
            Some(Cherry {
                equivalent: mark == "-",
                hash: hash.to_string(),
                subject: subject.to_string(),
            })
        })
        .collect()
}

/// The commits `head` (HEAD when `None`) has that `upstream` does not.
pub fn cherries(workdir: &Path, upstream: &str, head: Option<&str>) -> Result<Vec<Cherry>, String> {
    for rev in std::iter::once(upstream).chain(head) {
        crate::log::LogFilter::valid_range(rev)?;
    }
    let mut args = vec!["cherry", "-v", "--abbrev=7", upstream];
    args.extend(head);
    let output = GitCommand::new(workdir, args.iter().map(|arg| arg.to_string()).collect())
        .run()
        .map_err(|err| err.to_string())?;
    if output.success {
        Ok(parse_cherries(&output.stdout))
    } else {
        Err(output.summary())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_are_sorted_into_kinds() {
        let text = "refs/heads/main\0abc1234\0origin/main\0*\0Latest\n\
                    refs/remotes/origin/HEAD\0abc1234\0\0 \0Latest\n\
                    refs/remotes/origin/main\0abc1234\0\0 \0Latest\n\
                    refs/tags/v1\0def5678\0\0 \0Release\n";
        let refs = parse_refs(text);
        assert_eq!(refs.len(), 3, "origin/HEAD is an alias");
        assert_eq!(refs[0].kind, RefKind::Local);
        assert!(refs[0].is_head);
        assert_eq!(refs[0].upstream.as_deref(), Some("origin/main"));
        assert_eq!(refs[1].name, "origin/main");
        assert_eq!(refs[2].kind, RefKind::Tag);
    }

    #[test]
    fn a_relation_reads_as_ahead_and_behind() {
        let mut info = parse_refs("refs/heads/x\0a\0\0 \0s\n").remove(0);
        info.behind_ahead = Some((1, 2));
        assert_eq!(info.relation(), "2 ahead, 1 behind");
        info.behind_ahead = Some((0, 0));
        assert_eq!(info.relation(), "");
    }

    #[test]
    fn cherries_say_whether_an_equivalent_is_upstream() {
        let cherries = parse_cherries("+ abc1234 New work\n- def5678 Already there\n");
        assert!(!cherries[0].equivalent);
        assert_eq!(cherries[0].subject, "New work");
        assert!(cherries[1].equivalent);
    }
}
