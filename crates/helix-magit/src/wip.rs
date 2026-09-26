//! Work-in-progress refs: uncommitted work, kept in commits nobody sees.
//!
//! As in Magit, each branch has two hidden refs, `refs/wip/index/<branch>`
//! and `refs/wip/wtree/<branch>` (`<branch>` being `refs/heads/main`, or
//! `HEAD` when detached). Saving adds a commit to each: the index as it is,
//! and the working tree's tracked files as they are, each on top of the
//! previous save — or on top of HEAD when HEAD has moved past the chain, so
//! a chain always starts from the commit the work is on. A save that would
//! record nothing new adds nothing.
//!
//! Nothing here touches the index, the working tree or any branch: the
//! working tree's commit is built in a separate index file.

use std::path::{Path, PathBuf};

use crate::command::GitCommand;

/// The two chains of one branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WipRefs {
    pub index: String,
    pub worktree: String,
}

fn git(workdir: &Path, args: &[&str]) -> Option<String> {
    run(workdir, args, &[]).ok()
}

/// Runs git with extra environment; the trimmed output, or git's reason.
fn run(workdir: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<String, String> {
    let mut command = GitCommand::new(workdir, args.iter().map(|arg| arg.to_string()).collect());
    for (key, value) in env {
        command = command.with_env(*key, *value);
    }
    let output = command.run().map_err(|err| err.to_string())?;
    if output.success {
        Ok(output.stdout.trim().to_string())
    } else {
        Err(output.summary())
    }
}

/// The wip refs of the checked-out branch.
pub fn refs(workdir: &Path) -> WipRefs {
    let branch = git(workdir, &["symbolic-ref", "--quiet", "HEAD"])
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "HEAD".to_string());
    WipRefs {
        index: format!("refs/wip/index/{branch}"),
        worktree: format!("refs/wip/wtree/{branch}"),
    }
}

/// What a save recorded: the new commits, when there was something new.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Saved {
    pub index: Option<String>,
    pub worktree: Option<String>,
}

/// Saves the index and the working tree's tracked files — only `paths` of
/// them when given, as after writing one file — to the wip refs.
///
/// Before the first commit there is nothing to hang a chain on, and nothing
/// is saved.
pub fn save(workdir: &Path, message: &str, paths: Option<&[PathBuf]>) -> Result<Saved, String> {
    let Some(head) = git(workdir, &["rev-parse", "--verify", "--quiet", "HEAD"]) else {
        return Ok(Saved::default());
    };
    let refs = refs(workdir);
    let mut saved = Saved::default();

    // The index, unless it holds a conflict, which has no tree.
    if let Ok(tree) = run(workdir, &["write-tree"], &[]) {
        saved.index = record(workdir, &refs.index, &head, &tree, message)?;
    }

    // The working tree, through a copy of the index so the real one is
    // never touched.
    let git_dir = crate::status::git_dir(workdir).ok_or("no git directory")?;
    let scratch = git_dir.join("helix-wip-index");
    let real = git_dir.join("index");
    if real.exists() {
        std::fs::copy(&real, &scratch).map_err(|err| err.to_string())?;
        // With the real index's time: git takes an entry changed in the
        // same second the index was written as possibly stale, and reads
        // the file. A copy dated now would make such an entry — a file
        // rewritten at the same size just after a commit — look unchanged.
        let modified = std::fs::metadata(&real)
            .and_then(|meta| meta.modified())
            .map_err(|err| err.to_string())?;
        std::fs::File::options()
            .write(true)
            .open(&scratch)
            .and_then(|file| file.set_modified(modified))
            .map_err(|err| err.to_string())?;
    } else {
        let _ = std::fs::remove_file(&scratch);
    }
    let scratch_str = scratch.display().to_string();
    let env = [("GIT_INDEX_FILE", scratch_str.as_str())];
    let mut add: Vec<String> = vec!["add".into(), "--update".into(), "--".into()];
    match paths {
        Some(paths) => add.extend(paths.iter().map(|path| path.display().to_string())),
        None => add.push(".".into()),
    }
    let add: Vec<&str> = add.iter().map(String::as_str).collect();
    let tree = run(workdir, &add, &env).and_then(|_| run(workdir, &["write-tree"], &env));
    let _ = std::fs::remove_file(&scratch);
    saved.worktree = record(workdir, &refs.worktree, &head, &tree?, message)?;
    Ok(saved)
}

/// Adds a commit of `tree` to the chain at `wip_ref`, unless it records
/// nothing new; returns the new commit.
fn record(
    workdir: &Path,
    wip_ref: &str,
    head: &str,
    tree: &str,
    message: &str,
) -> Result<Option<String>, String> {
    let tip = git(workdir, &["rev-parse", "--verify", "--quiet", wip_ref]);
    // Continue the chain while it still builds on HEAD; start over from
    // HEAD once HEAD has moved past it.
    let parent = match &tip {
        Some(tip) if run(workdir, &["merge-base", "--is-ancestor", head, tip], &[]).is_ok() => {
            tip.clone()
        }
        _ => head.to_string(),
    };
    let parent_tree = git(workdir, &["rev-parse", &format!("{parent}^{{tree}}")]);
    if parent_tree.as_deref() == Some(tree) {
        return Ok(None);
    }

    let args = ["commit-tree", tree, "-p", &parent, "-m", message];
    let commit = match run(workdir, &args, &[]) {
        Ok(commit) => commit,
        // No identity configured: a private commit still has to have one.
        Err(_) => run(
            workdir,
            &args,
            &[
                ("GIT_AUTHOR_NAME", "Helix wip"),
                ("GIT_AUTHOR_EMAIL", "wip@helix.invalid"),
                ("GIT_COMMITTER_NAME", "Helix wip"),
                ("GIT_COMMITTER_EMAIL", "wip@helix.invalid"),
            ],
        )?,
    };
    let old = tip.unwrap_or_else(|| "0".repeat(40));
    run(
        workdir,
        &["update-ref", "-m", message, wip_ref, &commit, &old],
        &[],
    )?;
    Ok(Some(commit))
}
