//! An in-memory Org-Roam v2 knowledge graph.
//!
//! [`RoamGraph`] holds the [`Node`]s parsed out of an Org-Roam directory and
//! the [`Link`]s between them, and answers the two queries the editor needs
//! while browsing a note: what does this node point at, and what points back
//! at it.

mod graph;
mod link;
mod node;

pub use graph::RoamGraph;
pub use link::Link;
pub use node::Node;

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
