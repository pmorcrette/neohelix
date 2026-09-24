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
    let startup = helix_roam::startup::Startup::of(&settings);
    let (text, line) = text_and_line(editor);

    let (_, next) = match helix_roam::restructure::todo_step(&text, line, &settings, forward) {
        Ok(step) => step,
        Err(err) => {
            editor.set_error(err.to_string());
            return;
        }
    };
    let finishing = next
        .as_deref()
        .is_some_and(|state| settings.done_keywords.iter().any(|done| done == state));
    if finishing {
        let blocked = open_blockers(editor, &text, line, &settings);
        if !blocked.is_empty() {
            editor.set_error(format!("Blocked by {}", blocked.join(", ")));
            return;
        }
    }

    let changed =
        helix_roam::logging::change_state(&text, line, &settings, &startup, next.as_deref(), now());

    match changed {
        Ok(change) => {
            let done = match (change.repeated, &change.state) {
                (true, state) => format!(
                    "Repeated: back to {}, dates moved on",
                    state.as_deref().unwrap_or("no state")
                ),
                (false, Some(state)) => format!("State {state}"),
                (false, None) => "Cleared the state".to_string(),
            };
            apply_to_buffer(editor, done, change.text);
            if let Some(pending) = change.note {
                ask_for_note(pending);
            }
        }
        Err(err) => editor.set_error(err.to_string()),
    }
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
fn ask_for_note(pending: helix_roam::logging::PendingNote) {
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
                let text = doc!(cx.editor).text().to_string();
                let after = helix_roam::logging::write_note(&text, &pending, input);
                apply_to_buffer(cx.editor, "Noted".to_string(), after);
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
                ask_for_note(pending);
            }
        }
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
    /// A habit's consistency graph, empty for anything else.
    pub habit: Vec<helix_roam::habit::Cell>,
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
                habit: habit_graph(editor, entry.node, today),
            }
        })
        .collect()
}

/// The consistency graph of a node that is a habit.
///
/// The history is in the entry's logbook, which the index does not keep,
/// so the file is read: from its buffer if it is open, which is newer than
/// the disk, and otherwise from the disk.
fn habit_graph(
    editor: &Editor,
    node: &helix_roam::Node,
    today: helix_roam::Date,
) -> Vec<helix_roam::habit::Cell> {
    let is_habit = node
        .properties
        .iter()
        .any(|(key, value)| key == "style" && value.eq_ignore_ascii_case("habit"));
    if !is_habit {
        return Vec::new();
    }
    let text = match editor.document_by_path(&node.file_path) {
        Some(doc) => doc.text().to_string(),
        None => match std::fs::read_to_string(&node.file_path) {
            Ok(text) => text,
            Err(_) => return Vec::new(),
        },
    };
    helix_roam::habit::parse(&text, node.line)
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
    let nodes: Vec<&helix_roam::Node> = graph
        .nodes()
        .filter(|node| in_agenda_scope(editor, &node.file_path))
        .collect();

    helix_roam::agenda::filtered_todo_list(nodes, filter)
        .into_iter()
        .map(|node| AgendaLine {
            habit: Vec::new(),
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
    let settings = helix_roam::FileSettings::scan(&text);
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
    let settings = helix_roam::FileSettings::scan(&doc.text().to_string());
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

/// Exports the buffer next to its file, as `name.md`, `name.html` or
/// `name.tex`.
pub fn export(editor: &mut Editor, backend: helix_roam::export::Backend) {
    let doc = doc!(editor);
    let Some(source) = doc.path().map(Path::to_path_buf) else {
        editor.set_error("Save the buffer first: the export is written next to it");
        return;
    };
    let text = doc.text().to_string();

    let exported = {
        let graph = editor.roam.read();
        helix_roam::export::export(&text, &source, backend, &GraphLinks(&graph))
    };
    let target = source.with_extension(backend.extension());
    if let Err(err) = std::fs::write(&target, &exported.content) {
        editor.set_error(format!("Could not write {}: {err}", target.display()));
        return;
    }

    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    match exported.warnings.as_slice() {
        [] => editor.set_status(format!("Exported to {name}")),
        [one] => editor.set_status(format!("Exported to {name}; {one}")),
        many => editor.set_status(format!(
            "Exported to {name}; {} links could not be resolved, first: {}",
            many.len(),
            many[0]
        )),
    }
}

pub fn export_markdown(editor: &mut Editor) {
    export(editor, helix_roam::export::Backend::Markdown);
}

pub fn export_html(editor: &mut Editor) {
    export(editor, helix_roam::export::Backend::Html);
}

pub fn export_latex(editor: &mut Editor) {
    export(editor, helix_roam::export::Backend::Latex);
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
