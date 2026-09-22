//! Indexing an Org-Roam directory into a [`RoamGraph`].
//!
//! Walking a notes directory and parsing every file is blocking work, so the
//! async entry points hand it to [`tokio::task::spawn_blocking`] and only take
//! the graph's write lock once the parsing is done.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::parser::{self, LinkTarget, ParsedFile};
use crate::{Link, RoamGraph};

/// What an indexing run found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexStats {
    pub files: usize,
    pub nodes: usize,
    pub links: usize,
    /// Files that could not be read or walked.
    pub errors: usize,
}

/// Collects the `.org` files under `root`, honouring ignore files.
///
/// Unreadable entries are counted rather than aborting the walk: one
/// unreadable note should not cost the user their whole index.
pub fn collect_org_files(root: &Path) -> (Vec<PathBuf>, usize) {
    let mut files = Vec::new();
    let mut errors = 0;

    for entry in ignore::WalkBuilder::new(root).hidden(false).build() {
        match entry {
            Ok(entry) => {
                let path = entry.path();
                if entry.file_type().is_some_and(|ft| ft.is_file()) && parser::is_org_file(path) {
                    files.push(path.to_path_buf());
                }
            }
            Err(_) => errors += 1,
        }
    }

    (files, errors)
}

/// Parses every `.org` file under `root` into a fresh graph.
///
/// This blocks; call it from [`scan_directory_async`] or another blocking
/// context.
pub fn scan_directory(root: &Path) -> (RoamGraph, IndexStats) {
    let (files, mut errors) = collect_org_files(root);

    let mut parsed = Vec::with_capacity(files.len());
    for path in files {
        match std::fs::read_to_string(&path) {
            Ok(text) => parsed.push(parser::parse_org(&text, path)),
            Err(_) => errors += 1,
        }
    }

    let mut graph = RoamGraph::new();
    let stats = apply_parsed(&mut graph, &parsed, errors);
    (graph, stats)
}

/// Inserts every node first, then the links.
///
/// Doing it in two passes means a link is never dropped just because its
/// target sits in a file that has not been parsed yet.
fn apply_parsed(graph: &mut RoamGraph, parsed: &[ParsedFile], errors: usize) -> IndexStats {
    let mut stats = IndexStats {
        files: parsed.len(),
        errors,
        ..IndexStats::default()
    };

    for file in parsed {
        for node in &file.nodes {
            graph.insert_node(node.clone());
            stats.nodes += 1;
        }
        for (key, id) in &file.refs {
            graph.register_ref(key.clone(), *id);
        }
        for (key, id) in &file.citations {
            graph.add_citation(key.clone(), *id);
        }
    }

    for file in parsed {
        for link in &file.links {
            match &link.target {
                LinkTarget::Id(target) => {
                    graph.add_link_deferred(link.source, *target, Link::Id);
                    stats.links += 1;
                }
                // A non-`id:` link is only an edge when some node claims it as
                // a ref; otherwise it is an ordinary URL or file link.
                LinkTarget::Ref(key) => {
                    if let Some(target) = graph.resolve_ref(key) {
                        graph.add_link_deferred(link.source, target, Link::Ref);
                        stats.links += 1;
                    }
                }
            }
        }
    }

    graph.resolve_pending_links();
    stats
}

/// Re-indexes a single file whose contents are already in hand.
///
/// The file's previous nodes are dropped first, so renamed titles and deleted
/// headlines do not survive as stale entries. Links from other files that
/// pointed at a node this file just gained are re-placed, because they stay
/// parked in the graph until their target appears.
pub fn reindex_file(graph: &mut RoamGraph, path: &Path, text: &str) -> IndexStats {
    graph.remove_nodes_in_file(path);
    let parsed = parser::parse_org(text, path);
    apply_parsed(graph, std::slice::from_ref(&parsed), 0)
}

/// Re-indexes a single file, reading it from disk.
pub fn reindex_file_from_disk(graph: &mut RoamGraph, path: &Path) -> std::io::Result<IndexStats> {
    let text = std::fs::read_to_string(path)?;
    Ok(reindex_file(graph, path, &text))
}

/// Indexes `root` off the async runtime and installs the result in `graph`.
///
/// The walk and every parse happen on a blocking thread; the write lock is
/// taken only to swap the finished graph in, so typing stays responsive while
/// a large notes directory is indexed.
pub async fn scan_directory_async(
    graph: Arc<RwLock<RoamGraph>>,
    root: PathBuf,
) -> Result<IndexStats, tokio::task::JoinError> {
    let (mut scanned, stats) = tokio::task::spawn_blocking(move || scan_directory(&root)).await?;

    let mut graph = graph.write();
    // A rebuild replaces the index, but not what the session has learned
    // about ids living outside it. Those files are never scanned, so a
    // location dropped here has no way of coming back.
    for (id, path) in graph.locations() {
        scanned.register_location(id, path);
    }
    *graph = scanned;

    Ok(stats)
}

/// Re-indexes one saved file off the async runtime.
///
/// Parsing runs on a blocking thread and the lock is taken only for the
/// update, which touches just this file's nodes.
pub async fn reindex_file_async(
    graph: Arc<RwLock<RoamGraph>>,
    path: PathBuf,
    text: String,
) -> Result<IndexStats, tokio::task::JoinError> {
    tokio::task::spawn_blocking(move || {
        let parsed = parser::parse_org(&text, &path);
        let mut graph = graph.write();
        graph.remove_nodes_in_file(&path);
        apply_parsed(&mut graph, std::slice::from_ref(&parsed), 0)
    })
    .await
}
