//! The Org-Roam panel: what reaches the node you are in, and how.

use helix_roam::{Link, Node, Uuid};
use helix_view::graphics::Rect;
use helix_view::Editor;
use tui::buffer::Buffer as Surface;
use tui::widgets::{Block, Widget};

use crate::compositor::{Component, Context, Event, EventResult};

/// Width of the panel, unless the terminal is too narrow for it.
const WIDTH: u16 = 34;
/// Leave at least this much room for the document being edited.
const MIN_DOCUMENT_WIDTH: u16 = 30;

/// One row of a section: what reaches the node, and from where.
struct Entry {
    what: String,
    location: String,
    /// `None` for an unlinked reference, which is not a link yet.
    kind: Option<Link>,
}

/// A run of entries under a heading.
struct Section {
    title: String,
    entries: Vec<Entry>,
    /// Shown instead of the entries when there are none.
    empty: String,
}

/// A panel listing what reaches the nodes in the focused document.
///
/// It is passive: every event falls through to the editor underneath, so the
/// panel stays open while editing. Links are read from the graph on each
/// render rather than cached, so a background re-index shows up on the next
/// frame; unlinked references are not, because finding them reads every file
/// in the notes directory.
pub struct RoamPanel;

impl RoamPanel {
    pub const ID: &'static str = "roam-backlinks";

    /// The sections to draw, pinned node first when there is one.
    fn sections(editor: &Editor) -> Vec<Section> {
        let mut sections = Vec::new();

        if let Some(pinned) = editor.roam_pinned {
            let graph = editor.roam.read();
            match graph.get_node(&pinned) {
                Some(node) => {
                    let (ids, refs) = links_to(editor, std::slice::from_ref(&pinned));
                    sections.push(Section {
                        title: format!("Pinned: {}", node.title),
                        entries: ids,
                        empty: "no links".to_string(),
                    });
                    if !refs.is_empty() {
                        sections.push(Section {
                            title: "  via refs".to_string(),
                            entries: refs,
                            empty: String::new(),
                        });
                    }
                }
                None => sections.push(Section {
                    title: "Pinned".to_string(),
                    entries: Vec::new(),
                    empty: "that node is gone from the index".to_string(),
                }),
            }
        }

        let here: Vec<Uuid> = {
            let doc = doc!(editor);
            match doc.path() {
                Some(path) => editor
                    .roam
                    .read()
                    .nodes_in_file(path)
                    .map(|node| node.id)
                    .collect(),
                None => Vec::new(),
            }
        };

        if here.is_empty() {
            sections.push(Section {
                title: "Backlinks".to_string(),
                entries: Vec::new(),
                empty: match doc!(editor).path() {
                    Some(_) => "no indexed node in this file".to_string(),
                    None => "not a saved file".to_string(),
                },
            });
            return sections;
        }

        let (ids, refs) = links_to(editor, &here);
        sections.push(Section {
            title: format!("Backlinks ({})", ids.len()),
            entries: ids,
            empty: "nothing links here yet".to_string(),
        });
        sections.push(Section {
            // Upstream's own name for links that arrive through a
            // `:ROAM_REFS:` key rather than through an `id:` link.
            title: format!("Reflinks ({})", refs.len()),
            entries: refs,
            empty: "no refs point here".to_string(),
        });

        let unlinked = match &editor.roam_unlinked {
            // The cache belongs to one node; showing another node's
            // references under this one's heading would be a lie.
            Some(found) if here.contains(&found.node) => found
                .entries
                .iter()
                .map(|(what, location)| Entry {
                    what: what.clone(),
                    location: location.clone(),
                    kind: None,
                })
                .collect(),
            _ => Vec::new(),
        };
        sections.push(Section {
            title: format!("Unlinked ({})", unlinked.len()),
            entries: unlinked,
            empty: "run :roam-unlinked-references".to_string(),
        });

        sections
    }
}

/// The links reaching any of `nodes`, split by how they arrive.
fn links_to(editor: &Editor, nodes: &[Uuid]) -> (Vec<Entry>, Vec<Entry>) {
    let graph = editor.roam.read();
    let mut ids = Vec::new();
    let mut refs = Vec::new();

    // A file holds several nodes, and two of them can share a backlink
    // source, so the same source may legitimately appear twice.
    for id in nodes {
        for (source, link) in graph.get_backlinks(id) {
            let entry = Entry {
                what: source.title.clone(),
                location: location_of(source),
                kind: Some(*link),
            };
            match link {
                Link::Id => ids.push(entry),
                Link::Ref => refs.push(entry),
            }
        }
    }

    for side in [&mut ids, &mut refs] {
        side.sort_by(|a, b| a.what.cmp(&b.what).then(a.location.cmp(&b.location)));
        side.dedup_by(|a, b| a.what == b.what && a.location == b.location);
    }

    (ids, refs)
}

/// `path:line` of a node, relative to the working directory when possible.
fn location_of(node: &Node) -> String {
    let path = helix_stdx::path::get_relative_path(&node.file_path);
    format!("{}:{}", path.display(), node.line + 1)
}

impl Component for RoamPanel {
    fn render(&mut self, viewport: Rect, surface: &mut Surface, cx: &mut Context) {
        // Docked, the documents made room for it; otherwise it covers their
        // right edge.
        let area = match cx.editor.dock.area_of(Self::ID) {
            Some(area) => area,
            None => {
                let width = WIDTH.min(viewport.width.saturating_sub(MIN_DOCUMENT_WIDTH));
                if width < 12 {
                    // Nothing legible fits; stay out of the way rather than
                    // clipping.
                    return;
                }
                // Sit above the statusline, as the editor view does.
                viewport.intersection(Rect::new(
                    viewport.width.saturating_sub(width),
                    0,
                    width,
                    viewport.height.saturating_sub(1),
                ))
            }
        };

        let popup_style = cx.editor.theme.get("ui.popup");
        let text_style = cx.editor.theme.get("ui.text");
        let title_style = cx.editor.theme.get("ui.text.focus");
        let heading_style = cx.editor.theme.get("ui.menu.selected");
        let muted_style = cx.editor.theme.get("ui.virtual");

        surface.clear_with(area, popup_style);

        let sections = Self::sections(cx.editor);
        let block = Block::bordered().title("Roam").border_style(popup_style);
        let inner = block.inner(area);
        block.render(area, surface);

        if inner.height == 0 {
            return;
        }

        let bottom = inner.y + inner.height;
        let mut y = inner.y;
        let mut put = |y: u16, text: &str, style, elide_start: bool| {
            surface.set_string_truncated(
                inner.x,
                y,
                text,
                inner.width as usize,
                |_| style,
                true,
                elide_start,
            );
        };

        for section in &sections {
            if y >= bottom {
                break;
            }
            put(y, &section.title, heading_style, false);
            y += 1;

            if section.entries.is_empty() {
                if !section.empty.is_empty() && y < bottom {
                    put(y, &format!("  {}", section.empty), muted_style, false);
                    y += 1;
                }
                continue;
            }

            // Two lines per entry: what reaches the node, then where it is.
            for entry in &section.entries {
                if y >= bottom {
                    break;
                }
                let marker = match entry.kind {
                    Some(Link::Id) => "→ ",
                    Some(Link::Ref) => "⇢ ",
                    None => "· ",
                };
                put(y, &format!("{marker}{}", entry.what), title_style, false);
                y += 1;

                if y < bottom {
                    // A long path is more useful from its tail, so this one
                    // elides the start.
                    put(y, &format!("  {}", entry.location), text_style, true);
                    y += 1;
                }
            }
        }
    }

    fn handle_event(&mut self, _event: &Event, _cx: &mut Context) -> EventResult {
        // Passive panel: editing continues underneath it.
        EventResult::Ignored(None)
    }

    fn id(&self) -> Option<&'static str> {
        Some(Self::ID)
    }
}
