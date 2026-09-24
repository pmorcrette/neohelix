//! A thin Git layer over `gix`.
//!
//! Only what the status view needs: which files changed, and the diff of each
//! one. Diffs are produced as unified text and handed to
//! [`crate::parse_unified_diff`], so the model has a single source of truth
//! and works the same whether a diff came from here or from `git diff`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use imara_diff::{Algorithm, InternedInput, Token};

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

/// A path left in conflict by a merge, rebase or cherry-pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unmerged {
    pub path: PathBuf,
    /// How it conflicts, in `git status`'s words: "both modified", …
    pub state: &'static str,
}

/// `git status`'s name for a conflict, from which of the base, ours and
/// theirs the index holds.
fn conflict_state([base, ours, theirs]: [bool; 3]) -> &'static str {
    match (base, ours, theirs) {
        (true, true, true) => "both modified",
        (false, true, true) => "both added",
        (true, true, false) => "deleted by them",
        (true, false, true) => "deleted by us",
        (false, true, false) => "added by us",
        (false, false, true) => "added by them",
        _ => "both deleted",
    }
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

    /// The same diffs as [`Repository::worktree_diff`], split into changes
    /// to tracked files and files git does not track yet, which the status
    /// buffer shows as separate sections.
    ///
    /// A file added with `--intent-to-add` has an index entry, so it counts
    /// as tracked, as `git status` counts it.
    pub fn worktree_diffs(&self) -> Result<(Vec<FileDiff>, Vec<FileDiff>)> {
        let mut tracked = Vec::new();
        let mut untracked = Vec::new();
        for entry in self.worktree_status()? {
            let Some(diff) = self.diff_entry(&entry)? else {
                continue;
            };
            if entry.untracked && self.index_blob(&entry.path).is_none() {
                untracked.push(diff);
            } else {
                tracked.push(diff);
            }
        }
        Ok((tracked, untracked))
    }

    /// The diff of one status entry, or `None` when there is nothing to show.
    fn diff_entry(&self, entry: &StatusEntry) -> Result<Option<FileDiff>> {
        let absolute = self.workdir.join(&entry.path);

        // The old side is the index, not HEAD: these are the *unstaged*
        // changes, and `stage` applies the resulting patch to the index. Using
        // HEAD here would produce a patch that does not match what it is
        // applied to as soon as the file already has something staged.
        let old = if entry.untracked {
            Vec::new()
        } else {
            self.index_blob(&entry.path).unwrap_or_default()
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
            // A conflicted path has up to three entries, none of them "the"
            // staged version; they are listed by `unmerged` instead.
            if entry.stage() != gix::index::entry::Stage::Unconflicted {
                continue;
            }
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

        // A path HEAD has and the index does not is a staged deletion, and
        // walking the index alone never meets it.
        let in_index: std::collections::HashSet<Vec<u8>> = index
            .entries()
            .iter()
            .map(|entry| entry.path(&index).to_vec())
            .collect();
        for (rela_path, id) in self.head_blobs() {
            let key = gix::path::into_bstr(rela_path.as_path()).to_vec();
            if in_index.contains(&key) {
                continue;
            }
            let head = self.blob(id).unwrap_or_default();
            let status_entry = StatusEntry {
                path: rela_path,
                status: FileStatus::Deleted,
                untracked: false,
            };
            if let Some(diff) = self.diff_contents(&status_entry, &head, &[]) {
                diffs.push(diff);
            }
        }

        diffs.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(diffs)
    }

    /// Every file in HEAD's tree, with its blob. Empty before the first
    /// commit.
    fn head_blobs(&self) -> Vec<(PathBuf, gix::ObjectId)> {
        let Some(tree) = self
            .inner
            .head_commit()
            .ok()
            .and_then(|commit| commit.tree().ok())
        else {
            return Vec::new();
        };
        let mut recorder = gix::traverse::tree::Recorder::default();
        if tree.traverse().breadthfirst(&mut recorder).is_err() {
            return Vec::new();
        }
        recorder
            .records
            .into_iter()
            .filter(|entry| entry.mode.is_blob_or_symlink())
            .map(|entry| (PathBuf::from(entry.filepath.to_string()), entry.oid))
            .collect()
    }

    /// The paths the index holds in conflict, with how they conflict.
    pub fn unmerged(&self) -> Result<Vec<Unmerged>> {
        let index = self.inner.index_or_empty().map_err(git)?;
        let mut paths: Vec<(PathBuf, [bool; 3])> = Vec::new();
        for entry in index.entries() {
            let stage = entry.stage_raw() as usize;
            if stage == 0 {
                continue;
            }
            let path = PathBuf::from(entry.path(&index).to_string());
            match paths.last_mut() {
                Some((last, stages)) if *last == path => stages[stage - 1] = true,
                _ => {
                    let mut stages = [false; 3];
                    stages[stage - 1] = true;
                    paths.push((path, stages));
                }
            }
        }
        Ok(paths
            .into_iter()
            .map(|(path, stages)| Unmerged {
                path,
                state: conflict_state(stages),
            })
            .collect())
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

    /// Throws away the selected part of `file`'s changes. They cannot be
    /// got back, so the caller asks first.
    ///
    /// An unstaged change is reverse-applied to the working tree, leaving it
    /// as the index has it. A staged change is removed from both the index
    /// and the working tree, as Magit does; the working tree is checked
    /// first, so when it has moved on from what was staged nothing changes.
    pub fn discard(&self, file: &FileDiff, selection: &Selection, staged: bool) -> Result<()> {
        if file.binary {
            return self.discard_binary(file, selection, staged);
        }
        let reversed = self.worktree_reversed(file, selection)?;
        if staged {
            self.unstage(file, selection)?;
        }
        self.write_worktree(&file.path, reversed)
    }

    /// Reverse-applies the selected part of a staged change to the working
    /// tree only, leaving the index alone: Magit's `v`.
    pub fn reverse(&self, file: &FileDiff, selection: &Selection) -> Result<()> {
        if file.binary {
            return Err(Error::BinaryFile(file.path.clone()));
        }
        let reversed = self.worktree_reversed(file, selection)?;
        self.write_worktree(&file.path, reversed)
    }

    /// Applies the selected part of a commit's change to the working tree,
    /// or with `reverse` takes it back out — Magit's `a` and `v` in a
    /// commit. The working tree has to have the lines the change expects;
    /// if not, nothing is written.
    pub fn apply_to_worktree(
        &self,
        file: &FileDiff,
        selection: &Selection,
        reverse: bool,
    ) -> Result<()> {
        if file.binary {
            return Err(Error::BinaryFile(file.path.clone()));
        }
        if reverse {
            let reversed = self.worktree_reversed(file, selection)?;
            return self.write_worktree(&file.path, reversed);
        }
        let patch = crate::build_partial_patch(file, selection).ok_or(Error::NothingSelected)?;
        let current = match std::fs::read(self.workdir.join(&file.path)) {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(err) => return Err(err.into()),
        };
        let current =
            String::from_utf8(current).map_err(|_| Error::BinaryFile(file.path.clone()))?;
        let applied = crate::apply_patch(&current, &patch, false)?;
        let deletes = applied.is_empty() && file.status == FileStatus::Deleted;
        self.write_worktree(&file.path, (!deletes).then_some(applied))
    }

    /// Deletes a file git does not track.
    pub fn discard_untracked(&self, rela_path: &Path) -> Result<()> {
        std::fs::remove_file(self.workdir.join(rela_path))?;
        Ok(())
    }

    /// What the working tree file becomes with the selection reversed out of
    /// it; `None` when that leaves a new file with nothing in it, which is
    /// then deleted rather than kept empty.
    fn worktree_reversed(&self, file: &FileDiff, selection: &Selection) -> Result<Option<String>> {
        let patch = crate::build_reverse_patch(file, selection).ok_or(Error::NothingSelected)?;
        let current = match std::fs::read(self.workdir.join(&file.path)) {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(err) => return Err(err.into()),
        };
        let current =
            String::from_utf8(current).map_err(|_| Error::BinaryFile(file.path.clone()))?;
        let reversed = crate::apply_patch(&current, &patch, true)?;
        let deletes = reversed.is_empty() && file.status == FileStatus::Added;
        Ok((!deletes).then_some(reversed))
    }

    fn write_worktree(&self, rela_path: &Path, content: Option<String>) -> Result<()> {
        let path = self.workdir.join(rela_path);
        match content {
            Some(content) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(path, content)?;
            }
            None => match std::fs::remove_file(path) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err.into()),
                _ => {}
            },
        }
        Ok(())
    }

    /// A binary file can only be discarded whole, by putting back the
    /// index's copy — or, for a staged change, HEAD's in both places.
    fn discard_binary(&self, file: &FileDiff, selection: &Selection, staged: bool) -> Result<()> {
        if *selection != Selection::File || staged {
            return Err(Error::BinaryFile(file.path.clone()));
        }
        let path = self.workdir.join(&file.path);
        match self.index_blob(&file.path) {
            Some(content) => std::fs::write(path, content)?,
            None => std::fs::remove_file(path)?,
        }
        Ok(())
    }

    /// `git add -u`: every change to a tracked file, staged.
    pub fn stage_all(&self) -> Result<()> {
        self.run_git(&["add", "--update", "--", "."])
    }

    /// Everything staged goes back to unstaged, the working tree untouched.
    pub fn unstage_all(&self) -> Result<()> {
        if self.inner.head_id().is_ok() {
            self.run_git(&["reset", "--quiet", "--", "."])
        } else {
            // Before the first commit there is no HEAD to reset to.
            self.run_git(&["rm", "-r", "--cached", "--quiet", "--", "."])
        }
    }

    fn run_git(&self, args: &[&str]) -> Result<()> {
        let output = crate::GitCommand::new(
            &self.workdir,
            args.iter().map(|arg| arg.to_string()).collect(),
        )
        .run()?;
        if output.success {
            Ok(())
        } else {
            Err(Error::Git(output.summary()))
        }
    }

    /// The shared half of staging and unstaging.
    ///
    /// Rather than shelling out to `git apply --cached`, the patch is applied
    /// to the index's own copy of the file in memory; the result is written as
    /// a blob and the index entry repointed at it. Nothing touches the working
    /// tree, so a failed apply cannot cost the user their edits.
    fn update_index(&self, file: &FileDiff, selection: &Selection, reverse: bool) -> Result<()> {
        let patch = if reverse {
            crate::build_reverse_patch(file, selection)
        } else {
            crate::build_partial_patch(file, selection)
        }
        .ok_or(Error::NothingSelected)?;

        let rela_path = file.path.clone();
        let base = self.index_blob(&rela_path).unwrap_or_default();
        let base = String::from_utf8(base).map_err(|_| Error::BinaryFile(rela_path.clone()))?;

        let updated = crate::apply_patch(&base, &patch, reverse)?;

        // Staging all of a deletion, or unstaging all of an addition, takes
        // the path out of the index — `git rm --cached` — rather than
        // leaving an empty file tracked in its place.
        let removes_path = updated.is_empty()
            && match file.status {
                FileStatus::Deleted => !reverse,
                FileStatus::Added => reverse,
                _ => false,
            };
        if removes_path {
            let index = self.inner.index_or_empty().map_err(git)?;
            let mut index = gix::fs::FileSnapshot::into_owned_or_cloned(index);
            let path = gix::path::into_bstr(rela_path.as_path()).into_owned();
            index.remove_entries(|_, entry_path, _| entry_path == path.as_slice());
            index
                .write(gix::index::write::Options::default())
                .map_err(git)?;
            return Ok(());
        }

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

    /// The contents of a path as of HEAD, which is the old side of the
    /// *staged* diff.
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
///
/// The hunks are emitted here rather than through imara-diff's own
/// `UnifiedDiff` printer, which mis-states the first hunk's `old_start`: it
/// writes every line before the first change as context but reports a start
/// `context_len` lines later, so any patch whose first change is more than
/// three lines into the file is unusable. Emitting them directly also lets the
/// `\ No newline at end of file` marker survive, which that printer drops.
pub fn unified_diff(old: &str, new: &str) -> String {
    const CONTEXT: u32 = 3;

    let input = InternedInput::new(old, new);
    let diff = imara_diff::Diff::compute(Algorithm::Histogram, &input);
    let (before, after) = (&input.before, &input.after);

    let mut out = String::new();
    let hunks: Vec<_> = diff.hunks().collect();
    let mut index = 0;

    while index < hunks.len() {
        // Hunks closer than twice the context would print overlapping
        // context, so git merges them into one; so do we.
        let mut end = index + 1;
        while end < hunks.len()
            && hunks[end].before.start <= hunks[end - 1].before.end + 2 * CONTEXT
        {
            end += 1;
        }
        let group = &hunks[index..end];

        let before_start = group[0].before.start.saturating_sub(CONTEXT);
        let before_end = (group[group.len() - 1].before.end + CONTEXT).min(before.len() as u32);
        let after_start = group[0].after.start.saturating_sub(CONTEXT);
        let after_end = (group[group.len() - 1].after.end + CONTEXT).min(after.len() as u32);

        let _ = writeln!(
            out,
            "@@ -{} +{} @@",
            format_range(before_start, before_end - before_start),
            format_range(after_start, after_end - after_start),
        );

        let mut pos = before_start;
        for hunk in group {
            emit_tokens(
                &mut out,
                &input,
                ' ',
                &before[pos as usize..hunk.before.start as usize],
            );
            emit_tokens(
                &mut out,
                &input,
                '-',
                &before[hunk.before.start as usize..hunk.before.end as usize],
            );
            emit_tokens(
                &mut out,
                &input,
                '+',
                &after[hunk.after.start as usize..hunk.after.end as usize],
            );
            pos = hunk.before.end;
        }
        emit_tokens(
            &mut out,
            &input,
            ' ',
            &before[pos as usize..before_end as usize],
        );

        index = end;
    }

    out
}

/// Formats one side of a hunk header from a 0-based start and a count.
///
/// An empty range is anchored at the position itself, as git does for a pure
/// insertion; anything else is 1-based.
fn format_range(start: u32, count: u32) -> String {
    if count == 0 {
        format!("{start},0")
    } else {
        format!("{},{count}", start + 1)
    }
}

/// Writes each token with its prefix, marking a missing trailing newline.
fn emit_tokens(out: &mut String, input: &InternedInput<&str>, prefix: char, tokens: &[Token]) {
    for &token in tokens {
        let text = input.interner[token];
        let _ = write!(out, "{prefix}{text}");
        if !text.ends_with('\n') {
            let _ = writeln!(out);
            let _ = writeln!(out, "\\ No newline at end of file");
        }
    }
}
