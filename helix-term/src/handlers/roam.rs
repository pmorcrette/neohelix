//! Remembering where `:ID:`s live, so a link can leave the notes directory.
//!
//! The index knows the files it scanned. An `id:` link into a file outside
//! the notes directory resolves to nothing, however real its target is. Every
//! `.org` file the editor opens leaves its ids here, so following a link into
//! a file you have visited works even when the indexer has never seen it.

use helix_event::register_hook;
use helix_view::events::DocumentDidOpen;
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

pub(super) fn register_hooks() {
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        remember_ids(event.editor, event.doc);
        Ok(())
    });
}
