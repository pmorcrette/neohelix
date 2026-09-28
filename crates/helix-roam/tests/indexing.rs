//! End-to-end indexing: real files on disk through to graph queries.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use helix_roam::scanner::{self, reindex_file};
use helix_roam::RoamGraph;
use parking_lot::RwLock;

const RUST_ID: &str = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
const HELIX_ID: &str = "6ba7b811-9dad-11d1-80b4-00c04fd430c8";
const EDITORS_ID: &str = "6ba7b812-9dad-11d1-80b4-00c04fd430c8";

fn write(dir: &Path, name: &str, body: &str) {
    fs::write(dir.join(name), body).unwrap();
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();

    write(
        dir.path(),
        "rust.org",
        &format!(
            ":PROPERTIES:\n:ID: {RUST_ID}\n:ROAM_REFS: https://rust-lang.org\n:END:\n\
             #+title: Rust\n#+filetags: :lang:\n\nA systems language.\n"
        ),
    );
    // helix.org links to rust.org, which is walked first: the link is seen
    // before its target only if the walk happens to order them that way, so
    // this also covers the two-pass insert.
    write(
        dir.path(),
        "helix.org",
        &format!(
            ":PROPERTIES:\n:ID: {HELIX_ID}\n:END:\n#+title: Helix\n\n\
             Written in [[id:{RUST_ID}][Rust]], an [[id:{EDITORS_ID}][editor]].\n\
             Cites [[https://rust-lang.org]] too.\n"
        ),
    );
    write(
        dir.path(),
        "editors.org",
        &format!(":PROPERTIES:\n:ID: {EDITORS_ID}\n:END:\n#+title: Editors\n"),
    );
    // Not an Org file, and must not be parsed.
    write(dir.path(), "notes.md", "# Not org\n");

    dir
}

fn titles(links: &[(&helix_roam::Node, &helix_roam::Link)]) -> Vec<String> {
    let mut titles: Vec<_> = links.iter().map(|(node, _)| node.title.clone()).collect();
    titles.sort();
    titles
}

/// The distinct nodes on one end of a set of links; a node linking twice (an
/// id link and a ref citation, say) shows up once.
fn distinct_titles(links: &[(&helix_roam::Node, &helix_roam::Link)]) -> Vec<String> {
    let mut titles = titles(links);
    titles.dedup();
    titles
}

#[test]
fn scans_a_directory_into_a_connected_graph() {
    let dir = fixture();
    let (graph, stats) = scanner::scan_directory(dir.path());

    assert_eq!(stats.files, 3, "only .org files are parsed");
    assert_eq!(stats.nodes, 3);
    assert_eq!(stats.errors, 0);
    assert_eq!(graph.node_count(), 3);

    let rust: uuid::Uuid = RUST_ID.parse().unwrap();
    let helix: uuid::Uuid = HELIX_ID.parse().unwrap();

    assert_eq!(distinct_titles(&graph.get_backlinks(&rust)), ["Helix"]);
    assert_eq!(
        distinct_titles(&graph.get_forward_links(&helix)),
        ["Editors", "Rust"]
    );
    assert_eq!(graph.pending_link_count(), 0, "every link resolved");
}

#[test]
fn a_roam_ref_citation_becomes_a_ref_link() {
    let dir = fixture();
    let (graph, _) = scanner::scan_directory(dir.path());

    let rust: uuid::Uuid = RUST_ID.parse().unwrap();
    let kinds: Vec<_> = graph
        .get_backlinks(&rust)
        .iter()
        .map(|(_, link)| **link)
        .collect();

    // Helix links to Rust twice: once by id, once by citing its :ROAM_REFS:.
    assert_eq!(kinds.len(), 2);
    assert!(kinds.contains(&helix_roam::Link::Id));
    assert!(kinds.contains(&helix_roam::Link::Ref));
}

#[test]
fn reindexing_a_file_replaces_only_its_own_nodes() {
    let dir = fixture();
    let (mut graph, _) = scanner::scan_directory(dir.path());
    let rust: uuid::Uuid = RUST_ID.parse().unwrap();
    let helix: uuid::Uuid = HELIX_ID.parse().unwrap();

    // The user retitles Helix and drops its link to Editors.
    let edited = format!(
        ":PROPERTIES:\n:ID: {HELIX_ID}\n:END:\n#+title: Helix Editor\n\n\
         Written in [[id:{RUST_ID}][Rust]].\n"
    );
    let path = dir.path().join("helix.org");
    fs::write(&path, &edited).unwrap();
    reindex_file(&mut graph, &path, &edited);

    assert_eq!(graph.node_count(), 3, "no node was duplicated");
    assert_eq!(graph.get_node(&helix).unwrap().title, "Helix Editor");
    assert_eq!(titles(&graph.get_forward_links(&helix)), ["Rust"]);
    assert_eq!(
        distinct_titles(&graph.get_backlinks(&rust)),
        ["Helix Editor"]
    );
}

#[test]
fn a_backlink_survives_the_target_being_created_later() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "source.org",
        &format!(":PROPERTIES:\n:ID: {HELIX_ID}\n:END:\n#+title: Source\n\n[[id:{RUST_ID}]]\n"),
    );

    let (mut graph, _) = scanner::scan_directory(dir.path());
    assert_eq!(graph.pending_link_count(), 1, "target does not exist yet");

    // The user creates the missing note and saves it.
    let body = format!(":PROPERTIES:\n:ID: {RUST_ID}\n:END:\n#+title: Rust\n");
    let path = dir.path().join("rust.org");
    fs::write(&path, &body).unwrap();
    reindex_file(&mut graph, &path, &body);

    let rust: uuid::Uuid = RUST_ID.parse().unwrap();
    assert_eq!(titles(&graph.get_backlinks(&rust)), ["Source"]);
    assert_eq!(graph.pending_link_count(), 0);
}

#[test]
fn deleting_a_node_from_a_file_removes_it_from_the_graph() {
    let dir = fixture();
    let (mut graph, _) = scanner::scan_directory(dir.path());

    let path = dir.path().join("editors.org");
    let emptied = "#+title: Editors\n\nThe id is gone.\n";
    fs::write(&path, emptied).unwrap();
    reindex_file(&mut graph, &path, emptied);

    let editors: uuid::Uuid = EDITORS_ID.parse().unwrap();
    assert_eq!(graph.node_count(), 2);
    assert!(!graph.contains_node(&editors));
}

#[tokio::test]
async fn async_scan_and_reindex_share_one_graph() {
    let dir = fixture();
    let graph = Arc::new(RwLock::new(RoamGraph::new()));

    let stats = scanner::scan_directory_async(Arc::clone(&graph), dir.path().to_path_buf())
        .await
        .unwrap();
    assert_eq!(stats.nodes, 3);
    assert_eq!(graph.read().node_count(), 3);

    let path = dir.path().join("editors.org");
    let edited = format!(":PROPERTIES:\n:ID: {EDITORS_ID}\n:END:\n#+title: Text Editors\n");
    scanner::reindex_file_async(Arc::clone(&graph), path, edited)
        .await
        .unwrap();

    let editors: uuid::Uuid = EDITORS_ID.parse().unwrap();
    assert_eq!(
        graph.read().get_node(&editors).unwrap().title,
        "Text Editors"
    );
    assert_eq!(graph.read().node_count(), 3);
}

mod id_locations {
    use helix_roam::{parser, Node, RoamGraph, Uuid};

    fn uuid(last: u8) -> Uuid {
        let mut bytes = [0u8; 16];
        bytes[15] = last;
        Uuid::from_bytes(bytes)
    }

    #[test]
    fn every_id_a_file_declares_is_found() {
        let text = "\
:PROPERTIES:
:ID:       00000000-0000-0000-0000-000000000001
:END:
#+title: Whatever

* A heading
  :PROPERTIES:
  :id:       00000000-0000-0000-0000-000000000002
  :END:
Not an id: 00000000-0000-0000-0000-000000000003.
";
        // Both spellings of the key, and nothing that merely looks like one.
        assert_eq!(parser::ids_in(text), [uuid(1), uuid(2)]);
    }

    #[test]
    fn a_node_answers_for_its_own_location() {
        let mut graph = RoamGraph::new();
        graph.insert_node(Node::new(uuid(1), "Indexed", "/notes/a.org"));

        assert_eq!(
            graph.location(&uuid(1)),
            Some(std::path::Path::new("/notes/a.org"))
        );
        // It is a node, so it is not what the location cache is for.
        assert_eq!(graph.location_count(), 0);
    }

    #[test]
    fn an_id_the_index_never_saw_is_still_reachable() {
        let mut graph = RoamGraph::new();
        graph.register_location(uuid(2), "/elsewhere/b.org");

        assert_eq!(
            graph.location(&uuid(2)),
            Some(std::path::Path::new("/elsewhere/b.org"))
        );
        assert_eq!(graph.location_count(), 1);
        assert_eq!(graph.location(&uuid(9)), None);
    }

    #[test]
    fn re_reading_a_file_forgets_what_it_no_longer_declares() {
        let mut graph = RoamGraph::new();
        graph.register_location(uuid(2), "/notes/a.org");
        graph.register_location(uuid(3), "/elsewhere/b.org");

        graph.remove_nodes_in_file(std::path::Path::new("/notes/a.org"));

        assert_eq!(graph.location(&uuid(2)), None);
        // Another file's ids are none of its business.
        assert_eq!(
            graph.location(&uuid(3)),
            Some(std::path::Path::new("/elsewhere/b.org"))
        );
    }
}
