//! Glue between the editor and the Org-Roam indexer.
//!
//! Both entry points return immediately: the walking and parsing happen on a
//! blocking thread, and the graph's write lock is taken only to install the
//! result.

use std::path::{Path, PathBuf};
use std::sync::Arc;

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

/// Applies an Org restructuring to the focused document.
///
/// The transformations are text-to-text, so the result is diffed against the
/// buffer rather than replacing it wholesale: that keeps the undo history and
/// the cursor meaningful instead of marking every line as changed.
fn restructure(
    editor: &mut Editor,
    what: &'static str,
    transform: impl FnOnce(&str) -> Result<String, helix_roam::restructure::Error>,
) {
    let doc = doc_mut!(editor);
    let before = doc.text().clone();

    match transform(&before.to_string()) {
        Ok(after) => {
            let after = helix_core::Rope::from(after.as_str());
            if after == before {
                editor.set_status(format!("{what}: nothing to change"));
                return;
            }
            let transaction = helix_core::diff::compare_ropes(&before, &after);
            let view = view!(editor).id;
            doc_mut!(editor).apply(&transaction, view);
            editor.set_status(what);
        }
        Err(err) => editor.set_error(err.to_string()),
    }
}

/// Turns a file whose whole content is one heading into a file-level node.
pub fn promote_buffer(editor: &mut Editor) {
    restructure(editor, "Promoted the buffer", |text| {
        helix_roam::restructure::promote_buffer(text)
    });
}

/// Turns a file-level node into a single heading holding the file.
pub fn demote_buffer(editor: &mut Editor) {
    restructure(editor, "Demoted the buffer", |text| {
        helix_roam::restructure::demote_buffer(text)
    });
}

/// Rewrites this buffer's legacy `roam:` links as `id:` links.
pub fn replace_roam_links(editor: &mut Editor) {
    let graph = Arc::clone(&editor.roam);

    restructure(editor, "Replaced roam: links", move |text| {
        let graph = graph.read();
        Ok(helix_roam::restructure::replace_roam_links(text, |title| {
            graph
                .nodes()
                .find(|node| node.matches_title(title))
                .map(|node| node.id)
        }))
    });
}

/// Extracts the subtree at the cursor into a file of its own.
///
/// The new file is named after the subtree's title, beside the current one.
/// Nothing is written until both halves are known to be sound: a failure to
/// create the file leaves the source buffer untouched.
pub fn extract_subtree(editor: &mut Editor) {
    let (view, doc) = current_ref!(editor);
    let Some(source) = doc.path().map(Path::to_path_buf) else {
        editor.set_error("the buffer has no path to extract beside");
        return;
    };

    let text = doc.text().clone();
    let line = text.char_to_line(doc.selection(view.id).primary().cursor(text.slice(..)));

    let extraction = match helix_roam::restructure::extract_subtree(
        &text.to_string(),
        line,
        helix_roam::Uuid::new_v4(),
    ) {
        Ok(extraction) => extraction,
        Err(err) => {
            editor.set_error(err.to_string());
            return;
        }
    };

    let target = source
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{}.org", slugify(&extraction.title)));

    if target.exists() {
        editor.set_error(format!("{} already exists", target.display()));
        return;
    }
    if let Err(err) = std::fs::write(&target, &extraction.extracted) {
        editor.set_error(format!("could not write {}: {err}", target.display()));
        return;
    }

    let after = helix_core::Rope::from(extraction.remaining.as_str());
    let transaction = helix_core::diff::compare_ropes(&text, &after);
    let view = view!(editor).id;
    doc_mut!(editor).apply(&transaction, view);

    editor.set_status(format!("Extracted to {}", target.display()));
}

/// A file name that is safe on every platform and still readable.
fn slugify(title: &str) -> String {
    let slug: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();

    let slug = slug.trim_matches('-').to_string();
    // Collapse the runs the mapping above creates.
    let mut out = String::with_capacity(slug.len());
    for c in slug.chars() {
        if c != '-' || !out.ends_with('-') {
            out.push(c);
        }
    }

    if out.is_empty() {
        "node".to_string()
    } else {
        out
    }
}

/// What the refile picker needs to know about a candidate target.
pub struct RefileTarget {
    pub title: String,
    pub path: std::path::PathBuf,
    pub id: helix_roam::Uuid,
}

/// The nodes a subtree could be refiled into.
///
/// Snapshots the graph rather than holding its lock, like the node picker, so
/// the background indexer stays free while the picker is open.
pub fn refile_targets(editor: &Editor) -> Vec<RefileTarget> {
    let graph = editor.roam.read();
    graph
        .nodes()
        .map(|node| RefileTarget {
            title: node.title.clone(),
            path: node.file_path.clone(),
            id: node.id,
        })
        .collect()
}

/// Moves the subtree at the cursor into `target`.
///
/// Both files are written only once both halves have been computed, so a
/// failure on either side leaves the pair as it was rather than losing the
/// subtree between them.
pub fn refile_into(editor: &mut Editor, target_path: &Path, target_id: helix_roam::Uuid) {
    let (view, doc) = current_ref!(editor);
    let source_path = doc.path().map(Path::to_path_buf);
    let text = doc.text().clone();
    let line = text.char_to_line(doc.selection(view.id).primary().cursor(text.slice(..)));

    if source_path.as_deref() == Some(target_path) {
        // Moving a subtree inside its own file would need the cut and the
        // insertion to be computed against the same text; refusing is honest.
        editor.set_error("Refiling within the same file is not supported yet");
        return;
    }

    let target_text = match std::fs::read_to_string(target_path) {
        Ok(text) => text,
        Err(err) => {
            editor.set_error(format!("could not read {}: {err}", target_path.display()));
            return;
        }
    };

    let refiling = match helix_roam::restructure::refile_subtree(
        &text.to_string(),
        line,
        &target_text,
        target_id,
    ) {
        Ok(refiling) => refiling,
        Err(err) => {
            editor.set_error(err.to_string());
            return;
        }
    };

    // The target is on disk and may not be open, so it is written directly.
    if let Err(err) = std::fs::write(target_path, &refiling.target) {
        editor.set_error(format!("could not write {}: {err}", target_path.display()));
        return;
    }

    let after = helix_core::Rope::from(refiling.source.as_str());
    let transaction = helix_core::diff::compare_ropes(&text, &after);
    let view = view!(editor).id;
    doc_mut!(editor).apply(&transaction, view);

    editor.set_status(format!(
        "Refiled \"{}\" into {}",
        refiling.title,
        target_path.display()
    ));
}

/// The register a stored link goes into.
///
/// A register rather than a field of its own, so the link can also be pasted
/// with Helix's ordinary paste instead of only by the command below.
const LINK_REGISTER: char = 'o';

/// The file's link abbreviations, which are per file rather than global.
fn abbreviations(doc: &helix_view::Document) -> Vec<(String, String)> {
    helix_roam::FileSettings::scan(&doc.text().to_string()).link_abbreviations
}

/// Byte offset of the cursor in the focused document.
fn cursor_offset(editor: &Editor) -> usize {
    let (view, doc) = current_ref!(editor);
    let text = doc.text().slice(..);
    text.char_to_byte(doc.selection(view.id).primary().cursor(text))
}

/// Moves the cursor to a byte offset, remembering where it came from.
fn jump_to_byte(editor: &mut Editor, byte: usize) {
    let view_id = view!(editor).id;
    let doc_id = doc!(editor).id();
    let selection = doc!(editor).selection(view_id).clone();

    let (view, doc) = current!(editor);
    view.push_jump(doc, (doc_id, selection));

    let char_at = doc.text().byte_to_char(byte.min(doc.text().len_bytes()));
    doc.set_selection(view_id, helix_core::Selection::point(char_at));
}

/// Follows the Org link under the cursor.
pub fn follow_link(editor: &mut Editor) {
    let offset = cursor_offset(editor);
    let (text, abbrevs, dir) = {
        let doc = doc!(editor);
        (
            doc.text().to_string(),
            abbreviations(doc),
            doc.path()
                .and_then(|p| p.parent())
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(".")),
        )
    };

    let Some(link) = helix_roam::hyperlink::link_at(&text, offset, &abbrevs) else {
        editor.set_error("No Org link under the cursor");
        return;
    };

    match link.kind {
        helix_roam::LinkKind::Id(id) => follow_node(editor, id),
        helix_roam::LinkKind::Roam(title) => {
            let found = editor
                .roam
                .read()
                .nodes()
                .find(|node| node.matches_title(&title))
                .map(|node| node.id);
            match found {
                Some(id) => follow_node(editor, id),
                None => editor.set_error(format!("No node titled \"{title}\"")),
            }
        }
        helix_roam::LinkKind::File { path, search } => {
            follow_file(editor, &dir.join(&path), search)
        }
        helix_roam::LinkKind::Url(url) => open_externally(editor, &url),
        helix_roam::LinkKind::Mailto(who) => open_externally(editor, &format!("mailto:{who}")),
        helix_roam::LinkKind::Headline(title) => {
            follow_in_buffer(editor, &text, &format!("* {title}"), "headline")
        }
        helix_roam::LinkKind::CustomId(id) => {
            follow_in_buffer(editor, &text, &format!(":CUSTOM_ID: {id}"), "CUSTOM_ID")
        }
        helix_roam::LinkKind::Target(target) => {
            follow_in_buffer(editor, &text, &format!("<<{target}>>"), "target")
        }
        helix_roam::LinkKind::Other { scheme, .. } => {
            editor.set_error(format!("Links of type `{scheme}:` are not handled"));
        }
    }
}

/// Opens the file a node lives in, at the node.
fn follow_node(editor: &mut Editor, id: helix_roam::Uuid) {
    let found = editor
        .roam
        .read()
        .get_node(&id)
        .map(|node| (node.file_path.clone(), node.line));

    let Some((path, line)) = found else {
        editor.set_error(format!("No indexed node with id {id}"));
        return;
    };

    if let Err(err) = editor.open(&path, helix_view::editor::Action::Replace) {
        editor.set_error(format!("could not open {}: {err}", path.display()));
        return;
    }

    let doc = doc!(editor);
    if line >= doc.text().len_lines() {
        editor.set_error("The node's line no longer exists; re-index the directory.");
        return;
    }
    let byte = doc.text().line_to_byte(line);
    jump_to_byte(editor, byte);
}

/// Opens a file link, landing where its `::` part asks.
fn follow_file(
    editor: &mut Editor,
    path: &Path,
    search: Option<helix_roam::hyperlink::FileSearch>,
) {
    if let Err(err) = editor.open(path, helix_view::editor::Action::Replace) {
        editor.set_error(format!("could not open {}: {err}", path.display()));
        return;
    }

    use helix_roam::hyperlink::FileSearch;
    let text = doc!(editor).text().to_string();
    let byte = match search {
        None => return,
        // Org writes line numbers one-based.
        Some(FileSearch::Line(line)) => {
            let doc = doc!(editor);
            let line = line.saturating_sub(1);
            if line >= doc.text().len_lines() {
                editor.set_error(format!("{} has no line {line}", path.display()));
                return;
            }
            doc.text().line_to_byte(line)
        }
        Some(FileSearch::Headline(title)) => match find_headline(&text, &title) {
            Some(byte) => byte,
            None => {
                editor.set_error(format!("no headline \"{title}\" in {}", path.display()));
                return;
            }
        },
        Some(FileSearch::Text(needle)) => match text.find(&needle) {
            Some(byte) => byte,
            None => {
                editor.set_error(format!("\"{needle}\" not found in {}", path.display()));
                return;
            }
        },
    };

    jump_to_byte(editor, byte);
}

/// Byte offset of a headline with this title, at any level.
fn find_headline(text: &str, title: &str) -> Option<usize> {
    let mut offset = 0;
    for line in text.lines() {
        let trimmed = line.trim_start_matches('*');
        if trimmed.len() != line.len() && trimmed.trim() == title {
            return Some(offset);
        }
        offset += line.len() + 1;
    }
    None
}

/// Jumps to the first occurrence of `needle` in the current buffer.
fn follow_in_buffer(editor: &mut Editor, text: &str, needle: &str, what: &str) {
    // A headline is matched on its own line rather than as loose text.
    let found = if what == "headline" {
        find_headline(text, needle.trim_start_matches("* "))
    } else {
        text.find(needle)
    };

    match found {
        Some(byte) => jump_to_byte(editor, byte),
        None => editor.set_error(format!("No {what} matching {needle}")),
    }
}

/// Hands a URL to the system, which is what following one means.
fn open_externally(editor: &mut Editor, url: &str) {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };

    match std::process::Command::new(program)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => editor.set_status(format!("Opened {url}")),
        Err(err) => editor.set_error(format!("could not run {program}: {err}")),
    }
}

/// Stores a link to the cursor's location, for inserting elsewhere.
///
/// Prefers the id of the node the cursor is in, since that is the link that
/// survives the file being renamed or the headline being reworded.
pub fn store_link(editor: &mut Editor) {
    let offset = cursor_offset(editor);
    let Some(path) = doc!(editor).path().map(Path::to_path_buf) else {
        editor.set_error("the buffer has no path to link to");
        return;
    };
    let text = doc!(editor).text().to_string();
    let line = doc!(editor).text().byte_to_line(offset);

    // Read from the buffer rather than looked up in the index: the buffer is
    // what the user is looking at, and it may carry an `:ID:` added since the
    // last indexing — or before the first one has finished.
    let node = helix_roam::restructure::entry_at(&text, line);

    let stored = match node {
        Some((id, title)) => helix_roam::hyperlink::format_link(&format!("id:{id}"), Some(&title)),
        None => {
            // No node here: a file link with the line, which at least lands.
            let title = text
                .lines()
                .nth(line)
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .unwrap_or("")
                .to_string();
            helix_roam::hyperlink::format_link(
                &format!("file:{}::{}", path.display(), line + 1),
                Some(&title),
            )
        }
    };

    match editor.registers.write(LINK_REGISTER, vec![stored.clone()]) {
        Ok(()) => editor.set_status(format!("Stored {stored}")),
        Err(err) => editor.set_error(err.to_string()),
    }
}

/// Inserts the stored link at the cursor.
pub fn insert_stored_link(editor: &mut Editor) {
    let Some(stored) = editor
        .registers
        .first(LINK_REGISTER, editor)
        .map(|value| value.into_owned())
    else {
        editor.set_error("No link stored yet");
        return;
    };

    let (view, doc) = current!(editor);
    let selection = doc.selection(view.id).clone();
    let transaction =
        helix_core::Transaction::change_by_selection(doc.text(), &selection, |range| {
            (range.head, range.head, Some(stored.as_str().into()))
        });
    doc.apply(&transaction, view.id);
}

/// Moves to the next Org link in the buffer.
pub fn goto_next_link(editor: &mut Editor) {
    move_to_link(editor, true);
}

/// Moves to the previous Org link in the buffer.
pub fn goto_previous_link(editor: &mut Editor) {
    move_to_link(editor, false);
}

fn move_to_link(editor: &mut Editor, forward: bool) {
    let offset = cursor_offset(editor);
    let (text, abbrevs) = {
        let doc = doc!(editor);
        (doc.text().to_string(), abbreviations(doc))
    };

    let found = if forward {
        helix_roam::hyperlink::next_link(&text, offset, &abbrevs)
    } else {
        helix_roam::hyperlink::previous_link(&text, offset, &abbrevs)
    };

    match found {
        Some(link) => {
            let view_id = view!(editor).id;
            let doc = doc_mut!(editor);
            let char_at = doc.text().byte_to_char(link.range.start);
            doc.set_selection(view_id, helix_core::Selection::point(char_at));
        }
        None => editor.set_status(if forward {
            "No further links"
        } else {
            "No earlier links"
        }),
    }
}

/// Gives the entry at the cursor an `:ID:`, so it can be linked to.
///
/// This is what `roam-node-insert` needs underneath it: without it a node can
/// only be created by typing a drawer by hand.
pub fn create_id(editor: &mut Editor) {
    let offset = cursor_offset(editor);
    let text = doc!(editor).text().to_string();
    let line = doc!(editor).text().byte_to_line(offset);

    match helix_roam::restructure::ensure_id(&text, line, helix_roam::Uuid::new_v4()) {
        Ok(helix_roam::restructure::IdOutcome::Existing(id)) => {
            editor.set_status(format!("Already has id {id}"));
        }
        Ok(helix_roam::restructure::IdOutcome::Created { text: after, id }) => {
            let before = doc!(editor).text().clone();
            let after = helix_core::Rope::from(after.as_str());
            let transaction = helix_core::diff::compare_ropes(&before, &after);
            let view = view!(editor).id;
            doc_mut!(editor).apply(&transaction, view);
            editor.set_status(format!("Created id {id}"));
        }
        Err(err) => editor.set_error(err.to_string()),
    }
}
