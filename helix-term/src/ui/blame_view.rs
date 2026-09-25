//! Blame: a file's lines beside the commits that last changed them.
//!
//! Each run of lines from one commit is headed by that commit — hash, date,
//! author, subject — and `b` blames the revision before the one under the
//! cursor, so a line's history can be walked back one change at a time; `q`
//! walks forward again.

use std::path::{Path, PathBuf};

use helix_magit::blame::{blame, BlameLine};
use helix_magit::log::LogFilter;
use helix_view::graphics::Rect;
use helix_view::input::{KeyCode, KeyModifiers};
use tui::buffer::Buffer as Surface;
use tui::widgets::{Block, Widget};

use crate::compositor::{Component, Context, Event, EventResult};
use crate::ui::diff_view::{Cell, DiffView, RowRenderer};
use crate::ui::log_view::LogView;

/// One revision of the file as blamed: which, and where the cursor was.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Place {
    rev: Option<String>,
    path: PathBuf,
    cursor: usize,
}

pub struct BlameView {
    workdir: PathBuf,
    /// The revision blamed; `None` is the working tree.
    rev: Option<String>,
    path: PathBuf,
    lines: Vec<BlameLine>,
    cursor: usize,
    scroll: usize,
    error: Option<String>,
    /// The revisions `b` stepped back from, most recent last.
    history: Vec<Place>,
}

/// The width of the commit column beside the code.
const COMMIT_COLUMN: usize = 46;

impl BlameView {
    pub const ID: &'static str = "magit-blame";

    /// Blames `path` (relative to `workdir`) with the cursor on `line`.
    pub fn new(workdir: PathBuf, path: PathBuf, rev: Option<String>, line: usize) -> Self {
        let mut view = Self {
            workdir,
            rev,
            path,
            lines: Vec::new(),
            cursor: line,
            scroll: 0,
            error: None,
            history: Vec::new(),
        };
        view.load();
        view
    }

    fn load(&mut self) {
        match blame(&self.workdir, &self.path, self.rev.as_deref()) {
            Ok(lines) => {
                self.lines = lines;
                self.error = None;
            }
            Err(err) => {
                self.lines = Vec::new();
                self.error = Some(err);
            }
        }
        self.cursor = self.cursor.min(self.lines.len().saturating_sub(1));
    }

    fn current(&self) -> Option<&BlameLine> {
        self.lines.get(self.cursor)
    }

    /// Whether line `index` starts a run of lines from one commit.
    fn starts_chunk(&self, index: usize) -> bool {
        index == 0 || self.lines[index].hash != self.lines[index - 1].hash
    }

    fn move_cursor(&mut self, delta: isize) {
        let last = self.lines.len().saturating_sub(1) as isize;
        self.cursor = (self.cursor as isize + delta).clamp(0, last.max(0)) as usize;
    }

    /// `n` / `p`: the start of the next or previous run.
    fn move_chunk(&mut self, forward: bool) {
        if forward {
            if let Some(next) = (self.cursor + 1..self.lines.len()).find(|&i| self.starts_chunk(i))
            {
                self.cursor = next;
            }
        } else {
            let start = (0..=self.cursor)
                .rev()
                .find(|&i| self.starts_chunk(i))
                .unwrap_or(0);
            self.cursor = if start < self.cursor || start == 0 {
                start
            } else {
                (0..start)
                    .rev()
                    .find(|&i| self.starts_chunk(i))
                    .unwrap_or(0)
            };
        }
    }

    /// `b`: blames the version before the commit under the cursor, on the
    /// line the cursor's line had there.
    fn step_back(&mut self) {
        let Some(line) = self.current().cloned() else {
            return;
        };
        if line.is_uncommitted() {
            // The working tree's own edit: its "before" is HEAD.
            self.go_to(Some("HEAD".to_string()), self.path.clone(), self.cursor);
            return;
        }
        let Some((previous, filename)) = line.previous.clone() else {
            self.error = Some(format!(
                "{} added this line; there is no earlier version",
                line.short()
            ));
            return;
        };
        self.go_to(
            Some(previous),
            PathBuf::from(filename),
            line.orig_line.saturating_sub(1),
        );
    }

    fn go_to(&mut self, rev: Option<String>, path: PathBuf, cursor: usize) {
        self.history.push(Place {
            rev: self.rev.clone(),
            path: self.path.clone(),
            cursor: self.cursor,
        });
        self.rev = rev;
        self.path = path;
        self.cursor = cursor;
        self.load();
    }

    /// `q`: back to the revision `b` came from. Returns false when there is
    /// none, and the view should close.
    fn step_forward(&mut self) -> bool {
        let Some(place) = self.history.pop() else {
            return false;
        };
        self.rev = place.rev;
        self.path = place.path;
        self.cursor = place.cursor;
        self.load();
        true
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
        if let Some(error) = &self.error {
            return format!("Blame: {error}");
        }
        let at = match &self.rev {
            Some(rev) => format!(" at {}", &rev[..rev.len().min(12)]),
            None => String::new(),
        };
        let depth = if self.history.is_empty() {
            String::new()
        } else {
            format!(" ({} back; q returns)", self.history.len())
        };
        format!("Blame: {}{at}{depth}", self.path.display())
    }

    /// The heading of a run: short hash, date, author, subject.
    fn heading(line: &BlameLine) -> String {
        if line.is_uncommitted() {
            return "Not committed yet".to_string();
        }
        format!(
            "{} {} {}: {}",
            line.short(),
            line.date(),
            line.author,
            line.summary
        )
    }
}

impl Component for BlameView {
    fn render(&mut self, viewport: Rect, surface: &mut Surface, cx: &mut Context) {
        let area = viewport.intersection(Rect::new(
            0,
            0,
            viewport.width,
            viewport.height.saturating_sub(1),
        ));
        let theme = &cx.editor.theme;
        let popup = theme.get("ui.popup");
        surface.clear_with(area, popup);
        let block = Block::bordered().title(self.title()).border_style(popup);
        let inner = block.inner(area);
        block.render(area, surface);
        if inner.width < 10 || inner.height == 0 {
            return;
        }

        self.scroll_into_view(inner.height as usize);
        let cursor_style = theme.get("ui.cursorline.primary");
        let heading_style = theme.get("ui.text.focus");
        let rule_style = theme.get("ui.virtual");
        let code_style = theme.get("ui.text");
        let renderer = RowRenderer::new(cx.editor);
        let column = COMMIT_COLUMN.min(inner.width as usize / 3);
        let number_width = self.lines.len().to_string().len();

        for (index, line) in self
            .lines
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(inner.height as usize)
        {
            let y = inner.y + (index - self.scroll) as u16;
            let focused = index == self.cursor;
            let with_cursor = |style: helix_view::graphics::Style| {
                if focused {
                    style.patch(cursor_style)
                } else {
                    style
                }
            };
            if focused {
                surface.set_style(Rect::new(inner.x, y, inner.width, 1), cursor_style);
            }

            let (left, style) = if self.starts_chunk(index) {
                (Self::heading(line), heading_style)
            } else {
                ("│".to_string(), rule_style)
            };
            surface.set_string_truncated(
                inner.x,
                y,
                &left,
                column.saturating_sub(1),
                |_| with_cursor(style),
                true,
                false,
            );

            let number = format!("{:>number_width$} ", index + 1);
            let x = inner.x + column as u16;
            surface.set_string_truncated(
                x,
                y,
                &number,
                number.len(),
                |_| with_cursor(rule_style),
                false,
                false,
            );
            let code_x = x + number.len() as u16;
            let width = (inner.x + inner.width).saturating_sub(code_x) as usize;
            renderer.put_highlighted(
                surface,
                Cell {
                    x: code_x,
                    y,
                    width,
                },
                &line.content,
                with_cursor(code_style),
                &self.path,
            );
        }
    }

    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        let Event::Key(key) = event else {
            return EventResult::Consumed(None);
        };
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('q'), KeyModifiers::NONE) => {
                if !self.step_forward() {
                    return EventResult::Consumed(Some(Box::new(|compositor, _| {
                        compositor.remove(BlameView::ID);
                    })));
                }
            }
            (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => self.move_cursor(1),
            (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => self.move_cursor(-1),
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => self.move_cursor(20),
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => self.move_cursor(-20),
            (KeyCode::Char('g'), KeyModifiers::NONE) => self.cursor = 0,
            (KeyCode::Char('G'), _) => self.cursor = self.lines.len().saturating_sub(1),
            (KeyCode::Char('n'), KeyModifiers::NONE) => self.move_chunk(true),
            (KeyCode::Char('p'), KeyModifiers::NONE) => self.move_chunk(false),
            (KeyCode::Char('b'), KeyModifiers::NONE) => {
                self.error = None;
                self.step_back();
            }
            (KeyCode::Enter, _) => {
                let Some(line) = self.current() else {
                    return EventResult::Consumed(None);
                };
                if line.is_uncommitted() {
                    self.error = Some("Not committed yet: see the file's diff".to_string());
                    return EventResult::Consumed(None);
                }
                match DiffView::commit(&self.workdir, &line.hash) {
                    Ok(view) => {
                        return EventResult::Consumed(Some(Box::new(move |compositor, _| {
                            compositor.push(Box::new(view));
                        })))
                    }
                    Err(err) => self.error = Some(err),
                }
            }
            // Copy the line's commit.
            (KeyCode::Char('w'), KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                match self.current().filter(|line| !line.is_uncommitted()) {
                    Some(line) => {
                        let hash = line.short().to_string();
                        crate::magit::copy_value(
                            cx.editor,
                            &self.workdir,
                            &hash,
                            helix_magit::AskKind::Revision,
                        );
                    }
                    None => cx.editor.set_error("Not committed yet"),
                }
            }
            // Edit the line's commit: an interactive rebase stopping there.
            (KeyCode::Char('e'), KeyModifiers::NONE) => {
                let Some(line) = self.current().filter(|line| !line.is_uncommitted()) else {
                    self.error = Some("Not committed yet".to_string());
                    return EventResult::Consumed(None);
                };
                let hash = line.hash.clone();
                let workdir = self.workdir.clone();
                return EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                    compositor.remove(BlameView::ID);
                    crate::magit::edit_commit(compositor, cx, workdir, &hash);
                })));
            }
            // The file's log, followed through renames.
            (KeyCode::Char('l'), KeyModifiers::NONE) => {
                let filter = LogFilter {
                    range: self.rev.clone(),
                    path: Some(self.path.clone()),
                    follow: true,
                    ..LogFilter::default()
                };
                let workdir = self.workdir.clone();
                return EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.push(Box::new(LogView::new(workdir, filter)));
                })));
            }
            // Visit the file at this line — the working tree's file, which
            // is only the same text when blaming the working tree.
            (KeyCode::Char('v'), KeyModifiers::NONE) => {
                let path = self.workdir.join(&self.path);
                let line = self.cursor;
                return EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                    compositor.remove(BlameView::ID);
                    crate::magit::close_views(compositor);
                    crate::roam::open_at(cx.editor, &path, line);
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

/// The path of `file` inside the repository at `workdir`.
pub fn relative_to(workdir: &Path, file: &Path) -> Option<PathBuf> {
    let workdir = std::fs::canonicalize(workdir).unwrap_or_else(|_| workdir.to_path_buf());
    let file = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    file.strip_prefix(&workdir).ok().map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(hash: char, previous: Option<(&str, &str)>, orig: usize) -> BlameLine {
        BlameLine {
            hash: hash.to_string().repeat(40),
            previous: previous.map(|(h, f)| (h.to_string(), f.to_string())),
            orig_line: orig,
            ..BlameLine::default()
        }
    }

    fn view(lines: Vec<BlameLine>) -> BlameView {
        BlameView {
            workdir: PathBuf::from("/repo"),
            rev: None,
            path: PathBuf::from("f.txt"),
            lines,
            cursor: 0,
            scroll: 0,
            error: None,
            history: Vec::new(),
        }
    }

    #[test]
    fn runs_of_one_commit_are_one_chunk() {
        let mut blame = view(vec![
            line('a', None, 1),
            line('a', None, 2),
            line('b', None, 1),
        ]);
        assert!(blame.starts_chunk(0));
        assert!(!blame.starts_chunk(1));
        assert!(blame.starts_chunk(2));

        blame.move_chunk(true);
        assert_eq!(blame.cursor, 2);
        blame.move_chunk(false);
        assert_eq!(blame.cursor, 0);
    }

    #[test]
    fn a_line_with_no_earlier_version_says_so() {
        let mut blame = view(vec![line('a', None, 1)]);
        blame.step_back();
        assert!(blame
            .error
            .as_deref()
            .unwrap()
            .contains("no earlier version"));
        assert!(blame.history.is_empty());
        assert!(
            !blame.step_forward(),
            "nothing to return to: the view closes"
        );
    }
}
