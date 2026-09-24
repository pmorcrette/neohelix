//! The Magit-style log: the commit graph, and the commits to act on.
//!
//! One row per line `git log --graph` prints. Lines that only draw the graph
//! are shown but skipped by the cursor, which always rests on a commit.

use std::path::PathBuf;

use helix_magit::log::{read_log, LogEntry, LogFilter};
use helix_magit::transient::MenuKind;
use helix_view::graphics::Rect;
use helix_view::input::{KeyCode, KeyModifiers};
use helix_view::Editor;
use tui::buffer::Buffer as Surface;
use tui::widgets::{Block, Widget};

use crate::compositor::{Component, Compositor, Context, Event, EventResult};
use crate::ui::diff_view::DiffView;
use crate::ui::transient::TransientOverlay;

pub struct LogView {
    workdir: PathBuf,
    filter: LogFilter,
    entries: Vec<LogEntry>,
    /// Index into `entries`, always a commit's line when there is one.
    cursor: usize,
    scroll: usize,
    /// git's complaint, when the log could not be read.
    error: Option<String>,
}

/// Which filter a prompt sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refine {
    Message,
    Author,
    File,
    Range,
}

impl LogView {
    pub const ID: &'static str = "magit-log";

    pub fn new(workdir: PathBuf, filter: LogFilter) -> Self {
        let mut view = Self {
            workdir,
            filter,
            entries: Vec::new(),
            cursor: 0,
            scroll: 0,
            error: None,
        };
        view.reload();
        view
    }

    /// Reads the log again, keeping the cursor on the same commit when it
    /// is still there.
    pub fn reload(&mut self) {
        let current = self.current_hash().map(str::to_string);
        match read_log(&self.workdir, &self.filter) {
            Ok(entries) => {
                self.entries = entries;
                self.error = None;
            }
            Err(err) => {
                self.entries = Vec::new();
                self.error = Some(err);
            }
        }
        self.cursor = current
            .and_then(|hash| {
                self.entries
                    .iter()
                    .position(|entry| entry.hash.as_deref() == Some(hash.as_str()))
            })
            .or_else(|| self.entries.iter().position(|entry| entry.hash.is_some()))
            .unwrap_or(0);
    }

    fn current_hash(&self) -> Option<&str> {
        self.entries.get(self.cursor)?.hash.as_deref()
    }

    /// Moves by `delta` commits, stepping over graph-only lines.
    fn move_cursor(&mut self, delta: isize) {
        let commits: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.hash.is_some())
            .map(|(index, _)| index)
            .collect();
        if commits.is_empty() {
            return;
        }
        let at = commits
            .iter()
            .position(|&index| index >= self.cursor)
            .unwrap_or(0) as isize;
        let target = (at + delta).clamp(0, commits.len() as isize - 1) as usize;
        self.cursor = commits[target];
    }

    fn refine(&mut self, what: Refine, value: &str) -> Result<(), String> {
        let value = value.trim();
        let value = (!value.is_empty()).then(|| value.to_string());
        match what {
            Refine::Message => self.filter.grep = value,
            Refine::Author => self.filter.author = value,
            Refine::File => self.filter.path = value.map(PathBuf::from),
            Refine::Range => {
                if let Some(range) = &value {
                    LogFilter::valid_range(range)?;
                }
                self.filter.all = false;
                self.filter.range = value;
            }
        }
        self.reload();
        Ok(())
    }

    fn scroll_into_view(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + height {
            self.scroll = self.cursor + 1 - height;
        }
    }

    fn title(&self) -> String {
        match &self.error {
            Some(error) => format!("Log: {error}"),
            None => format!(
                "Log: {} ({} commits{})",
                self.filter.describe(),
                self.entries.iter().filter(|e| e.hash.is_some()).count(),
                if self.entries.iter().filter(|e| e.hash.is_some()).count() >= self.filter.limit {
                    ", + for more"
                } else {
                    ""
                }
            ),
        }
    }
}

/// Asks for a value, then refines the open log with it.
fn refine_prompt(what: Refine, current: Option<String>, editor: &Editor) -> crate::ui::Prompt {
    let label = match what {
        Refine::Message => "Message matches (empty clears): ",
        Refine::Author => "Author matches (empty clears): ",
        Refine::File => "Touching file (empty clears): ",
        Refine::Range => "Revision or range (empty for HEAD): ",
    };
    let mut prompt = crate::ui::Prompt::new(
        label.into(),
        None,
        |_, _| Vec::new(),
        move |cx, input, event| {
            if event != crate::ui::PromptEvent::Validate {
                return;
            }
            let input = input.to_string();
            cx.jobs.callback(async move {
                Ok(crate::job::Callback::EditorCompositor(Box::new(
                    move |editor: &mut Editor, compositor: &mut Compositor| {
                        if let Some(log) = compositor.find_id::<LogView>(LogView::ID) {
                            if let Err(err) = log.refine(what, &input) {
                                editor.set_error(err);
                            }
                        }
                    },
                )))
            });
        },
    );
    if let Some(current) = current {
        prompt = prompt.with_line(current, editor);
    }
    prompt
}

/// The log of a revision asked for, from the log menu's `o`.
pub fn range_prompt(workdir: PathBuf, filter: LogFilter) -> crate::ui::Prompt {
    crate::ui::Prompt::new(
        if filter.reflog {
            "Reflog of (a branch or HEAD): "
        } else {
            "Log of (revision or range): "
        }
        .into(),
        None,
        |_, _| Vec::new(),
        move |cx, input, event| {
            if event != crate::ui::PromptEvent::Validate || input.trim().is_empty() {
                return;
            }
            let range = input.trim().to_string();
            if let Err(err) = LogFilter::valid_range(&range) {
                cx.editor.set_error(err);
                return;
            }
            let mut filter = filter.clone();
            filter.range = Some(range);
            let workdir = workdir.clone();
            cx.jobs.callback(async move {
                Ok(crate::job::Callback::EditorCompositor(Box::new(
                    move |_: &mut Editor, compositor: &mut Compositor| {
                        compositor.push(Box::new(LogView::new(workdir, filter)));
                    },
                )))
            });
        },
    )
}

impl Component for LogView {
    fn render(&mut self, viewport: Rect, surface: &mut Surface, cx: &mut Context) {
        let area = viewport.intersection(Rect::new(
            0,
            0,
            viewport.width,
            viewport.height.saturating_sub(1),
        ));
        let theme = &cx.editor.theme;
        let popup_style = theme.get("ui.popup");
        surface.clear_with(area, popup_style);

        let block = Block::bordered()
            .title(self.title())
            .border_style(popup_style);
        let inner = block.inner(area);
        block.render(area, surface);
        if inner.height == 0 || inner.width == 0 {
            return;
        }
        if self.entries.is_empty() {
            let text = if self.error.is_some() {
                "The log could not be read"
            } else {
                "No commits match"
            };
            surface.set_string_truncated(
                inner.x,
                inner.y,
                text,
                inner.width as usize,
                |_| theme.get("ui.virtual"),
                true,
                false,
            );
            return;
        }

        self.scroll_into_view(inner.height as usize);
        let cursor_style = theme.get("ui.cursorline.primary");
        let graph_style = theme.get("ui.virtual");
        let hash_style = theme.get("ui.virtual");
        let refs_style = theme.get("ui.text.focus");
        let text_style = theme.get("ui.text");
        let right_style = theme.get("ui.virtual");
        let width = inner.width as usize;

        for (offset, entry) in self
            .entries
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(inner.height as usize)
        {
            let y = inner.y + (offset - self.scroll) as u16;
            let focused = offset == self.cursor;
            let style = |style: helix_view::graphics::Style| {
                if focused {
                    style.patch(cursor_style)
                } else {
                    style
                }
            };
            if focused {
                surface.set_style(Rect::new(inner.x, y, inner.width, 1), cursor_style);
            }

            // Author and date on the right, when there is room for them.
            let right = match entry.hash {
                Some(_) => format!(" {} {}", entry.author, entry.date),
                None => String::new(),
            };
            let right_width = helix_core::unicode::width::UnicodeWidthStr::width(right.as_str());
            let show_right = right_width * 3 < width;
            let left_end = inner.x
                + if show_right {
                    width - right_width
                } else {
                    width
                } as u16;

            let mut parts: Vec<(String, helix_view::graphics::Style)> =
                vec![(format!("{} ", entry.graph), graph_style)];
            if let Some(hash) = &entry.hash {
                parts.push((format!("{hash} "), hash_style));
                if !entry.refs.is_empty() {
                    parts.push((format!("({}) ", entry.refs.join(", ")), refs_style));
                }
                parts.push((entry.subject.clone(), text_style));
            }

            let mut x = inner.x;
            for (text, part_style) in parts {
                if x >= left_end {
                    break;
                }
                let (next, _) = surface.set_string_truncated(
                    x,
                    y,
                    &text,
                    (left_end - x) as usize,
                    |_| style(part_style),
                    true,
                    false,
                );
                x = next;
            }
            if show_right {
                surface.set_string_truncated(
                    left_end,
                    y,
                    &right,
                    right_width,
                    |_| style(right_style),
                    false,
                    false,
                );
            }
        }
    }

    fn handle_event(&mut self, event: &Event, _cx: &mut Context) -> EventResult {
        let Event::Key(key) = event else {
            return EventResult::Consumed(None);
        };

        let refine = |what: Refine, current: Option<String>| {
            EventResult::Consumed(Some(Box::new(
                move |compositor: &mut Compositor, cx: &mut Context| {
                    compositor.push(Box::new(refine_prompt(what, current, cx.editor)));
                },
            )))
        };

        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('q'), KeyModifiers::NONE) => {
                return EventResult::Consumed(Some(Box::new(|compositor, _| {
                    compositor.remove(LogView::ID);
                })));
            }
            (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => self.move_cursor(1),
            (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => self.move_cursor(-1),
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => self.move_cursor(10),
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => self.move_cursor(-10),
            (KeyCode::Char('g'), KeyModifiers::NONE) => {
                self.cursor = 0;
                self.move_cursor(0);
            }
            (KeyCode::Char('G'), _) => {
                self.cursor = self.entries.len();
                self.move_cursor(isize::MAX / 2);
            }
            (KeyCode::Enter, _) => {
                let Some(hash) = self.current_hash() else {
                    return EventResult::Consumed(None);
                };
                match DiffView::commit(&self.workdir, hash) {
                    Ok(view) => {
                        return EventResult::Consumed(Some(Box::new(move |compositor, _| {
                            compositor.push(Box::new(view));
                        })))
                    }
                    Err(err) => self.error = Some(err),
                }
            }
            (KeyCode::Char('+'), _) => {
                self.filter.limit *= 2;
                self.reload();
            }
            (KeyCode::Char('/'), _) => return refine(Refine::Message, self.filter.grep.clone()),
            (KeyCode::Char('@'), _) => return refine(Refine::Author, self.filter.author.clone()),
            (KeyCode::Char('f'), KeyModifiers::NONE) => {
                return refine(
                    Refine::File,
                    self.filter
                        .path
                        .as_ref()
                        .map(|path| path.display().to_string()),
                )
            }
            (KeyCode::Char('o'), KeyModifiers::NONE) => {
                return refine(Refine::Range, self.filter.range.clone())
            }
            (KeyCode::Char('='), _) => {
                // Back to the plain log of HEAD.
                self.filter = LogFilter {
                    limit: self.filter.limit,
                    ..LogFilter::default()
                };
                self.reload();
            }
            (KeyCode::Char('J'), _) => {
                let workdir = self.workdir.clone();
                return EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    let overlay = crate::magit::views_overlay(compositor, workdir);
                    compositor.push(Box::new(overlay));
                })));
            }
            (KeyCode::Char(key @ ('Q' | '!')), _) => {
                let workdir = self.workdir.clone();
                return EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.push(Box::new(crate::magit::command_prompt(workdir, key == '!')));
                })));
            }
            // Every menu by its dispatch key, aimed at the commit under the
            // cursor: `A` then `A` cherry-picks it, `V` then `V` reverts it,
            // `X` resets to it, `r` rebases from it.
            (KeyCode::Char(key), _) if MenuKind::for_key(key).is_some() => {
                let kind = MenuKind::for_key(key).unwrap_or(MenuKind::Main);
                let mut overlay = TransientOverlay::new(
                    kind.menu(),
                    self.filter.describe(),
                    self.workdir.clone(),
                );
                if let Some(hash) = self.current_hash() {
                    overlay = overlay.with_commit(hash.to_string());
                }
                return EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.push(Box::new(overlay));
                })));
            }
            _ => {}
        }
        EventResult::Consumed(None)
    }

    fn id(&self) -> Option<&'static str> {
        Some(Self::ID)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(hash: Option<&str>, graph: &str) -> LogEntry {
        LogEntry {
            graph: graph.to_string(),
            hash: hash.map(str::to_string),
            subject: hash.map(|h| format!("subject {h}")).unwrap_or_default(),
            ..LogEntry::default()
        }
    }

    fn view(entries: Vec<LogEntry>) -> LogView {
        LogView {
            workdir: PathBuf::from("/repo"),
            filter: LogFilter::default(),
            entries,
            cursor: 0,
            scroll: 0,
            error: None,
        }
    }

    #[test]
    fn the_cursor_steps_over_graph_only_lines() {
        let mut log = view(vec![
            entry(Some("a"), "*"),
            entry(None, "|\\"),
            entry(Some("b"), "| *"),
            entry(None, "|/"),
            entry(Some("c"), "*"),
        ]);
        log.move_cursor(1);
        assert_eq!(log.current_hash(), Some("b"));
        log.move_cursor(1);
        assert_eq!(log.current_hash(), Some("c"));
        log.move_cursor(1);
        assert_eq!(log.current_hash(), Some("c"), "stays on the last commit");
        log.move_cursor(-5);
        assert_eq!(log.current_hash(), Some("a"));
    }

    #[test]
    fn a_range_that_looks_like_an_option_is_refused() {
        let mut log = view(Vec::new());
        assert!(log.refine(Refine::Range, "--output=x").is_err());
        assert_eq!(log.filter.range, None);
    }
}
