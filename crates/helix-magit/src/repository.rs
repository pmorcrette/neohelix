//! A thin Git layer over `gix`.
//!
//! Only what the status view needs: which files changed, and the diff of each
//! one. Diffs are produced as unified text and handed to
//! [`crate::parse_unified_diff`], so the model has a single source of truth
//! and works the same whether a diff came from here or from `git diff`.

use std::path::{Path, PathBuf};

use imara_diff::{Algorithm, BasicLineDiffPrinter, InternedInput, UnifiedDiffConfig};

use crate::diff::{FileDiff, FileStatus};

/// What can go wrong talking to a repository.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no git repository found for {0}")]
    NotARepository(PathBuf),
    #[error("the repository has no working tree")]
    NoWorkTree,
    #[error("git error: {0}")]
    Git(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Helper for the many distinct `gix` error types, none of which is worth
/// modelling separately here.
fn git<E: std::fmt::Display>(err: E) -> Error {
    Error::Git(err.to_string())
}

/// One entry of the status view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    /// Path relative to the working tree root.
    pub path: PathBuf,
    pub status: FileStatus,
    /// Untracked files have no diff base.
    pub untracked: bool,
}

/// An open repository.
pub struct Repository {
    inner: gix::Repository,
    workdir: PathBuf,
}

impl std::fmt::Debug for Repository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Repository")
            .field("workdir", &self.workdir)
            .finish_non_exhaustive()
    }
}

impl Repository {
    /// Discovers the repository containing `path`.
    pub fn discover(path: &Path) -> Result<Self> {
        let inner = gix::discover(path).map_err(|_| Error::NotARepository(path.to_path_buf()))?;
        let workdir = inner.workdir().ok_or(Error::NoWorkTree)?.to_path_buf();
        Ok(Self { inner, workdir })
    }

    /// The working tree root.
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// The checked-out branch, or `None` when HEAD is detached.
    pub fn head_branch(&self) -> Option<String> {
        self.inner
            .head_name()
            .ok()
            .flatten()
            .map(|name| name.shorten().to_string())
    }

    /// A short description of HEAD, for the status header.
    pub fn head_description(&self) -> String {
        match self.head_branch() {
            Some(branch) => branch,
            None => self
                .inner
                .head_id()
                .map(|id| format!("detached at {}", id.shorten_or_id()))
                .unwrap_or_else(|_| "no commits yet".to_string()),
        }
    }

    /// Files that differ between the index and the working tree.
    ///
    /// Untracked files are included: a status view that hides new files is not
    /// much use.
    pub fn worktree_status(&self) -> Result<Vec<StatusEntry>> {
        use gix::status::index_worktree::Item;
        use gix::status::plumbing::index_as_worktree::{Change, EntryStatus};
        use gix::status::UntrackedFiles;

        let platform = self
            .inner
            .status(gix::progress::Discard)
            .map_err(git)?
            .untracked_files(UntrackedFiles::Files);

        let iter = platform.into_index_worktree_iter(Vec::new()).map_err(git)?;

        let mut entries = Vec::new();
        for item in iter {
            let item = item.map_err(git)?;
            let entry = match item {
                Item::Modification {
                    rela_path, status, ..
                } => {
                    let path = PathBuf::from(rela_path.to_string());
                    match status {
                        EntryStatus::Change(Change::Removed) => StatusEntry {
                            path,
                            status: FileStatus::Deleted,
                            untracked: false,
                        },
                        EntryStatus::Change(Change::Modification { .. }) => StatusEntry {
                            path,
                            status: FileStatus::Modified,
                            untracked: false,
                        },
                        EntryStatus::Change(Change::Type { .. }) => StatusEntry {
                            path,
                            status: FileStatus::TypeChanged,
                            untracked: false,
                        },
                        // `git add --intent-to-add` leaves a file that git
                        // still reports as new.
                        EntryStatus::IntentToAdd => StatusEntry {
                            path,
                            status: FileStatus::Added,
                            untracked: true,
                        },
                        _ => continue,
                    }
                }
                // The dirwalk also reports ignored and tracked entries.
                Item::DirectoryContents { entry, .. }
                    if entry.status == gix::dir::entry::Status::Untracked =>
                {
                    StatusEntry {
                        path: PathBuf::from(entry.rela_path.to_string()),
                        status: FileStatus::Added,
                        untracked: true,
                    }
                }
                Item::Rewrite {
                    source,
                    dirwalk_entry,
                    ..
                } => StatusEntry {
                    path: PathBuf::from(dirwalk_entry.rela_path.to_string()),
                    status: FileStatus::Renamed {
                        from: PathBuf::from(source.rela_path().to_string()),
                    },
                    untracked: false,
                },
                _ => continue,
            };
            entries.push(entry);
        }

        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(entries)
    }

    /// The diff of every changed file, index side against the working tree.
    pub fn worktree_diff(&self) -> Result<Vec<FileDiff>> {
        let mut diffs = Vec::new();
        for entry in self.worktree_status()? {
            diffs.extend(self.diff_entry(&entry)?);
        }
        Ok(diffs)
    }

    /// The diff of one status entry, or `None` when there is nothing to show.
    fn diff_entry(&self, entry: &StatusEntry) -> Result<Option<FileDiff>> {
        let absolute = self.workdir.join(&entry.path);

        let old = if entry.untracked {
            Vec::new()
        } else {
            self.blob_at_head(&entry.path).unwrap_or_default()
        };
        let new = match std::fs::read(&absolute) {
            Ok(content) => content,
            // The file is gone; that is a deletion, not an error.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(err) => return Err(err.into()),
        };

        if is_binary(&old) || is_binary(&new) {
            return Ok(Some(FileDiff {
                path: entry.path.clone(),
                status: entry.status.clone(),
                hunks: Vec::new(),
                folded: false,
                binary: true,
            }));
        }

        let old = String::from_utf8_lossy(&old).into_owned();
        let new = String::from_utf8_lossy(&new).into_owned();
        let body = unified_diff(&old, &new);
        if body.is_empty() {
            return Ok(None);
        }

        // Give the parser the file header it expects, so that one code path
        // builds every `FileDiff`.
        let source = entry_source(entry).display().to_string();
        let target = entry.path.display().to_string();
        let header = match &entry.status {
            FileStatus::Added => format!(
                "diff --git a/{target} b/{target}\nnew file mode 100644\n--- /dev/null\n+++ b/{target}\n"
            ),
            FileStatus::Deleted => format!(
                "diff --git a/{target} b/{target}\ndeleted file mode 100644\n--- a/{target}\n+++ /dev/null\n"
            ),
            FileStatus::Renamed { from } => format!(
                "diff --git a/{0} b/{target}\nrename from {0}\nrename to {target}\n--- a/{0}\n+++ b/{target}\n",
                from.display()
            ),
            _ => format!("diff --git a/{source} b/{target}\n--- a/{source}\n+++ b/{target}\n"),
        };

        Ok(crate::parse_unified_diff(&format!("{header}{body}"))
            .into_iter()
            .next())
    }

    /// The contents of a path as of HEAD.
    fn blob_at_head(&self, rela_path: &Path) -> Option<Vec<u8>> {
        let commit = self.inner.head_commit().ok()?;
        let tree = commit.tree().ok()?;
        let entry = tree.lookup_entry_by_path(rela_path).ok()??;
        let object = entry.object().ok()?;
        Some(object.detach().data)
    }
}

fn entry_source(entry: &StatusEntry) -> &Path {
    match &entry.status {
        FileStatus::Renamed { from } | FileStatus::Copied { from } => from,
        _ => &entry.path,
    }
}

/// git's own heuristic: a NUL byte near the start means binary.
fn is_binary(content: &[u8]) -> bool {
    content.iter().take(8000).any(|&byte| byte == 0)
}

/// Produces the hunks of a unified diff, without any file header.
pub fn unified_diff(old: &str, new: &str) -> String {
    let input = InternedInput::new(old, new);
    let diff = imara_diff::Diff::compute(Algorithm::Histogram, &input);
    diff.unified_diff(
        &BasicLineDiffPrinter(&input.interner),
        UnifiedDiffConfig::default(),
        &input,
    )
    .to_string()
}
