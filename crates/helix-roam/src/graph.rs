use std::collections::HashMap;

use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use petgraph::Direction;
use uuid::Uuid;

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
