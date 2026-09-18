//! A thin Git layer over `gix`.
//!
//! Only what the status view needs: which files changed, and the diff of each
//! one. Diffs are produced as unified text and handed to
//! [`crate::parse_unified_diff`], so the model has a single source of truth
//! and works the same whether a diff came from here or from `git diff`.

use std::fmt;
use std::path::{Path, PathBuf};

use imara_diff::{Algorithm, InternedInput, UnifiedDiffConfig, UnifiedDiffPrinter};
use imara_diff::{Interner, Token};

use crate::diff::{FileDiff, FileStatus};
use crate::patch::Selection;

/// What can go wrong talking to a repository.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no git repository found for {0}")]
    NotARepository(PathBuf),
    #[error("the repository has no working tree")]
    NoWorkTree,
    #[error("git error: {0}")]
    Git(String),
    #[error("the selection contains no change to apply")]
    NothingSelected,
    #[error("{0} is binary; stage the whole file instead")]
    BinaryFile(PathBuf),
    #[error(transparent)]
    Apply(#[from] crate::patch::ApplyError),
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

        Ok(self.diff_contents(entry, &old, &new))
    }

    /// Builds the `FileDiff` between two blobs of one path.
    fn diff_contents(&self, entry: &StatusEntry, old: &[u8], new: &[u8]) -> Option<FileDiff> {
        if is_binary(old) || is_binary(new) {
            return Some(FileDiff {
                path: entry.path.clone(),
                status: entry.status.clone(),
                hunks: Vec::new(),
                folded: false,
                binary: true,
            });
        }

        let old = String::from_utf8_lossy(old).into_owned();
        let new = String::from_utf8_lossy(new).into_owned();
        let body = unified_diff(&old, &new);
        if body.is_empty() {
            return None;
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

        crate::parse_unified_diff(&format!("{header}{body}"))
            .into_iter()
            .next()
    }

    /// The diff of every file that differs between HEAD and the index.
    ///
    /// These are the staged changes, and the ones `unstage` reverses.
    pub fn staged_diff(&self) -> Result<Vec<FileDiff>> {
        let index = self.inner.index_or_empty().map_err(git)?;
        let mut diffs = Vec::new();

        for entry in index.entries() {
            let rela_path = PathBuf::from(entry.path(&index).to_string());
            let staged = self.blob(entry.id).unwrap_or_default();
            let head = self.blob_at_head(&rela_path).unwrap_or_default();

            if head == staged {
                continue;
            }

            let status = if head.is_empty() {
                FileStatus::Added
            } else {
                FileStatus::Modified
            };
            let status_entry = StatusEntry {
                path: rela_path,
                status,
                untracked: false,
            };

            if let Some(diff) = self.diff_contents(&status_entry, &head, &staged) {
                diffs.push(diff);
            }
        }

        diffs.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(diffs)
    }

    /// Applies the selected part of `file`'s diff to the index.
    ///
    /// `file` must come from [`Repository::worktree_diff`], whose old side is
    /// the index.
    pub fn stage(&self, file: &FileDiff, selection: &Selection) -> Result<()> {
        self.update_index(file, selection, false)
    }

    /// Reverses the selected part of `file`'s diff out of the index.
    ///
    /// `file` must come from [`Repository::staged_diff`], whose new side is
    /// the index.
    pub fn unstage(&self, file: &FileDiff, selection: &Selection) -> Result<()> {
        self.update_index(file, selection, true)
    }

    /// The shared half of staging and unstaging.
    ///
    /// Rather than shelling out to `git apply --cached`, the patch is applied
    /// to the index's own copy of the file in memory; the result is written as
    /// a blob and the index entry repointed at it. Nothing touches the working
    /// tree, so a failed apply cannot cost the user their edits.
    fn update_index(&self, file: &FileDiff, selection: &Selection, reverse: bool) -> Result<()> {
        let patch = crate::build_partial_patch(file, selection).ok_or(Error::NothingSelected)?;

        let rela_path = file.path.clone();
        let base = self.index_blob(&rela_path).unwrap_or_default();
        let base = String::from_utf8(base).map_err(|_| Error::BinaryFile(rela_path.clone()))?;

        let updated = crate::apply_patch(&base, &patch, reverse)?;
        let oid = self
            .inner
            .write_blob(updated.as_bytes())
            .map_err(git)?
            .detach();

        let index = self.inner.index_or_empty().map_err(git)?;
        let mut index = gix::fs::FileSnapshot::into_owned_or_cloned(index);
        let path = gix::path::into_bstr(rela_path.as_path()).into_owned();

        match index
            .entry_mut_by_path_and_stage(path.as_ref(), gix::index::entry::Stage::Unconflicted)
        {
            Some(entry) => {
                entry.id = oid;
                // The index no longer matches what was last stat'ed on disk;
                // zeroing it makes git re-read rather than trust the cache.
                entry.stat = gix::index::entry::Stat::default();
            }
            None => {
                index.dangerously_push_entry(
                    gix::index::entry::Stat::default(),
                    oid,
                    gix::index::entry::Flags::empty(),
                    gix::index::entry::Mode::FILE,
                    path.as_ref(),
                );
                index.sort_entries();
            }
        }

        index
            .write(gix::index::write::Options::default())
            .map_err(git)?;
        Ok(())
    }

    /// The contents of a path as recorded in the index.
    fn index_blob(&self, rela_path: &Path) -> Option<Vec<u8>> {
        let index = self.inner.index_or_empty().ok()?;
        let path = gix::path::into_bstr(rela_path);
        let entry = index.entry_by_path(path.as_ref())?;
        self.blob(entry.id)
    }

    fn blob(&self, id: gix::ObjectId) -> Option<Vec<u8>> {
        Some(self.inner.find_object(id).ok()?.detach().data)
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
        &NewlinePreservingPrinter(&input.interner),
        UnifiedDiffConfig::default(),
        &input,
    )
    .to_string()
}

/// A unified-diff printer that marks a line lacking a trailing newline.
///
/// imara-diff's own printer silently adds the newline back, which would be a
/// correctness bug here: staging a file whose last line lost its newline would
/// write the newline into the index. git's `\ No newline at end of file`
/// marker carries that fact, and the parser reads it back.
struct NewlinePreservingPrinter<'a>(&'a Interner<&'a str>);

impl NewlinePreservingPrinter<'_> {
    fn write_token(&self, mut f: impl fmt::Write, prefix: char, token: Token) -> fmt::Result {
        let text = self.0[token];
        write!(f, "{prefix}{text}")?;
        if !text.ends_with('\n') {
            writeln!(f)?;
            writeln!(f, "\\ No newline at end of file")?;
        }
        Ok(())
    }
}

impl UnifiedDiffPrinter for NewlinePreservingPrinter<'_> {
    fn display_header(
        &self,
        mut f: impl fmt::Write,
        start_before: u32,
        start_after: u32,
        len_before: u32,
        len_after: u32,
    ) -> fmt::Result {
        writeln!(
            f,
            "@@ -{},{} +{},{} @@",
            start_before + 1,
            len_before,
            start_after + 1,
            len_after
        )
    }

    fn display_context_token(&self, f: impl fmt::Write, token: Token) -> fmt::Result {
        self.write_token(f, ' ', token)
    }

    fn display_hunk(
        &self,
        mut f: impl fmt::Write,
        before: &[Token],
        after: &[Token],
    ) -> fmt::Result {
        for &token in before {
            self.write_token(&mut f, '-', token)?;
        }
        for &token in after {
            self.write_token(&mut f, '+', token)?;
        }
        Ok(())
    }
}
