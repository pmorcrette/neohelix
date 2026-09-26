//! The graph as a picture: Graphviz's DOT language, for the whole graph or
//! for the neighbourhood of one node.
//!
//! Upstream Org-Roam shells out to Graphviz rather than laying the graph out
//! itself, and so does the fork: this module writes the DOT text, and the
//! editor runs `dot` on it if `dot` is installed. Graphviz is optional; the
//! DOT file is useful without it, and its absence is reported rather than
//! treated as an error.

use std::collections::{BTreeMap, HashSet, VecDeque};

use uuid::Uuid;

use crate::RoamGraph;

/// Titles longer than this are wrapped onto several lines, so one long
/// title does not stretch its box across the picture.
const WRAP: usize = 30;

/// The nodes within `depth` links of `center`, following links both ways:
/// a note that links here is as much a neighbour as one this links to.
pub fn neighbourhood(graph: &RoamGraph, center: Uuid, depth: usize) -> HashSet<Uuid> {
    let mut seen = HashSet::from([center]);
    let mut queue = VecDeque::from([(center, 0)]);

    while let Some((id, distance)) = queue.pop_front() {
        if distance == depth {
            continue;
        }
        let near = graph
            .get_forward_links(&id)
            .into_iter()
            .chain(graph.get_backlinks(&id))
            .map(|(node, _)| node.id);
        for next in near {
            if seen.insert(next) {
                queue.push_back((next, distance + 1));
            }
        }
    }
    seen
}

/// A DOT string, with `"` and `\` escaped and long titles wrapped at word
/// boundaries.
fn label(title: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for word in title.split_whitespace() {
        match lines.last_mut() {
            Some(line) if line.chars().count() + 1 + word.chars().count() <= WRAP => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
        .iter()
        .map(|line| line.replace('\\', "\\\\").replace('"', "\\\""))
        .collect::<Vec<_>>()
        .join("\\n")
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The graph, or the part of it within `depth` links of a node, as DOT.
///
/// Output is sorted, so the same graph always gives the same text — which
/// is what lets a DOT file be diffed or kept under version control.
pub fn dot(graph: &RoamGraph, center: Option<(Uuid, usize)>) -> String {
    let chosen: Option<HashSet<Uuid>> = center.map(|(id, depth)| neighbourhood(graph, id, depth));
    let keep = |id: &Uuid| chosen.as_ref().is_none_or(|chosen| chosen.contains(id));

    let mut nodes: Vec<_> = graph.nodes().filter(|node| keep(&node.id)).collect();
    nodes.sort_by(|a, b| a.title.cmp(&b.title).then(a.id.cmp(&b.id)));

    // Two links between the same notes are one edge in a picture; it is
    // dashed only when every link behind it came through `:ROAM_REFS:`.
    let mut edges: BTreeMap<(Uuid, Uuid), bool> = BTreeMap::new();
    for node in &nodes {
        for (target, link) in graph.get_forward_links(&node.id) {
            if keep(&target.id) && target.id != node.id {
                let only_refs = edges.entry((node.id, target.id)).or_insert(true);
                *only_refs &= link.is_ref();
            }
        }
    }

    let mut out = vec![
        "digraph \"org-roam\" {".to_string(),
        "  graph [rankdir=LR, overlap=false, splines=true];".to_string(),
        "  node [shape=box, style=\"rounded,filled\", fillcolor=\"#f4f4f4\", fontname=\"sans-serif\"];"
            .to_string(),
        "  edge [color=\"#888888\"];".to_string(),
    ];
    for node in &nodes {
        let highlight = if center.is_some_and(|(id, _)| id == node.id) {
            ", fillcolor=\"#ffe8a3\", penwidth=2"
        } else {
            ""
        };
        out.push(format!(
            "  \"{}\" [label=\"{}\", tooltip=\"{}\"{highlight}];",
            node.id,
            label(&node.title),
            escape(&node.file_path.to_string_lossy()),
        ));
    }
    for ((from, to), only_refs) in &edges {
        let style = if *only_refs { " [style=dashed]" } else { "" };
        out.push(format!("  \"{from}\" -> \"{to}\"{style};"));
    }
    out.push("}".to_string());

    let mut text = out.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Link, Node};

    fn id(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    /// a → b → c → d, and d ⇢ e through a ref.
    fn chain() -> RoamGraph {
        let mut graph = RoamGraph::new();
        for (n, title) in [(1, "A"), (2, "B"), (3, "C \"quoted\""), (4, "D"), (5, "E")] {
            graph.insert_node(Node::new(id(n), title, format!("/notes/{n}.org")));
        }
        for (from, to) in [(1, 2), (2, 3), (3, 4), (1, 2)] {
            graph.add_link(id(from), id(to), Link::Id);
        }
        graph.add_link(id(4), id(5), Link::Ref);
        graph
    }

    #[test]
    fn a_neighbourhood_follows_links_both_ways() {
        let graph = chain();
        let near = neighbourhood(&graph, id(2), 1);
        assert_eq!(near, HashSet::from([id(1), id(2), id(3)]));
        let far = neighbourhood(&graph, id(2), 2);
        assert!(far.contains(&id(4)));
        assert!(!far.contains(&id(5)));
        assert!(neighbourhood(&graph, id(2), 3).contains(&id(5)));
    }

    #[test]
    fn the_whole_graph_merges_repeated_links_and_marks_refs() {
        let text = dot(&chain(), None);
        assert!(
            text.contains(&format!("\"{}\" [label=\"E\"", id(5))),
            "{text}"
        );
        let edges = text.lines().filter(|line| line.contains("->")).count();
        assert_eq!(edges, 4, "{text}");
        // A link through `:ROAM_REFS:` is drawn apart from an `id:` link.
        assert!(
            text.contains(&format!("\"{}\" -> \"{}\" [style=dashed];", id(4), id(5))),
            "{text}"
        );
    }

    #[test]
    fn a_neighbourhood_is_drawn_with_its_centre_marked() {
        let text = dot(&chain(), Some((id(2), 1)));
        assert!(!text.contains("label=\"D\""), "{text}");
        assert!(
            text.contains(&format!(
                "\"{}\" [label=\"B\", tooltip=\"/notes/2.org\", fillcolor=\"#ffe8a3\"",
                id(2)
            )),
            "{text}"
        );
        assert!(
            text.contains(&format!("\"{}\" -> \"{}\";", id(2), id(3))),
            "{text}"
        );
        assert!(
            !text.contains(&format!("\"{}\" -> \"{}\";", id(3), id(4))),
            "{text}"
        );
    }

    #[test]
    fn titles_are_escaped_and_wrapped() {
        assert_eq!(label("C \"quoted\""), "C \\\"quoted\\\"");
        assert_eq!(
            label("a title long enough that it has to be wrapped"),
            "a title long enough that it\\nhas to be wrapped"
        );
    }

    #[test]
    fn the_same_graph_gives_the_same_text() {
        assert_eq!(dot(&chain(), None), dot(&chain(), None));
    }
}
