//! The repository list: the repositories under some directories, each with
//! its branch, how far it is ahead of or behind its upstream, and whether
//! it has uncommitted changes.
//!
//! As in Magit, the search goes down a fixed number of levels and stops at
//! a repository: one inside another's working tree is not listed.

use std::path::{Path, PathBuf};

use crate::command::GitCommand;

/// The repositories under `roots`, down to `depth` levels below each root
/// (the root itself being level 0), sorted and without duplicates.
///
/// Hidden directories are not searched: they hold tools' caches, not the
/// user's projects.
pub fn find(roots: &[PathBuf], depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for root in roots {
        search(root, depth, &mut found);
    }
    found.sort();
    found.dedup();
    found
}

fn search(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    // `.git` is a directory in a repository, and a file in a worktree or a
    // submodule.
    if dir.join(".git").exists() {
        found.push(dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf()));
        return;
    }
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| entry.path())
        .collect();
    children.sort();
    for child in children {
        search(&child, depth - 1, found);
    }
}

/// One line of the list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub path: PathBuf,
    /// The checked-out branch; `None` when HEAD is detached.
    pub branch: Option<String>,
    pub upstream: Option<String>,
    /// Commits not yet on the upstream, and commits on it not yet here.
    pub ahead: usize,
    pub behind: usize,
    /// Changed or untracked files.
    pub dirty: bool,
    /// Why the repository could not be read, when it could not.
    pub error: Option<String>,
}

impl Summary {
    /// `main → origin/main ↑2 ↓1 *`: the branch, its upstream, how far
    /// apart they are, and a star for uncommitted changes.
    pub fn describe(&self) -> String {
        if let Some(error) = &self.error {
            return format!("({error})");
        }
        let mut text = self
            .branch
            .clone()
            .unwrap_or_else(|| "(detached)".to_string());
        if let Some(upstream) = &self.upstream {
            text.push_str(&format!(" → {upstream}"));
            if self.ahead > 0 {
                text.push_str(&format!(" ↑{}", self.ahead));
            }
            if self.behind > 0 {
                text.push_str(&format!(" ↓{}", self.behind));
            }
        }
        if self.dirty {
            text.push_str(" *");
        }
        text
    }
}

/// Reads one repository's state, with a single `git status`.
pub fn summarize(path: &Path) -> Summary {
    let args = ["status", "--porcelain=v2", "--branch", "--no-renames"]
        .iter()
        .map(|arg| arg.to_string())
        .collect();
    let mut summary = match GitCommand::new(path, args).run() {
        Ok(output) if output.success => parse_status(&output.stdout),
        Ok(output) => Summary {
            error: Some(output.summary()),
            ..Summary::default()
        },
        Err(err) => Summary {
            error: Some(err.to_string()),
            ..Summary::default()
        },
    };
    summary.path = path.to_path_buf();
    summary
}

/// Reads `git status --porcelain=v2 --branch` output.
pub fn parse_status(text: &str) -> Summary {
    let mut summary = Summary::default();
    for line in text.lines() {
        if let Some(header) = line.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.head" if value != "(detached)" => summary.branch = Some(value.to_string()),
                "branch.upstream" => summary.upstream = Some(value.to_string()),
                "branch.ab" => {
                    for part in value.split_whitespace() {
                        if let Some(ahead) = part.strip_prefix('+') {
                            summary.ahead = ahead.parse().unwrap_or(0);
                        } else if let Some(behind) = part.strip_prefix('-') {
                            summary.behind = behind.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
        } else if !line.is_empty() {
            summary.dirty = true;
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_is_summarized() {
        let summary = parse_status(
            "# branch.oid 1234\n# branch.head main\n# branch.upstream origin/main\n\
             # branch.ab +2 -1\n? new.txt\n",
        );
        assert_eq!(summary.branch.as_deref(), Some("main"));
        assert_eq!((summary.ahead, summary.behind, summary.dirty), (2, 1, true));
        assert_eq!(summary.describe(), "main → origin/main ↑2 ↓1 *");

        let clean = parse_status("# branch.oid 1234\n# branch.head (detached)\n");
        assert_eq!(clean.describe(), "(detached)");
    }

    #[test]
    fn repositories_are_found_down_to_the_depth_given() {
        let root = tempfile::tempdir().unwrap();
        let make = |path: &str| std::fs::create_dir_all(root.path().join(path)).unwrap();
        make("a/.git");
        // Inside a repository: not listed.
        make("a/vendor/b/.git");
        make("group/c/.git");
        make("group/deeper/d/.git");
        make(".hidden/e/.git");
        let names = |depth| -> Vec<String> {
            find(&[root.path().to_path_buf()], depth)
                .iter()
                .map(|path| {
                    path.strip_prefix(root.path().canonicalize().unwrap())
                        .unwrap()
                        .display()
                        .to_string()
                })
                .collect()
        };
        assert_eq!(names(1), ["a"]);
        assert_eq!(names(2), ["a", "group/c"]);
        assert_eq!(names(3), ["a", "group/c", "group/deeper/d"]);
    }
}
