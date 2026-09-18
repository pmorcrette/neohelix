//! What the index stores about a node, beyond its title and its links.

use helix_roam::Timestamp;
use helix_roam::{parse_org, scan_directory, NodeQuery};

fn ts(year: i32, month: u32, day: u32) -> Timestamp {
    Timestamp {
        year,
        month,
        day,
        hour: None,
        minute: None,
        active: true,
    }
}

const SAMPLE: &str = "\
#+title: Project
#+TODO: TODO NEXT | DONE
* NEXT [#A] Ship the parser
SCHEDULED: <2026-09-18 Fri> DEADLINE: <2026-09-25 Fri>
:PROPERTIES:
:ID: 6ba7b810-9dad-11d1-80b4-00c04fd430c8
:EFFORT: 2h
:END:
Cites [cite:@knuth1984] and [cite/t:@dijkstra1968;@hoare1969].
** A heading with no id of its own
*** DONE Nested and finished             :deep:
:PROPERTIES:
:ID: 6ba7b811-9dad-11d1-80b4-00c04fd430c8
:END:
";

#[test]
fn a_headlines_metadata_is_stored_rather_than_discarded() {
    let file = parse_org(SAMPLE, "p.org");
    let node = file
        .nodes
        .iter()
        .find(|n| n.title == "Ship the parser")
        .expect("the headline is a node");

    assert_eq!(node.level, 1);
    assert_eq!(node.todo.as_ref().unwrap().keyword, "NEXT");
    assert!(!node.todo.as_ref().unwrap().done);
    assert_eq!(node.priority, Some('A'));
    assert_eq!(node.scheduled, Some(ts(2026, 9, 18)));
    assert_eq!(node.deadline, Some(ts(2026, 9, 25)));
}

#[test]
fn a_done_keyword_is_recorded_as_done() {
    let file = parse_org(SAMPLE, "p.org");
    let node = file
        .nodes
        .iter()
        .find(|n| n.title == "Nested and finished")
        .unwrap();

    assert!(node.todo.as_ref().unwrap().done);
    assert_eq!(node.tags, ["deep"]);
}

#[test]
fn the_outline_path_includes_ancestors_that_are_not_nodes() {
    let file = parse_org(SAMPLE, "p.org");
    let node = file
        .nodes
        .iter()
        .find(|n| n.title == "Nested and finished")
        .unwrap();

    // The middle heading has no `:ID:`, so it is not a node — but it is still
    // a level of the outline and must appear.
    assert_eq!(
        node.outline_path,
        ["Ship the parser", "A heading with no id of its own"]
    );
    assert_eq!(node.level, 3);
}

#[test]
fn drawer_properties_are_kept_without_duplicating_the_modelled_ones() {
    let file = parse_org(SAMPLE, "p.org");
    let node = file
        .nodes
        .iter()
        .find(|n| n.title == "Ship the parser")
        .unwrap();

    assert_eq!(node.properties, [("effort".to_string(), "2h".to_string())]);
}

#[test]
fn citations_are_indexed_as_their_own_relation() {
    let file = parse_org(SAMPLE, "p.org");
    let keys: Vec<&str> = file.citations.iter().map(|(k, _)| k.as_str()).collect();

    // Both bracket styles, and several keys in one bracket.
    assert_eq!(keys, ["knuth1984", "dijkstra1968", "hoare1969"]);
}

#[test]
fn the_graph_can_be_asked_who_cites_a_key() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("p.org"), SAMPLE).unwrap();
    let (graph, _) = scan_directory(dir.path());

    let citing = graph.cited_by("knuth1984");
    assert_eq!(citing.len(), 1);
    assert_eq!(citing[0].title, "Ship the parser");

    assert!(graph.cited_by("nobody").is_empty());
}

#[test]
fn a_query_filters_on_the_new_fields() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("p.org"), SAMPLE).unwrap();
    let (graph, _) = scan_directory(dir.path());

    // Unfinished only: the DONE node is excluded, and so is the file node,
    // which has no state at all.
    let unfinished = graph.query(&NodeQuery::unfinished());
    assert_eq!(unfinished.len(), 1);
    assert_eq!(unfinished[0].title, "Ship the parser");

    // Due before its deadline finds nothing; due on it finds the node.
    let query = NodeQuery::unfinished().due_by(ts(2026, 9, 24));
    assert!(graph.query(&query).is_empty());
    let query = NodeQuery::unfinished().due_by(ts(2026, 9, 25));
    assert_eq!(graph.query(&query).len(), 1);

    // A tag the DONE node carries, combined with doneness.
    let query = NodeQuery {
        done: Some(true),
        ..NodeQuery::default()
    }
    .with_tag("deep");
    assert_eq!(graph.query(&query).len(), 1);

    // A property from the drawer. Two nodes match, not one: the nested node
    // inherits `:EFFORT:` from the entry above it, which is what property
    // inheritance means.
    let query = NodeQuery {
        property: Some(("EFFORT".into(), "2h".into())),
        ..NodeQuery::default()
    };
    assert_eq!(graph.query(&query).len(), 2);

    // Asking only what an entry declares itself still distinguishes them.
    let declared = graph
        .nodes()
        .filter(|node| node.properties.iter().any(|(key, _)| key == "effort"))
        .count();
    assert_eq!(declared, 1);
}

#[test]
fn the_default_query_matches_every_node() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("p.org"), SAMPLE).unwrap();
    let (graph, _) = scan_directory(dir.path());

    assert_eq!(graph.query(&NodeQuery::default()).len(), graph.node_count());
}

#[test]
fn timestamps_with_a_time_and_with_a_repeater_are_both_read() {
    let file = parse_org(
        "* TODO Standup\nSCHEDULED: <2026-09-18 Fri 09:30 +1w>\n:PROPERTIES:\n:ID: a\n:END:\n",
        "p.org",
    );
    let at = file.nodes[0].scheduled.unwrap();

    assert_eq!(at.date(), (2026, 9, 18));
    assert_eq!((at.hour, at.minute), (Some(9), Some(30)));
    assert!(at.active, "an angle-bracket timestamp is active");
}

#[test]
fn an_inactive_timestamp_is_marked_as_such() {
    let file = parse_org(
        "* Note\nDEADLINE: [2026-09-18 Fri]\n:PROPERTIES:\n:ID: a\n:END:\n",
        "p.org",
    );
    assert!(!file.nodes[0].deadline.unwrap().active);
}

#[test]
fn reindexing_a_file_forgets_the_citations_it_used_to_make() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.org");
    std::fs::write(&path, SAMPLE).unwrap();
    let (mut graph, _) = scan_directory(dir.path());
    assert_eq!(graph.cited_by("knuth1984").len(), 1);

    // The citation is gone from the file; it must be gone from the graph.
    helix_roam::reindex_file(
        &mut graph,
        &path,
        "* TODO Ship the parser\n:PROPERTIES:\n:ID: 6ba7b810-9dad-11d1-80b4-00c04fd430c8\n:END:\n",
    );
    assert!(
        graph.cited_by("knuth1984").is_empty(),
        "a citation outlived the text that made it"
    );
}
