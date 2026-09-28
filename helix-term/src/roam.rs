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
    if !config.roam.enable {
        return;
    }

    let graph = editor.roam.clone();
    let path = path.to_path_buf();
    let is_org = is_org_file(&path);

    tokio::spawn(async move {
        if is_org {
            if let Err(err) = scanner::reindex_file_async(graph.clone(), path.clone(), text).await {
                log::error!("failed to re-index {}: {err}", path.display());
            }
        }
        // Any file can be a setup file — `.setup` is a common name for one —
        // so this is asked of every save, not only of Org files.
        match scanner::reindex_setup_dependents(graph, path.clone()).await {
            Ok(0) => {}
            Ok(count) => log::info!(
                "{} changed: re-indexed {count} files reading it",
                path.display()
            ),
            Err(err) => log::error!("failed to re-index what reads {}: {err}", path.display()),
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
#[derive(Debug, Clone)]
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
    settings_of(doc).link_abbreviations
}

/// A document's settings, its `#+SETUPFILE:`s included when it has a path
/// to resolve them against.
fn settings_of(doc: &helix_view::Document) -> helix_roam::FileSettings {
    let text = doc.text().to_string();
    match doc.path() {
        Some(path) => helix_roam::FileSettings::scan_at(&text, path),
        None => helix_roam::FileSettings::scan(&text),
    }
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
        helix_roam::LinkKind::Other { scheme, rest } if scheme == "attachment" => {
            follow_attachment(editor, &text, offset, &rest);
        }
        helix_roam::LinkKind::Other { scheme, .. } => {
            editor.set_error(format!("Links of type `{scheme}:` are not handled"));
        }
    }
}

/// Opens the file a node lives in, at the node.
fn follow_node(editor: &mut Editor, id: helix_roam::Uuid) {
    let found = {
        let graph = editor.roam.read();
        graph
            .get_node(&id)
            .map(|node| (node.file_path.clone(), node.line))
            // The index only knows the notes directory. A link into a file
            // opened from elsewhere is still followable, because opening it
            // left the id's location behind.
            .or_else(|| graph.location(&id).map(|path| (path.to_path_buf(), 0)))
    };

    let Some((path, line)) = found else {
        editor.set_error(format!(
            "No node with id {id} in the index, and no file opened this session declares it"
        ));
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
    let stored = match link_here(editor) {
        Ok(link) => link,
        Err(err) => return editor.set_error(err),
    };
    match editor.registers.write(LINK_REGISTER, vec![stored.clone()]) {
        Ok(()) => editor.set_status(format!("Stored {stored}")),
        Err(err) => editor.set_error(err.to_string()),
    }
}

/// A link to the cursor's location: the id of the node it is in when it
/// has one, else the file and line.
fn link_here(editor: &Editor) -> Result<String, &'static str> {
    let offset = cursor_offset(editor);
    let Some(path) = doc!(editor).path().map(Path::to_path_buf) else {
        return Err("the buffer has no path to link to");
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
            // A headline is described by its words, not its stars.
            let title = text
                .lines()
                .nth(line)
                .map(|l| l.trim().trim_start_matches('*').trim())
                .filter(|l| !l.is_empty())
                .unwrap_or("")
                .to_string();
            helix_roam::hyperlink::format_link(
                &format!("file:{}::{}", path.display(), line + 1),
                Some(&title),
            )
        }
    };
    Ok(stored)
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

/// The daily note for `date`, created if there is none yet.
fn ensure_daily(editor: &mut Editor, date: helix_roam::Date) -> Result<PathBuf, String> {
    let directory = editor.config().roam.dailies_directory();
    let path = directory.join(format!("{}.org", date.to_iso()));
    if path.exists() {
        return Ok(path);
    }

    let id = helix_roam::Uuid::new_v4();
    let contents = format!(
        ":PROPERTIES:\n:ID:       {id}\n:END:\n#+title: {}\n\n",
        date.to_iso()
    );
    std::fs::create_dir_all(&directory)
        .map_err(|err| format!("could not create {}: {err}", directory.display()))?;
    std::fs::write(&path, &contents)
        .map_err(|err| format!("could not write {}: {err}", path.display()))?;
    // Indexed at once, so a link to today resolves before any save.
    helix_roam::reindex_file(&mut editor.roam.write(), &path, &contents);
    Ok(path)
}

/// Opens the daily note for `date`, creating it if there is none.
pub fn open_daily(editor: &mut Editor, date: helix_roam::Date) {
    let path = match ensure_daily(editor, date) {
        Ok(path) => path,
        Err(err) => {
            editor.set_error(err);
            return;
        }
    };
    if let Err(err) = editor.open(&path, helix_view::editor::Action::Replace) {
        editor.set_error(format!("could not open {}: {err}", path.display()));
    }
}

/// Adds an entry to today's note without leaving the current buffer.
///
/// Org-Roam's `roam-dailies-capture-today`, with its default template,
/// `* %?`: the entry is a headline. If today's note is open, its buffer
/// gets the entry, unsaved like any other change; if not, the file does.
pub fn daily_capture(editor: &mut Editor, entry: &str) {
    let entry = entry.trim();
    if entry.is_empty() {
        return;
    }
    let today = helix_roam::Date::today();
    let path = match ensure_daily(editor, today) {
        Ok(path) => path,
        Err(err) => {
            editor.set_error(err);
            return;
        }
    };
    let append = |text: &str| {
        let mut out = text.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("* {entry}\n"));
        out
    };

    let result = match editor
        .document_by_path(&path)
        .map(|doc| (doc.id(), doc.text().to_string()))
    {
        Some((id, text)) => apply_to_document(editor, id, &append(&text)).map_err(str::to_string),
        None => std::fs::read_to_string(&path)
            .map_err(|err| err.to_string())
            .and_then(|text| {
                let after = append(&text);
                std::fs::write(&path, &after).map_err(|err| err.to_string())?;
                helix_roam::reindex_file(&mut editor.roam.write(), &path, &after);
                Ok(())
            }),
    };
    match result {
        Ok(()) => editor.set_status(format!("Added to {}", today.to_iso())),
        Err(err) => editor.set_error(format!("Not added: {err}")),
    }
}

/// A file picker over the dailies directory.
pub fn dailies_picker(editor: &mut Editor) -> Option<Box<dyn crate::compositor::Component>> {
    let directory = editor.config().roam.dailies_directory();
    if !directory.is_dir() {
        editor.set_error(format!("No daily notes yet in {}", directory.display()));
        return None;
    }
    Some(Box::new(crate::ui::overlay::overlaid(
        crate::ui::file_picker(editor, directory),
    )))
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

/// `org-capture`'s templates, or the built-in one: a task filed into
/// `inbox.org`, stamped and linked to where it was captured, as Org's own
/// default template is.
pub fn org_capture_templates(editor: &Editor) -> Vec<helix_view::editor::OrgCaptureTemplate> {
    let configured = &editor.config().roam.capture;
    if !configured.is_empty() {
        return configured.clone();
    }
    vec![helix_view::editor::OrgCaptureTemplate {
        key: "t".into(),
        description: "Task".into(),
        kind: helix_view::editor::OrgCaptureKind::Entry,
        file: "inbox.org".into(),
        id: None,
        regexp: None,
        clock: false,
        outline: Vec::new(),
        datetree: false,
        tree_type: helix_view::editor::OrgCaptureTree::Day,
        template: "* TODO %?\n  %U\n  %a".into(),
        prepend: false,
        immediate: false,
        buffer: false,
        clock_in: false,
        clock_keep: false,
        clock_resume: false,
    }]
}

/// The file with the running clock, and the line of the entry it runs in.
fn clocked_entry(editor: &Editor) -> Option<(PathBuf, String, usize)> {
    let (home, text) = find_running_clock(editor)?;
    let path = match home {
        ClockHome::Buffer(id) => editor.document(id)?.path()?.to_path_buf(),
        ClockHome::Disk(path) => path,
    };
    let clock = helix_roam::clock::running(&text)?;
    let line = helix_roam::clock::entry_of(&text, &clock)?;
    Some((path, text, line))
}

/// What a capture's escapes refer to, taken where it starts: the link to
/// the cursor, the selection (when more than a character), the file, the
/// yank and clipboard registers, the entry being clocked.
pub fn org_capture_context(editor: &Editor) -> helix_roam::org_capture::Context {
    let (view, doc) = current_ref!(editor);
    let selection = doc.selection(view.id).primary();
    let initial = if selection.len() > 1 {
        selection.fragment(doc.text().slice(..)).to_string()
    } else {
        String::new()
    };
    let register = |name: char| {
        editor
            .registers
            .first(name, editor)
            .map(|value| value.into_owned())
            .unwrap_or_default()
    };
    let clocked = clocked_entry(editor).map(|(path, text, line)| {
        let title = clock_title(&text);
        let link = match helix_roam::restructure::entry_at(&text, line) {
            Some((id, title)) => {
                helix_roam::hyperlink::format_link(&format!("id:{id}"), Some(&title))
            }
            None => helix_roam::hyperlink::format_link(
                &format!("file:{}::{}", path.display(), line + 1),
                Some(&title),
            ),
        };
        (title, link)
    });
    let (date, time) = now();
    helix_roam::org_capture::Context {
        link: link_here(editor).ok(),
        initial: initial.trim_end_matches('\n').to_string(),
        file: doc.path().map(Path::to_path_buf),
        kill: register(editor.config().default_yank_register),
        clipboard: register('+'),
        clocked,
        user: std::env::var("USER").unwrap_or_default(),
        ..helix_roam::org_capture::Context::at(date, time)
    }
}

/// What a template's place starts from, found in the target's text when
/// the capture is filed, as that text may have changed since.
enum Anchor {
    Top,
    Line(usize),
    Id(String),
    Regexp(helix_core::regex::Regex),
}

impl Anchor {
    fn line(&self, text: &str) -> Result<Option<usize>, String> {
        match self {
            Anchor::Top => Ok(None),
            Anchor::Line(line) => Ok(Some(*line)),
            Anchor::Id(id) => helix_roam::org_capture::id_line(text, id)
                .map(Some)
                .ok_or_else(|| format!("no entry has the id {id}")),
            Anchor::Regexp(regexp) => text
                .lines()
                .position(|line| regexp.is_match(line))
                .map(Some)
                .ok_or_else(|| format!("no line matches {}", regexp.as_str())),
        }
    }
}

/// The file a template's capture goes into, and what it starts from there.
fn capture_target(
    editor: &Editor,
    template: &helix_view::editor::OrgCaptureTemplate,
) -> Result<(PathBuf, Anchor), String> {
    if template.clock {
        let (path, _, line) = clocked_entry(editor).ok_or("no clock is running")?;
        return Ok((path, Anchor::Line(line)));
    }
    if let Some(id) = &template.id {
        let target = refile_targets(editor)
            .into_iter()
            .find(|target| target.id.to_string().eq_ignore_ascii_case(id.trim()))
            .ok_or_else(|| format!("no node has the id {id}"))?;
        return Ok((target.path, Anchor::Id(id.clone())));
    }
    let file = Path::new(&template.file);
    let path = if file.is_absolute() {
        file.to_path_buf()
    } else {
        notes_directory(editor).join(file)
    };
    let anchor = match &template.regexp {
        Some(pattern) => Anchor::Regexp(
            helix_core::regex::Regex::new(pattern)
                .map_err(|err| format!("the template's regexp: {err}"))?,
        ),
        None => Anchor::Top,
    };
    Ok((path, anchor))
}

/// Files `expanded` where `template` says, clocking into it when the
/// template asks, and returns the file and where the cursor goes in it.
///
/// `opened` is when its capture buffer opened, if it had one: a `clock-in`
/// capture's clock then runs from that moment until now, and is stopped
/// (unless `clock-keep`) since the capture is over.
fn file_capture(
    editor: &mut Editor,
    template: &helix_view::editor::OrgCaptureTemplate,
    expanded: &str,
    date: helix_roam::Date,
    opened: Option<helix_roam::clock::Moment>,
) -> Result<(PathBuf, usize), String> {
    use helix_roam::org_capture::{place, Kind, Place, Tree};
    use helix_view::editor::{OrgCaptureKind, OrgCaptureTree};

    let (path, anchor) = capture_target(editor, template)?;
    if editor.document_by_path(&path).is_none() && !path.exists() {
        path.parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, ""))
            .map_err(|err| format!("could not create {}: {err}", path.display()))?;
    }
    // The clock this capture takes over is stopped first, so that its own
    // file is read afterwards with the clock closed; it stops when the
    // capture started.
    let now = now_moment();
    let started = opened.unwrap_or(now);
    let interrupted = if template.clock_in {
        interrupt_clock(editor, started)
            .map_err(|err| format!("the running clock could not be stopped: {err}"))?
    } else {
        None
    };
    let finished = opened.is_some() && !template.clock_keep;

    let kind = match template.kind {
        OrgCaptureKind::Entry => Kind::Entry,
        OrgCaptureKind::Item => Kind::Item,
        OrgCaptureKind::Checkitem => Kind::CheckItem,
        OrgCaptureKind::Plain => Kind::Plain,
    };
    let tree = match template.tree_type {
        OrgCaptureTree::Day => Tree::Day,
        OrgCaptureTree::Week => Tree::Week,
        OrgCaptureTree::Month => Tree::Month,
    };
    let mut cursor = 0;
    edit_file(editor, &path, |_, text| {
        let at = Place {
            line: anchor.line(text)?,
            outline: template.outline.clone(),
            datetree: template.datetree,
            tree,
            prepend: template.prepend,
        };
        let insertion = place(text, &at, kind, expanded, date);
        cursor = insertion.cursor;
        let mut after = text.to_string();
        after.insert_str(insertion.at, &insertion.text);
        if template.clock_in {
            let line = after[..cursor].matches('\n').count();
            after = helix_roam::clock::clock_in(&after, line, started)
                .map_err(|err| format!("not clocked in: {err}"))?;
            if finished {
                after = helix_roam::clock::clock_out(&after, now)
                    .map_err(|err| format!("not clocked out: {err}"))?
                    .0;
            }
        }
        Ok(after)
    })?;
    if template.clock_in && !finished {
        editor.org_clock = Some(path.clone());
    }
    if let Some(interrupted) = interrupted.filter(|_| finished && template.clock_resume) {
        resume_clock(editor, interrupted, now)
            .map_err(|err| format!("the interrupted clock was not resumed: {err}"))?;
    }
    Ok((path, cursor))
}

/// An entry whose clock a capture stopped: its file, its headline and the
/// line it was on, to find it again after the capture moved it.
struct Interrupted {
    path: PathBuf,
    headline: String,
    line: usize,
}

/// Stops the running clock at `when` (or when it started, if that is
/// later), and says which entry it was on.
fn interrupt_clock(
    editor: &mut Editor,
    when: helix_roam::clock::Moment,
) -> Result<Option<Interrupted>, String> {
    let Some((path, text, line)) = clocked_entry(editor) else {
        return Ok(None);
    };
    let headline = text.lines().nth(line).unwrap_or_default().to_string();
    edit_file(editor, &path, |_, text| {
        let start = helix_roam::clock::running(text).map_or(when, |clock| clock.start);
        helix_roam::clock::clock_out(text, when.max(start))
            .map(|(after, _)| after)
            .map_err(|err| err.to_string())
    })?;
    Ok(Some(Interrupted {
        path,
        headline,
        line,
    }))
}

/// Clocks back into the entry a capture interrupted: its headline nearest
/// to where it was.
fn resume_clock(
    editor: &mut Editor,
    interrupted: Interrupted,
    now: helix_roam::clock::Moment,
) -> Result<(), String> {
    edit_file(editor, &interrupted.path, |_, text| {
        let line = text
            .lines()
            .enumerate()
            .filter(|(_, line)| *line == interrupted.headline)
            .min_by_key(|(at, _)| at.abs_diff(interrupted.line))
            .map(|(at, _)| at)
            .ok_or_else(|| format!("{} is gone", interrupted.headline.trim()))?;
        helix_roam::clock::clock_in(text, line, now).map_err(|err| err.to_string())
    })?;
    editor.org_clock = Some(interrupted.path);
    Ok(())
}

/// How a capture's file is named in messages: from the notes directory.
fn capture_name(editor: &Editor, path: &Path) -> String {
    path.strip_prefix(notes_directory(editor))
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Files a capture where `template` says, then goes to it unless the
/// template is `immediate`, or shows it in a buffer of its own first when
/// the template asks for one. An open buffer for the file gets the text,
/// unsaved like any other change; otherwise the file does.
pub fn org_capture(
    editor: &mut Editor,
    template: &helix_view::editor::OrgCaptureTemplate,
    cx: &helix_roam::org_capture::Context,
) {
    let expanded = helix_roam::org_capture::expand(&template.template, cx);
    if template.buffer {
        return open_capture_buffer(editor, template, &expanded, cx.date);
    }
    let (path, cursor) = match file_capture(editor, template, &expanded, cx.date, None) {
        Ok(filed) => filed,
        Err(err) => return editor.set_error(format!("Not captured: {err}")),
    };
    let name = capture_name(editor, &path);
    if template.immediate {
        return editor.set_status(format!("Captured into {name}"));
    }
    if let Err(err) = editor.open(&path, helix_view::editor::Action::Replace) {
        return editor.set_error(format!("could not open {name}: {err}"));
    }
    jump_to_byte(editor, cursor);
    // A capture is for typing: straight into insert mode, at `%?`.
    editor.mode = helix_view::document::Mode::Insert;
    editor.set_status(format!("Captured into {name}"));
}

/// Shows a capture in a buffer of its own, beside what was being edited,
/// until it is written (filed) or closed without writing (dropped).
fn open_capture_buffer(
    editor: &mut Editor,
    template: &helix_view::editor::OrgCaptureTemplate,
    expanded: &str,
    date: helix_roam::Date,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(1);

    // The clock running now is the one filing it stops, when the capture
    // buffer is focused and no longer shows where it is.
    if template.clock_in {
        if let Some((path, _, _)) = clocked_entry(editor) {
            editor.org_clock = Some(path);
        }
    }
    let (text, cursor) = helix_roam::org_capture::take_cursor(expanded);
    let directory = helix_loader::cache_dir().join("capture");
    let buffer = directory.join(format!(
        "capture-{}-{}.org",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let written = std::fs::create_dir_all(&directory).and_then(|()| std::fs::write(&buffer, &text));
    if let Err(err) = written {
        return editor.set_error(format!("could not prepare the capture: {err}"));
    }
    if let Err(err) = editor.open(&buffer, helix_view::editor::Action::HorizontalSplit) {
        return editor.set_error(format!("could not open the capture: {err}"));
    }
    jump_to_byte(
        editor,
        cursor.unwrap_or_else(|| text.trim_end_matches('\n').len()),
    );
    editor.mode = helix_view::document::Mode::Insert;
    editor
        .pending_captures
        .push(helix_view::editor::PendingCapture {
            buffer,
            template: template.clone(),
            date,
            opened: now_moment(),
        });
    editor.set_status(format!(
        "Capture ({}): :w files it, :q! drops it, :org-capture-refile files it elsewhere",
        template.description
    ));
}

/// Files the capture whose buffer `path` was just written, and closes it.
/// Returns whether the write was a capture's.
pub fn capture_if_written(editor: &mut Editor, path: &Path) -> bool {
    let Some(index) = editor
        .pending_captures
        .iter()
        .position(|pending| pending.buffer == path)
    else {
        return false;
    };
    let pending = editor.pending_captures[index].clone();
    finish_capture(editor, index, &pending.template, pending.date);
    true
}

/// Files the capture in the pending buffer `index` with `template`, which
/// is its own or one pointing elsewhere; on success the buffer is closed
/// and its file removed, on failure both stay for another try.
fn finish_capture(
    editor: &mut Editor,
    index: usize,
    template: &helix_view::editor::OrgCaptureTemplate,
    date: helix_roam::Date,
) {
    let buffer = editor.pending_captures[index].buffer.clone();
    let opened = editor.pending_captures[index].opened;
    let text = match editor.document_by_path(&buffer) {
        Some(doc) => doc.text().to_string(),
        None => std::fs::read_to_string(&buffer).unwrap_or_default(),
    };
    if text.trim().is_empty() {
        return editor.set_error("The capture is empty; :q! drops it");
    }
    match file_capture(editor, template, &text, date, Some(opened)) {
        Ok((path, _)) => {
            editor.pending_captures.remove(index);
            let name = capture_name(editor, &path);
            // Closed once the write that filed it is done with the buffer:
            // the save handling still reaches for it after this returns.
            tokio::spawn(crate::job::dispatch(move |editor, _| {
                if let Some(id) = editor.document_by_path(&buffer).map(|doc| doc.id()) {
                    let _ = editor.close_document(id, true);
                }
                let _ = std::fs::remove_file(&buffer);
                editor.set_status(format!("Captured into {name}"));
            }));
        }
        Err(err) => editor.set_error(format!("Not captured: {err}")),
    }
}

/// Whether the focused buffer is a capture waiting to be filed.
pub fn in_capture_buffer(editor: &Editor) -> Option<usize> {
    let path = doc!(editor).path()?;
    editor
        .pending_captures
        .iter()
        .position(|pending| pending.buffer == path)
}

/// Files the capture in the focused buffer under `target` instead of where
/// its template says: Org's refile from a capture buffer.
pub fn refile_capture(editor: &mut Editor, target: &RefileTarget) {
    let Some(index) = in_capture_buffer(editor) else {
        return editor.set_error("Not in a capture buffer");
    };
    let pending = editor.pending_captures[index].clone();
    let template = helix_view::editor::OrgCaptureTemplate {
        id: Some(target.id.to_string()),
        regexp: None,
        clock: false,
        outline: Vec::new(),
        datetree: false,
        ..pending.template
    };
    finish_capture(editor, index, &template, pending.date);
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

    let settings = file_settings(editor);
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
    let from = (!from.is_empty()).then_some(from);
    let entry = format!(
        "- {}",
        helix_roam::logging::state_heading(Some(to), from, &stamp)
    );

    let (text, line) = text_and_line(editor);
    let after = helix_roam::restructure::log_entry(&text, line, &entry);
    apply_to_buffer(editor, format!("Logged {to}"), after);
}

/// The file's own settings, which decide what a keyword or a cookie is.
fn file_settings(editor: &Editor) -> helix_roam::FileSettings {
    settings_of(doc!(editor))
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
/// A change worked out on some text: the new text, what to say about it,
/// and the note it waits for, if any.
struct Edited {
    text: String,
    done: String,
    note: Option<helix_roam::logging::PendingNote>,
}

/// Moves the entry at `line` in `text` to its next or previous state,
/// recording what the file asks for and refusing what its dependencies
/// block. Which buffer, if any, the text is in is the caller's business.
fn state_change(
    editor: &Editor,
    text: &str,
    line: usize,
    settings: &helix_roam::FileSettings,
    forward: bool,
) -> Result<Edited, String> {
    let (_, next) = helix_roam::restructure::todo_step(text, line, settings, forward)
        .map_err(|err| err.to_string())?;
    state_set(editor, text, line, settings, next.as_deref())
}

/// Puts the entry at `line` in `next` (or no state), with what
/// [`state_change`] does around it.
fn state_set(
    editor: &Editor,
    text: &str,
    line: usize,
    settings: &helix_roam::FileSettings,
    next: Option<&str>,
) -> Result<Edited, String> {
    if let Some(state) = next {
        if !settings.is_todo_keyword(state) {
            return Err(format!("{state} is not a TODO keyword of this file"));
        }
    }
    let startup = helix_roam::startup::Startup::of(settings);
    let finishing =
        next.is_some_and(|state| settings.done_keywords.iter().any(|done| done == state));
    if finishing {
        let blocked = open_blockers(editor, text, line, settings);
        if !blocked.is_empty() {
            return Err(format!("Blocked by {}", blocked.join(", ")));
        }
    }

    let change = helix_roam::logging::change_state(text, line, settings, &startup, next, now())
        .map_err(|err| err.to_string())?;
    let done = match (change.repeated, &change.state) {
        (true, state) => format!(
            "Repeated: back to {}, dates moved on",
            state.as_deref().unwrap_or("no state")
        ),
        (false, Some(state)) => format!("State {state}"),
        (false, None) => "Cleared the state".to_string(),
    };
    Ok(Edited {
        text: change.text,
        done,
        note: change.note,
    })
}

fn cycle_todo(editor: &mut Editor, forward: bool) {
    let settings = file_settings(editor);
    let (text, line) = text_and_line(editor);

    match state_change(editor, &text, line, &settings, forward) {
        Ok(edited) => {
            apply_to_buffer(editor, edited.done, edited.text);
            if let Some(pending) = edited.note {
                ask_for_note(pending, None);
            }
        }
        Err(err) => editor.set_error(err),
    }
}

/// Changes the file at `path` — its buffer if it is open, the file on disk
/// if not — and re-indexes it from the result, so a view built from the
/// index, like the agenda, shows the change at once.
fn edit_file(
    editor: &mut Editor,
    path: &Path,
    edit: impl FnOnce(&Editor, &str) -> Result<String, String>,
) -> Result<(), String> {
    let open = editor
        .document_by_path(path)
        .map(|doc| (doc.id(), doc.text().to_string()));
    let text = match &open {
        Some((_, text)) => text.clone(),
        None => std::fs::read_to_string(path)
            .map_err(|err| format!("could not read {}: {err}", path.display()))?,
    };
    let after = edit(editor, &text)?;
    if after == text {
        return Ok(());
    }
    match open {
        Some((id, _)) => apply_to_document(editor, id, &after)?,
        None => std::fs::write(path, &after)
            .map_err(|err| format!("could not write {}: {err}", path.display()))?,
    }
    helix_roam::reindex_file(&mut editor.roam.write(), path, &after);
    Ok(())
}

/// What the agenda can do to the entry on a line of it.
pub enum AgendaAction {
    /// Its next state, or its previous one.
    State(bool),
    /// Priority up, or down.
    Priority(bool),
    Schedule(String),
    Deadline(String),
    ClockIn,
    /// This state, or none.
    SetState(Option<String>),
    /// Add the tag, or remove it.
    Tag(String, bool),
}

/// Acts on the entry at `line` of `path` without opening it, returning what
/// to say about it. This is what lets the agenda stay on screen.
pub fn agenda_act(
    editor: &mut Editor,
    path: &Path,
    line: usize,
    action: AgendaAction,
) -> Result<String, String> {
    let mut said = String::new();
    let mut note = None;
    let mut clocked = false;

    edit_file(editor, path, |editor, text| {
        let settings = helix_roam::FileSettings::scan_at(text, path);
        match action {
            AgendaAction::State(forward) => {
                let edited = state_change(editor, text, line, &settings, forward)?;
                said = edited.done;
                note = edited.note;
                Ok(edited.text)
            }
            AgendaAction::Priority(raise) => {
                said = if raise {
                    "Priority up"
                } else {
                    "Priority down"
                }
                .to_string();
                helix_roam::restructure::change_priority(text, line, &settings, raise)
                    .map_err(|err| err.to_string())
            }
            AgendaAction::Schedule(ref input) | AgendaAction::Deadline(ref input) => {
                let which = match action {
                    AgendaAction::Schedule(_) => helix_roam::restructure::Planning::Scheduled,
                    _ => helix_roam::restructure::Planning::Deadline,
                };
                let date = parse_date_input(input.trim()).ok_or_else(|| {
                    format!("{input:?} is not a date; try today, +3, or 2026-09-18")
                })?;
                let stamp = format!("<{} {}>", date.to_iso(), date.weekday());
                let startup = helix_roam::startup::Startup::of(&settings);
                let replanned =
                    helix_roam::logging::replan(text, line, which, Some(&stamp), &startup, now())
                        .map_err(|err| err.to_string())?;
                said = format!("Set {stamp}");
                note = replanned.note;
                Ok(replanned.text)
            }
            AgendaAction::ClockIn => {
                clocked = true;
                said = "Clocked in".to_string();
                helix_roam::clock::clock_in(text, line, now_moment()).map_err(|err| err.to_string())
            }
            AgendaAction::SetState(ref state) => {
                let edited = state_set(editor, text, line, &settings, state.as_deref())?;
                said = edited.done;
                note = edited.note;
                Ok(edited.text)
            }
            AgendaAction::Tag(ref tag, add) => {
                said = format!("{} :{tag}:", if add { "Tagged" } else { "Untagged" });
                Ok(helix_roam::restructure::edit_tag(text, line, tag, add)
                    .map_err(|err| err.to_string())?
                    .unwrap_or_else(|| text.to_string()))
            }
        }
    })?;

    if clocked {
        editor.org_clock = Some(path.to_path_buf());
    }
    if let Some(pending) = note {
        ask_for_note(pending, Some(path.to_path_buf()));
    }
    Ok(said)
}

/// Moves the entry at `line` of `path` under `target`, from the agenda:
/// both files are changed where they are, buffer or disk, the target
/// first, so that a failure leaves the entry twice rather than nowhere.
pub fn agenda_refile(
    editor: &mut Editor,
    path: &Path,
    line: usize,
    target: &RefileTarget,
) -> Result<String, String> {
    if path == target.path {
        return Err("Refiling within the same file is not supported yet".to_string());
    }
    let target_text = match editor.document_by_path(&target.path) {
        Some(doc) => doc.text().to_string(),
        None => std::fs::read_to_string(&target.path)
            .map_err(|err| format!("could not read {}: {err}", target.path.display()))?,
    };
    let source_text = match editor.document_by_path(path) {
        Some(doc) => doc.text().to_string(),
        None => std::fs::read_to_string(path)
            .map_err(|err| format!("could not read {}: {err}", path.display()))?,
    };
    let refiling =
        helix_roam::restructure::refile_subtree(&source_text, line, &target_text, target.id)
            .map_err(|err| err.to_string())?;
    // The target first: should the source then fail to be written, the
    // entry is in both files rather than in neither.
    edit_file(editor, &target.path, |_, _| Ok(refiling.target.clone()))?;
    edit_file(editor, path, |_, _| Ok(refiling.source.clone()))?;
    Ok(format!(
        "Refiled \"{}\" under {}",
        refiling.title, target.title
    ))
}

/// What keeps the entry at `line` from being done, described for a message.
///
/// An entry named in `:BLOCKER:` may be in any file, so it is looked up in
/// the index. One the index does not know blocks: an entry that cannot be
/// checked is not known to be done.
fn open_blockers(
    editor: &Editor,
    text: &str,
    line: usize,
    settings: &helix_roam::FileSettings,
) -> Vec<String> {
    use helix_roam::dependencies::Blocker;

    let children = editor.config().roam.todo_dependencies;
    let graph = editor.roam.read();
    helix_roam::dependencies::blockers(text, line, settings, children)
        .into_iter()
        .filter_map(|blocker| match blocker {
            Blocker::Child(title) => Some(format!("the open child \"{title}\"")),
            Blocker::Sibling(title) => Some(format!("the earlier \"{title}\"")),
            Blocker::Unreadable(word) => {
                Some(format!("\"{word}\" in :BLOCKER:, which is not an id"))
            }
            Blocker::Named(id) => match graph.get_node(&id) {
                None => Some(format!("{id}, which is not in the index")),
                Some(node) => node
                    .todo
                    .as_ref()
                    .filter(|state| !state.done)
                    .map(|_| format!("\"{}\"", node.title)),
            },
        })
        .collect()
}

/// The date and time a log line records.
fn now() -> (helix_roam::Date, helix_roam::date::Time) {
    (helix_roam::Date::today(), helix_roam::date::Time::now())
}

/// Asks for the note a state change or a reschedule is waiting on.
///
/// Queued rather than pushed: the change that wants a note is often itself
/// the result of a prompt, which has no compositor to push onto. Escaping the
/// prompt records nothing, as aborting the note does in Org; the change it
/// followed stays made.
fn ask_for_note(pending: helix_roam::logging::PendingNote, file: Option<PathBuf>) {
    let label = if pending.heading.starts_with("CLOSING NOTE") {
        "Closing note: "
    } else {
        "Note: "
    };

    crate::job::dispatch_blocking(move |_editor, compositor| {
        let prompt = crate::ui::Prompt::new(
            label.into(),
            None,
            |_editor, _input| Vec::new(),
            move |cx, input, event| {
                if event != crate::ui::PromptEvent::Validate {
                    return;
                }
                // A note asked for from the agenda belongs in the entry's
                // file, not in whatever buffer is behind the agenda.
                match &file {
                    Some(path) => {
                        let result = edit_file(cx.editor, path, |_, text| {
                            Ok(helix_roam::logging::write_note(text, &pending, input))
                        });
                        match result {
                            Ok(()) => cx.editor.set_status("Noted"),
                            Err(err) => cx.editor.set_error(err),
                        }
                    }
                    None => {
                        let text = doc!(cx.editor).text().to_string();
                        let after = helix_roam::logging::write_note(&text, &pending, input);
                        apply_to_buffer(cx.editor, "Noted".to_string(), after);
                    }
                }
            },
        );
        compositor.push(Box::new(prompt));
    });
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

    let settings = file_settings(editor);
    let startup = helix_roam::startup::Startup::of(&settings);
    let (text, line) = text_and_line(editor);
    match helix_roam::logging::replan(&text, line, which, stamp.as_deref(), &startup, now()) {
        Ok(replanned) => {
            let done = match (&stamp, replanned.logged) {
                (Some(stamp), false) => format!("Set {stamp}"),
                (Some(stamp), true) => format!("Set {stamp}, and logged the change"),
                (None, false) => "Cleared it".to_string(),
                (None, true) => "Cleared it, and logged the change".to_string(),
            };
            apply_to_buffer(editor, done, replanned.text);
            if let Some(pending) = replanned.note {
                ask_for_note(pending, None);
            }
        }
        Err(err) => editor.set_error(err.to_string()),
    }
}

/// A capture's date answer as the timestamp its escape writes: the date
/// as a date prompt takes it (`today`, `+3`, `2026-10-01`; empty is
/// today), then a time (`14:30`) when one is typed.
pub fn capture_date_answer(input: &str, active: bool) -> Result<String, String> {
    let mut words: Vec<&str> = input.split_whitespace().collect();
    let time = match words.last().and_then(|word| word.split_once(':')) {
        Some((hour, minute)) => {
            let hour: u32 = hour
                .parse()
                .map_err(|_| format!("`{input}` has no valid time"))?;
            let minute: u32 = minute
                .parse()
                .map_err(|_| format!("`{input}` has no valid time"))?;
            if hour > 23 || minute > 59 {
                return Err(format!("`{input}` has no valid time"));
            }
            words.pop();
            Some(helix_roam::date::Time { hour, minute })
        }
        None => None,
    };
    let date = match words.join(" ").as_str() {
        "" => helix_roam::Date::today(),
        text => parse_date_input(text).ok_or_else(|| format!("`{text}` is not a date"))?,
    };
    Ok(helix_roam::org_capture::timestamp(date, time, active))
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

/// Tags used on any headline of the notes, for completing a tag prompt.
pub fn known_tags(editor: &Editor) -> Vec<String> {
    let mut tags: Vec<String> = {
        let graph = editor.roam.read();
        let collected = graph
            .entries()
            .flat_map(|entry| entry.tags.iter().cloned())
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
#[derive(Debug, Clone, Default)]
pub struct AgendaLine {
    pub when: String,
    pub what: String,
    /// Empty for a block's heading, which is no entry.
    pub path: PathBuf,
    pub line: usize,
    /// A habit's consistency graph, empty for anything else.
    pub habit: Vec<helix_roam::habit::Cell>,
    /// Every tag of the entry, inherited ones included.
    pub tags: Vec<String>,
    pub category: String,
    /// The `:Effort:` in minutes.
    pub effort: Option<u32>,
    /// The entry's title alone, without state, priority or tags.
    pub title: String,
    pub todo: Option<String>,
    /// The day and time of day the line is for, in a view by day.
    pub day: Option<helix_roam::Date>,
    pub time: Option<(u32, u32)>,
    /// The entry's drawer, for the column view.
    pub properties: Vec<(String, String)>,
    pub priority: Option<char>,
    /// Minutes clocked on the entry, a running clock until now.
    pub clocked: i64,
}

impl AgendaLine {
    /// A column's value for this line: `TODO`, `PRIORITY`, `TAGS`,
    /// `CLOCKSUM` or a property.
    pub fn column(&self, name: &str) -> String {
        match name.to_ascii_uppercase().as_str() {
            "TODO" => self.todo.clone().unwrap_or_default(),
            "PRIORITY" => self.priority.map(String::from).unwrap_or_default(),
            "TAGS" if self.tags.is_empty() => String::new(),
            "TAGS" => format!(":{}:", self.tags.join(":")),
            "CLOCKSUM" if self.clocked == 0 => String::new(),
            "CLOCKSUM" => helix_roam::clock::format_duration(self.clocked),
            "ITEM" => self.title.clone(),
            _ => self
                .properties
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.clone())
                .unwrap_or_default(),
        }
    }
}

impl AgendaLine {
    /// The heading of a block of a custom view, or any line that is no
    /// entry: a time grid's hour, a report's total.
    fn heading(title: String) -> Self {
        Self {
            when: format!("── {title}"),
            ..Self::default()
        }
    }

    pub fn is_heading(&self) -> bool {
        self.path.as_os_str().is_empty()
    }
}

/// What an agenda view is narrowed to, as Org's `/`, `<`, `_` and `=`
/// narrow its agenda buffer. Each part left empty lets everything through.
#[derive(Debug, Clone, Default)]
pub struct AgendaFilter {
    /// Tags wanted (`true`) or refused (`false`).
    pub tags: Vec<(bool, String)>,
    pub category: Option<String>,
    /// `<`, `>` or `=`, and minutes.
    pub effort: Option<(char, u32)>,
    pub regex: Option<helix_core::regex::Regex>,
}

impl AgendaFilter {
    /// Reads `+work -home urgent`: a tag without a sign is wanted.
    pub fn parse_tags(input: &str) -> Vec<(bool, String)> {
        input
            .split_whitespace()
            .filter_map(|word| {
                let (wanted, tag) = match word.strip_prefix('-') {
                    Some(tag) => (false, tag),
                    None => (true, word.strip_prefix('+').unwrap_or(word)),
                };
                let tag = tag.trim_matches(':');
                (!tag.is_empty()).then(|| (wanted, tag.to_string()))
            })
            .collect()
    }

    /// Reads `<0:30`, `>1:00`, `=45` or `20` (at most, as Org's default).
    pub fn parse_effort(input: &str) -> Result<(char, u32), String> {
        let input = input.trim();
        let (op, rest) = match input.chars().next() {
            Some(op @ ('<' | '>' | '=')) => (op, &input[1..]),
            _ => ('<', input),
        };
        effort_minutes(rest)
            .map(|minutes| (op, minutes))
            .ok_or_else(|| format!("`{input}` is not an effort; try <0:30 or >1:00"))
    }

    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
            && self.category.is_none()
            && self.effort.is_none()
            && self.regex.is_none()
    }

    /// Whether the line stays in view. Lines that are no entry always do.
    pub fn matches(&self, line: &AgendaLine) -> bool {
        if line.is_heading() {
            return true;
        }
        let tags_ok = self
            .tags
            .iter()
            .all(|(wanted, tag)| line.tags.contains(tag) == *wanted);
        let category_ok = self
            .category
            .as_ref()
            .is_none_or(|category| &line.category == category);
        // An entry without an effort is not known to be short, nor long.
        let effort_ok = self.effort.is_none_or(|(op, minutes)| {
            line.effort.is_some_and(|effort| match op {
                '<' => effort <= minutes,
                '>' => effort >= minutes,
                _ => effort == minutes,
            })
        });
        let regex_ok = self
            .regex
            .as_ref()
            .is_none_or(|regex| regex.is_match(&line.what));
        tags_ok && category_ok && effort_ok && regex_ok
    }

    /// What the filter keeps, for the status line.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if !self.tags.is_empty() {
            let tags: Vec<String> = self
                .tags
                .iter()
                .map(|(wanted, tag)| format!("{}{tag}", if *wanted { '+' } else { '-' }))
                .collect();
            parts.push(tags.join(""));
        }
        if let Some(category) = &self.category {
            parts.push(format!("category {category}"));
        }
        if let Some((op, minutes)) = self.effort {
            parts.push(format!("effort {op}{}:{:02}", minutes / 60, minutes % 60));
        }
        if let Some(regex) = &self.regex {
            parts.push(format!("{{{}}}", regex.as_str()));
        }
        parts.join(", ")
    }
}

/// `1:30`, `90` or `1h30` as minutes.
fn effort_minutes(text: &str) -> Option<u32> {
    let text = text.trim();
    if let Some((hours, minutes)) = text.split_once(':') {
        return Some(hours.trim().parse::<u32>().ok()? * 60 + minutes.trim().parse::<u32>().ok()?);
    }
    if let Some((hours, minutes)) = text.split_once('h') {
        let minutes = minutes.trim_end_matches(['m', 'i', 'n']);
        let minutes = if minutes.is_empty() {
            0
        } else {
            minutes.parse().ok()?
        };
        return Some(hours.trim().parse::<u32>().ok()? * 60 + minutes);
    }
    text.trim_end_matches(['m', 'i', 'n']).parse().ok()
}

/// How a view by day is shown, which its keys change: the span it starts
/// from and what it adds to the entries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ViewOptions {
    /// Spans moved from today's: `-1` is the previous week of a week view.
    pub shift: i64,
    /// Log mode: what was closed and clocked on each day too.
    pub log: bool,
    /// A clock report of the span after the entries.
    pub report: bool,
    /// The hours of the day between the timed entries.
    pub grid: bool,
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
pub fn agenda_lines(editor: &Editor, days: i64, options: &ViewOptions) -> Vec<AgendaLine> {
    agenda_lines_matching(editor, days, None, options)
}

/// The agenda over `days` days, of the entries `filter` matches, shown as
/// `options` say.
fn agenda_lines_matching(
    editor: &Editor,
    days: i64,
    filter: Option<&helix_roam::search::Match>,
    options: &ViewOptions,
) -> Vec<AgendaLine> {
    use helix_roam::agenda::Reason;

    let today = helix_roam::Date::today();
    let from = today.offset_by(options.shift * days.max(1));
    let graph = editor.roam.read();
    let entries: Vec<&helix_roam::Entry> = graph
        .entries()
        .filter(|entry| in_agenda_scope(editor, &entry.file_path))
        .filter(|entry| filter.is_none_or(|filter| filter.matches(entry)))
        .collect();

    // Every day of the span has its heading, even an empty one, and its
    // rows: the agenda's items, the log's, the grid's hours. Rows with a
    // time come first, in time order, as Org shows them.
    let mut by_day: std::collections::BTreeMap<helix_roam::Date, Vec<AgendaLine>> = (0..days
        .max(1))
        .map(|offset| (from.offset_by(offset), Vec::new()))
        .collect();

    for item in helix_roam::agenda::agenda(entries.iter().copied(), from, days) {
        let what = match (item.reason, item.days_left) {
            (Reason::Deadline, Some(days)) if days < 0 => format!("Deadline: {} d. ago", -days),
            (Reason::Deadline, Some(0)) => "Deadline: today".to_string(),
            (Reason::Deadline, Some(days)) => format!("Deadline: in {days} d."),
            (Reason::Deadline, None) => "Deadline".to_string(),
            (Reason::Scheduled, _) => "Scheduled".to_string(),
            (Reason::Timestamp, _) => String::new(),
        };
        let line = AgendaLine {
            habit: habit_graph(editor, item.entry, today),
            day: Some(item.day),
            time: item.time,
            ..entry_line(item.entry, day_row(&item.entry.category, item.time, &what))
        };
        by_day.entry(item.day).or_default().push(line);
    }

    for (day, entry, text) in helix_roam::agenda::diary(entries.iter().copied(), from, days) {
        let line = AgendaLine {
            day: Some(day),
            what: text.clone(),
            title: text,
            ..entry_line(entry, day_row(&entry.category, None, "Diary"))
        };
        by_day.entry(day).or_default().push(line);
    }

    if options.log {
        for item in helix_roam::agenda::log(entries.iter().copied(), from, days) {
            let (hour, minute) = item.time;
            let what = match item.what {
                helix_roam::agenda::Logged::Closed => format!("{hour:02}:{minute:02} Closed"),
                helix_roam::agenda::Logged::Clocked { end } => {
                    let start = helix_roam::clock::moment(
                        item.day,
                        helix_roam::date::Time { hour, minute },
                    );
                    let end_of_day = end.rem_euclid(1440);
                    format!(
                        "{hour:02}:{minute:02}-{:02}:{:02} Clocked ({})",
                        end_of_day / 60,
                        end_of_day % 60,
                        helix_roam::clock::format_duration(end - start)
                    )
                }
            };
            let line = AgendaLine {
                day: Some(item.day),
                time: Some(item.time),
                ..entry_line(item.entry, day_row(&item.entry.category, None, &what))
            };
            by_day.entry(item.day).or_default().push(line);
        }
    }

    let now = helix_roam::date::Time::now();
    let mut lines = Vec::new();
    for (day, mut rows) in by_day {
        if options.grid {
            for hour in (8..=20).step_by(2) {
                rows.push(AgendaLine {
                    time: Some((hour, 0)),
                    ..AgendaLine::heading(String::new())
                });
            }
            if day == today {
                rows.push(AgendaLine {
                    time: Some((now.hour, now.minute)),
                    ..AgendaLine::heading(String::new())
                });
            }
            for row in rows.iter_mut().filter(|row| row.is_heading()) {
                let (hour, minute) = row.time.unwrap_or_default();
                let mark = if day == today && (hour, minute) == (now.hour, now.minute) {
                    "now ─ ─ ─ ─ ─ ─"
                } else {
                    "┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄"
                };
                row.when = format!("{:<12}{hour:02}:{minute:02} {mark}", "");
            }
        }
        // Timed rows first, by time; a grid hour before an entry at the
        // same time; the rest in the order they came.
        rows.sort_by_key(|row| (row.time.is_none(), row.time, !row.is_heading()));
        let label = if day == today { " · today" } else { "" };
        lines.push(AgendaLine::heading(format!(
            "{} {}{label}",
            day.weekday(),
            day.to_iso()
        )));
        lines.extend(rows);
    }

    if options.report {
        let start = helix_roam::clock::moment(from, helix_roam::date::Time { hour: 0, minute: 0 });
        let end = start + days.max(1) * 1440;
        let report =
            helix_roam::agenda::clock_report(entries.iter().copied(), start, end, now_moment());
        let total: i64 = report.iter().map(|(_, minutes)| minutes).sum();
        lines.push(AgendaLine::heading(format!(
            "Clock report: {}",
            helix_roam::clock::format_duration(total)
        )));
        for (entry, minutes) in report {
            let duration = helix_roam::clock::format_duration(minutes);
            lines.push(entry_line(entry, day_row(&entry.category, None, &duration)));
        }
    }
    lines
}

/// The first column of a row under a day: category, time, what it is.
fn day_row(category: &str, time: Option<(u32, u32)>, what: &str) -> String {
    let time = time
        .map(|(hour, minute)| format!("{hour:02}:{minute:02} "))
        .unwrap_or_default();
    format!("  {category:<10}{time}{what}")
        .trim_end()
        .to_string()
}

/// The consistency graph of an entry that is a habit.
///
/// The history is in the entry's logbook, which the index does not keep,
/// so the file is read: from its buffer if it is open, which is newer than
/// the disk, and otherwise from the disk.
fn habit_graph(
    editor: &Editor,
    entry: &helix_roam::Entry,
    today: helix_roam::Date,
) -> Vec<helix_roam::habit::Cell> {
    let is_habit = entry
        .property("style")
        .is_some_and(|value| value.eq_ignore_ascii_case("habit"));
    if !is_habit {
        return Vec::new();
    }
    let text = match editor.document_by_path(&entry.file_path) {
        Some(doc) => doc.text().to_string(),
        None => match std::fs::read_to_string(&entry.file_path) {
            Ok(text) => text,
            Err(_) => return Vec::new(),
        },
    };
    helix_roam::habit::parse(&text, entry.line)
        .map(|habit| {
            helix_roam::habit::consistency(
                &habit,
                today,
                helix_roam::habit::DAYS_BEFORE,
                helix_roam::habit::DAYS_AFTER,
            )
        })
        .unwrap_or_default()
}

/// Everything unfinished, whether or not it has a date.
pub fn todo_lines(editor: &Editor) -> Vec<AgendaLine> {
    filtered_todo_lines(editor, &helix_roam::agenda::TodoFilter::default())
}

/// The unfinished nodes matching `filter`.
pub fn filtered_todo_lines(
    editor: &Editor,
    filter: &helix_roam::agenda::TodoFilter,
) -> Vec<AgendaLine> {
    let graph = editor.roam.read();
    let entries: Vec<&helix_roam::Entry> = graph
        .entries()
        .filter(|entry| in_agenda_scope(editor, &entry.file_path))
        .collect();

    helix_roam::agenda::filtered_todo_list(entries, filter)
        .into_iter()
        .map(|entry| entry_line(entry, entry.category.clone()))
        .collect()
}

/// The entries in the agenda's files that `matcher` matches, in file
/// order: Org's tags view.
pub fn match_lines(editor: &Editor, matcher: &helix_roam::search::Match) -> Vec<AgendaLine> {
    let graph = editor.roam.read();
    let mut entries: Vec<&helix_roam::Entry> = graph
        .entries()
        .filter(|entry| in_agenda_scope(editor, &entry.file_path))
        .filter(|entry| matcher.matches(entry))
        .collect();
    entries.sort_by(|a, b| (&a.file_path, a.line).cmp(&(&b.file_path, b.line)));
    entries
        .into_iter()
        .map(|entry| entry_line(entry, entry.category.clone()))
        .collect()
}

/// The entries in the agenda's files whose headline and text have what
/// `search` looks for: Org's search view. The text is read from the
/// file's buffer when it is open, from the disk otherwise.
pub fn search_lines(editor: &Editor, search: &helix_roam::search::Search) -> Vec<AgendaLine> {
    let graph = editor.roam.read();
    let files: std::collections::BTreeSet<&Path> = graph
        .entries()
        .map(|entry| entry.file_path.as_path())
        .filter(|path| in_agenda_scope(editor, path))
        .collect();
    let mut lines = Vec::new();
    for path in files {
        let text = match editor.document_by_path(path) {
            Some(doc) => doc.text().to_string(),
            None => match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(_) => continue,
            },
        };
        let all: Vec<&str> = text.lines().collect();
        for entry in graph.entries_in_file(path) {
            let own = all
                .get(entry.line..entry.end.min(all.len()))
                .unwrap_or_default()
                .join("\n");
            if search.matches(&own) {
                lines.push(entry_line(entry, entry.category.clone()));
            }
        }
    }
    lines
}

/// The projects with nothing to do next, as `stuck-projects` defines them.
pub fn stuck_lines(editor: &Editor) -> Result<Vec<AgendaLine>, String> {
    let config = editor.config().roam.stuck_projects.clone();
    let is_project = helix_roam::search::Match::parse(&config.query, helix_roam::Date::today())?;
    let graph = editor.roam.read();
    let mut files: Vec<(&Path, &[helix_roam::Entry])> = graph
        .entries_by_file()
        .filter(|(path, _)| in_agenda_scope(editor, path))
        .collect();
    files.sort_by_key(|(path, _)| *path);
    Ok(files
        .into_iter()
        .flat_map(|(_, entries)| {
            helix_roam::agenda::stuck_projects(
                entries,
                |entry| is_project.matches(entry),
                &config.todo,
                &config.tags,
            )
        })
        .map(|entry| entry_line(entry, entry.category.clone()))
        .collect())
}

/// The lines of one block of a custom agenda view.
pub fn block_lines(
    editor: &Editor,
    block: &helix_view::editor::AgendaBlock,
) -> Result<Vec<AgendaLine>, String> {
    use helix_roam::search::{Match, Search};
    use helix_view::editor::AgendaBlockKind;

    let today = helix_roam::Date::today();
    let query = block.query.trim();
    Ok(match block.kind {
        AgendaBlockKind::Agenda => {
            let filter = (!query.is_empty())
                .then(|| Match::parse(query, today))
                .transpose()?;
            agenda_lines_matching(editor, block.days, filter.as_ref(), &ViewOptions::default())
        }
        AgendaBlockKind::Todo => match_lines(editor, &Match::parse(&format!("/!{query}"), today)?),
        AgendaBlockKind::Tags => match_lines(editor, &Match::parse(query, today)?),
        AgendaBlockKind::TagsTodo => {
            match_lines(editor, &Match::parse(&format!("{query}/!"), today)?)
        }
        AgendaBlockKind::Search => search_lines(editor, &Search::parse(query)?),
        AgendaBlockKind::Stuck => stuck_lines(editor)?,
    })
}

/// A custom agenda view's lines: its blocks one after the other, each under
/// a heading when there are several.
pub fn custom_view_lines(
    editor: &Editor,
    view: &helix_view::editor::AgendaView,
) -> Result<Vec<AgendaLine>, String> {
    use helix_view::editor::AgendaBlockKind;

    let mut lines = Vec::new();
    for block in &view.blocks {
        if view.blocks.len() > 1 {
            let title = block.title.clone().unwrap_or_else(|| {
                let query = block.query.trim();
                match block.kind {
                    AgendaBlockKind::Agenda if block.days == 1 => "Today".to_string(),
                    AgendaBlockKind::Agenda => format!("Next {} days", block.days),
                    AgendaBlockKind::Todo if query.is_empty() => "Tasks".to_string(),
                    AgendaBlockKind::Todo => format!("Tasks: {query}"),
                    AgendaBlockKind::Tags => format!("Match: {query}"),
                    AgendaBlockKind::TagsTodo => format!("Tasks matching: {query}"),
                    AgendaBlockKind::Search => format!("Search: {query}"),
                    AgendaBlockKind::Stuck => "Stuck projects".to_string(),
                }
            });
            lines.push(AgendaLine::heading(title));
        }
        lines.extend(block_lines(editor, block)?);
    }
    Ok(lines)
}

/// An entry as a line of a list that is not by day: `when` says what the
/// first column shows.
fn entry_line(entry: &helix_roam::Entry, when: String) -> AgendaLine {
    AgendaLine {
        when,
        what: agenda_title(entry),
        path: entry.file_path.clone(),
        line: entry.line,
        tags: entry.all_tags().cloned().collect(),
        category: entry.category.clone(),
        effort: entry.property("effort").and_then(effort_minutes),
        title: entry.title.clone(),
        todo: entry.todo.as_ref().map(|state| state.keyword.clone()),
        properties: entry.properties.clone(),
        priority: entry.priority,
        clocked: {
            let now = now_moment();
            entry
                .clocks
                .iter()
                .map(|&(start, end)| (end.unwrap_or(now) - start).max(0))
                .sum()
        },
        ..AgendaLine::default()
    }
}

/// How an agenda view is written to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgendaExport {
    Text,
    Html,
    ICalendar,
}

impl AgendaExport {
    /// By the file's extension: `.html`, `.ics`, anything else as text.
    pub fn for_path(path: &Path) -> Self {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some(ext) if ext.eq_ignore_ascii_case("html") || ext.eq_ignore_ascii_case("htm") => {
                Self::Html
            }
            Some(ext) if ext.eq_ignore_ascii_case("ics") => Self::ICalendar,
            _ => Self::Text,
        }
    }
}

/// The lines of an agenda view as a file: plain text as shown, an HTML
/// page, or an iCalendar of its dated entries (and its undated tasks).
pub fn agenda_export(lines: &[AgendaLine], title: &str, format: AgendaExport) -> String {
    match format {
        AgendaExport::Text => {
            let width = lines
                .iter()
                .map(|line| line.when.chars().count())
                .max()
                .unwrap_or(0);
            let mut out = format!("{title}\n\n");
            for line in lines {
                let pad = width - line.when.chars().count();
                let row = format!("{}{}  {}", line.when, " ".repeat(pad), line.what);
                out.push_str(row.trim_end());
                out.push('\n');
            }
            out
        }
        AgendaExport::Html => {
            let escape = |text: &str| {
                text.replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
                    .replace('"', "&quot;")
            };
            let mut out = format!(
                "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>{0}</title>\n\
                 <style>body{{font-family:sans-serif}}td{{padding:0 1em}}tr.heading td{{font-weight:bold;padding-top:1em}}</style>\n\
                 </head>\n<body>\n<h1>{0}</h1>\n<table>\n",
                escape(title)
            );
            for line in lines {
                if line.is_heading() {
                    out.push_str(&format!(
                        "<tr class=\"heading\"><td colspan=\"2\">{}</td></tr>\n",
                        escape(line.when.trim_start_matches("── "))
                    ));
                } else {
                    out.push_str(&format!(
                        "<tr><td>{}</td><td>{}</td></tr>\n",
                        escape(line.when.trim()),
                        escape(&line.what)
                    ));
                }
            }
            out.push_str("</table>\n</body>\n</html>\n");
            out
        }
        AgendaExport::ICalendar => {
            // Commas, semicolons and backslashes are escaped in text values.
            let escape = |text: &str| {
                text.replace('\\', "\\\\")
                    .replace(',', "\\,")
                    .replace(';', "\\;")
            };
            let stamp = {
                let (date, time) = now();
                format!(
                    "{}T{:02}{:02}00Z",
                    date.to_iso().replace('-', ""),
                    time.hour,
                    time.minute
                )
            };
            let mut out = String::from(
                "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//neohelix//agenda//EN\r\n",
            );
            let mut seen = std::collections::BTreeSet::new();
            for line in lines.iter().filter(|line| !line.is_heading()) {
                let uid = format!(
                    "{}-{}-{}@neohelix",
                    line.path
                        .display()
                        .to_string()
                        .replace(['/', '\\', ' '], "_"),
                    line.line,
                    line.day.map(|day| day.to_iso()).unwrap_or_default()
                );
                if !seen.insert(uid.clone()) {
                    continue;
                }
                match line.day {
                    Some(day) => {
                        let date = day.to_iso().replace('-', "");
                        let start = match line.time {
                            Some((hour, minute)) => {
                                format!("DTSTART:{date}T{hour:02}{minute:02}00")
                            }
                            None => format!("DTSTART;VALUE=DATE:{date}"),
                        };
                        out.push_str(&format!(
                            "BEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:{stamp}\r\n{start}\r\nSUMMARY:{}\r\nCATEGORIES:{}\r\nEND:VEVENT\r\n",
                            escape(&line.title),
                            escape(&line.category)
                        ));
                    }
                    None if line.todo.is_some() => {
                        out.push_str(&format!(
                            "BEGIN:VTODO\r\nUID:{uid}\r\nDTSTAMP:{stamp}\r\nSUMMARY:{}\r\nCATEGORIES:{}\r\nEND:VTODO\r\n",
                            escape(&line.title),
                            escape(&line.category)
                        ));
                    }
                    None => {}
                }
            }
            out.push_str("END:VCALENDAR\r\n");
            out
        }
    }
}

/// An entry's title as an agenda shows it: state, priority, title, then
/// its own tags.
fn agenda_title(entry: &helix_roam::Entry) -> String {
    let mut out = String::new();
    if let Some(state) = &entry.todo {
        out.push_str(&state.keyword);
        out.push(' ');
    }
    if let Some(priority) = entry.priority {
        out.push_str(&format!("[#{priority}] "));
    }
    out.push_str(&entry.title);
    if !entry.tags.is_empty() {
        out.push_str(&format!("  :{}:", entry.tags.join(":")));
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

/// Inserts a list item after the one at the cursor.
pub fn list_insert_item(editor: &mut Editor) {
    let (text, line) = text_and_line(editor);
    match helix_roam::list::insert_item(&text, line) {
        Some((after, at)) => {
            // The cursor goes to the end of the new item, where typing
            // continues, rather than to its start.
            apply_and_go(editor, "Inserted an item".to_string(), after, at);
            let view = view!(editor).id;
            let doc = doc_mut!(editor);
            let end = doc
                .text()
                .line(at.min(doc.text().len_lines() - 1))
                .len_chars();
            let at_char = doc.text().line_to_char(at) + end.saturating_sub(1);
            doc.set_selection(view, helix_core::Selection::point(at_char));
        }
        None => editor.set_error("No list item at the cursor"),
    }
}

/// Renumbers the ordered list at the cursor.
pub fn list_renumber(editor: &mut Editor) {
    let (text, line) = text_and_line(editor);
    apply_to_buffer(
        editor,
        "Renumbered".to_string(),
        helix_roam::list::renumber(&text, line),
    );
}

/// Moves a list item and its children in or out a level.
fn shift_item(editor: &mut Editor, deeper: bool) {
    let (text, line) = text_and_line(editor);
    match helix_roam::list::shift_item(&text, line, deeper) {
        Some(after) => apply_to_buffer(editor, "Moved the item".to_string(), after),
        None => editor.set_error("The item cannot move that way"),
    }
}

pub fn list_demote_item(editor: &mut Editor) {
    shift_item(editor, true);
}

pub fn list_promote_item(editor: &mut Editor) {
    shift_item(editor, false);
}

/// Ticks or unticks the checkbox at the cursor.
pub fn toggle_checkbox(editor: &mut Editor) {
    let (text, line) = text_and_line(editor);
    match helix_roam::list::toggle_checkbox(&text, line) {
        Some(after) => apply_to_buffer(editor, "Toggled".to_string(), after),
        None => editor.set_error("No list item at the cursor"),
    }
}

/// Brings every `[n/m]` and `[p%]` cookie in the buffer up to date.
pub fn update_cookies(editor: &mut Editor) {
    let (text, _) = text_and_line(editor);
    apply_to_buffer(
        editor,
        "Updated the cookies".to_string(),
        helix_roam::list::update_cookies(&text),
    );
}

/// Applies a table operation, reporting when there is no table.
fn table_op(
    editor: &mut Editor,
    done: &'static str,
    op: impl FnOnce(&str, usize) -> Option<String>,
) {
    let (text, line) = text_and_line(editor);
    match op(&text, line) {
        Some(after) => apply_to_buffer(editor, done.to_string(), after),
        None => editor.set_error("No Org table at the cursor"),
    }
}

/// Realigns the table at the cursor.
pub fn table_align(editor: &mut Editor) {
    table_op(editor, "Aligned the table", helix_roam::table::align);
}

/// Inserts a row below the cursor's.
pub fn table_insert_row(editor: &mut Editor) {
    table_op(editor, "Inserted a row", helix_roam::table::insert_row);
}

/// Inserts a separator below the cursor's row.
pub fn table_insert_separator(editor: &mut Editor) {
    table_op(
        editor,
        "Inserted a separator",
        helix_roam::table::insert_separator,
    );
}

/// Removes the cursor's row.
pub fn table_delete_row(editor: &mut Editor) {
    table_op(editor, "Removed the row", helix_roam::table::delete_row);
}

/// The column the cursor sits in, from the pipes before it.
fn cursor_column(editor: &Editor) -> usize {
    let (view, doc) = current_ref!(editor);
    let text = doc.text();
    let cursor = doc.selection(view.id).primary().cursor(text.slice(..));
    let line = text.char_to_line(cursor);
    let start = text.line_to_char(line);
    let within = text.char_to_byte(cursor) - text.char_to_byte(start);

    helix_roam::table::column_at(&text.line(line).to_string(), within)
}

/// Inserts a column at the cursor's.
pub fn table_insert_column(editor: &mut Editor) {
    let column = cursor_column(editor);
    table_op(editor, "Inserted a column", |text, line| {
        helix_roam::table::insert_column(text, line, column)
    });
}

/// Removes the cursor's column.
pub fn table_delete_column(editor: &mut Editor) {
    let column = cursor_column(editor);
    table_op(editor, "Removed the column", |text, line| {
        helix_roam::table::delete_column(text, line, column)
    });
}

/// Moves the cursor to the next or previous cell, realigning first.
///
/// Realigning first is what makes the jump land where the eye expects: an
/// edited cell has usually changed the column widths.
pub fn table_move_cell(editor: &mut Editor, forward: bool) {
    let (text, line) = text_and_line(editor);
    let Some(aligned) = helix_roam::table::align(&text, line) else {
        editor.set_error("No Org table at the cursor");
        return;
    };

    let column = cursor_column(editor);
    apply_to_buffer(editor, "Moved".to_string(), aligned);

    let view = view!(editor).id;
    let doc = doc_mut!(editor);
    let row = doc
        .text()
        .line(line.min(doc.text().len_lines() - 1))
        .to_string();

    // The cell after (or before) the one the cursor was in, found by counting
    // pipes rather than by guessing at widths.
    let pipes: Vec<usize> = row
        .char_indices()
        .filter(|(_, c)| *c == '|')
        .map(|(i, _)| i)
        .collect();
    let wanted = if forward {
        column + 1
    } else {
        column.saturating_sub(1)
    };

    if let Some(open) = pipes.get(wanted) {
        let byte = doc.text().line_to_byte(line) + open + 2;
        let at = doc.text().byte_to_char(byte.min(doc.text().len_bytes()));
        doc.set_selection(view, helix_core::Selection::point(at));
    }
}

pub fn table_next_cell(editor: &mut Editor) {
    table_move_cell(editor, true);
}

pub fn table_previous_cell(editor: &mut Editor) {
    table_move_cell(editor, false);
}

/// Reports a recalculation, with how many fields failed.
fn recalculated(editor: &mut Editor, what: String, result: Result<(String, usize), String>) {
    match result {
        Ok((after, 0)) => apply_to_buffer(editor, what, after),
        Ok((after, errors)) => {
            apply_to_buffer(editor, what, after);
            editor.set_error(format!(
                "{errors} field{} could not be computed (#ERROR)",
                if errors == 1 { "" } else { "s" }
            ));
        }
        Err(error) => editor.set_error(error),
    }
}

/// Recalculates the table at the cursor from its `#+TBLFM:` line.
pub fn table_recalculate(editor: &mut Editor) {
    let (text, line) = text_and_line(editor);
    let result = helix_roam::formula::recalculate(&text, line);
    let what = match &result {
        Ok(done) if done.formulas == 1 => "Applied 1 formula".to_string(),
        Ok(done) => format!("Applied {} formulas", done.formulas),
        Err(_) => String::new(),
    };
    recalculated(editor, what, result.map(|done| (done.text, done.errors)));
}

/// Recalculates the table at the cursor until it stops changing.
pub fn table_iterate(editor: &mut Editor) {
    let (text, line) = text_and_line(editor);
    let result = helix_roam::formula::iterate(&text, line);
    recalculated(
        editor,
        "The table has settled".to_string(),
        result.map(|done| (done.text, done.errors)),
    );
}

/// Recalculates every table in the buffer that has formulas.
pub fn table_recalculate_all(editor: &mut Editor) {
    let (text, _) = text_and_line(editor);
    match helix_roam::formula::recalculate_all(&text) {
        Ok((_, 0, _)) => editor.set_error("No table in the buffer has a #+TBLFM: line"),
        Ok((after, tables, errors)) => recalculated(
            editor,
            format!(
                "Recalculated {tables} table{}",
                if tables == 1 { "" } else { "s" }
            ),
            Ok((after, errors)),
        ),
        Err(error) => editor.set_error(error),
    }
}

// ── Subtree clipboard, sorting and dynamic blocks ─────────────────────────

/// Copies the subtree at the cursor into the editor's Org clipboard.
pub fn copy_subtree(editor: &mut Editor) {
    let (text, line) = text_and_line(editor);

    match helix_roam::clip::copy_subtree(&text, line) {
        Some(clip) => {
            editor.set_status(format!("Copied {} lines", clip.text.lines().count()));
            editor.org_clip = Some(clip);
        }
        None => editor.set_error("No subtree at the cursor"),
    }
}

/// Removes the subtree at the cursor, keeping it for a paste.
pub fn cut_subtree(editor: &mut Editor) {
    let (text, line) = text_and_line(editor);

    match helix_roam::clip::cut_subtree(&text, line) {
        Some((after, clip)) => {
            let done = format!("Cut {} lines", clip.text.lines().count());
            editor.org_clip = Some(clip);
            apply_to_buffer(editor, done, after);
        }
        None => editor.set_error("No subtree at the cursor"),
    }
}

/// Pastes the stored subtree as a sibling of the entry at the cursor.
pub fn paste_subtree(editor: &mut Editor) {
    let Some(clip) = editor.org_clip.clone() else {
        editor.set_error("Nothing to paste; copy or cut a subtree first");
        return;
    };

    let (text, line) = text_and_line(editor);
    let (after, at) = helix_roam::clip::paste_subtree(&text, line, &clip);
    apply_and_go(editor, "Pasted the subtree".to_string(), after, at);
}

/// Clones the subtree at the cursor, from `N` or `N +1w`.
pub fn clone_subtree(editor: &mut Editor, input: &str) {
    let mut parts = input.split_whitespace();

    let Some(times) = parts.next().and_then(|n| n.parse().ok()).filter(|n| *n > 0) else {
        editor.set_error("Give a number of copies, e.g. `3` or `3 +1w`");
        return;
    };

    let shift = match parts.next().map(helix_roam::clip::parse_shift) {
        Some(Ok(shift)) => Some(shift),
        Some(Err(err)) => {
            editor.set_error(err.to_string());
            return;
        }
        None => None,
    };

    let (text, line) = text_and_line(editor);
    match helix_roam::clip::clone_subtree(&text, line, times, shift) {
        Ok(after) => apply_to_buffer(editor, format!("Cloned {times} times"), after),
        Err(err) => editor.set_error(err.to_string()),
    }
}

/// Reads `deadline` or `-deadline` into a key and a direction.
fn sort_request(editor: &mut Editor, input: &str) -> Option<(helix_roam::SortKey, bool)> {
    let input = input.trim();
    // A leading `-` reverses, which is shorter to type than a second command
    // and reads the way a descending sort is written elsewhere.
    let (reverse, name) = match input.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, input),
    };

    match helix_roam::SortKey::parse(name) {
        Some(key) => Some((key, reverse)),
        None => {
            editor.set_error(format!(
                "Sort by one of: {}",
                helix_roam::SortKey::names().join(", ")
            ));
            None
        }
    }
}

/// Sorts the children of the entry at the cursor.
pub fn sort_entries(editor: &mut Editor, input: &str) {
    let Some((key, reverse)) = sort_request(editor, input) else {
        return;
    };

    let (text, line) = text_and_line(editor);
    match helix_roam::sort::sort_entries(&text, line, &key, reverse) {
        Ok(after) => apply_to_buffer(editor, "Sorted the entries".to_string(), after),
        Err(err) => editor.set_error(err.to_string()),
    }
}

/// Sorts the list items that are siblings of the one at the cursor.
pub fn sort_list(editor: &mut Editor, input: &str) {
    let Some((key, reverse)) = sort_request(editor, input) else {
        return;
    };

    let (text, line) = text_and_line(editor);
    match helix_roam::sort::sort_list(&text, line, &key, reverse) {
        Some(after) => apply_to_buffer(editor, "Sorted the list".to_string(), after),
        None => editor.set_error("No list with siblings to sort at the cursor"),
    }
}

/// Sorts the table rows around the cursor by the column it is in.
pub fn sort_table(editor: &mut Editor, input: &str) {
    let Some((key, reverse)) = sort_request(editor, input) else {
        return;
    };

    let column = cursor_column(editor);
    let (text, line) = text_and_line(editor);
    match helix_roam::sort::sort_table(&text, line, column, &key, reverse) {
        Some(after) => apply_to_buffer(editor, format!("Sorted by column {}", column + 1), after),
        None => editor.set_error("No table rows to sort at the cursor"),
    }
}

/// The dynamic-block generators this build knows about.
///
/// Returning `None` for an unknown name is what lets a buffer hold a block
/// this fork cannot write yet without the refresh destroying its contents.
fn generate(text: &str, block: &helix_roam::dynamic::DynamicBlock) -> Option<Vec<String>> {
    match block.name.to_ascii_lowercase().as_str() {
        "columnview" => Some(helix_roam::dynamic::columnview(text, block)),
        "clocktable" => {
            let (today, time) = now();
            Some(helix_roam::clock::clocktable(
                text,
                block,
                today,
                helix_roam::clock::moment(today, time),
            ))
        }
        _ => None,
    }
}

/// Regenerates the dynamic block at the cursor.
pub fn dblock_update(editor: &mut Editor) {
    let (text, line) = text_and_line(editor);

    match helix_roam::dynamic::refresh(&text, line, generate) {
        Some(after) => apply_to_buffer(editor, "Updated the block".to_string(), after),
        None => editor.set_error("No dynamic block at the cursor this build can write"),
    }
}

/// Regenerates every dynamic block in the buffer.
pub fn dblock_update_all(editor: &mut Editor) {
    let (text, _) = text_and_line(editor);
    let (after, done) = helix_roam::dynamic::refresh_all(&text, generate);

    if done == 0 {
        editor.set_error("No dynamic block in this buffer this build can write");
        return;
    }
    apply_to_buffer(editor, format!("Updated {done} blocks"), after);
}

// ── Outline navigation, sparse trees and narrowing ────────────────────────

/// Moves the cursor to a heading found by `find`, reporting when there is none.
fn goto_heading(
    editor: &mut Editor,
    what: &'static str,
    find: impl Fn(&[helix_roam::outline::Entry], usize) -> Option<usize>,
) {
    let (text, line) = text_and_line(editor);
    let entries = helix_roam::outline::headings(&text);

    match find(&entries, line) {
        Some(target) => {
            let byte = doc!(editor).text().line_to_byte(target);
            jump_to_byte(editor, byte);
        }
        None => editor.set_error(format!("No {what} heading")),
    }
}

pub fn goto_next_heading(editor: &mut Editor) {
    goto_heading(editor, "next", helix_roam::outline::next);
}

pub fn goto_previous_heading(editor: &mut Editor) {
    goto_heading(editor, "previous", helix_roam::outline::previous);
}

pub fn goto_next_sibling_heading(editor: &mut Editor) {
    goto_heading(editor, "next sibling", helix_roam::outline::next_sibling);
}

pub fn goto_previous_sibling_heading(editor: &mut Editor) {
    goto_heading(
        editor,
        "previous sibling",
        helix_roam::outline::previous_sibling,
    );
}

pub fn goto_parent_heading(editor: &mut Editor) {
    goto_heading(editor, "parent", helix_roam::outline::parent);
}

/// Shows the path from the top of the file down to the entry at the cursor.
pub fn show_outline_path(editor: &mut Editor) {
    let (text, line) = text_and_line(editor);
    let entries = helix_roam::outline::headings(&text);
    let path = helix_roam::outline::outline_path(&entries, line);

    if path.is_empty() {
        editor.set_status("Above every heading");
        return;
    }
    editor.set_status(path.join(" / "));
}

/// Every headline in the buffer, for a picker to choose from.
pub fn buffer_headings(editor: &Editor) -> Vec<(usize, String)> {
    let text = doc!(editor).text().to_string();

    helix_roam::outline::headings(&text)
        .into_iter()
        .map(|entry| {
            // Indent by depth so the list reads as the outline it is.
            let title = format!("{}{}", "  ".repeat(entry.level - 1), entry.title);
            (entry.line, title)
        })
        .collect()
}

/// Puts the cursor on a heading chosen from the picker.
pub fn goto_heading_line(editor: &mut Editor, line: usize) {
    let byte = doc!(editor).text().line_to_byte(line);
    jump_to_byte(editor, byte);
}

/// Replaces the buffer's folds with the ones `ranges` asks for.
fn fold_ranges(editor: &mut Editor, ranges: Vec<(usize, usize)>) -> usize {
    let doc = doc_mut!(editor);
    doc.folds_mut().clear();

    for (start, end) in &ranges {
        doc.folds_mut()
            .insert(helix_core::fold::Fold::new(*start, *end));
    }
    // Out of what was just hidden, or the next redraw would open it again.
    crate::commands::reveal_cursors(editor);
    ranges.len()
}

/// Hides everything but the entries matching `input` and the path to them.
pub fn sparse_tree(editor: &mut Editor, input: &str) {
    let filter = helix_roam::outline::Filter::parse(input);
    if filter.is_empty() {
        editor.set_error("Give something to match: `TODO`, `:work:`, `#A` or `/text`");
        return;
    }

    let text = doc!(editor).text().to_string();
    let ranges = helix_roam::outline::sparse_tree(&text, &filter);
    let hidden = fold_ranges(editor, ranges);

    if hidden == 0 {
        editor.set_status("Everything matches");
        return;
    }
    editor.set_status(format!("Sparse tree: {hidden} ranges hidden"));
}

/// Hides everything outside the subtree at the cursor.
pub fn narrow_to_subtree(editor: &mut Editor) {
    let (text, line) = text_and_line(editor);
    let ranges = helix_roam::outline::narrow(&text, line);

    if ranges.is_empty() {
        editor.set_error("No subtree at the cursor to narrow to");
        return;
    }
    fold_ranges(editor, ranges);
    editor.set_status("Narrowed to the subtree");
}

/// The folds `#+STARTUP:` asks for, replacing whatever is folded now.
///
/// Returns how many ranges it hid. Used when a file opens and when the user
/// asks to get back to how the file opened, which is Org's `C-u C-u TAB`.
pub fn apply_startup_folds(doc: &mut helix_view::Document) -> usize {
    let text = doc.text().to_string();
    let settings = settings_of(doc);
    let startup = helix_roam::startup::Startup::of(&settings);
    let ranges = helix_roam::startup::opening_ranges(&text, &startup);

    let folds = doc.folds_mut();
    folds.clear();
    for (start, end) in &ranges {
        folds.insert(helix_core::fold::Fold::new(*start, *end));
    }
    ranges.len()
}

/// Puts the buffer back the way `#+STARTUP:` says it opens.
pub fn startup_visibility(editor: &mut Editor) {
    let hidden = apply_startup_folds(doc_mut!(editor));
    crate::commands::reveal_cursors(editor);
    if hidden == 0 {
        editor.set_status("This file opens with everything showing");
    } else {
        editor.set_status(format!(
            "Back to how the file opens: {hidden} ranges hidden"
        ));
    }
}

/// Brings back everything a narrowing or a sparse tree hid.
pub fn widen(editor: &mut Editor) {
    let doc = doc_mut!(editor);
    if doc.folds().is_empty() {
        editor.set_status("Nothing is hidden");
        return;
    }

    doc.folds_mut().clear();
    editor.set_status("Widened");
}

// ── Emphasis, blocks, footnotes and citations ─────────────────────────────

/// The cursor as a character index, which is what the markup layer counts in.
fn cursor_char(editor: &Editor) -> usize {
    let (view, doc) = current_ref!(editor);
    doc.selection(view.id)
        .primary()
        .cursor(doc.text().slice(..))
}

/// The primary selection as character indices.
fn selection_chars(editor: &Editor) -> (usize, usize) {
    let (view, doc) = current_ref!(editor);
    let range = doc.selection(view.id).primary();
    (range.from(), range.to())
}

/// Wraps or unwraps the selection in an emphasis marker.
pub fn toggle_emphasis(editor: &mut Editor, input: &str) {
    let Some(kind) = helix_roam::markup::Emphasis::parse(input) else {
        editor.set_error(format!(
            "Emphasise one of: {}",
            helix_roam::markup::Emphasis::names().join(", ")
        ));
        return;
    };

    let (from, to) = selection_chars(editor);
    let text = doc!(editor).text().to_string();

    match helix_roam::markup::toggle_emphasis(&text, from, to, kind) {
        Some(after) => apply_to_buffer(editor, format!("Toggled {input}"), after),
        None => editor.set_error("Select something to emphasise first"),
    }
}

/// Inserts a structure block, wrapping the selection when there is one.
pub fn insert_block(editor: &mut Editor, input: &str) {
    let mut words = input.trim().splitn(2, char::is_whitespace);
    let Some(name) = words.next().filter(|name| !name.is_empty()) else {
        editor.set_error(format!(
            "Name a block: {}",
            helix_roam::markup::block_names().join(", ")
        ));
        return;
    };
    let argument = words.next();

    let (from, to) = selection_chars(editor);
    let text = doc!(editor).text().clone();
    let first = text.char_to_line(from);
    // A selection's end is one past its last character, so on a range ending
    // at a line break it names the line below.
    let last = text.char_to_line(to.saturating_sub(1).max(from));
    let source = text.to_string();

    // A cursor is a one-character selection in Helix, so "nothing selected"
    // is a span of at most one character rather than an empty one.
    let (after, line) = if to.saturating_sub(from) <= 1 {
        helix_roam::markup::insert_block(&source, first, name, argument)
    } else {
        helix_roam::markup::wrap_block(&source, first, last, name, argument)
    };

    apply_and_go(editor, format!("Inserted a {name} block"), after, line);
}

/// Adds a footnote and puts the cursor where its text goes.
pub fn footnote_new(editor: &mut Editor) {
    let offset = cursor_char(editor);
    let text = doc!(editor).text().to_string();
    let (after, at) = helix_roam::markup::insert_footnote(&text, offset);

    let before = doc!(editor).text().clone();
    let rope = helix_core::Rope::from(after.as_str());
    let transaction = helix_core::diff::compare_ropes(&before, &rope);
    let view = view!(editor).id;
    doc_mut!(editor).apply(&transaction, view);

    let doc = doc_mut!(editor);
    let at = at.min(doc.text().len_chars());
    doc.set_selection(view, helix_core::Selection::point(at));
    editor.set_status("Added a footnote");
}

/// Jumps between a footnote's reference and its definition.
pub fn footnote_goto(editor: &mut Editor) {
    let offset = cursor_char(editor);
    let (text, line) = text_and_line(editor);

    let Some(label) = helix_roam::markup::footnote_at(&text, offset) else {
        editor.set_error("No footnote at the cursor");
        return;
    };

    // Whichever end the cursor is not on.
    let here = helix_roam::markup::footnote_definition(&text, &label);
    let target = if here == Some(line) {
        helix_roam::markup::footnote_reference(&text, &label)
    } else {
        here
    };

    match target {
        Some(target) => {
            let byte = doc!(editor).text().line_to_byte(target);
            jump_to_byte(editor, byte);
        }
        None => editor.set_error(format!("[fn:{label}] has only one end")),
    }
}

/// Renumbers the numeric footnotes in reference order.
pub fn footnote_renumber(editor: &mut Editor) {
    let text = doc!(editor).text().to_string();
    let after = helix_roam::markup::renumber_footnotes(&text);
    apply_to_buffer(editor, "Renumbered the footnotes".to_string(), after);
}

/// The bibliography files this buffer declares, resolved against it.
fn bibliography_files(editor: &Editor) -> Vec<PathBuf> {
    let doc = doc!(editor);
    let settings = settings_of(doc);
    let beside = doc
        .path()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| notes_directory(editor));

    settings
        .bibliography
        .iter()
        .map(|name| {
            let path = Path::new(name.trim());
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                beside.join(path)
            }
        })
        .collect()
}

/// Every key the buffer could cite: its bibliographies', plus the ones the
/// graph has already seen.
///
/// Both, because a notes directory often cites keys that no `.bib` in it
/// declares — the bibliography lives with the paper, not with the notes.
pub fn citation_keys(editor: &Editor) -> Vec<String> {
    let mut keys: Vec<String> = bibliography_files(editor)
        .into_iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .flat_map(|bib| helix_roam::markup::bib_keys(&bib))
        .collect();

    keys.extend(editor.roam.read().citation_keys().map(str::to_string));
    keys.sort();
    keys.dedup();
    keys
}

/// Puts a `[cite:@key]` at the cursor.
pub fn insert_citation(editor: &mut Editor, key: &str) {
    let key = key.trim();
    if key.is_empty() {
        editor.set_error("Give a citation key");
        return;
    }

    let offset = cursor_char(editor);
    let text = doc!(editor).text().to_string();
    let after = helix_roam::markup::insert_citation(&text, offset, key);
    apply_to_buffer(editor, format!("Cited {key}"), after);
}

/// Opens the bibliography at the entry the cursor cites.
pub fn follow_citation(editor: &mut Editor) {
    let offset = cursor_char(editor);
    let text = doc!(editor).text().to_string();

    let Some(key) = helix_roam::markup::citation_at(&text, offset) else {
        editor.set_error("No citation at the cursor");
        return;
    };

    let needle = format!("{{{key}");
    for path in bibliography_files(editor) {
        let Ok(bib) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(line) = bib
            .lines()
            .position(|l| l.trim_start().starts_with('@') && l.contains(&needle))
        else {
            continue;
        };

        if let Err(err) = editor.open(&path, helix_view::editor::Action::Replace) {
            editor.set_error(format!("Failed to open '{}': {err}", path.display()));
            return;
        }
        let byte = doc!(editor).text().line_to_byte(line);
        jump_to_byte(editor, byte);
        editor.set_status(format!("Found {key}"));
        return;
    }

    editor.set_error(format!("No bibliography here defines {key}"));
}

// ── The Roam panel: pinning, unlinked refs and diagnosis ──────────────────

/// The node the cursor is in, by the buffer rather than by the index.
///
/// The buffer is usually ahead of the index — a heading given an `:ID:` a
/// moment ago is not a node until the file is saved — so the id comes from
/// the text and only then is looked up.
fn node_at_cursor(editor: &Editor) -> Option<(helix_roam::Uuid, String)> {
    let (text, line) = text_and_line(editor);
    helix_roam::restructure::entry_at(&text, line)
}

/// Pins the panel to the node at the cursor.
pub fn pin_node(editor: &mut Editor) {
    let Some((id, title)) = node_at_cursor(editor) else {
        editor.set_error("No node at the cursor; give it an :ID: first");
        return;
    };

    editor.roam_pinned = Some(id);
    editor.set_status(format!("Pinned {title}"));
}

/// Lets the panel go back to following the cursor alone.
pub fn unpin_node(editor: &mut Editor) {
    match editor.roam_pinned.take() {
        Some(_) => editor.set_status("Unpinned"),
        None => editor.set_status("Nothing was pinned"),
    }
}

/// Puts the unlinked references of the node at the cursor into the panel.
pub fn cache_unlinked(editor: &mut Editor, found: &[Unlinked]) {
    let Some((id, _)) = node_at_cursor(editor) else {
        return;
    };

    editor.roam_unlinked = Some(helix_view::editor::RoamUnlinked {
        node: id,
        entries: found
            .iter()
            .map(|reference| {
                let path = helix_stdx::path::get_relative_path(&reference.path);
                (
                    reference.text.trim().to_string(),
                    format!("{}:{}", path.display(), reference.line + 1),
                )
            })
            .collect(),
    });
}

/// Reports what the index believes about the node at the cursor.
///
/// The only way to tell a parser bug from a malformed drawer: the buffer says
/// one thing, the index says another, and until you can see both you are
/// guessing which one is wrong.
pub fn diagnose_node(editor: &mut Editor) {
    let Some((id, title)) = node_at_cursor(editor) else {
        editor.set_error("No node at the cursor; give it an :ID: first");
        return;
    };

    let modified = doc!(editor).is_modified();
    let graph = editor.roam.read();
    let Some(node) = graph.get_node(&id) else {
        drop(graph);
        let why = if modified {
            "this buffer has unsaved changes, and the index reads files"
        } else {
            "run :roam-reindex, or check the :ID: drawer"
        };
        editor.set_error(format!("The index has no node {id}: {why}"));
        return;
    };

    let none = "—".to_string();
    let join = |values: &[String]| {
        if values.is_empty() {
            none.clone()
        } else {
            values.join(", ")
        }
    };

    let backlinks = graph.get_backlinks(&id);
    let ids = backlinks.iter().filter(|(_, link)| link.is_id()).count();
    let body = vec![
        ("id", id.to_string()),
        ("title in buffer", title),
        ("title in index", node.title.clone()),
        ("file", node.file_path.display().to_string()),
        ("line", (node.line + 1).to_string()),
        ("level", node.level.to_string()),
        ("aliases", join(&node.aliases)),
        ("tags", join(&node.tags)),
        ("outline path", join(&node.outline_path)),
        (
            "backlinks",
            format!("{ids} id, {} ref", backlinks.len() - ids),
        ),
        ("links out", graph.get_forward_links(&id).len().to_string()),
        (
            "properties",
            node.properties
                .iter()
                .map(|(key, _)| key.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        ),
        (
            "buffer",
            if modified {
                "modified since the index last read it".to_string()
            } else {
                "matches what the index read".to_string()
            },
        ),
    ];
    drop(graph);

    editor.autoinfo = Some(helix_view::info::Info::new("Node at the cursor", &body));
}

/// Every node in the buffer, as the headline it belongs to rather than the
/// `:ID:` line that declares it.
///
/// One pass: the index knows where a node's `:ID:` is, but a count drawn on
/// the drawer line would sit under the headline it is about.
fn nodes_by_headline(text: &str) -> Vec<(usize, helix_roam::Uuid)> {
    let mut found = Vec::new();
    let mut headline = None;

    for (line, raw) in text.lines().enumerate() {
        if raw.starts_with('*') && raw.trim_start_matches('*').starts_with([' ', '\t']) {
            headline = Some(line);
            continue;
        }

        let trimmed = raw.trim();
        let Some(rest) = trimmed
            .strip_prefix(":ID:")
            .or_else(|| trimmed.strip_prefix(":id:"))
        else {
            continue;
        };
        if let Ok(id) = rest.trim().parse::<helix_roam::Uuid>() {
            // A file-level `:ID:` sits above every headline and belongs to
            // the file, which has no headline to be drawn beside.
            if let Some(at) = headline {
                found.push((at, id));
            }
        }
    }

    found
}

/// Draws each headline's backlink count beside it, or clears them if shown.
///
/// Toggling rather than always on: the count is worth seeing while writing
/// about how notes connect, and noise the rest of the time.
///
/// What is drawn is a snapshot. `:roam-reindex` refreshes it, because that
/// command already waits on the rebuild and gets the editor back afterwards.
/// A save does not: re-indexing one file is spawned and forgotten, with no
/// editor to return to. Reading the graph every frame instead would cost a
/// lock and a scan of the buffer sixty times a second. Switching the counts
/// off and on again re-reads them.
pub fn toggle_backlink_counts(editor: &mut Editor) {
    if !doc!(editor).roam_counts.is_empty() {
        doc_mut!(editor).roam_counts.clear();
        editor.set_status("Backlink counts off");
        return;
    }

    refresh_backlink_counts(editor);
    if doc!(editor).roam_counts.is_empty() {
        editor.set_status("No node in this buffer has a backlink");
    } else {
        editor.set_status("Backlink counts on");
    }
}

/// Recomputes the counts the buffer is already showing.
pub fn refresh_backlink_counts(editor: &mut Editor) {
    let text = doc!(editor).text().clone();
    let nodes = nodes_by_headline(&text.to_string());

    let counts: Vec<_> = {
        let graph = editor.roam.read();
        nodes
            .into_iter()
            .filter_map(|(line, id)| {
                let count = graph.get_backlinks(&id).len();
                if count == 0 || line + 1 >= text.len_lines() {
                    return None;
                }
                // At the line's end, before its newline, so the count reads as
                // part of the headline rather than as the next line's start.
                let at = text.line_to_char(line + 1) - 1;
                Some(helix_core::text_annotations::InlineAnnotation::new(
                    at,
                    format!(" ←{count}"),
                ))
            })
            .collect()
    };

    doc_mut!(editor).roam_counts = counts;
}

// ── Index maintenance and inspection ──────────────────────────────────────

/// A row of the index browser.
pub struct IndexRow {
    pub kind: &'static str,
    pub what: String,
    pub location: String,
    pub path: PathBuf,
    pub line: usize,
}

/// Everything the index holds, as rows to look through.
///
/// Nodes, the refs that reach them and the citations they make, in one list:
/// telling a parser bug from a malformed file means seeing what was read, and
/// three separate views would hide the case where one of them is empty.
pub fn index_rows(editor: &Editor) -> Vec<IndexRow> {
    let graph = editor.roam.read();
    let mut rows = Vec::new();

    for node in graph.nodes() {
        let path = node.file_path.clone();
        let location = format!(
            "{}:{}",
            helix_stdx::path::get_relative_path(&path).display(),
            node.line + 1
        );
        let links = graph.get_forward_links(&node.id).len();
        let backlinks = graph.get_backlinks(&node.id).len();

        rows.push(IndexRow {
            kind: "node",
            what: format!("{} ({backlinks} in, {links} out)", node.title),
            location,
            path,
            line: node.line,
        });
    }

    for (key, node) in graph.refs() {
        rows.push(IndexRow {
            kind: "ref",
            what: format!("{key} → {}", node.title),
            location: helix_stdx::path::get_relative_path(&node.file_path)
                .display()
                .to_string(),
            path: node.file_path.clone(),
            line: node.line,
        });
    }

    let keys: Vec<String> = graph.citation_keys().map(str::to_string).collect();
    for key in keys {
        for node in graph.cited_by(&key) {
            rows.push(IndexRow {
                kind: "cite",
                what: format!("{key} ← {}", node.title),
                location: helix_stdx::path::get_relative_path(&node.file_path)
                    .display()
                    .to_string(),
                path: node.file_path.clone(),
                line: node.line,
            });
        }
    }

    rows.sort_by(|a, b| a.kind.cmp(b.kind).then(a.what.cmp(&b.what)));
    rows
}

/// Reports the fork's own state, so a bug report can carry it.
pub fn report_state(editor: &mut Editor) {
    let config = editor.config();
    let directory = config.roam.directory();
    let enabled = config.roam.enable;
    let agenda = config.roam.agenda_files.len();

    let (nodes, links, refs, locations, pending, citations) = {
        let graph = editor.roam.read();
        (
            graph.node_count(),
            graph
                .nodes()
                .map(|n| graph.get_forward_links(&n.id).len())
                .sum::<usize>(),
            graph.refs().count(),
            graph.location_count(),
            graph.pending_link_count(),
            graph.citation_keys().count(),
        )
    };

    let (files, unreadable) = helix_roam::scanner::collect_org_files(&directory);
    let body = vec![
        ("neohelix", env!("CARGO_PKG_VERSION").to_string()),
        ("indexing", if enabled { "on" } else { "off" }.to_string()),
        ("notes directory", directory.display().to_string()),
        (
            "org files there",
            format!(
                "{}{}",
                files.len(),
                if unreadable > 0 {
                    format!(" ({unreadable} unreadable)")
                } else {
                    String::new()
                }
            ),
        ),
        ("nodes", nodes.to_string()),
        ("links", links.to_string()),
        ("refs", refs.to_string()),
        ("citation keys", citations.to_string()),
        ("ids outside the index", locations.to_string()),
        // Links whose target has not been seen yet. A number that stays high
        // after a rebuild means links pointing outside the notes directory.
        ("unresolved links", pending.to_string()),
        (
            "agenda files",
            if agenda == 0 {
                "the whole notes directory".to_string()
            } else {
                format!("{agenda} configured")
            },
        ),
    ];

    editor.autoinfo = Some(helix_view::info::Info::new("Org-Roam state", &body));
}

// ── Source blocks ─────────────────────────────────────────────────────────

/// The source blocks of the focused buffer and the line the cursor is on.
fn source_blocks(editor: &Editor) -> (String, usize, Vec<helix_roam::source::SourceBlock>) {
    let (text, line) = text_and_line(editor);
    let blocks = helix_roam::source::blocks(&text);
    (text, line, blocks)
}

/// Moves to the next source block's `#+begin_src` line.
pub fn src_next(editor: &mut Editor) {
    let (_, line, blocks) = source_blocks(editor);
    match helix_roam::source::next_block(&blocks, line) {
        Some(block) => goto_heading_line(editor, block.begin),
        None => editor.set_status("No source block below"),
    }
}

/// Moves to the previous source block's `#+begin_src` line.
pub fn src_previous(editor: &mut Editor) {
    let (_, line, blocks) = source_blocks(editor);
    match helix_roam::source::previous_block(&blocks, line) {
        Some(block) => goto_heading_line(editor, block.begin),
        None => editor.set_status("No source block above"),
    }
}

/// From a block to its `#+RESULTS:`, or from a result back to its block.
pub fn src_result(editor: &mut Editor) {
    let (text, line, blocks) = source_blocks(editor);

    if let Some(block) = helix_roam::source::block_at(&blocks, line) {
        match helix_roam::source::result_of(&text, block) {
            Some(result) => goto_heading_line(editor, result),
            None => editor.set_status("This block has no results"),
        }
        return;
    }
    match helix_roam::source::block_of_result(&text, &blocks, line) {
        Some(block) => goto_heading_line(editor, block.begin),
        None => editor.set_error("Not in a source block or on a #+RESULTS: line"),
    }
}

/// Helix's name for an Org source block's language, where they differ.
fn helix_language(org: &str) -> String {
    match org {
        "sh" | "shell" | "zsh" => "bash",
        "emacs-lisp" | "elisp" => "elisp",
        "C" => "c",
        "C++" => "cpp",
        "js" => "javascript",
        "ts" => "typescript",
        "py" => "python",
        other => return other.to_ascii_lowercase(),
    }
    .to_string()
}

/// Opens the source block at the cursor in a buffer of its own, where the
/// language's tooling — highlighting, its language server, formatting —
/// sees ordinary code. Writing that buffer puts the code back in the block.
///
/// This is Org's `C-c '`. The buffer is a file in a temporary directory with
/// the language's extension, because that is what a language server needs to
/// take it on; the block itself is still only highlighted in place.
pub fn edit_src(editor: &mut Editor) {
    let (text, line, blocks) = source_blocks(editor);
    let Some(block) = helix_roam::source::block_at(&blocks, line).cloned() else {
        editor.set_error("Not in a source block");
        return;
    };
    let org_doc = doc!(editor).id();
    let body = helix_roam::source::body(&text, &block);
    let language = block.language.clone().unwrap_or_default();

    // A block already open for editing goes back to its buffer rather than
    // opening a second copy whose writes would fight the first.
    let existing = editor
        .org_src_edits
        .iter()
        .find(|(_, edit)| edit.org_doc == org_doc && edit.body == body)
        .map(|(path, _)| path.clone());
    if let Some(path) = existing {
        if let Err(err) = editor.open(&path, helix_view::editor::Action::VerticalSplit) {
            editor.set_error(format!("Could not reopen the block: {err}"));
        }
        return;
    }

    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join("neohelix-src").join(format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let stem = block
        .name
        .as_deref()
        .filter(|name| !name.contains(['/', '\\']))
        .unwrap_or("block");
    let extension = helix_roam::source::extension(if language.is_empty() {
        "txt"
    } else {
        &language
    });
    let path = dir.join(format!("{stem}.{extension}"));

    if let Err(err) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, &body)) {
        editor.set_error(format!("Could not create {}: {err}", path.display()));
        return;
    }
    editor.org_src_edits.insert(
        path.clone(),
        helix_view::editor::OrgSrcEdit {
            org_doc,
            begin_line: block.begin,
            body,
        },
    );

    let doc_id = match editor.open(&path, helix_view::editor::Action::VerticalSplit) {
        Ok(id) => id,
        Err(err) => {
            editor.org_src_edits.remove(&path);
            editor.set_error(format!("Could not open the block: {err}"));
            return;
        }
    };

    // An extension Helix does not know leaves the buffer without a language;
    // the block's own name may still be one it knows.
    if !language.is_empty() && doc!(editor).language_name().is_none() {
        let loader = editor.syn_loader.load();
        let set = doc_mut!(editor, &doc_id)
            .set_language_by_language_id(&helix_language(&language), &loader)
            .is_ok();
        drop(loader);
        if set {
            editor.refresh_language_servers(doc_id);
        }
    }

    editor.set_status("Editing the block: :w puts it back, :q when done");
}

/// Puts an edited block back when its editing buffer is written.
///
/// Called for every write; does nothing unless `path` is a block being
/// edited. The Org buffer is changed but not saved, as in Org: the user sees
/// the change and decides when to write it.
pub fn sync_src_edit(editor: &mut Editor, path: &Path, code: String) {
    let Some(edit) = editor.org_src_edits.get(path).cloned() else {
        return;
    };
    let Some(org) = editor.documents.get(&edit.org_doc) else {
        editor
            .set_error("The Org buffer this block came from was closed; nothing was written back");
        return;
    };

    let text = org.text().to_string();
    let blocks = helix_roam::source::blocks(&text);
    // The block whose body is still the one handed out, nearest to where it
    // was: lines above it may have moved it, and an identical block elsewhere
    // must not be the one overwritten.
    let Some(block) = blocks
        .iter()
        .filter(|block| helix_roam::source::body(&text, block) == edit.body)
        .min_by_key(|block| block.begin.abs_diff(edit.begin_line))
    else {
        editor.set_error(
            "The block changed in the Org buffer since it was opened; nothing was written back",
        );
        return;
    };

    let after = helix_roam::source::replace_body(&text, block, &code);
    let begin = block.begin;
    // What the Org buffer will now hold, which is what the next write must
    // find: the code as it reads once escaped and unescaped again.
    let written = helix_roam::source::blocks(&after)
        .into_iter()
        .find(|block| block.begin == begin)
        .map(|block| helix_roam::source::body(&after, &block))
        .unwrap_or_default();

    if let Err(err) = apply_to_document(editor, edit.org_doc, &after) {
        editor.set_error(err);
        return;
    }

    editor.org_src_edits.insert(
        path.to_path_buf(),
        helix_view::editor::OrgSrcEdit {
            org_doc: edit.org_doc,
            begin_line: begin,
            body: written,
        },
    );
    editor.set_status("Written back into the block");
}

/// Replaces the text of a document that may not be the focused one.
///
/// A transaction maps a view's selection through the change, so it needs a
/// view the document has been shown in; the change also goes into that
/// document's undo history.
fn apply_to_document(
    editor: &mut Editor,
    doc_id: helix_view::DocumentId,
    after: &str,
) -> Result<(), &'static str> {
    let doc = editor
        .documents
        .get(&doc_id)
        .ok_or("the buffer was closed")?;
    let transaction = helix_core::diff::compare_ropes(doc.text(), &helix_core::Rope::from(after));
    let view_id = editor
        .tree
        .views()
        .find(|(view, _)| view.doc == doc_id)
        .map(|(view, _)| view.id)
        .or_else(|| doc.selections().keys().next().copied())
        .ok_or("the buffer has no view to apply the change in")?;

    let doc = editor.documents.get_mut(&doc_id).unwrap();
    doc.apply(&transaction, view_id);
    if editor.tree.contains(view_id) {
        let view = editor.tree.get_mut(view_id);
        let doc = editor.documents.get_mut(&doc_id).unwrap();
        doc.append_changes_to_history(view);
    }
    Ok(())
}

/// Forgets a block's editing buffer when it closes, and removes its file.
pub fn forget_src_edit(editor: &mut Editor, path: &Path) {
    if editor.org_src_edits.remove(path).is_some() {
        let _ = std::fs::remove_file(path);
        if let Some(dir) = path.parent() {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

/// Removes every block editing file, when the editor exits.
pub fn forget_all_src_edits(editor: &mut Editor) {
    let paths: Vec<PathBuf> = editor.org_src_edits.keys().cloned().collect();
    for path in paths {
        forget_src_edit(editor, &path);
    }
}

/// Writes every block with a `:tangle` target to its file.
///
/// The buffer is tangled as it is, not as it was last saved, which is what
/// Org does too.
pub fn tangle(editor: &mut Editor) {
    let doc = doc!(editor);
    let Some(org_path) = doc.path().map(Path::to_path_buf) else {
        editor.set_error("Save the buffer first: :tangle yes names files after it");
        return;
    };
    let text = doc.text().to_string();

    let files = match helix_roam::source::tangle(&text, &org_path) {
        Ok(files) => files,
        Err(err) => {
            editor.set_error(format!("Nothing tangled: {err}"));
            return;
        }
    };
    if files.is_empty() {
        editor.set_status("Nothing to tangle: no block has a :tangle target");
        return;
    }

    let mut written = Vec::new();
    let mut failed = Vec::new();
    let mut blocks = 0;
    for file in &files {
        match write_tangled(file) {
            Ok(()) => {
                blocks += file.blocks;
                written.push(
                    file.path
                        .strip_prefix(org_path.parent().unwrap_or(Path::new("")))
                        .unwrap_or(&file.path)
                        .display()
                        .to_string(),
                );
            }
            Err(err) => failed.push(format!("{}: {err}", file.path.display())),
        }
    }

    if failed.is_empty() {
        editor.set_status(format!(
            "Tangled {blocks} block{} into {}",
            if blocks == 1 { "" } else { "s" },
            written.join(", ")
        ));
    } else {
        editor.set_error(format!(
            "Tangled {} of {} files; failed: {}",
            written.len(),
            files.len(),
            failed.join("; ")
        ));
    }
}

fn write_tangled(file: &helix_roam::source::Tangled) -> std::io::Result<()> {
    if let Some(parent) = file.path.parent().filter(|p| !p.as_os_str().is_empty()) {
        if !parent.exists() {
            if !file.mkdirp {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "its directory does not exist (add :mkdirp yes)",
                ));
            }
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(&file.path, &file.content)?;

    #[cfg(unix)]
    if file.executable {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&file.path)?.permissions();
        permissions.set_mode(permissions.mode() | 0o111);
        std::fs::set_permissions(&file.path, permissions)?;
    }
    Ok(())
}

// ── Clocking ──────────────────────────────────────────────────────────────

fn now_moment() -> helix_roam::clock::Moment {
    let (date, time) = now();
    helix_roam::clock::moment(date, time)
}

/// Where a file's text is: an open buffer, or only the file on disk.
enum ClockHome {
    Buffer(helix_view::DocumentId),
    Disk(PathBuf),
}

/// The file with the running clock, and its text, looking first at the
/// focused buffer and then at the file this editor last clocked into.
fn find_running_clock(editor: &Editor) -> Option<(ClockHome, String)> {
    let focused = doc!(editor);
    let text = focused.text().to_string();
    if helix_roam::clock::running(&text).is_some() {
        return Some((ClockHome::Buffer(focused.id()), text));
    }

    let path = editor.org_clock.as_ref()?;
    if let Some(doc) = editor.document_by_path(path) {
        let text = doc.text().to_string();
        return helix_roam::clock::running(&text)
            .is_some()
            .then(|| (ClockHome::Buffer(doc.id()), text));
    }
    let text = std::fs::read_to_string(path).ok()?;
    helix_roam::clock::running(&text)
        .is_some()
        .then(|| (ClockHome::Disk(path.clone()), text))
}

/// Writes changed text back where it came from. A file not open in the
/// editor is written on disk; an open buffer is changed and left unsaved,
/// like every other edit.
fn write_home(editor: &mut Editor, home: &ClockHome, after: &str) -> Result<(), String> {
    match home {
        ClockHome::Buffer(id) => apply_to_document(editor, *id, after).map_err(str::to_string),
        ClockHome::Disk(path) => std::fs::write(path, after)
            .map_err(|err| format!("could not write {}: {err}", path.display())),
    }
}

/// The title of the entry a running clock belongs to, for messages.
fn clock_title(text: &str) -> String {
    let settings = helix_roam::FileSettings::scan(text);
    helix_roam::clock::running(text)
        .and_then(|clock| helix_roam::clock::entry_of(text, &clock))
        .and_then(|line| text.lines().nth(line))
        .and_then(|line| helix_roam::parser::parse_headline_title(line, &settings))
        .unwrap_or_else(|| "the entry".to_string())
}

/// Stops the running clock wherever it is, returning what it said.
fn stop_running_clock(editor: &mut Editor) -> Option<Result<String, String>> {
    let (home, text) = find_running_clock(editor)?;
    let title = clock_title(&text);
    let result = helix_roam::clock::clock_out(&text, now_moment())
        .map_err(|err| err.to_string())
        .and_then(|(after, minutes)| {
            write_home(editor, &home, &after)?;
            Ok(format!(
                "Clocked out of {title}: {}",
                helix_roam::clock::format_duration(minutes)
            ))
        });
    Some(result)
}

/// Starts a clock on the entry at the cursor, stopping any other first:
/// there is one clock at a time, as in Org.
pub fn clock_in(editor: &mut Editor) {
    let stopped = match stop_running_clock(editor) {
        Some(Ok(message)) => Some(message),
        Some(Err(err)) => {
            editor.set_error(format!("The running clock could not be stopped: {err}"));
            return;
        }
        None => None,
    };

    let (text, line) = text_and_line(editor);
    match helix_roam::clock::clock_in(&text, line, now_moment()) {
        Ok(after) => {
            editor.org_clock = doc!(editor).path().map(Path::to_path_buf);
            let title = clock_title(&after);
            let done = match stopped {
                Some(stopped) => format!("{stopped}; clocked in to {title}"),
                None => format!("Clocked in to {title}"),
            };
            apply_to_buffer(editor, done, after);
        }
        Err(err) => editor.set_error(format!("Not clocked in: {err}")),
    }
}

/// Stops the running clock, in whichever file it is.
pub fn clock_out(editor: &mut Editor) {
    match stop_running_clock(editor) {
        Some(Ok(message)) => editor.set_status(message),
        Some(Err(err)) => editor.set_error(err),
        None => editor.set_error("No clock is running"),
    }
}

/// Throws the running clock away.
pub fn clock_cancel(editor: &mut Editor) {
    let Some((home, text)) = find_running_clock(editor) else {
        editor.set_error("No clock is running");
        return;
    };
    let title = clock_title(&text);
    let result = helix_roam::clock::clock_cancel(&text)
        .map_err(|err| err.to_string())
        .and_then(|after| write_home(editor, &home, &after));
    match result {
        Ok(()) => editor.set_status(format!("Cancelled the clock on {title}")),
        Err(err) => editor.set_error(err),
    }
}

/// Jumps to the entry the running clock belongs to.
pub fn clock_goto(editor: &mut Editor) {
    let Some((home, text)) = find_running_clock(editor) else {
        editor.set_error("No clock is running");
        return;
    };
    let clock = helix_roam::clock::running(&text).unwrap();
    let line = helix_roam::clock::entry_of(&text, &clock).unwrap_or(clock.line);

    let target = match home {
        ClockHome::Buffer(id) => editor
            .documents
            .get(&id)
            .and_then(|doc| doc.path().map(Path::to_path_buf)),
        ClockHome::Disk(path) => Some(path),
    };
    if let Some(path) = target {
        if let Err(err) = editor.open(&path, helix_view::editor::Action::Replace) {
            editor.set_error(format!("Could not open {}: {err}", path.display()));
            return;
        }
    }
    goto_heading_line(editor, line);
    let running = helix_roam::clock::format_duration(clock.minutes(now_moment()));
    editor.set_status(format!("Clocked in for {running}"));
}

/// Inserts a clock report for the file at the cursor, or refreshes the one
/// the cursor is in.
pub fn clock_report(editor: &mut Editor) {
    let (text, line) = text_and_line(editor);
    if helix_roam::dynamic::block_at(&text, line).is_some() {
        dblock_update(editor);
        return;
    }

    let mut lines: Vec<&str> = text.lines().collect();
    let at = (line + 1).min(lines.len());
    lines.splice(
        at..at,
        ["#+BEGIN: clocktable :maxlevel 2 :scope file", "#+END:"],
    );
    let mut inserted = lines.join("\n");
    if text.ends_with('\n') || text.is_empty() {
        inserted.push('\n');
    }
    match helix_roam::dynamic::refresh(&inserted, at, generate) {
        Some(after) => apply_to_buffer(editor, "Inserted a clock report".to_string(), after),
        None => editor.set_error("Could not write the clock report"),
    }
}

// ── Export ────────────────────────────────────────────────────────────────

/// Resolves `id:` links through the graph, as the export needs them.
struct GraphLinks<'a>(&'a helix_roam::RoamGraph);

impl helix_roam::export::Resolve for GraphLinks<'_> {
    fn id(&self, id: &helix_roam::Uuid) -> Option<helix_roam::export::IdTarget> {
        let node = self.0.get_node(id)?;
        // A file node is the file itself; a headline node is an anchor in it,
        // computed the way the exporter computes that headline's own.
        let anchor = (node.level > 0).then(|| {
            let custom = node
                .properties
                .iter()
                .find(|(key, _)| key == "custom_id")
                .map(|(_, value)| value.as_str());
            helix_roam::export::anchor_for(&node.title, custom)
        });
        Some(helix_roam::export::IdTarget {
            file: node.file_path.clone(),
            anchor,
            title: node.title.clone(),
        })
    }
}

/// Resolves `id:` links through the shared graph, locking it for each
/// link: what an export running in the background uses.
struct SharedLinks(std::sync::Arc<parking_lot::RwLock<helix_roam::RoamGraph>>);

impl helix_roam::export::Resolve for SharedLinks {
    fn id(&self, id: &helix_roam::Uuid) -> Option<helix_roam::export::IdTarget> {
        GraphLinks(&self.0.read()).id(id)
    }
}

/// What `:org-export` was asked for.
#[derive(Debug, Clone, Copy)]
pub struct ExportRequest {
    pub backend: helix_roam::export::Backend,
    /// The subtree at the cursor only.
    pub subtree: bool,
    /// The body without the document around it.
    pub body_only: bool,
    /// In the background, the editor staying usable.
    pub background: bool,
    /// Compile the LaTeX to a PDF (always in the background).
    pub pdf: bool,
}

/// Exports the buffer (or its subtree at the cursor) next to its file:
/// `name.html`, `name.tex`, … A subtree goes to its `:EXPORT_FILE_NAME:`,
/// or to `name-<its anchor>`, so that it does not replace the whole
/// file's export.
pub fn export(editor: &mut Editor, request: ExportRequest) {
    let doc = doc!(editor);
    let Some(source) = doc.path().map(Path::to_path_buf) else {
        editor.set_error("Save the buffer first: the export is written next to it");
        return;
    };
    let full = doc.text().to_string();
    let backend = request.backend;
    let dir = source.parent().unwrap_or(Path::new("")).to_path_buf();
    let stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();

    let (text, target) = if request.subtree {
        let (_, line) = text_and_line(editor);
        let Some(found) = helix_roam::export::subtree(&full, line) else {
            editor.set_error("The cursor is not in a subtree");
            return;
        };
        let name = found
            .file_name
            .clone()
            .unwrap_or_else(|| format!("{stem}-{}", found.anchor));
        let target = dir.join(name).with_extension(backend.extension());
        (found.text, target)
    } else {
        (full, source.with_extension(backend.extension()))
    };
    let compiler = editor.config().roam.latex_compiler.clone();
    let graph = editor.roam.clone();
    let work = move || -> Result<String, String> {
        let exported = helix_roam::export::export_with(
            &text,
            &source,
            backend,
            &SharedLinks(graph),
            request.body_only,
        );
        std::fs::write(&target, exported.bytes())
            .map_err(|err| format!("Could not write {}: {err}", target.display()))?;
        let mut written = target
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        if request.pdf {
            let pdf = compile_pdf(&target, &compiler)?;
            written = pdf
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
        }
        Ok(match exported.warnings.as_slice() {
            [] => format!("Exported to {written}"),
            [one] => format!("Exported to {written}; {one}"),
            many => format!(
                "Exported to {written}; {} warnings, first: {}",
                many.len(),
                many[0]
            ),
        })
    };

    if request.background || request.pdf {
        editor.set_status(if request.pdf {
            "Exporting and compiling the PDF in the background…"
        } else {
            "Exporting in the background…"
        });
        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(work)
                .await
                .unwrap_or_else(|err| Err(format!("The export stopped: {err}")));
            crate::job::dispatch(move |editor, _| match result {
                Ok(said) => editor.set_status(said),
                Err(err) => editor.set_error(err),
            })
            .await;
        });
        return;
    }
    match work() {
        Ok(said) => editor.set_status(said),
        Err(err) => editor.set_error(err),
    }
}

/// Makes the PDF of the LaTeX file `tex`, in its directory: with
/// `compiler` (`%f` the file) when it is set, else `latexmk`, else
/// `pdflatex` twice (the second run settles the references).
fn compile_pdf(tex: &Path, compiler: &[String]) -> Result<PathBuf, String> {
    let dir = tex.parent().unwrap_or(Path::new("."));
    let file = tex
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let found = |program: &str| helix_stdx::env::which(program).is_ok();
    let runs: Vec<Vec<String>> = if !compiler.is_empty() {
        vec![compiler
            .iter()
            .map(|arg| arg.replace("%f", &file))
            .collect()]
    } else if found("latexmk") {
        vec![[
            "latexmk",
            "-pdf",
            "-interaction=nonstopmode",
            "-halt-on-error",
            &file,
        ]
        .map(String::from)
        .to_vec()]
    } else if let Some(engine) = ["pdflatex", "xelatex", "lualatex"]
        .into_iter()
        .find(|p| found(p))
    {
        let run = [engine, "-interaction=nonstopmode", "-halt-on-error", &file]
            .map(String::from)
            .to_vec();
        vec![run.clone(), run]
    } else {
        return Err(
            "No TeX found to make the PDF: install latexmk or pdflatex, or set roam.latex-compiler"
                .to_string(),
        );
    };
    for run in runs {
        let output = std::process::Command::new(&run[0])
            .args(&run[1..])
            .current_dir(dir)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|err| format!("Could not run {}: {err}", run[0]))?;
        if !output.status.success() {
            // TeX says what went wrong on a line starting with `!`.
            let log = String::from_utf8_lossy(&output.stdout);
            let why = log
                .lines()
                .find(|line| line.starts_with('!'))
                .unwrap_or("see the .log file")
                .to_string();
            return Err(format!("{} failed: {why}", run[0]));
        }
    }
    let pdf = tex.with_extension("pdf");
    if pdf.exists() {
        Ok(pdf)
    } else {
        Err(format!("{} made no PDF", tex.display()))
    }
}

/// Exports with the default flags: the whole buffer, in the foreground.
fn export_as(editor: &mut Editor, backend: helix_roam::export::Backend) {
    export(
        editor,
        ExportRequest {
            backend,
            subtree: false,
            body_only: false,
            background: false,
            pdf: false,
        },
    );
}

pub fn export_markdown(editor: &mut Editor) {
    export_as(editor, helix_roam::export::Backend::Markdown);
}

pub fn export_html(editor: &mut Editor) {
    export_as(editor, helix_roam::export::Backend::Html);
}

pub fn export_latex(editor: &mut Editor) {
    export_as(editor, helix_roam::export::Backend::Latex);
}

/// The buffer's scheduled, deadline and dated entries as an iCalendar file
/// next to it.
pub fn icalendar_export(editor: &mut Editor) {
    let doc = doc!(editor);
    let Some(source) = doc.path().map(Path::to_path_buf) else {
        editor.set_error("Save the buffer first: the calendar is written next to it");
        return;
    };
    let text = doc.text().to_string();
    let settings = helix_roam::FileSettings::scan_at(&text, &source);
    let entries = helix_roam::entry::entries(&text, &source, &settings);
    let name = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let calendar = helix_roam::icalendar::calendar(&entries, &name, &utc_stamp());
    let target = source.with_extension("ics");
    match std::fs::write(&target, calendar) {
        Ok(()) => editor.set_status(format!("Wrote {}", target.display())),
        Err(err) => editor.set_error(format!("Could not write {}: {err}", target.display())),
    }
}

/// Every agenda file's entries in one iCalendar file, `icalendar-file`.
pub fn icalendar_combine(editor: &mut Editor) {
    let target = {
        let configured = editor.config().roam.icalendar_file.clone();
        let configured = helix_stdx::path::expand_tilde(&configured).into_owned();
        if configured.is_absolute() {
            configured
        } else {
            notes_directory(editor).join(configured)
        }
    };
    let calendar = {
        let graph = editor.roam.read();
        let mut entries: Vec<&helix_roam::Entry> = graph
            .entries()
            .filter(|entry| in_agenda_scope(editor, &entry.file_path))
            .collect();
        entries.sort_by(|a, b| (&a.file_path, a.line).cmp(&(&b.file_path, b.line)));
        helix_roam::icalendar::calendar(entries, "Agenda", &utc_stamp())
    };
    match std::fs::write(&target, calendar) {
        Ok(()) => editor.set_status(format!("Wrote {}", target.display())),
        Err(err) => editor.set_error(format!("Could not write {}: {err}", target.display())),
    }
}

/// Now, as iCalendar writes a moment in UTC: `20260928T121500Z`.
fn utc_stamp() -> String {
    let (date, time) = now();
    format!(
        "{}T{:02}{:02}00Z",
        date.to_iso().replace('-', ""),
        time.hour,
        time.minute
    )
}

/// Publishes the project named `name`, or every project, in the
/// background; with `force`, every file, not only the changed ones.
pub fn publish(editor: &mut Editor, name: Option<&str>, force: bool) {
    let config = editor.config();
    let notes = notes_directory(editor);
    let resolve = |path: &Path| {
        let path = helix_stdx::path::expand_tilde(path).into_owned();
        if path.is_absolute() {
            path
        } else {
            notes.join(path)
        }
    };
    let chosen: Vec<&helix_view::editor::PublishProject> = config
        .roam
        .publish
        .iter()
        .filter(|project| name.is_none_or(|name| project.name == name))
        .collect();
    if chosen.is_empty() {
        return editor.set_error(match name {
            Some(name) => format!("No publishing project is called {name}"),
            None => "No publishing project: add [[editor.roam.publish]] to the config".to_string(),
        });
    }
    let mut projects = Vec::new();
    for project in chosen {
        let Some(backend) = helix_roam::export::Backend::parse(&project.backend) else {
            return editor.set_error(format!(
                "{}: unknown backend {}",
                project.name, project.backend
            ));
        };
        let exclude = match project
            .exclude
            .as_deref()
            .map(helix_core::regex::Regex::new)
        {
            Some(Ok(regex)) => Some(regex),
            Some(Err(err)) => {
                return editor.set_error(format!("{}: exclude is no regex: {err}", project.name))
            }
            None => None,
        };
        projects.push(helix_roam::publish::Project {
            name: project.name.clone(),
            base: resolve(&project.base_directory),
            publishing: resolve(&project.publishing_directory),
            backend,
            recursive: project.recursive,
            exclude,
            attachments: project.attachments.clone(),
            sitemap: project.sitemap.then(|| helix_roam::publish::Sitemap {
                file: project.sitemap_file.clone(),
                title: project.sitemap_title.clone(),
            }),
            body_only: project.body_only,
        });
    }
    let graph = editor.roam.clone();
    editor.set_status(format!("Publishing {} project(s)…", projects.len()));
    tokio::spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            let links = SharedLinks(graph);
            let mut said = Vec::new();
            let mut warnings = Vec::new();
            for project in &projects {
                let report = helix_roam::publish::publish(project, &links, force)
                    .map_err(|err| format!("{}: {err}", project.name))?;
                said.push(format!(
                    "{}: {} published, {} copied, {} up to date",
                    project.name,
                    report.published.len(),
                    report.copied.len(),
                    report.unchanged
                ));
                warnings.extend(report.warnings);
            }
            let mut message = said.join("; ");
            if let Some(first) = warnings.first() {
                message.push_str(&format!("; {} warning(s), first: {first}", warnings.len()));
            }
            Ok::<_, String>(message)
        })
        .await
        .unwrap_or_else(|err| Err(format!("Publishing stopped: {err}")));
        crate::job::dispatch(move |editor, _| match result {
            Ok(said) => editor.set_status(said),
            Err(err) => editor.set_error(err),
        })
        .await;
    });
}

// ── Babel ─────────────────────────────────────────────────────────────────

/// How long a block may run before it is killed. A block that hangs would
/// otherwise hold a process nobody can see.
const BABEL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Runs the source block at the cursor and writes its results under it.
///
/// Two gates, both required. The workspace must be trusted for code
/// execution — `:workspace-trust`, the same grant that allows its local
/// config; the default trust level, which starts language servers on its
/// own, does not reach this. And every run is confirmed, naming the program
/// and the directory, as Org does by default with `org-confirm-babel-evaluate`.
pub fn babel_execute(editor: &mut Editor) {
    let (text, line, blocks) = source_blocks(editor);
    let Some(block) = helix_roam::source::block_at(&blocks, line).cloned() else {
        editor.set_error("Not in a source block");
        return;
    };
    let doc = doc!(editor);
    let Some(org_path) = doc.path().map(Path::to_path_buf) else {
        editor.set_error("Save the buffer first: a block runs in its file's directory");
        return;
    };

    let workspace = doc.workspace_root().to_path_buf();
    let trusted = editor
        .workspace_trust
        .query(
            &workspace,
            helix_loader::workspace_trust::TrustQuery::CodeExecution,
        )
        .is_trusted();
    if !trusted {
        editor.set_error(format!(
            "Running code is not trusted in {}; :workspace-trust allows it",
            workspace.display()
        ));
        return;
    }

    let plan = match helix_roam::babel::plan(&text, &block, &org_path) {
        Ok(plan) => plan,
        Err(err) => {
            editor.set_error(format!("Not run: {err}"));
            return;
        }
    };

    let doc_id = doc.id();
    let body = helix_roam::source::body(&text, &block);
    let lines = body.lines().count();
    let question = format!(
        "Run the {} block ({lines} line{}) with {} in {}? [y/N] ",
        plan.language,
        if lines == 1 { "" } else { "s" },
        plan.program.join(" "),
        plan.dir.display()
    );

    crate::job::dispatch_blocking(move |_editor, compositor| {
        let mut pending = Some((plan, body));
        let prompt = crate::ui::Prompt::new(
            question.into(),
            None,
            |_editor, _input| Vec::new(),
            move |cx, input, event| {
                if event != crate::ui::PromptEvent::Validate {
                    return;
                }
                if !matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                    cx.editor.set_status("Not run");
                    return;
                }
                if let Some((plan, body)) = pending.take() {
                    run_block(cx.editor, doc_id, block.begin, body, plan);
                }
            },
        );
        compositor.push(Box::new(prompt));
    });
}

/// Starts the block's program in the background; the results are written
/// when it finishes.
fn run_block(
    editor: &mut Editor,
    doc_id: helix_view::DocumentId,
    begin: usize,
    body: String,
    plan: helix_roam::babel::Plan,
) {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join("neohelix-babel").join(format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let script = dir.join(format!("block.{}", plan.extension));
    let value = dir.join("value");
    if let Err(err) =
        std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&script, &plan.script))
    {
        editor.set_error(format!("Could not write the script: {err}"));
        return;
    }

    editor.set_status(format!("Running the {} block…", plan.language));
    tokio::spawn(async move {
        let started = std::time::Instant::now();
        let mut command = tokio::process::Command::new(&plan.program[0]);
        command
            .args(&plan.program[1..])
            .arg(&script)
            .current_dir(&plan.dir)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        if plan.value_file {
            command.arg(&value);
        }
        command.args(&plan.args);

        let outcome = match tokio::time::timeout(BABEL_TIMEOUT, command.output()).await {
            Err(_) => Err(format!(
                "{} did not finish within {} s and was stopped",
                plan.program[0],
                BABEL_TIMEOUT.as_secs()
            )),
            Ok(Err(err)) => Err(format!("could not start {}: {err}", plan.program[0])),
            Ok(Ok(output)) => {
                let stdout = if plan.value_file {
                    std::fs::read_to_string(&value).unwrap_or_default()
                } else {
                    String::from_utf8_lossy(&output.stdout).to_string()
                };
                Ok((
                    output.status,
                    stdout,
                    String::from_utf8_lossy(&output.stderr).to_string(),
                ))
            }
        };
        let elapsed = started.elapsed();
        let _ = std::fs::remove_dir_all(&dir);

        crate::job::dispatch(move |editor, _| {
            finish_block(editor, doc_id, begin, &body, &plan, outcome, elapsed);
        })
        .await;
    });
}

type BlockOutcome = Result<(std::process::ExitStatus, String, String), String>;

/// Writes a finished block's results, and says how it went.
fn finish_block(
    editor: &mut Editor,
    doc_id: helix_view::DocumentId,
    begin: usize,
    body: &str,
    plan: &helix_roam::babel::Plan,
    outcome: BlockOutcome,
    elapsed: std::time::Duration,
) {
    let (status, stdout, stderr) = match outcome {
        Ok(done) => done,
        Err(err) => {
            editor.set_error(err);
            return;
        }
    };
    let failed = !status.success();

    // Results are written for a failed run only if it printed something:
    // replacing good results with nothing would lose them for no gain.
    if !(failed && stdout.trim().is_empty()) {
        let Some(doc) = editor.documents.get(&doc_id) else {
            editor.set_error("The buffer was closed while the block ran");
            return;
        };
        let text = doc.text().to_string();
        let blocks = helix_roam::source::blocks(&text);
        let Some(block) = blocks
            .iter()
            .filter(|block| helix_roam::source::body(&text, block) == body)
            .min_by_key(|block| block.begin.abs_diff(begin))
        else {
            editor.set_error("The block changed while it ran; its results were not written");
            return;
        };
        let results = helix_roam::babel::results_lines(&stdout, plan);
        let after = helix_roam::babel::write_results(&text, block, &results);
        if let Err(err) = apply_to_document(editor, doc_id, &after) {
            editor.set_error(err);
            return;
        }
    }

    let took = format!("{:.2} s", elapsed.as_secs_f64());
    if failed {
        let how = match status.code() {
            Some(code) => format!("exited with code {code}"),
            None => "was killed by a signal".to_string(),
        };
        // The last line: a traceback ends with what went wrong, and a shell
        // error is usually one line anyway.
        let why = stderr
            .lines()
            .rfind(|line| !line.trim().is_empty())
            .map(|line| format!(": {line}"))
            .unwrap_or_default();
        editor.set_error(format!("{} {how} after {took}{why}", plan.program[0]));
    } else if helix_roam::babel::value_is_output(plan) {
        editor.set_status(format!(
            "Ran in {took}; {} blocks give their output, not a value (:results output)",
            plan.language
        ));
    } else {
        editor.set_status(format!("Ran in {took}"));
    }
}

// ── Pretty display ────────────────────────────────────────────────────────

/// Recomputes which entities and links of a document are drawn as what they
/// stand for.
pub fn refresh_pretty(doc: &mut helix_view::Document) {
    let text = doc.text().to_string();
    doc.org_conceals = helix_core::conceal::Conceals::new(
        helix_roam::pretty::conceals(&text)
            .into_iter()
            .map(|pretty| helix_core::conceal::Conceal {
                start: pretty.start,
                end: pretty.end,
                replacement: pretty.replacement,
            })
            .collect(),
    );
}

/// Draws the buffer's entities and links as written, or as what they stand
/// for.
pub fn toggle_pretty(editor: &mut Editor) {
    let doc = doc_mut!(editor);
    doc.org_pretty = !doc.org_pretty;
    if doc.org_pretty {
        refresh_pretty(doc);
        editor.set_status("Entities and links drawn as what they stand for");
    } else {
        doc.org_conceals.clear();
        editor.set_status("Entities and links drawn as written");
    }
}

// ── Column view ───────────────────────────────────────────────────────────

/// Recomputes a document's column view from its text, returning the header.
pub fn refresh_columns(doc: &mut helix_view::Document) -> String {
    let rope = doc.text().clone();
    let text = rope.to_string();
    let columns = helix_roam::columns::format_of(&text);
    let (header, rows) = helix_roam::columns::layout(&text, &columns);

    doc.org_columns = rows
        .into_iter()
        .map(|(line, row)| {
            // At the headline's end, before its newline, like backlink counts.
            let at = if line + 1 < rope.len_lines() {
                rope.line_to_char(line + 1) - 1
            } else {
                rope.len_chars()
            };
            helix_core::text_annotations::InlineAnnotation::new(at, row)
        })
        .collect();
    header
}

/// Shows or hides the column view of the buffer.
///
/// While it shows, it is recomputed on every change, so setting a property
/// or a TODO state updates its row as it happens.
pub fn toggle_columns(editor: &mut Editor) {
    let doc = doc_mut!(editor);
    if doc.org_columns_on {
        doc.org_columns_on = false;
        doc.org_columns.clear();
        editor.set_status("Column view off");
        return;
    }
    doc.org_columns_on = true;
    let header = refresh_columns(doc);
    editor.set_status(format!("Columns: {header}"));
}

// ── Attachments ───────────────────────────────────────────────────────────

/// The directory of the Org file in the focused buffer.
fn org_dir(editor: &Editor) -> Option<PathBuf> {
    doc!(editor)
        .path()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

/// Copies a file into the attachment directory of the entry at the cursor.
///
/// The entry gets an `:ID:` first if it has none, since that is what names
/// the directory, and the `ATTACH` tag, as in Org. The original stays where
/// it is: Org's default is to copy.
pub fn attach(editor: &mut Editor, input: &str) {
    let source = helix_stdx::path::expand_tilde(Path::new(input.trim())).to_path_buf();
    if input.trim().is_empty() {
        return;
    }
    if !source.is_file() {
        editor.set_error(format!("{} is not a file", source.display()));
        return;
    }
    let Some(base) = org_dir(editor) else {
        editor.set_error("Save the buffer first: attachments live next to it");
        return;
    };

    let (mut text, line) = text_and_line(editor);
    if let Ok(helix_roam::restructure::IdOutcome::Created { text: with_id, .. }) =
        helix_roam::restructure::ensure_id(&text, line, helix_roam::Uuid::new_v4())
    {
        text = with_id;
    }
    let Some(dir) = helix_roam::attach::attach_dir(&text, line, &base) else {
        editor.set_error("The entry has no :ID: or :DIR: to attach to");
        return;
    };

    let Some(name) = source.file_name() else {
        editor.set_error(format!("{} has no file name", source.display()));
        return;
    };
    let target = dir.join(name);
    let replaced = target.exists();
    if let Err(err) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::copy(&source, &target)) {
        editor.set_error(format!("Could not attach to {}: {err}", dir.display()));
        return;
    }

    if let Ok(Some(tagged)) =
        helix_roam::restructure::edit_tag(&text, line, helix_roam::attach::TAG, true)
    {
        text = tagged;
    }
    let shown = dir
        .strip_prefix(&base)
        .unwrap_or(&dir)
        .display()
        .to_string();
    let done = format!(
        "{} {} in {shown}",
        if replaced { "Replaced" } else { "Attached" },
        name.to_string_lossy()
    );
    apply_to_buffer(editor, done.clone(), text);
    // Nothing to change in the buffer (a second file on a tagged entry)
    // still attached something, and should say so.
    editor.set_status(done);
}

/// A picker over the files attached to the entry at the cursor.
pub fn attachment_picker(editor: &mut Editor) -> Option<Box<dyn crate::compositor::Component>> {
    let Some(base) = org_dir(editor) else {
        editor.set_error("Save the buffer first: attachments live next to it");
        return None;
    };
    let (text, line) = text_and_line(editor);
    let Some(dir) = helix_roam::attach::attach_dir(&text, line, &base) else {
        editor.set_error("The entry has no :ID: or :DIR:, so nothing is attached to it");
        return None;
    };
    if helix_roam::attach::list(&dir).is_empty() {
        editor.set_error(format!("Nothing attached in {}", dir.display()));
        return None;
    }
    Some(Box::new(crate::ui::overlay::overlaid(
        crate::ui::file_picker(editor, dir),
    )))
}

/// Opens `[[attachment:name]]` from the entry the link is in.
fn follow_attachment(editor: &mut Editor, text: &str, offset: usize, name: &str) {
    let Some(base) = org_dir(editor) else {
        editor.set_error("Save the buffer first: attachments live next to it");
        return;
    };
    let line = text[..offset.min(text.len())].matches('\n').count();
    let Some(dir) = helix_roam::attach::attach_dir(text, line, &base) else {
        editor.set_error("The entry this link is in has no :ID: or :DIR:");
        return;
    };
    let path = dir.join(name.trim());
    if !path.exists() {
        editor.set_error(format!(
            "No attachment {} in {}",
            name.trim(),
            dir.display()
        ));
        return;
    }
    if let Err(err) = editor.open(&path, helix_view::editor::Action::Replace) {
        editor.set_error(format!("Could not open {}: {err}", path.display()));
    }
}

// ── Graph visualisation ───────────────────────────────────────────────────

/// Where a program would be found on `PATH`, if anywhere.
fn on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// Draws the graph — all of it, or the nodes within `depth` links of the one
/// at the cursor — and opens the picture.
///
/// The DOT file is always written. Rendering it needs Graphviz's `dot`,
/// which is looked for rather than required: without it, the message says
/// where the DOT file is, and that is still something any Graphviz viewer
/// can open.
pub fn graph(editor: &mut Editor, depth: Option<usize>) {
    let center = match depth {
        None => None,
        Some(depth) => match node_at_cursor(editor) {
            Some((id, _)) => Some((id, depth)),
            None => {
                editor.set_error("No node at the cursor to draw the neighbourhood of");
                return;
            }
        },
    };

    let text = {
        let graph = editor.roam.read();
        if graph.is_empty() {
            drop(graph);
            editor.set_error("The index is empty: nothing to draw");
            return;
        }
        if let Some((id, _)) = center {
            if !graph.contains_node(&id) {
                drop(graph);
                editor.set_error(
                    "The node at the cursor is not in the index yet; save and try again",
                );
                return;
            }
        }
        helix_roam::visual::dot(&graph, center)
    };

    let dir = std::env::temp_dir().join("neohelix-graph");
    let stem = match center {
        Some((id, depth)) => format!("{id}-{depth}"),
        None => "roam".to_string(),
    };
    let dot_file = dir.join(format!("{stem}.dot"));
    if let Err(err) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&dot_file, &text))
    {
        editor.set_error(format!("Could not write {}: {err}", dot_file.display()));
        return;
    }
    let nodes = text.lines().filter(|line| line.contains("[label=")).count();

    let Some(dot) = on_path("dot") else {
        editor.set_status(format!(
            "Wrote {} ({nodes} nodes); Graphviz's dot is not installed, so it was not rendered",
            dot_file.display()
        ));
        return;
    };

    let svg = dir.join(format!("{stem}.svg"));
    editor.set_status(format!("Rendering {nodes} nodes…"));
    tokio::spawn(async move {
        let rendered = tokio::process::Command::new(&dot)
            .arg("-Tsvg")
            .arg("-o")
            .arg(&svg)
            .arg(&dot_file)
            .stdin(std::process::Stdio::null())
            .output()
            .await;
        crate::job::dispatch(move |editor, _| match rendered {
            Ok(output) if output.status.success() => {
                // Without a program to hand it to, the picture is still made:
                // say where it is rather than report a failure.
                let opener = if cfg!(target_os = "macos") {
                    "open"
                } else if cfg!(target_os = "windows") {
                    "explorer"
                } else {
                    "xdg-open"
                };
                if cfg!(target_os = "windows") || on_path(opener).is_some() {
                    open_externally(editor, &svg.to_string_lossy());
                } else {
                    editor.set_status(format!(
                        "Rendered {}; {opener} is not installed to open it",
                        svg.display()
                    ));
                }
            }
            Ok(output) => editor.set_error(format!(
                "dot failed: {}",
                String::from_utf8_lossy(&output.stderr)
                    .lines()
                    .next()
                    .unwrap_or("")
            )),
            Err(err) => editor.set_error(format!("could not run dot: {err}")),
        })
        .await;
    });
}

// ── Inline tasks ──────────────────────────────────────────────────────────

/// Inserts an inline task below the cursor's line, with its `END` line.
///
/// Without a keyword, as Org's `org-inlinetask-default-state` leaves it: the
/// state is set by cycling it like any other task.
pub fn insert_inline_task(editor: &mut Editor, title: &str) {
    let title = title.trim();
    if title.is_empty() {
        return;
    }
    let (text, line) = text_and_line(editor);
    let stars = "*".repeat(helix_roam::restructure::INLINE_TASK_LEVEL);
    let mut lines: Vec<&str> = text.lines().collect();
    let at = (line + 1).min(lines.len());
    let task = format!("{stars} {title}");
    let end = format!("{stars} END");
    lines.splice(at..at, [task.as_str(), end.as_str()]);

    let mut after = lines.join("\n");
    if text.ends_with('\n') || text.is_empty() {
        after.push('\n');
    }
    apply_and_go(editor, "Inserted an inline task".to_string(), after, at);
}

// ── org-protocol ──────────────────────────────────────────────────────────

/// Opens `path` with the cursor at the start of `line` (0-based), centred.
pub(crate) fn open_at(editor: &mut Editor, path: &Path, line: usize) -> bool {
    if let Err(err) = editor.open(path, helix_view::editor::Action::Replace) {
        editor.set_error(format!("could not open {}: {err}", path.display()));
        return false;
    }
    let text = doc!(editor).text().clone();
    let at = text.line_to_char(line.min(text.len_lines().saturating_sub(1)));
    let view_id = view!(editor).id;
    doc_mut!(editor).set_selection(view_id, helix_core::Selection::point(at));
    let (view, doc) = current!(editor);
    helix_view::align_view(doc, view, helix_view::Align::Center);
    true
}

/// Acts on an `org-protocol://` URL the editor was started with.
///
/// * `capture` appends the page — a link and the selected text as a quote —
///   to `inbox.org` in the notes directory, or, when `template=` names one
///   of the configured node templates, makes a node from it.
/// * `roam-ref` opens the node whose `:ROAM_REFS:` has the page, or makes
///   one, as Org-Roam does.
/// * `roam-node` opens a node.
/// * `store-link` keeps the link for `:org-insert-link`.
///
/// The index is still being built when this runs, so nodes are found by
/// reading the notes directory.
pub fn handle_protocol(editor: &mut Editor, url: &str) {
    use helix_roam::protocol::Request;

    let request = match helix_roam::protocol::parse(url) {
        Ok(request) => request,
        Err(err) => {
            editor.set_error(format!("org-protocol: {err}"));
            return;
        }
    };
    let notes = notes_directory(editor);

    match request {
        Request::StoreLink { url, title } => {
            let title = (!title.is_empty()).then_some(title.as_str());
            let stored = helix_roam::hyperlink::format_link(&url, title);
            match editor.registers.write(LINK_REGISTER, vec![stored.clone()]) {
                Ok(()) => editor.set_status(format!("Stored {stored}")),
                Err(err) => editor.set_error(err.to_string()),
            }
        }
        Request::Capture {
            template,
            url,
            title,
            body,
        } => {
            let addition = helix_roam::protocol::capture_addition(&url, &title, &body);
            let chosen = template.and_then(|key| {
                capture_templates(editor)
                    .into_iter()
                    .find(|template| template.key == key)
            });
            if let Some(template) = chosen {
                let name = if title.trim().is_empty() {
                    url.as_str()
                } else {
                    title.as_str()
                };
                capture_node(editor, &template, name);
                let text = doc!(editor).text().to_string();
                let joined = if text.ends_with('\n') || text.is_empty() {
                    format!("{text}{addition}")
                } else {
                    format!("{text}\n{addition}")
                };
                apply_to_buffer(editor, format!("Captured {name}"), joined);
                return;
            }

            let inbox = notes.join("inbox.org");
            let mut text = std::fs::read_to_string(&inbox)
                .unwrap_or_else(|_| "#+title: Inbox\n\n".to_string());
            if !text.ends_with('\n') {
                text.push('\n');
            }
            let line = text.lines().count();
            let (date, time) = now();
            text.push_str(&helix_roam::protocol::inbox_entry(
                &url,
                &title,
                &body,
                &helix_roam::date::log_stamp(date, time),
            ));
            if let Err(err) =
                std::fs::create_dir_all(&notes).and_then(|()| std::fs::write(&inbox, &text))
            {
                editor.set_error(format!("could not write {}: {err}", inbox.display()));
                return;
            }
            if open_at(editor, &inbox, line) {
                editor.set_status(format!("Captured into {}", inbox.display()));
            }
        }
        Request::RoamRef {
            reference,
            title,
            body,
        } => {
            if let Some((path, line)) =
                helix_roam::protocol::find_property(&notes, "ROAM_REFS", &reference)
            {
                if open_at(editor, &path, line) {
                    editor.set_status(format!("Already a note about {reference}"));
                }
                return;
            }
            let name = if title.trim().is_empty() {
                &reference
            } else {
                &title
            };
            let slug = helix_roam::capture::slugify(name);
            let slug = if slug.is_empty() {
                "ref".to_string()
            } else {
                slug
            };
            let mut path = notes.join(format!("{slug}.org"));
            let mut n = 1;
            while path.exists() {
                n += 1;
                path = notes.join(format!("{slug}-{n}.org"));
            }
            let content = helix_roam::protocol::ref_node(
                helix_roam::Uuid::new_v4(),
                &reference,
                &title,
                &body,
            );
            if let Err(err) =
                std::fs::create_dir_all(&notes).and_then(|()| std::fs::write(&path, &content))
            {
                editor.set_error(format!("could not write {}: {err}", path.display()));
                return;
            }
            helix_roam::reindex_file(&mut editor.roam.write(), &path, &content);
            if open_at(editor, &path, content.lines().count()) {
                editor.set_status(format!("New note about {reference}"));
            }
        }
        Request::RoamNode(id) => {
            match helix_roam::protocol::find_property(&notes, "ID", &id.to_string()) {
                Some((path, line)) => {
                    open_at(editor, &path, line);
                }
                None => {
                    editor.set_error(format!("org-protocol: no node {id} in {}", notes.display()))
                }
            }
        }
    }
}

// ── Encrypted subtrees ────────────────────────────────────────────────────

/// Why writing `doc` would put a `:crypt:` entry on disk in clear, if it
/// would.
pub fn crypt_guard(doc: &helix_view::Document) -> Option<String> {
    let is_org = doc
        .path()
        .and_then(|path| path.extension())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("org"));
    if !is_org {
        return None;
    }
    let clear = helix_roam::crypt::in_clear(&doc.text().to_string()).len();
    (clear > 0).then(|| {
        format!(
            "{clear} :crypt: entr{} would be written in clear; :org-encrypt-entries first, or :w! to write anyway",
            if clear == 1 { "y" } else { "ies" }
        )
    })
}

/// Runs `gpg` on `input`, returning what it wrote or the last thing it
/// complained about.
///
/// A passphrase, when there is one, is the first line of `input` and read
/// with `--passphrase-fd 0`: never on the command line, where any user
/// could read it, and never in a file.
fn gpg(args: &[&str], input: String) -> Result<String, String> {
    use std::io::Write;

    let mut child = std::process::Command::new("gpg")
        .args(["--batch", "--quiet", "--armor", "--yes"])
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| format!("could not run gpg: {err}"))?;

    // Written from a thread: gpg may fill its output pipe before it has
    // read all its input.
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let output = child
        .wait_with_output()
        .map_err(|err| format!("gpg failed: {err}"))?;
    let _ = writer.join();

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(stderr
            .lines()
            .rfind(|line| !line.trim().is_empty())
            .unwrap_or("gpg failed")
            .trim_start_matches("gpg: ")
            .to_string())
    }
}

/// Encrypts one body: to its key, or with the passphrase when it has none.
fn encrypt_body(
    text: &str,
    body: &helix_roam::crypt::Body,
    passphrase: Option<&str>,
) -> Result<String, String> {
    let plain = helix_roam::crypt::body_text(text, body);
    match helix_roam::crypt::key(text, body.headline) {
        Some(key) => gpg(&["--encrypt", "--recipient", &key], plain),
        None => {
            let passphrase = passphrase.ok_or("no passphrase")?;
            gpg(
                &[
                    "--symmetric",
                    "--pinentry-mode",
                    "loopback",
                    "--passphrase-fd",
                    "0",
                ],
                format!("{passphrase}\n{plain}"),
            )
        }
    }
}

/// Asks for a passphrase without showing it, then calls `then` with it.
fn ask_passphrase(label: &'static str, then: impl FnOnce(&mut Editor, String) + Send + 'static) {
    crate::job::dispatch_blocking(move |_editor, compositor| {
        let mut then = Some(then);
        let prompt = crate::ui::Prompt::new(
            label.into(),
            None,
            |_editor, _input| Vec::new(),
            move |cx, input, event| {
                if event != crate::ui::PromptEvent::Validate {
                    return;
                }
                if let Some(then) = then.take() {
                    then(cx.editor, input.to_string());
                }
            },
        )
        .masked();
        compositor.push(Box::new(prompt));
    });
}

/// Asks for a new passphrase twice, as a mistyped one would lose the text.
fn ask_new_passphrase(then: impl FnOnce(&mut Editor, String) + Send + 'static) {
    ask_passphrase("Passphrase: ", move |_editor, first| {
        ask_passphrase("Repeat passphrase: ", move |editor, second| {
            if first.is_empty() {
                editor.set_error("An empty passphrase would protect nothing; not encrypted");
            } else if first != second {
                editor.set_error("The passphrases differ; not encrypted");
            } else {
                then(editor, first);
            }
        });
    });
}

/// Encrypts the given entries of the focused buffer, by their headlines.
fn encrypt_entries_now(editor: &mut Editor, headlines: &[usize], passphrase: Option<&str>) {
    let mut text = doc!(editor).text().to_string();
    let mut done = 0;
    // Last first, so an earlier body's line numbers are not shifted.
    for &headline in headlines.iter().rev() {
        let Some(body) = helix_roam::crypt::body(&text, headline) else {
            continue;
        };
        if body.end == body.start || helix_roam::crypt::is_encrypted(&text, &body) {
            continue;
        }
        match encrypt_body(&text, &body, passphrase) {
            Ok(armoured) => {
                text = helix_roam::crypt::replace(&text, &body, &armoured);
                done += 1;
            }
            Err(err) => {
                editor.set_error(format!("Not encrypted: {err}"));
                return;
            }
        }
    }
    apply_to_buffer(
        editor,
        format!(
            "Encrypted {done} entr{}",
            if done == 1 { "y" } else { "ies" }
        ),
        text,
    );
}

/// Encrypts the entry at the cursor (Org's `org-encrypt-entry`).
pub fn encrypt_entry(editor: &mut Editor) {
    let (text, line) = text_and_line(editor);
    let Some(body) = helix_roam::crypt::body(&text, line) else {
        editor.set_error("Not in an entry");
        return;
    };
    if helix_roam::crypt::is_encrypted(&text, &body) {
        editor.set_status("Already encrypted");
        return;
    }
    if body.end == body.start {
        editor.set_status("Nothing to encrypt");
        return;
    }
    let headline = body.headline;
    if helix_roam::crypt::key(&text, headline).is_some() {
        encrypt_entries_now(editor, &[headline], None);
    } else {
        ask_new_passphrase(move |editor, passphrase| {
            encrypt_entries_now(editor, &[headline], Some(&passphrase));
        });
    }
}

/// Encrypts every `:crypt:` entry in clear (Org's `org-encrypt-entries`).
pub fn encrypt_entries(editor: &mut Editor) {
    let text = doc!(editor).text().to_string();
    let clear: Vec<usize> = helix_roam::crypt::in_clear(&text)
        .into_iter()
        .map(|body| body.headline)
        .collect();
    if clear.is_empty() {
        editor.set_status("No :crypt: entry is in clear");
        return;
    }
    let needs_passphrase = clear
        .iter()
        .any(|&headline| helix_roam::crypt::key(&text, headline).is_none());
    if needs_passphrase {
        ask_new_passphrase(move |editor, passphrase| {
            encrypt_entries_now(editor, &clear, Some(&passphrase));
        });
    } else {
        encrypt_entries_now(editor, &clear, None);
    }
}

/// Decrypts the entry at the cursor (Org's `org-decrypt-entry`).
///
/// The passphrase is asked for here rather than by gpg's own pinentry,
/// which in a terminal would draw over the editor. For a key without a
/// passphrase, leaving it empty works.
pub fn decrypt_entry(editor: &mut Editor) {
    let (text, line) = text_and_line(editor);
    let Some(body) = helix_roam::crypt::body(&text, line) else {
        editor.set_error("Not in an entry");
        return;
    };
    if !helix_roam::crypt::is_encrypted(&text, &body) {
        editor.set_status("Not encrypted");
        return;
    }
    let doc_id = doc!(editor).id();
    let headline = body.headline;
    ask_passphrase("Passphrase: ", move |editor, passphrase| {
        // The buffer may have changed while the prompt was open.
        let Some(doc) = editor.documents.get(&doc_id) else {
            return;
        };
        let text = doc.text().to_string();
        let Some(body) = helix_roam::crypt::body(&text, headline)
            .filter(|body| helix_roam::crypt::is_encrypted(&text, body))
        else {
            editor.set_error("The entry changed; not decrypted");
            return;
        };
        let armoured = helix_roam::crypt::body_text(&text, &body);
        match gpg(
            &[
                "--decrypt",
                "--pinentry-mode",
                "loopback",
                "--passphrase-fd",
                "0",
            ],
            format!("{passphrase}\n{armoured}"),
        ) {
            Ok(plain) => {
                let after = helix_roam::crypt::replace(&text, &body, &plain);
                if let Err(err) = apply_to_document(editor, doc_id, &after) {
                    editor.set_error(err);
                } else {
                    editor.set_status(
                        "Decrypted; it is encrypted again only by :org-encrypt-entries",
                    );
                }
            }
            Err(err) => editor.set_error(format!("Not decrypted: {err}")),
        }
    });
}
