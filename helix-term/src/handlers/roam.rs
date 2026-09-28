//! What opening an Org file does: remember where its `:ID:`s live, and fold
//! it the way its `#+STARTUP:` asks.
//!
//! The index knows the files it scanned. An `id:` link into a file outside
//! the notes directory resolves to nothing, however real its target is. Every
//! `.org` file the editor opens leaves its ids here, so following a link into
//! a file you have visited works even when the indexer has never seen it.

use helix_event::register_hook;
use helix_view::events::{DocumentDidChange, DocumentDidClose, DocumentDidOpen};
use helix_view::{DocumentId, Editor};

/// Records the ids an opened Org file declares.
fn remember_ids(editor: &mut Editor, id: DocumentId) {
    if !editor.config().roam.enable {
        return;
    }

    let Some(doc) = editor.documents.get(&id) else {
        return;
    };
    let is_org = doc
        .path()
        .and_then(|path| path.extension())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("org"));
    if !is_org {
        return;
    }

    let Some(path) = doc.path().map(ToOwned::to_owned) else {
        return;
    };
    let ids = helix_roam::parser::ids_in(&doc.text().to_string());
    if ids.is_empty() {
        return;
    }

    let mut graph = editor.roam.write();
    for id in ids {
        graph.register_location(id, path.clone());
    }
}

/// Folds a newly opened Org file the way its `#+STARTUP:` asks.
///
/// Not gated on the Roam index being enabled: this is Org, not Org-Roam, and
/// a file saying `#+STARTUP: overview` means it with or without a graph.
fn fold_on_open(editor: &mut Editor, id: DocumentId) {
    let pretty = editor.config().roam.pretty;
    let Some(doc) = editor.documents.get_mut(&id) else {
        return;
    };
    let is_org = doc
        .path()
        .and_then(|path| path.extension())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("org"));
    if is_org {
        crate::roam::apply_startup_folds(doc);
        if pretty {
            doc.org_pretty = true;
            crate::roam::refresh_pretty(doc);
        }
    }
}

pub(super) fn register_hooks() {
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        remember_ids(event.editor, event.doc);
        fold_on_open(event.editor, event.doc);
        Ok(())
    });
    // A column view is only useful if it is current.
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        if event.doc.org_columns_on {
            crate::roam::refresh_columns(event.doc);
        }
        if event.doc.org_pretty {
            crate::roam::refresh_pretty(event.doc);
        }
        Ok(())
    });
    // A source block's editing buffer is a temporary file; closing it is
    // the end of that edit.
    register_hook!(move |event: &mut DocumentDidClose<'_>| {
        if let Some(path) = event.doc.path().map(ToOwned::to_owned) {
            crate::roam::forget_src_edit(event.editor, &path);
        }
        Ok(())
    });
}
