//! Versions of a file side by side: what Magit does with Ediff.
//!
//! Helix has no diff mode, but it has splits and a diff gutter against any
//! base. So each version is written where a buffer can open it, under
//! `.git/helix/ediff/<version>/`, and the editor lays them out beside each
//! other with the gutter set against the version they are compared with.
//! The index's version can be edited: writing it stages it.

use std::path::{Path, PathBuf};

use crate::command::GitCommand;

/// A version of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Version {
    Head,
    Index,
    /// A conflict's stage: 1 is the base, 2 ours, 3 theirs.
    Stage(u8),
    /// Any commit-ish.
    Rev(String),
    Worktree,
}

impl Version {
    /// How the version is named: in its buffer's path, and to the user.
    pub fn label(&self) -> String {
        match self {
            Version::Head => "HEAD".into(),
            Version::Index => "index".into(),
            Version::Stage(1) => "base".into(),
            Version::Stage(2) => "ours".into(),
            Version::Stage(_) => "theirs".into(),
            Version::Rev(rev) => rev.clone(),
            Version::Worktree => "worktree".into(),
        }
    }

    /// `git show`'s name for the file at this version.
    fn spec(&self, path: &Path) -> Option<String> {
        let path = path.display();
        match self {
            Version::Head => Some(format!("HEAD:{path}")),
            Version::Index => Some(format!(":{path}")),
            Version::Stage(stage) => Some(format!(":{stage}:{path}")),
            Version::Rev(rev) => Some(format!("{rev}:{path}")),
            Version::Worktree => None,
        }
    }
}

fn git(workdir: &Path, args: &[&str]) -> Option<String> {
    let output = GitCommand::new(workdir, args.iter().map(|arg| arg.to_string()).collect())
        .run()
        .ok()?;
    output.success.then_some(output.stdout)
}

/// The file's content at `version`; `None` when it is not there (a file
/// added since, or deleted).
pub fn content(workdir: &Path, version: &Version, path: &Path) -> Option<String> {
    match version.spec(path) {
        Some(spec) => git(workdir, &["show", &spec]),
        None => std::fs::read_to_string(workdir.join(path)).ok(),
    }
}

/// Writes `content` where a buffer for `version` of `path` opens it, and
/// returns that place.
pub fn materialize(
    workdir: &Path,
    version: &Version,
    path: &Path,
    content: &str,
) -> Result<PathBuf, String> {
    let git_dir = crate::status::git_dir(workdir).ok_or("not in a git repository")?;
    // A revision such as `HEAD~1` or `stash@{0}` makes a plain directory name.
    let label: String = version
        .label()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let file = git_dir.join("helix").join("ediff").join(label).join(path);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    std::fs::write(&file, content).map_err(|err| err.to_string())?;
    Ok(file)
}

/// Stages `content` as `path`'s version in the index, keeping its mode.
pub fn write_index(workdir: &Path, path: &Path, content: &str) -> Result<(), String> {
    let git_dir = crate::status::git_dir(workdir).ok_or("not in a git repository")?;
    let scratch = git_dir.join("helix").join("ediff-index-blob");
    if let Some(parent) = scratch.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    std::fs::write(&scratch, content).map_err(|err| err.to_string())?;
    let hash = git(
        workdir,
        &[
            "hash-object",
            "-w",
            "--no-filters",
            &scratch.display().to_string(),
        ],
    )
    .ok_or("git hash-object failed")?;
    let _ = std::fs::remove_file(&scratch);
    let path_text = path.display().to_string();
    let mode = git(workdir, &["ls-files", "--stage", "--", &path_text])
        .and_then(|line| line.split_whitespace().next().map(str::to_string))
        .unwrap_or_else(|| "100644".into());
    git(
        workdir,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("{mode},{},{path_text}", hash.trim()),
        ],
    )
    .map(|_| ())
    .ok_or_else(|| format!("could not stage {path_text}"))
}

/// The paths `git diff --name-only` lists with `args`: what a comparison
/// offers to choose from.
pub fn changed_paths(workdir: &Path, args: &[&str]) -> Vec<String> {
    let mut full = vec!["diff", "--name-only"];
    full.extend_from_slice(args);
    git(workdir, &full)
        .map(|text| text.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// The paths in conflict.
pub fn unmerged_paths(workdir: &Path) -> Vec<String> {
    changed_paths(workdir, &["--diff-filter=U"])
}

/// What `E E` does with a path, Magit's dwim: resolve it when it is in
/// conflict, compare its unstaged changes when it has some, else its
/// staged ones.
pub fn dwim(workdir: &Path, path: &str) -> Option<Dwim> {
    if unmerged_paths(workdir).iter().any(|p| p == path) {
        return Some(Dwim::Resolve);
    }
    if changed_paths(workdir, &["--", path])
        .iter()
        .any(|p| p == path)
    {
        return Some(Dwim::Unstaged);
    }
    if changed_paths(workdir, &["--cached", "--", path])
        .iter()
        .any(|p| p == path)
    {
        return Some(Dwim::Staged);
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dwim {
    Resolve,
    Unstaged,
    Staged,
}
