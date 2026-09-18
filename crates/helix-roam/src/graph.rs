use std::collections::HashMap;
use std::path::Path;

use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use petgraph::Direction;
use uuid::Uuid;

use crate::query::NodeQuery;
use crate::{Link, Node};

/// An in-memory Org-Roam v2 knowledge graph.
///
/// Nodes are addressed by their `:ID:` property; the [`NodeIndex`] values
/// petgraph hands out stay an implementation detail so that indices can never
/// be held across a mutation. `RoamGraph` is `Send + Sync`, so a shared
/// instance lives behind the caller's lock of choice — typically
/// `Arc<RwLock<RoamGraph>>`.
///
/// The `Uuid -> NodeIndex` map turns an `:ID:` into a graph position in
/// `O(1)`, and petgraph keeps a separate incoming and outgoing edge chain per
/// node, so backlinks cost the same as forward links: no query scans the
/// graph.
#[derive(Debug, Clone, Default)]
pub struct RoamGraph {
    graph: DiGraph<Node, Link>,
    /// Maps a node's `:ID:` to its position in `graph`.
    indices: HashMap<Uuid, NodeIndex>,
    /// Links whose endpoints were not both known when they were recorded.
    ///
    /// A file may link to a node in a file that has not been indexed yet, so
    /// [`RoamGraph::add_link_deferred`] parks those here until
    /// [`RoamGraph::resolve_pending_links`] can place them.
    pending: Vec<(Uuid, Uuid, Link)>,
    /// Bibliography keys, each mapped to the nodes citing them.
    citations: HashMap<String, Vec<Uuid>>,
    /// `:ROAM_REFS:` keys, each mapped to the node claiming it.
    ///
    /// A link whose target is not an `[[id:…]]` resolves through this map, so
    /// citing a node's external identifier produces a [`Link::Ref`] edge.
    refs: HashMap<String, Uuid>,
}

impl RoamGraph {
    /// Creates an empty graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts `node`, or replaces the node already registered under the same
    /// `:ID:`.
    ///
    /// Replacing keeps every edge attached to the node, so re-parsing a file
    /// refreshes titles, tags and aliases without dropping the links other
    /// files point at it with.
    pub fn insert_node(&mut self, node: Node) {
        match self.indices.get(&node.id) {
            Some(&index) => self.graph[index] = node,
            None => {
                let id = node.id;
                let index = self.graph.add_node(node);
                self.indices.insert(id, index);
            }
        }
    }

    /// Records that `source` links to `target` through `link`.
    ///
    /// Returns `false` and leaves the graph untouched when either endpoint is
    /// unknown: an Org file may be parsed before the file it links to, and a
    /// dangling edge would have no node to hang off.
    pub fn add_link(&mut self, source: Uuid, target: Uuid, link: Link) -> bool {
        let (Some(&source), Some(&target)) = (self.indices.get(&source), self.indices.get(&target))
        else {
            return false;
        };

        self.graph.add_edge(source, target, link);
        true
    }

    /// The nodes linking *to* `node_id`, with the link each one used.
    ///
    /// Locating the node is `O(1)`, and only that node's incoming edge chain
    /// is walked — never the whole graph — so the call costs `O(1)` plus the
    /// number of backlinks it returns.
    ///
    /// Returns an empty vector for an unknown id. The order is unspecified.
    pub fn get_backlinks(&self, node_id: &Uuid) -> Vec<(&Node, &Link)> {
        self.neighbours(node_id, Direction::Incoming)
    }

    /// The nodes `node_id` links *out* to, with the link each one uses.
    ///
    /// Costs `O(1)` plus the number of links returned, like
    /// [`RoamGraph::get_backlinks`].
    ///
    /// Returns an empty vector for an unknown id. The order is unspecified.
    pub fn get_forward_links(&self, node_id: &Uuid) -> Vec<(&Node, &Link)> {
        self.neighbours(node_id, Direction::Outgoing)
    }

    /// Records a link, parking it if either endpoint is still unknown.
    ///
    /// Indexing visits files in directory order, so a link is routinely seen
    /// before its target. Unlike [`RoamGraph::add_link`], this never drops
    /// one: [`RoamGraph::resolve_pending_links`] places it once both ends are
    /// in the graph.
    pub fn add_link_deferred(&mut self, source: Uuid, target: Uuid, link: Link) {
        if !self.add_link(source, target, link) {
            self.pending.push((source, target, link));
        }
    }

    /// Places every parked link whose endpoints are now both known.
    ///
    /// Returns how many were placed. Links whose source no longer exists are
    /// discarded; links whose target is still missing stay parked, so a node
    /// added by a later save picks up the backlinks pointing at it.
    pub fn resolve_pending_links(&mut self) -> usize {
        let mut placed = 0;
        let mut still_pending = Vec::new();

        for (source, target, link) in std::mem::take(&mut self.pending) {
            if self.add_link(source, target, link) {
                placed += 1;
            } else if self.indices.contains_key(&source) {
                still_pending.push((source, target, link));
            }
        }

        self.pending = still_pending;
        placed
    }

    /// The number of links still waiting for their target to be indexed.
    pub fn pending_link_count(&self) -> usize {
        self.pending.len()
    }

    /// Drops every node parsed out of `path`, along with its links.
    ///
    /// Returns the ids that were removed, so a caller re-indexing the file can
    /// tell which nodes disappeared from it.
    pub fn remove_nodes_in_file(&mut self, path: &Path) -> Vec<Uuid> {
        let doomed: Vec<Uuid> = self
            .graph
            .node_weights()
            .filter(|node| node.file_path == path)
            .map(|node| node.id)
            .collect();

        for id in &doomed {
            self.remove_node(id);
        }

        // Links out of the removed nodes must not linger as pending work.
        self.pending
            .retain(|(source, _, _)| self.indices.contains_key(source));

        doomed
    }

    /// Registers a `:ROAM_REFS:` key for `node_id`.
    pub fn register_ref(&mut self, key: impl Into<String>, node_id: Uuid) {
        self.refs.insert(key.into(), node_id);
    }

    /// The node claiming `key` as one of its `:ROAM_REFS:`.
    /// Records that `node_id` cites `key`.
    pub fn add_citation(&mut self, key: impl Into<String>, node_id: Uuid) {
        let citing = self.citations.entry(key.into()).or_default();
        if !citing.contains(&node_id) {
            citing.push(node_id);
        }
    }

    /// The nodes citing `key`.
    pub fn cited_by(&self, key: &str) -> Vec<&Node> {
        self.citations
            .get(key)
            .map(|ids| ids.iter().filter_map(|id| self.get_node(id)).collect())
            .unwrap_or_default()
    }

    /// Every bibliography key the graph has seen.
    pub fn citation_keys(&self) -> impl Iterator<Item = &str> {
        self.citations.keys().map(String::as_str)
    }

    /// The nodes matching `query`, in insertion order.
    pub fn query(&self, query: &NodeQuery) -> Vec<&Node> {
        self.nodes().filter(|node| query.matches(node)).collect()
    }

    pub fn resolve_ref(&self, key: &str) -> Option<Uuid> {
        self.refs.get(key).copied()
    }

    /// Removes a node and every link touching it.
    ///
    /// `petgraph::Graph` fills the hole by moving its last node into the freed
    /// slot, which silently invalidates that node's cached index, so the
    /// `:ID:` map is repaired here rather than left stale.
    pub fn remove_node(&mut self, node_id: &Uuid) -> Option<Node> {
        // A node that is going away must not leave its citations behind.
        self.citations.retain(|_, citing| {
            citing.retain(|id| id != node_id);
            !citing.is_empty()
        });

        let index = self.indices.remove(node_id)?;
        let last = NodeIndex::new(self.graph.node_count() - 1);

        let removed = self.graph.remove_node(index)?;
        self.refs.retain(|_, claimant| claimant != node_id);

        if index != last {
            let moved = self.graph[index].id;
            self.indices.insert(moved, index);
        }

        Some(removed)
    }

    /// Looks a node up by its `:ID:`, in `O(1)`.
    pub fn get_node(&self, node_id: &Uuid) -> Option<&Node> {
        let index = *self.indices.get(node_id)?;
        self.graph.node_weight(index)
    }

    /// Whether a node with this `:ID:` is registered.
    pub fn contains_node(&self, node_id: &Uuid) -> bool {
        self.indices.contains_key(node_id)
    }

    /// The number of nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    /// The number of links in the graph.
    pub fn link_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Whether the graph holds no nodes.
    pub fn is_empty(&self) -> bool {
        self.graph.node_count() == 0
    }

    /// Every node parsed out of `path`, in no particular order.
    pub fn nodes_in_file<'a>(&'a self, path: &'a Path) -> impl Iterator<Item = &'a Node> + 'a {
        self.graph
            .node_weights()
            .filter(move |node| node.file_path == path)
    }

    /// Every node in the graph, in no particular order.
    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.graph.node_weights()
    }

    fn neighbours(&self, node_id: &Uuid, direction: Direction) -> Vec<(&Node, &Link)> {
        let Some(&index) = self.indices.get(node_id) else {
            return Vec::new();
        };

        self.graph
            .edges_directed(index, direction)
            .map(|edge| {
                let neighbour = match direction {
                    Direction::Incoming => edge.source(),
                    Direction::Outgoing => edge.target(),
                };
                (&self.graph[neighbour], edge.weight())
            })
            .collect()
    }
}
