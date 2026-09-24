//! An in-memory Org-Roam v2 knowledge graph.
//!
//! [`RoamGraph`] holds the [`Node`]s parsed out of an Org-Roam directory and
//! the [`Link`]s between them, and answers the two queries the editor needs
//! while browsing a note: what does this node point at, and what points back
//! at it.

pub mod agenda;
pub mod attach;
pub mod babel;
pub mod capture;
pub mod clip;
pub mod clock;
pub mod columns;
pub mod date;
pub mod dynamic;
pub mod export;
mod graph;
pub mod hyperlink;
mod link;
pub mod list;
pub mod logging;
pub mod markup;
mod node;
pub mod outline;
pub mod parser;
pub mod query;
pub mod restructure;
pub mod scanner;
pub mod sort;
pub mod source;
pub mod startup;
pub mod table;
pub mod unlinked;
pub mod visual;

pub use clip::Clip;
pub use date::Date;
pub use graph::RoamGraph;
pub use hyperlink::{find_links, link_at, LinkKind, OrgLink};
pub use link::Link;
pub use node::{Node, Repeater, RepeaterKind, RepeaterUnit, Timestamp, TodoState};
pub use parser::{parse_org, FileSettings, ParsedFile};
pub use query::NodeQuery;
pub use scanner::{reindex_file, scan_directory, IndexStats};
pub use sort::SortKey;
pub use uuid::Uuid;

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    /// `RoamGraph` has to cross thread boundaries so the editor can share one
    /// behind an `Arc<RwLock<_>>`.
    const fn _assert_thread_safe<T: Send + Sync>() {}
    const _: () = _assert_thread_safe::<RoamGraph>();

    fn node(title: &str) -> Node {
        Node::new(Uuid::new_v4(), title, format!("/notes/{title}.org"))
    }

    fn titles(links: &[(&Node, &Link)]) -> Vec<String> {
        let mut titles: Vec<_> = links.iter().map(|(node, _)| node.title.clone()).collect();
        titles.sort();
        titles
    }

    #[test]
    fn insert_node_registers_the_node_by_id() {
        let mut graph = RoamGraph::new();
        let rust = node("rust");
        let id = rust.id;

        assert!(graph.is_empty());
        graph.insert_node(rust);

        assert_eq!(graph.node_count(), 1);
        assert!(graph.contains_node(&id));
        assert_eq!(graph.get_node(&id).unwrap().title, "rust");
    }

    #[test]
    fn re_inserting_an_id_updates_the_node_and_keeps_its_links() {
        let mut graph = RoamGraph::new();
        let rust = node("rust");
        let helix = node("helix");
        let (rust_id, helix_id) = (rust.id, helix.id);

        graph.insert_node(rust);
        graph.insert_node(helix);
        assert!(graph.add_link(helix_id, rust_id, Link::Id));

        graph.insert_node(Node::new(rust_id, "Rust", "/notes/rust.org").with_aliases(["rustlang"]));

        assert_eq!(graph.node_count(), 2);
        assert_eq!(graph.get_node(&rust_id).unwrap().title, "Rust");
        assert!(graph.get_node(&rust_id).unwrap().matches_title("rustlang"));
        assert_eq!(titles(&graph.get_backlinks(&rust_id)), ["helix"]);
    }

    #[test]
    fn links_are_reported_from_both_ends() {
        let mut graph = RoamGraph::new();
        let rust = node("rust");
        let helix = node("helix");
        let editors = node("editors");
        let (rust_id, helix_id, editors_id) = (rust.id, helix.id, editors.id);

        graph.insert_node(rust);
        graph.insert_node(helix);
        graph.insert_node(editors);

        assert!(graph.add_link(helix_id, rust_id, Link::Id));
        assert!(graph.add_link(helix_id, editors_id, Link::Ref));
        assert_eq!(graph.link_count(), 2);

        assert_eq!(
            titles(&graph.get_forward_links(&helix_id)),
            ["editors", "rust"]
        );
        assert!(graph.get_forward_links(&rust_id).is_empty());

        assert_eq!(titles(&graph.get_backlinks(&rust_id)), ["helix"]);
        assert_eq!(titles(&graph.get_backlinks(&editors_id)), ["helix"]);
        assert!(graph.get_backlinks(&helix_id).is_empty());
    }

    #[test]
    fn link_kinds_are_preserved() {
        let mut graph = RoamGraph::new();
        let paper = node("paper");
        let citation = node("citation");
        let (paper_id, citation_id) = (paper.id, citation.id);

        graph.insert_node(paper);
        graph.insert_node(citation);
        graph.add_link(citation_id, paper_id, Link::Ref);

        let backlinks = graph.get_backlinks(&paper_id);
        assert_eq!(backlinks.len(), 1);
        assert!(backlinks[0].1.is_ref());
        assert!(!backlinks[0].1.is_id());
    }

    #[test]
    fn add_link_rejects_unknown_endpoints() {
        let mut graph = RoamGraph::new();
        let rust = node("rust");
        let rust_id = rust.id;
        let unknown = Uuid::new_v4();

        graph.insert_node(rust);

        assert!(!graph.add_link(rust_id, unknown, Link::Id));
        assert!(!graph.add_link(unknown, rust_id, Link::Id));
        assert_eq!(graph.link_count(), 0);
    }

    #[test]
    fn queries_on_unknown_ids_are_empty() {
        let graph = RoamGraph::new();
        let unknown = Uuid::new_v4();

        assert!(graph.get_node(&unknown).is_none());
        assert!(graph.get_backlinks(&unknown).is_empty());
        assert!(graph.get_forward_links(&unknown).is_empty());
    }

    #[test]
    fn removing_a_node_keeps_every_other_lookup_valid() {
        // petgraph fills the freed slot with its last node, which invalidates
        // that node's cached index unless the id map is repaired.
        let mut graph = RoamGraph::new();
        let nodes: Vec<Node> = (0..8).map(|i| node(&format!("n{i}"))).collect();
        let ids: Vec<Uuid> = nodes.iter().map(|n| n.id).collect();

        for node in nodes {
            graph.insert_node(node);
        }
        // A hub every other node links to.
        for (i, id) in ids.iter().enumerate() {
            if i != 0 {
                assert!(graph.add_link(*id, ids[0], Link::Id));
            }
        }

        // Remove from the middle, which is where the swap-with-last happens.
        assert_eq!(graph.remove_node(&ids[3]).unwrap().title, "n3");

        assert_eq!(graph.node_count(), 7);
        assert!(!graph.contains_node(&ids[3]));
        for (i, id) in ids.iter().enumerate() {
            if i == 3 {
                continue;
            }
            let found = graph.get_node(id).expect("node should still resolve");
            assert_eq!(found.id, *id, "id map points at the wrong node");
            assert_eq!(found.title, format!("n{i}"));
        }

        // n3's link to the hub went with it; the other six remain.
        assert_eq!(graph.get_backlinks(&ids[0]).len(), 6);
    }

    #[test]
    fn removing_the_last_node_is_handled() {
        let mut graph = RoamGraph::new();
        let a = node("a");
        let b = node("b");
        let (a_id, b_id) = (a.id, b.id);
        graph.insert_node(a);
        graph.insert_node(b);

        assert!(graph.remove_node(&b_id).is_some());
        assert_eq!(graph.get_node(&a_id).unwrap().title, "a");

        assert!(graph.remove_node(&a_id).is_some());
        assert!(graph.is_empty());
        assert!(graph.remove_node(&a_id).is_none());
    }

    #[test]
    fn remove_nodes_in_file_drops_only_that_file() {
        let mut graph = RoamGraph::new();
        let keep = node("keep");
        let drop_a = Node::new(Uuid::new_v4(), "a", "/notes/gone.org");
        let drop_b = Node::new(Uuid::new_v4(), "b", "/notes/gone.org");
        let (keep_id, drop_a_id) = (keep.id, drop_a.id);

        graph.insert_node(keep);
        graph.insert_node(drop_a);
        graph.insert_node(drop_b);
        assert!(graph.add_link(drop_a_id, keep_id, Link::Id));

        let removed = graph.remove_nodes_in_file(std::path::Path::new("/notes/gone.org"));

        assert_eq!(removed.len(), 2);
        assert_eq!(graph.node_count(), 1);
        assert!(graph.contains_node(&keep_id));
        assert!(graph.get_backlinks(&keep_id).is_empty());
    }

    #[test]
    fn deferred_links_are_placed_once_their_target_appears() {
        let mut graph = RoamGraph::new();
        let source = node("source");
        let source_id = source.id;
        let target_id = Uuid::new_v4();

        graph.insert_node(source);
        graph.add_link_deferred(source_id, target_id, Link::Id);

        assert_eq!(graph.pending_link_count(), 1);
        assert_eq!(graph.link_count(), 0);

        // The target is indexed later, by a save of another file.
        graph.insert_node(Node::new(target_id, "target", "/notes/target.org"));
        assert_eq!(graph.resolve_pending_links(), 1);

        assert_eq!(graph.pending_link_count(), 0);
        assert_eq!(titles(&graph.get_backlinks(&target_id)), ["source"]);
    }

    #[test]
    fn pending_links_are_dropped_with_their_source() {
        let mut graph = RoamGraph::new();
        let source = node("source");
        let source_id = source.id;
        graph.insert_node(source);
        graph.add_link_deferred(source_id, Uuid::new_v4(), Link::Id);

        graph.remove_nodes_in_file(std::path::Path::new("/notes/source.org"));

        assert_eq!(graph.pending_link_count(), 0);
        assert_eq!(graph.resolve_pending_links(), 0);
    }

    #[test]
    fn refs_resolve_to_the_node_claiming_them() {
        let mut graph = RoamGraph::new();
        let paper = node("paper");
        let paper_id = paper.id;
        graph.insert_node(paper);
        graph.register_ref("https://example.test/paper", paper_id);

        assert_eq!(
            graph.resolve_ref("https://example.test/paper"),
            Some(paper_id)
        );
        assert_eq!(graph.resolve_ref("https://example.test/other"), None);

        // A ref does not outlive the node that claimed it.
        graph.remove_node(&paper_id);
        assert_eq!(graph.resolve_ref("https://example.test/paper"), None);
    }

    #[test]
    fn nodes_carry_tags_and_aliases() {
        let rust = node("rust")
            .with_tags(["lang", "systems"])
            .with_aliases(["rustlang"]);

        assert_eq!(rust.tags, ["lang", "systems"]);
        assert!(rust.matches_title("rust"));
        assert!(rust.matches_title("rustlang"));
        assert!(!rust.matches_title("go"));
        assert_eq!(rust.file_path(), std::path::Path::new("/notes/rust.org"));
    }
}
