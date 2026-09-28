//! `#+SETUPFILE:`: a file whose settings live partly in another file.

use std::sync::Arc;

use helix_roam::{scan_directory, FileSettings};
use parking_lot::RwLock;

const NODE: &str = "#+SETUPFILE: setup/shared.setup\n* NEXT Write the thing\n:PROPERTIES:\n:ID: 6ba7b810-9dad-11d1-80b4-00c04fd430c8\n:END:\n";

fn write(dir: &std::path::Path, name: &str, text: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn a_setup_file_s_keywords_count_as_the_file_s_own() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "setup/shared.setup",
        "#+TODO: NEXT WAIT | DONE\n#+PROPERTY: Effort_ALL 0:30 1:00\n",
    );
    let note = write(dir.path(), "note.org", NODE);

    let settings = FileSettings::scan_at(NODE, &note);
    assert_eq!(settings.todo_keywords, ["NEXT", "WAIT"]);
    assert_eq!(
        settings.setup_files,
        [dir.path().join("setup/shared.setup")]
    );
    assert!(settings
        .properties
        .iter()
        .any(|(key, _)| key == "effort_all"));

    // Without the path, nothing to resolve against: Org's defaults.
    assert_eq!(FileSettings::scan(NODE).todo_keywords, ["TODO"]);

    // And what the index stores follows it: the keyword is not the title.
    let parsed = helix_roam::parse_org(NODE, &note);
    assert_eq!(parsed.nodes[0].title, "Write the thing");
}

#[test]
fn setup_files_nest_relative_to_the_one_naming_them_and_do_not_loop() {
    let dir = tempfile::tempdir().unwrap();
    // shared.setup names inner.setup beside itself, and inner names shared.
    write(
        dir.path(),
        "setup/shared.setup",
        "#+SETUPFILE: inner.setup\n#+TAGS: work\n",
    );
    write(
        dir.path(),
        "setup/inner.setup",
        "#+SETUPFILE: shared.setup\n#+TODO: NEXT | DONE\n",
    );
    let note = write(dir.path(), "note.org", NODE);

    let settings = FileSettings::scan_at(NODE, &note);
    assert_eq!(settings.todo_keywords, ["NEXT"]);
    assert_eq!(settings.declared_tags, ["work"]);
    assert_eq!(settings.setup_files.len(), 2);
}

#[test]
fn a_missing_setup_file_or_a_url_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let text = "#+SETUPFILE: nowhere.setup\n#+SETUPFILE: https://example.org/x.setup\n* TODO A\n";
    let note = write(dir.path(), "note.org", text);
    let settings = FileSettings::scan_at(text, &note);
    assert_eq!(settings.todo_keywords, ["TODO"]);
    assert!(settings.setup_files.is_empty());
}

#[tokio::test]
async fn changing_a_setup_file_reindexes_what_reads_it() {
    let dir = tempfile::tempdir().unwrap();
    let setup = write(dir.path(), "setup/shared.setup", "#+TODO: TODO | DONE\n");
    let note = write(dir.path(), "note.org", NODE);

    let (graph, _) = scan_directory(dir.path());
    let id = "6ba7b810-9dad-11d1-80b4-00c04fd430c8".parse().unwrap();
    // NEXT is not a keyword yet, so it is part of the title.
    assert_eq!(graph.get_node(&id).unwrap().title, "NEXT Write the thing");
    assert_eq!(graph.setup_dependents(&setup), std::slice::from_ref(&note));

    let graph = Arc::new(RwLock::new(graph));
    std::fs::write(&setup, "#+TODO: NEXT | DONE\n").unwrap();
    let count = helix_roam::scanner::reindex_setup_dependents(graph.clone(), setup.clone())
        .await
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(graph.read().get_node(&id).unwrap().title, "Write the thing");

    // A file that stops naming the setup file stops depending on it.
    helix_roam::reindex_file(&mut graph.write(), &note, "* Plain\n");
    assert!(graph.read().setup_dependents(&setup).is_empty());
}
