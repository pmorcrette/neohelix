//! The Org-Roam backlinks panel.

use helix_roam::{Link, Node};
use helix_view::graphics::Rect;
use helix_view::Editor;
use tui::buffer::Buffer as Surface;
use tui::widgets::{Block, Widget};

use crate::compositor::{Component, Context, Event, EventResult};

/// Width of the panel, unless the terminal is too narrow for it.
const WIDTH: u16 = 34;
/// Leave at least this much room for the document being edited.
const MIN_DOCUMENT_WIDTH: u16 = 30;

/// A backlink as the panel displays it.
struct Entry {
    title: String,
    location: String,
    kind: Link,
}

/// A panel listing what links to the nodes in the focused document.
///
/// It is passive: every event falls through to the editor underneath, so the
/// panel stays open while editing. It reads the graph on each render rather
/// than caching, so a background re-index shows up on the next frame.
pub struct RoamBacklinks;

impl RoamBacklinks {
    pub const ID: &'static str = "roam-backlinks";

    /// Collects the backlinks of every node in the focused document.
    fn entries(editor: &Editor) -> (Option<String>, Vec<Entry>) {
        let doc = doc!(editor);
        let Some(path) = doc.path().map(ToOwned::to_owned) else {
            return (None, Vec::new());
        };

        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());

        let graph = editor.roam.read();
        let mut entries = Vec::new();
        // A file holds several nodes, and two of them can share a backlink
        // source, so the same source may legitimately appear twice.
        for node in graph.nodes_in_file(&path) {
            entries.extend(
                graph
                    .get_backlinks(&node.id)
                    .into_iter()
                    .map(|(source, link)| Entry {
                        title: source.title.clone(),
                        location: location_of(source),
                        kind: *link,
                    }),
            );
        }

        entries.sort_by(|a, b| a.title.cmp(&b.title).then(a.location.cmp(&b.location)));
        entries.dedup_by(|a, b| a.title == b.title && a.location == b.location && a.kind == b.kind);

        (name, entries)
    }
}

/// `path:line` of a node, relative to the working directory when possible.
fn location_of(node: &Node) -> String {
    let path = helix_stdx::path::get_relative_path(&node.file_path);
    format!("{}:{}", path.display(), node.line + 1)
}

impl Component for RoamBacklinks {
    fn render(&mut self, viewport: Rect, surface: &mut Surface, cx: &mut Context) {
        let width = WIDTH.min(viewport.width.saturating_sub(MIN_DOCUMENT_WIDTH));
        if width < 12 {
            // Nothing legible fits; stay out of the way rather than clipping.
            return;
        }

        // Sit above the statusline, as the editor view does.
        let area = viewport.intersection(Rect::new(
            viewport.width.saturating_sub(width),
            0,
            width,
            viewport.height.saturating_sub(1),
        ));

        let popup_style = cx.editor.theme.get("ui.popup");
        let text_style = cx.editor.theme.get("ui.text");
        let title_style = cx.editor.theme.get("ui.text.focus");
        let muted_style = cx.editor.theme.get("ui.virtual");

        surface.clear_with(area, popup_style);

        let (name, entries) = Self::entries(cx.editor);
        let block = Block::bordered()
            .title(format!("Backlinks ({})", entries.len()))
            .border_style(popup_style);
        let inner = block.inner(area);
        block.render(area, surface);

        if inner.height == 0 {
            return;
        }

        if entries.is_empty() {
            let message = match name {
                Some(name) => format!("No backlinks to {name}"),
                None => "Not a saved file".to_string(),
            };
            surface.set_string_truncated(
                inner.x,
                inner.y,
                &message,
                inner.width as usize,
                |_| muted_style,
                true,
                true,
            );
            return;
        }

        // Two lines per entry: the node's title, then where it lives.
        let mut y = inner.y;
        for entry in &entries {
            if y >= inner.y + inner.height {
                break;
            }

            let marker = match entry.kind {
                Link::Id => "→ ",
                Link::Ref => "⇢ ",
            };
            surface.set_string_truncated(
                inner.x,
                y,
                &format!("{marker}{}", entry.title),
                inner.width as usize,
                |_| title_style,
                true,
                true,
            );
            y += 1;

            if y < inner.y + inner.height {
                surface.set_string_truncated(
                    inner.x,
                    y,
                    &format!("  {}", entry.location),
                    inner.width as usize,
                    |_| text_style,
                    true,
                    true,
                );
                y += 1;
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
