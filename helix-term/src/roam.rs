//! Glue between the editor and the Org-Roam indexer.
//!
//! Both entry points return immediately: the walking and parsing happen on a
//! blocking thread, and the graph's write lock is taken only to install the
//! result.

use std::path::Path;

use helix_roam::scanner;
use helix_view::Editor;

/// Indexes the configured notes directory in the background.
///
/// Called once at startup. Does nothing when indexing is disabled.
pub fn start_initial_index(editor: &Editor) {
    let config = editor.config();
    if !config.roam.enable {
        return;
    }

    let directory = config.roam.directory();
    let graph = editor.roam.clone();

    tokio::spawn(async move {
        log::debug!("indexing Org-Roam directory {}", directory.display());
        match scanner::scan_directory_async(graph, directory).await {
            Ok(stats) => log::info!(
                "indexed {} Org-Roam nodes and {} links from {} files ({} unreadable)",
                stats.nodes,
                stats.links,
                stats.files,
                stats.errors,
            ),
            Err(err) => log::error!("Org-Roam indexing failed: {err}"),
        }
    });
}

/// Re-indexes a single Org file that was just written.
///
/// `text` is what was saved, so the graph matches the file on disk without
/// reading it back.
pub fn reindex_saved_file(editor: &Editor, path: &Path, text: String) {
    let config = editor.config();
    if !config.roam.enable || !is_org_file(path) {
        return;
    }

    let graph = editor.roam.clone();
    let path = path.to_path_buf();

    tokio::spawn(async move {
        if let Err(err) = scanner::reindex_file_async(graph, path.clone(), text).await {
            log::error!("failed to re-index {}: {err}", path.display());
        }
    });
}

fn is_org_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("org"))
}
