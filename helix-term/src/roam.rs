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

/// The directory new nodes are created in.
fn notes_directory(editor: &Editor) -> PathBuf {
    // The config's own resolver, which also expands a leading `~` — repeating
    // the fallback here would have quietly dropped that.
    editor.config().roam.directory()
}

/// Titles and aliases of every indexed node, for completion.
pub fn node_titles(editor: &Editor) -> Vec<String> {
    let mut titles: Vec<String> = {
        let graph = editor.roam.read();
        graph
            .nodes()
            .flat_map(|node| {
                std::iter::once(node.title.clone()).chain(node.aliases.iter().cloned())
            })
            .filter(|title| !title.is_empty())
            .collect()
    };

    titles.sort_unstable();
    titles.dedup();
    titles
}

/// Inserts a link to the node called `title`, creating it if there is none.
///
/// Creating on the spot is the point of the command: a note is written by
/// naming what it links to, and the target catching up later.
pub fn insert_node_link(editor: &mut Editor, title: &str) {
    let title = title.trim();
    if title.is_empty() {
        return;
    }

    let existing = {
        let graph = editor.roam.read();
        // Bound to a local so the iterator is dropped before the guard is.
        let found = graph
            .nodes()
            .find(|node| node.matches_title(title))
            .map(|node| node.id);
        found
    };

    let id = match existing {
        Some(id) => id,
        None => match create_node(editor, title) {
            Some(id) => id,
            None => return,
        },
    };

    let link = helix_roam::hyperlink::format_link(&format!("id:{id}"), Some(title));
    let (view, doc) = current!(editor);
    let selection = doc.selection(view.id).clone();
    let transaction =
        helix_core::Transaction::change_by_selection(doc.text(), &selection, |range| {
            (range.head, range.head, Some(link.as_str().into()))
        });
    doc.apply(&transaction, view.id);
}

/// Writes a new file-level node titled `title`, and indexes it.
fn create_node(editor: &mut Editor, title: &str) -> Option<helix_roam::Uuid> {
    let directory = notes_directory(editor);
    let id = helix_roam::Uuid::new_v4();
    let path = directory.join(format!("{}.org", slugify(title)));

    if path.exists() {
        editor.set_error(format!("{} already exists", path.display()));
        return None;
    }

    let contents = format!(":PROPERTIES:\n:ID:       {id}\n:END:\n#+title: {title}\n\n");
    if let Err(err) = std::fs::create_dir_all(&directory) {
        editor.set_error(format!("could not create {}: {err}", directory.display()));
        return None;
    }
    if let Err(err) = std::fs::write(&path, &contents) {
        editor.set_error(format!("could not write {}: {err}", path.display()));
        return None;
    }

    // Index it now rather than waiting for a save that may never come: the
    // link about to be inserted has to resolve immediately.
    helix_roam::reindex_file(&mut editor.roam.write(), &path, &contents);
    editor.set_status(format!("Created {}", path.display()));
    Some(id)
}

/// Adds or removes a value in the node-at-point's property drawer.
fn edit_node_property(editor: &mut Editor, property: &'static str, value: &str, add: bool) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }

    let offset = cursor_offset(editor);
    let text = doc!(editor).text().to_string();
    let line = doc!(editor).text().byte_to_line(offset);

    match helix_roam::restructure::edit_property(&text, line, property, value, add) {
        Ok(Some(after)) => {
            let before = doc!(editor).text().clone();
            let after = helix_core::Rope::from(after.as_str());
            let transaction = helix_core::diff::compare_ropes(&before, &after);
            let view = view!(editor).id;
            doc_mut!(editor).apply(&transaction, view);
            editor.set_status(if add {
                format!("Added {value}")
            } else {
                format!("Removed {value}")
            });
        }
        Ok(None) => editor.set_status(if add {
            format!("{value} is already there")
        } else {
            format!("{value} was not there")
        }),
        Err(err) => editor.set_error(err.to_string()),
    }
}

/// Adds or removes a tag on the node at the cursor.
fn edit_node_tag(editor: &mut Editor, tag: &str, add: bool) {
    let tag = tag.trim();
    if tag.is_empty() {
        return;
    }

    let offset = cursor_offset(editor);
    let text = doc!(editor).text().to_string();
    let line = doc!(editor).text().byte_to_line(offset);

    match helix_roam::restructure::edit_tag(&text, line, tag, add) {
        Ok(Some(after)) => {
            let before = doc!(editor).text().clone();
            let after = helix_core::Rope::from(after.as_str());
            let transaction = helix_core::diff::compare_ropes(&before, &after);
            let view = view!(editor).id;
            doc_mut!(editor).apply(&transaction, view);
            editor.set_status(if add {
                format!("Added :{tag}:")
            } else {
                format!("Removed :{tag}:")
            });
        }
        Ok(None) => editor.set_status(if add {
            format!(":{tag}: is already there")
        } else {
            format!(":{tag}: was not there")
        }),
        Err(err) => editor.set_error(err.to_string()),
    }
}

/// Adds a tag to the node at the cursor.
pub fn tag_add(editor: &mut Editor, tag: &str) {
    edit_node_tag(editor, tag, true);
}

/// Removes a tag from the node at the cursor.
pub fn tag_remove(editor: &mut Editor, tag: &str) {
    edit_node_tag(editor, tag, false);
}

/// Adds an alias to the node at the cursor.
pub fn alias_add(editor: &mut Editor, alias: &str) {
    edit_node_property(editor, "ROAM_ALIASES", alias, true);
}

/// Removes an alias from the node at the cursor.
pub fn alias_remove(editor: &mut Editor, alias: &str) {
    edit_node_property(editor, "ROAM_ALIASES", alias, false);
}

/// Adds a ref to the node at the cursor.
pub fn ref_add(editor: &mut Editor, reference: &str) {
    edit_node_property(editor, "ROAM_REFS", reference, true);
}

/// Removes a ref from the node at the cursor.
pub fn ref_remove(editor: &mut Editor, reference: &str) {
    edit_node_property(editor, "ROAM_REFS", reference, false);
}

/// Opens a node chosen at random.
///
/// The way a large set of notes gets revisited rather than only added to.
pub fn random_node(editor: &mut Editor) {
    let candidates: Vec<(PathBuf, usize)> = {
        let graph = editor.roam.read();
        graph
            .nodes()
            .map(|node| (node.file_path.clone(), node.line))
            .collect()
    };

    if candidates.is_empty() {
        editor.set_status("No Org-Roam nodes indexed.");
        return;
    }

    // Nanoseconds are enough randomness for picking a note to reread, and
    // avoid a dependency for it.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.subsec_nanos() as usize)
        .unwrap_or(0);
    let (path, line) = &candidates[nanos % candidates.len()];

    if let Err(err) = editor.open(path, helix_view::editor::Action::Replace) {
        editor.set_error(format!("could not open {}: {err}", path.display()));
        return;
    }
    let byte = {
        let doc = doc!(editor);
        doc.text()
            .line_to_byte((*line).min(doc.text().len_lines() - 1))
    };
    jump_to_byte(editor, byte);
}

/// Opens the daily note for `date`, creating it if there is none.
pub fn open_daily(editor: &mut Editor, date: helix_roam::Date) {
    let directory = editor.config().roam.dailies_directory();
    let path = directory.join(format!("{}.org", date.to_iso()));

    if !path.exists() {
        let id = helix_roam::Uuid::new_v4();
        let contents = format!(
            ":PROPERTIES:\n:ID:       {id}\n:END:\n#+title: {}\n\n",
            date.to_iso()
        );

        if let Err(err) = std::fs::create_dir_all(&directory) {
            editor.set_error(format!("could not create {}: {err}", directory.display()));
            return;
        }
        if let Err(err) = std::fs::write(&path, &contents) {
            editor.set_error(format!("could not write {}: {err}", path.display()));
            return;
        }
        // Indexed at once, so a link to today resolves before any save.
        helix_roam::reindex_file(&mut editor.roam.write(), &path, &contents);
    }

    if let Err(err) = editor.open(&path, helix_view::editor::Action::Replace) {
        editor.set_error(format!("could not open {}: {err}", path.display()));
    }
}

/// Today's daily note.
pub fn daily_today(editor: &mut Editor) {
    open_daily(editor, helix_roam::Date::today());
}

/// The daily note for a typed date.
pub fn daily_on(editor: &mut Editor, text: &str) {
    match helix_roam::Date::parse_iso(text) {
        Some(date) => open_daily(editor, date),
        None => editor.set_error(format!("{text:?} is not a date like 2026-09-18")),
    }
}

/// The daily note nearest the current one, in `direction`.
///
/// Moves between notes that *exist* rather than stepping one day at a time,
/// since most days have no note and stepping would land on empty ones.
pub fn daily_step(editor: &mut Editor, forward: bool) {
    let directory = editor.config().roam.dailies_directory();

    let mut dates: Vec<helix_roam::Date> = match std::fs::read_dir(&directory) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name();
                let name = name.to_str()?.strip_suffix(".org")?;
                helix_roam::Date::parse_iso(name)
            })
            .collect(),
        Err(_) => Vec::new(),
    };

    if dates.is_empty() {
        editor.set_status("No daily notes yet");
        return;
    }
    dates.sort_unstable();

    // Where we are now: the open note's date, or today when elsewhere.
    let current = doc!(editor)
        .path()
        .and_then(|path| path.file_stem()?.to_str().map(str::to_string))
        .and_then(|stem| helix_roam::Date::parse_iso(&stem))
        .unwrap_or_else(helix_roam::Date::today);

    let found = if forward {
        dates.into_iter().find(|date| *date > current)
    } else {
        dates.into_iter().filter(|date| *date < current).next_back()
    };

    match found {
        Some(date) => open_daily(editor, date),
        None => editor.set_status(if forward {
            "No later daily note"
        } else {
            "No earlier daily note"
        }),
    }
}

/// Opens the dailies directory itself, for browsing.
pub fn daily_directory(editor: &mut Editor) -> PathBuf {
    editor.config().roam.dailies_directory()
}

/// Renames the node at the cursor, and the links that named it.
///
/// Links keep resolving regardless — they carry the id — but a description
/// repeating the old title would become a lie, so those are updated too.
pub fn rename_node(editor: &mut Editor, new_title: &str) {
    let new_title = new_title.trim();
    if new_title.is_empty() {
        return;
    }

    let offset = cursor_offset(editor);
    let text = doc!(editor).text().to_string();
    let line = doc!(editor).text().byte_to_line(offset);

    let Some((id, old_title)) = helix_roam::restructure::entry_at(&text, line) else {
        editor.set_error("No node at the cursor; give it an :ID: first");
        return;
    };

    let renamed = match helix_roam::restructure::rename_entry(&text, line, new_title) {
        Ok(renamed) => renamed,
        Err(err) => {
            editor.set_error(err.to_string());
            return;
        }
    };

    let before = doc!(editor).text().clone();
    let after = helix_core::Rope::from(renamed.as_str());
    let transaction = helix_core::diff::compare_ropes(&before, &after);
    let view = view!(editor).id;
    doc_mut!(editor).apply(&transaction, view);

    let updated = retitle_backlinks(editor, id, &old_title, new_title);
    editor.set_status(match updated {
        0 => format!("Renamed to \"{new_title}\""),
        1 => format!("Renamed to \"{new_title}\", and 1 link"),
        n => format!("Renamed to \"{new_title}\", and {n} links"),
    });
}

/// Rewrites the descriptions of links into `id` across the notes directory.
///
/// The graph's backlinks would be the cheaper source, but they only record
/// links whose *containing* file is itself a node: a file with no `:ID:` links
/// out without being an edge, and its descriptions would have been left
/// stale. Renaming is rare enough to afford one pass over the notes.
fn retitle_backlinks(editor: &mut Editor, id: helix_roam::Uuid, old: &str, new: &str) -> usize {
    let open_path = doc!(editor).path().map(Path::to_path_buf);
    let (sources, _) = helix_roam::scanner::collect_org_files(&notes_directory(editor));

    let mut updated = 0;
    for path in sources {
        // The open buffer is not written behind the editor's back.
        if open_path.as_deref() == Some(path.as_path()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(rewritten) = helix_roam::restructure::retitle_links(&text, id, old, new) else {
            continue;
        };
        if std::fs::write(&path, &rewritten).is_ok() {
            helix_roam::reindex_file(&mut editor.roam.write(), &path, &rewritten);
            updated += 1;
        }
    }

    updated
}

/// One place a node is named without being linked.
pub struct Unlinked {
    pub text: String,
    pub path: PathBuf,
    pub line: usize,
}

/// Every unlinked reference to the node at the cursor.
///
/// Reads the notes from disk rather than the graph: the graph holds nodes and
/// edges, and this question is about the prose between them.
pub fn unlinked_references(editor: &mut Editor) -> Vec<Unlinked> {
    let offset = cursor_offset(editor);
    let text = doc!(editor).text().to_string();
    let line = doc!(editor).text().byte_to_line(offset);

    let Some((id, title)) = helix_roam::restructure::entry_at(&text, line) else {
        editor.set_error("No node at the cursor; give it an :ID: first");
        return Vec::new();
    };

    // The node's own names: its title and any aliases it declares.
    let mut names = vec![title];
    {
        let graph = editor.roam.read();
        if let Some(node) = graph.get_node(&id) {
            names.extend(node.aliases.iter().cloned());
        }
    }

    let (files, _) = helix_roam::scanner::collect_org_files(&notes_directory(editor));
    let own_file = doc!(editor).path().map(Path::to_path_buf);

    let mut found = Vec::new();
    for path in files {
        // A node naming itself is not a reference worth making.
        if own_file.as_deref() == Some(path.as_path()) {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        found.extend(
            helix_roam::unlinked::find_in_text(&contents, &path, &names)
                .into_iter()
                .map(|reference| Unlinked {
                    text: reference.text.trim().to_string(),
                    path: reference.path,
                    line: reference.line,
                }),
        );
    }

    if found.is_empty() {
        editor.set_status("No unlinked references");
    }
    found
}

/// The templates a captured node can use.
///
/// Falls back to the built-in one, so capturing works before anything is
/// configured rather than reporting an empty list.
pub fn capture_templates(editor: &Editor) -> Vec<helix_roam::capture::Template> {
    let configured = &editor.config().roam.templates;
    if configured.is_empty() {
        return vec![helix_roam::capture::default_template()];
    }

    configured
        .iter()
        .map(|template| helix_roam::capture::Template {
            key: template.key.clone(),
            description: template.description.clone(),
            file: template.file.clone(),
            content: template.content.clone(),
        })
        .collect()
}

/// Creates a node from `template`, opens it, and leaves the cursor at `%?`.
pub fn capture_node(editor: &mut Editor, template: &helix_roam::capture::Template, title: &str) {
    let title = title.trim();
    if title.is_empty() {
        return;
    }

    let fields = helix_roam::capture::Fields {
        title: title.to_string(),
        slug: helix_roam::capture::slugify(title),
        id: helix_roam::Uuid::new_v4().to_string(),
        date: helix_roam::Date::today().to_iso(),
    };

    let capture = match helix_roam::capture::expand(template, &fields) {
        Ok(capture) => capture,
        Err(err) => {
            editor.set_error(err.to_string());
            return;
        }
    };

    let path = notes_directory(editor).join(&capture.path);
    if path.exists() {
        editor.set_error(format!("{} already exists", path.display()));
        return;
    }
    // A template may name a subdirectory, which need not exist yet.
    if let Some(parent) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            editor.set_error(format!("could not create {}: {err}", parent.display()));
            return;
        }
    }
    if let Err(err) = std::fs::write(&path, &capture.content) {
        editor.set_error(format!("could not write {}: {err}", path.display()));
        return;
    }

    helix_roam::reindex_file(&mut editor.roam.write(), &path, &capture.content);

    if let Err(err) = editor.open(&path, helix_view::editor::Action::Replace) {
        editor.set_error(format!("could not open {}: {err}", path.display()));
        return;
    }

    if let Some(byte) = capture.cursor {
        let view_id = view!(editor).id;
        let doc = doc_mut!(editor);
        let char_at = doc.text().byte_to_char(byte.min(doc.text().len_bytes()));
        doc.set_selection(view_id, helix_core::Selection::point(char_at));
    }

    editor.set_status(format!("Captured {}", path.display()));
}

/// Applies a text transformation to the focused buffer, reporting the outcome.
fn apply_to_buffer(editor: &mut Editor, done: String, after: String) {
    let before = doc!(editor).text().clone();
    let after = helix_core::Rope::from(after.as_str());
    if after == before {
        editor.set_status("Nothing to change");
        return;
    }

    let transaction = helix_core::diff::compare_ropes(&before, &after);
    let view = view!(editor).id;
    doc_mut!(editor).apply(&transaction, view);
    editor.set_status(done);
}

/// The buffer's text and the cursor's line, which every command here needs.
fn text_and_line(editor: &Editor) -> (String, usize) {
    let offset = cursor_offset(editor);
    let doc = doc!(editor);
    (doc.text().to_string(), doc.text().byte_to_line(offset))
}

/// Property keys the file already uses, for completion.
pub fn property_keys(editor: &Editor) -> Vec<String> {
    helix_roam::restructure::property_keys(&doc!(editor).text().to_string())
}

/// Sets a property on the entry at the cursor, from `KEY VALUE`.
pub fn set_property(editor: &mut Editor, input: &str) {
    let Some((key, value)) = input.trim().split_once(char::is_whitespace) else {
        editor.set_error("Give a key and a value, e.g. `CATEGORY work`");
        return;
    };

    let (text, line) = text_and_line(editor);
    match helix_roam::restructure::set_property(&text, line, key.trim(), value.trim()) {
        Ok(after) => apply_to_buffer(editor, format!("Set :{}:", key.trim()), after),
        Err(err) => editor.set_error(err.to_string()),
    }
}

/// Removes a property from the entry at the cursor.
pub fn remove_property(editor: &mut Editor, key: &str) {
    let key = key.trim();
    let (text, line) = text_and_line(editor);

    match helix_roam::restructure::remove_property(&text, line, key) {
        Ok(Some(after)) => apply_to_buffer(editor, format!("Removed :{key}:"), after),
        Ok(None) => editor.set_status(format!(":{key}: was not there")),
        Err(err) => editor.set_error(err.to_string()),
    }
}

/// Sets the effort estimate on the entry at the cursor.
pub fn set_effort(editor: &mut Editor, value: &str) {
    let (text, line) = text_and_line(editor);
    match helix_roam::restructure::set_property(&text, line, "EFFORT", value.trim()) {
        Ok(after) => apply_to_buffer(editor, format!("Effort {}", value.trim()), after),
        Err(err) => editor.set_error(err.to_string()),
    }
}

/// The effort values `roam-inc-effort` steps through.
///
/// Org takes these from `Effort_ALL`, so a file that declares one is honoured
/// and the list below is only the fallback.
const DEFAULT_EFFORTS: [&str; 9] = [
    "0:10", "0:20", "0:30", "1:00", "2:00", "3:00", "4:00", "6:00", "8:00",
];

/// Moves the effort estimate to the next value in the file's list.
pub fn increment_effort(editor: &mut Editor) {
    let (text, line) = text_and_line(editor);

    let settings = helix_roam::FileSettings::scan(&text);
    let declared: Vec<String> = settings
        .properties
        .iter()
        .find(|(key, _)| key == "effort_all")
        .map(|(_, value)| value.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    let values: Vec<&str> = if declared.is_empty() {
        DEFAULT_EFFORTS.to_vec()
    } else {
        declared.iter().map(String::as_str).collect()
    };

    let current = helix_roam::restructure::property_value(&text, line, "EFFORT");
    let next = match current
        .as_deref()
        .and_then(|c| values.iter().position(|v| *v == c))
    {
        // Past the end, wrap round rather than sticking at the largest.
        Some(at) => values[(at + 1) % values.len()],
        None => values[0],
    };

    match helix_roam::restructure::set_property(&text, line, "EFFORT", next) {
        Ok(after) => apply_to_buffer(editor, format!("Effort {next}"), after),
        Err(err) => editor.set_error(err.to_string()),
    }
}

/// Inserts an empty drawer under the entry at the cursor.
pub fn insert_drawer(editor: &mut Editor, name: &str) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }

    let (text, line) = text_and_line(editor);
    let after = helix_roam::restructure::insert_drawer(&text, line, name);
    apply_to_buffer(
        editor,
        format!("Inserted :{}: drawer", name.to_uppercase()),
        after,
    );
}

/// Records a dated note in the entry's `:LOGBOOK:`.
pub fn add_note(editor: &mut Editor, note: &str) {
    let note = note.trim();
    if note.is_empty() {
        return;
    }

    let stamp =
        helix_roam::date::log_stamp(helix_roam::Date::today(), helix_roam::date::Time::now());
    let (text, line) = text_and_line(editor);
    let after = helix_roam::restructure::log_entry(
        &text,
        line,
        &format!("- Note taken on {stamp} \\\\\n  {note}"),
    );

    apply_to_buffer(editor, "Noted".to_string(), after);
}

/// Records a TODO state change in the entry's `:LOGBOOK:`.
///
/// The state itself is not changed here: that is Task 1.5's business, and this
/// records what happened rather than causing it.
pub fn log_state_change(editor: &mut Editor, input: &str) {
    let (from, to) = match input.trim().split_once("->") {
        Some((from, to)) => (from.trim(), to.trim()),
        None => ("", input.trim()),
    };
    if to.is_empty() {
        editor.set_error("Give the new state, e.g. `TODO -> DONE`");
        return;
    }

    let stamp =
        helix_roam::date::log_stamp(helix_roam::Date::today(), helix_roam::date::Time::now());
    let entry = if from.is_empty() {
        format!("- State \"{to}\" from {stamp}")
    } else {
        format!("- State \"{to}\" from \"{from}\" {stamp}")
    };

    let (text, line) = text_and_line(editor);
    let after = helix_roam::restructure::log_entry(&text, line, &entry);
    apply_to_buffer(editor, format!("Logged {to}"), after);
}

/// The file's own settings, which decide what a keyword or a cookie is.
fn file_settings(editor: &Editor) -> helix_roam::FileSettings {
    helix_roam::FileSettings::scan(&doc!(editor).text().to_string())
}

/// Applies a transformation and moves the cursor to `line`.
fn apply_and_go(editor: &mut Editor, done: String, after: String, line: usize) {
    let before = doc!(editor).text().clone();
    let after = helix_core::Rope::from(after.as_str());
    let transaction = helix_core::diff::compare_ropes(&before, &after);
    let view = view!(editor).id;
    doc_mut!(editor).apply(&transaction, view);

    let doc = doc_mut!(editor);
    let line = line.min(doc.text().len_lines().saturating_sub(1));
    let at = doc.text().line_to_char(line);
    doc.set_selection(view, helix_core::Selection::point(at));
    editor.set_status(done);
}

/// Runs a structure transformation that can fail.
fn structure(
    editor: &mut Editor,
    done: &'static str,
    transform: impl FnOnce(&str, usize) -> Result<String, helix_roam::restructure::Error>,
) {
    let (text, line) = text_and_line(editor);
    match transform(&text, line) {
        Ok(after) => apply_to_buffer(editor, done.to_string(), after),
        Err(err) => editor.set_error(err.to_string()),
    }
}

/// Inserts a sibling headline after the current subtree.
pub fn insert_heading(editor: &mut Editor) {
    let (text, line) = text_and_line(editor);
    let (after, at) = helix_roam::restructure::insert_heading(&text, line);
    apply_and_go(editor, "Inserted a heading".to_string(), after, at);
}

/// Moves one headline out a level, leaving its children.
pub fn promote_heading(editor: &mut Editor) {
    structure(editor, "Promoted", |text, line| {
        helix_roam::restructure::shift_heading(text, line, false)
    });
}

/// Moves one headline in a level, leaving its children.
pub fn demote_heading(editor: &mut Editor) {
    structure(editor, "Demoted", |text, line| {
        helix_roam::restructure::shift_heading(text, line, true)
    });
}

/// Moves a headline and its children out a level.
pub fn promote_subtree(editor: &mut Editor) {
    structure(editor, "Promoted the subtree", |text, line| {
        helix_roam::restructure::shift_subtree(text, line, false)
    });
}

/// Moves a headline and its children in a level.
pub fn demote_subtree(editor: &mut Editor) {
    structure(editor, "Demoted the subtree", |text, line| {
        helix_roam::restructure::shift_subtree(text, line, true)
    });
}

/// Swaps the subtree with the sibling above or below it.
fn move_subtree(editor: &mut Editor, up: bool) {
    let (text, line) = text_and_line(editor);
    match helix_roam::restructure::move_subtree(&text, line, up) {
        Ok((after, at)) => apply_and_go(editor, "Moved the subtree".to_string(), after, at),
        Err(err) => editor.set_error(err.to_string()),
    }
}

pub fn move_subtree_up(editor: &mut Editor) {
    move_subtree(editor, true);
}

pub fn move_subtree_down(editor: &mut Editor) {
    move_subtree(editor, false);
}

/// Moves the headline to the next state its file declares.
fn cycle_todo(editor: &mut Editor, forward: bool) {
    let settings = file_settings(editor);
    let (text, line) = text_and_line(editor);

    match helix_roam::restructure::cycle_todo(&text, line, &settings, forward) {
        Ok(after) => apply_to_buffer(editor, "Cycled the state".to_string(), after),
        Err(err) => editor.set_error(err.to_string()),
    }
}

pub fn todo_next(editor: &mut Editor) {
    cycle_todo(editor, true);
}

pub fn todo_previous(editor: &mut Editor) {
    cycle_todo(editor, false);
}

/// Moves the priority towards `A`, or away from it.
fn change_priority(editor: &mut Editor, raise: bool) {
    let settings = file_settings(editor);
    let (text, line) = text_and_line(editor);

    match helix_roam::restructure::change_priority(&text, line, &settings, raise) {
        Ok(after) => apply_to_buffer(editor, "Changed the priority".to_string(), after),
        Err(err) => editor.set_error(err.to_string()),
    }
}

pub fn priority_up(editor: &mut Editor) {
    change_priority(editor, true);
}

pub fn priority_down(editor: &mut Editor) {
    change_priority(editor, false);
}

/// Sets the priority from a typed letter, or clears it when nothing is typed.
pub fn set_priority(editor: &mut Editor, input: &str) {
    let settings = file_settings(editor);
    let letter = input.trim().chars().next().map(|c| c.to_ascii_uppercase());
    let (text, line) = text_and_line(editor);

    match helix_roam::restructure::set_priority(&text, line, &settings, letter) {
        Ok(after) => apply_to_buffer(
            editor,
            match letter {
                Some(letter) => format!("Priority [#{letter}]"),
                None => "Cleared the priority".to_string(),
            },
            after,
        ),
        Err(err) => editor.set_error(err.to_string()),
    }
}

/// Writes a planning timestamp, reading `today`, `+3` or an ISO date.
fn set_planning(editor: &mut Editor, which: helix_roam::restructure::Planning, input: &str) {
    let input = input.trim();
    let stamp = if input.is_empty() {
        None
    } else {
        match parse_date_input(input) {
            Some(date) => Some(format!("<{} {}>", date.to_iso(), date.weekday())),
            None => {
                editor.set_error(format!(
                    "{input:?} is not a date; try `today`, `+3`, or `2026-09-18`"
                ));
                return;
            }
        }
    };

    let (text, line) = text_and_line(editor);
    match helix_roam::restructure::set_planning(&text, line, which, stamp.as_deref()) {
        Ok(after) => apply_to_buffer(
            editor,
            match &stamp {
                Some(stamp) => format!("Set {stamp}"),
                None => "Cleared it".to_string(),
            },
            after,
        ),
        Err(err) => editor.set_error(err.to_string()),
    }
}

/// Reads the shorthands a date prompt accepts.
///
/// Typing a full ISO date every time is the thing the roadmap asked to avoid;
/// `today`, `tomorrow` and `+n` cover most of what a planning line gets.
fn parse_date_input(input: &str) -> Option<helix_roam::Date> {
    let today = helix_roam::Date::today();

    match input.to_lowercase().as_str() {
        "today" | "." => return Some(today),
        "tomorrow" | "+1" => return Some(today.offset_by(1)),
        "yesterday" | "-1" => return Some(today.offset_by(-1)),
        _ => {}
    }

    if let Some(days) = input.strip_prefix('+').and_then(|n| n.parse::<i64>().ok()) {
        return Some(today.offset_by(days));
    }
    if let Some(days) = input.strip_prefix('-').and_then(|n| n.parse::<i64>().ok()) {
        return Some(today.offset_by(-days));
    }

    helix_roam::Date::parse_iso(input)
}

pub fn schedule(editor: &mut Editor, input: &str) {
    set_planning(editor, helix_roam::restructure::Planning::Scheduled, input);
}

pub fn deadline(editor: &mut Editor, input: &str) {
    set_planning(editor, helix_roam::restructure::Planning::Deadline, input);
}

/// Tags used anywhere in the graph, for completing a tag prompt.
pub fn known_tags(editor: &Editor) -> Vec<String> {
    let mut tags: Vec<String> = {
        let graph = editor.roam.read();
        let collected = graph
            .nodes()
            .flat_map(|node| node.tags.iter().cloned())
            .collect();
        collected
    };

    tags.sort_unstable();
    tags.dedup();
    tags
}

/// Moves the subtree at the cursor into the file's archive.
///
/// The archive is appended to, never overwritten, and the source buffer is
/// only changed once the archive has been written — so a failure loses
/// nothing.
pub fn archive_subtree(editor: &mut Editor) {
    let Some(source) = doc!(editor).path().map(Path::to_path_buf) else {
        editor.set_error("the buffer has no path to archive from");
        return;
    };
    let settings = file_settings(editor);
    let (text, line) = text_and_line(editor);
    let origin = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    let archived = match helix_roam::restructure::archive_subtree(&text, line, &settings, &origin) {
        Ok(archived) => archived,
        Err(err) => {
            editor.set_error(err.to_string());
            return;
        }
    };

    let target = source
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(&archived.target);

    let appended = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&target)
        .and_then(|mut file| {
            use std::io::Write;
            file.write_all(archived.archived.as_bytes())
        });

    if let Err(err) = appended {
        editor.set_error(format!("could not write {}: {err}", target.display()));
        return;
    }

    apply_to_buffer(
        editor,
        format!("Archived to {}", target.display()),
        archived.remaining,
    );
}

/// One line of an agenda, flattened for the picker.
pub struct AgendaLine {
    pub when: String,
    pub what: String,
    pub path: PathBuf,
    pub line: usize,
}

/// Whether a node's file is one the agenda should read.
///
/// Two filters, in order: the session's restriction, then the configured
/// `agenda-files`. A configuration naming nothing means no restriction, which
/// is not the same as naming files that are all missing.
fn in_agenda_scope(editor: &Editor, path: &Path) -> bool {
    if let Some(restriction) = &editor.agenda_restriction {
        return path == restriction;
    }

    let configured = editor.config().roam.agenda_files();
    configured.is_empty() || configured.iter().any(|allowed| allowed == path)
}

/// Builds the agenda for `days` days from today.
///
/// Snapshots what it needs from the graph rather than holding its lock, like
/// the other pickers, so indexing stays free while it is open.
pub fn agenda_lines(editor: &Editor, days: i64) -> Vec<AgendaLine> {
    let today = helix_roam::Date::today();
    let graph = editor.roam.read();
    let nodes: Vec<&helix_roam::Node> = graph
        .nodes()
        .filter(|node| in_agenda_scope(editor, &node.file_path))
        .collect();

    helix_roam::agenda::agenda(nodes, today, days)
        .into_iter()
        .map(|entry| {
            let marker = match entry.reason {
                helix_roam::agenda::Reason::Deadline => match entry.days_left {
                    Some(days) if days < 0 => format!("{} d. ago", -days),
                    Some(0) => "today".to_string(),
                    Some(days) => format!("in {days} d."),
                    None => "deadline".to_string(),
                },
                helix_roam::agenda::Reason::Scheduled => "scheduled".to_string(),
            };
            let kind = match entry.reason {
                helix_roam::agenda::Reason::Deadline => "Deadline",
                helix_roam::agenda::Reason::Scheduled => "Scheduled",
            };

            AgendaLine {
                when: format!(
                    "{} {}  {kind}: {marker}",
                    entry.day.to_iso(),
                    entry.day.weekday()
                ),
                what: agenda_title(entry.node),
                path: entry.node.file_path.clone(),
                line: entry.node.line,
            }
        })
        .collect()
}

/// Everything unfinished, whether or not it has a date.
pub fn todo_lines(editor: &Editor) -> Vec<AgendaLine> {
    let graph = editor.roam.read();
    let nodes: Vec<&helix_roam::Node> = graph
        .nodes()
        .filter(|node| in_agenda_scope(editor, &node.file_path))
        .collect();

    helix_roam::agenda::todo_list(nodes)
        .into_iter()
        .map(|node| AgendaLine {
            when: node
                .todo
                .as_ref()
                .map(|state| state.keyword.clone())
                .unwrap_or_default(),
            what: agenda_title(node),
            path: node.file_path.clone(),
            line: node.line,
        })
        .collect()
}

/// A node's title as an agenda shows it: priority, title, then tags.
fn agenda_title(node: &helix_roam::Node) -> String {
    let mut out = String::new();
    if let Some(priority) = node.priority {
        out.push_str(&format!("[#{priority}] "));
    }
    out.push_str(&node.title);
    if !node.tags.is_empty() {
        out.push_str(&format!("  :{}:", node.tags.join(":")));
    }
    out
}

/// Narrows the agenda to the file in the focused buffer.
pub fn agenda_restrict_to_file(editor: &mut Editor) {
    let Some(path) = doc!(editor).path().map(Path::to_path_buf) else {
        editor.set_error("the buffer has no path to restrict to");
        return;
    };

    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    editor.agenda_restriction = Some(path);
    editor.set_status(format!("Agenda restricted to {name}"));
}

/// Widens the agenda back to every file it may read.
pub fn agenda_restrict_clear(editor: &mut Editor) {
    match editor.agenda_restriction.take() {
        Some(_) => editor.set_status("Agenda restriction lifted"),
        None => editor.set_status("The agenda was not restricted"),
    }
}

/// Reports which files the agenda is reading, and why.
pub fn agenda_scope(editor: &Editor) -> String {
    if let Some(restriction) = &editor.agenda_restriction {
        return format!("restricted to {}", restriction.display());
    }

    let configured = editor.config().roam.agenda_files();
    if configured.is_empty() {
        "every indexed file".to_string()
    } else {
        format!("{} configured file(s)", configured.len())
    }
}
