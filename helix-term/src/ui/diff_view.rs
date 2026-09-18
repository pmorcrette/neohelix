//! The Magit-style status buffer: a diff you navigate and stage from.
//!
//! The model is a tree — section, file, hunk, line — but it is rendered and
//! navigated as a flat list of rows rebuilt whenever something folds or the
//! repository changes. That keeps "what is under the cursor", "what does a
//! keypress act on" and "what is on screen" one and the same question.

use std::path::{Path, PathBuf};

use helix_magit::diff::{DiffLineKind, FileDiff};
use helix_magit::{Repository, Selection};
use helix_view::graphics::Rect;
use helix_view::input::{KeyCode, KeyModifiers};
use helix_view::Editor;
use tui::buffer::Buffer as Surface;
use tui::text::Text;
use tui::widgets::{Block, Widget};

use crate::compositor::{Component, Context, Event, EventResult};

/// Which side of the index a section shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SectionKind {
    /// Index against working tree: what `s` can stage.
    Unstaged,
    /// HEAD against index: what `u` can unstage.
    Staged,
}

impl SectionKind {
    fn title(self) -> &'static str {
        match self {
            SectionKind::Unstaged => "Unstaged changes",
            SectionKind::Staged => "Staged changes",
        }
    }
}

struct Section {
    kind: SectionKind,
    files: Vec<FileDiff>,
    folded: bool,
}

/// One rendered line, and what it stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Section {
        section: usize,
    },
    File {
        section: usize,
        file: usize,
    },
    Hunk {
        section: usize,
        file: usize,
        hunk: usize,
    },
    Line {
        section: usize,
        file: usize,
        hunk: usize,
        line: usize,
    },
}

impl Row {
    /// How deep this row sits in the tree, for indentation.
    fn depth(self) -> u16 {
        match self {
            Row::Section { .. } => 0,
            Row::File { .. } => 1,
            Row::Hunk { .. } => 2,
            Row::Line { .. } => 3,
        }
    }

    fn section(self) -> usize {
        match self {
            Row::Section { section }
            | Row::File { section, .. }
            | Row::Hunk { section, .. }
            | Row::Line { section, .. } => section,
        }
    }
}

/// The status buffer.
pub struct DiffView {
    workdir: PathBuf,
    sections: Vec<Section>,
    rows: Vec<Row>,
    cursor: usize,
    scroll: usize,
    head: String,
    /// Set when an action fails, shown in place of the header.
    error: Option<String>,
}

impl DiffView {
    pub const ID: &'static str = "magit-status";

    /// Opens the status of the repository containing `path`.
    pub fn new(path: &Path) -> Result<Self, helix_magit::repository::Error> {
        let repository = Repository::discover(path)?;
        let mut view = Self {
            workdir: repository.workdir().to_path_buf(),
            sections: Vec::new(),
            rows: Vec::new(),
            cursor: 0,
            scroll: 0,
            head: repository.head_description(),
            error: None,
        };
        view.reload(&repository)?;
        Ok(view)
    }

    /// Re-reads the repository, keeping the cursor as close as it can.
    ///
    /// Staging changes the shape of the tree — a fully staged file leaves the
    /// unstaged section entirely — so the cursor is clamped rather than
    /// restored exactly.
    fn reload(&mut self, repository: &Repository) -> Result<(), helix_magit::repository::Error> {
        let folded: Vec<(SectionKind, PathBuf)> = self
            .sections
            .iter()
            .flat_map(|section| {
                section
                    .files
                    .iter()
                    .filter(|file| file.folded)
                    .map(move |file| (section.kind, file.path.clone()))
            })
            .collect();

        let mut unstaged = repository.worktree_diff()?;
        let mut staged = repository.staged_diff()?;

        // Folding is a view preference, so it survives a refresh.
        for file in unstaged.iter_mut() {
            file.folded = folded.contains(&(SectionKind::Unstaged, file.path.clone()));
        }
        for file in staged.iter_mut() {
            file.folded = folded.contains(&(SectionKind::Staged, file.path.clone()));
        }

        self.head = repository.head_description();
        self.sections = vec![
            Section {
                kind: SectionKind::Unstaged,
                files: unstaged,
                folded: false,
            },
            Section {
                kind: SectionKind::Staged,
                files: staged,
                folded: false,
            },
        ];
        self.rebuild_rows();
        Ok(())
    }

    /// Flattens the tree into the rows currently visible.
    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();

        for (section_index, section) in self.sections.iter().enumerate() {
            if section.files.is_empty() {
                continue;
            }
            rows.push(Row::Section {
                section: section_index,
            });
            if section.folded {
                continue;
            }

            for (file_index, file) in section.files.iter().enumerate() {
                rows.push(Row::File {
                    section: section_index,
                    file: file_index,
                });
                if file.folded {
                    continue;
                }

                for (hunk_index, hunk) in file.hunks.iter().enumerate() {
                    rows.push(Row::Hunk {
                        section: section_index,
                        file: file_index,
                        hunk: hunk_index,
                    });
                    if hunk.folded {
                        continue;
                    }

                    for line_index in 0..hunk.lines.len() {
                        rows.push(Row::Line {
                            section: section_index,
                            file: file_index,
                            hunk: hunk_index,
                            line: line_index,
                        });
                    }
                }
            }
        }

        self.rows = rows;
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
    }

    fn current_row(&self) -> Option<Row> {
        self.rows.get(self.cursor).copied()
    }

    fn file_at(&self, row: Row) -> Option<&FileDiff> {
        match row {
            Row::Section { .. } => None,
            Row::File { section, file, .. }
            | Row::Hunk { section, file, .. }
            | Row::Line { section, file, .. } => self.sections.get(section)?.files.get(file),
        }
    }

    /// Turns the cursor's position into the selection an action applies to.
    fn selection_at(&self, row: Row) -> Option<Selection> {
        match row {
            Row::Section { .. } => None,
            Row::File { .. } => Some(Selection::File),
            Row::Hunk { hunk, .. } => Some(Selection::Hunk(hunk)),
            Row::Line { hunk, line, .. } => Some(Selection::Lines {
                hunk,
                lines: vec![line],
            }),
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        self.cursor = (self.cursor as isize + delta).clamp(0, last as isize) as usize;
    }

    /// Moves to the row that contains the current one.
    fn move_to_parent(&mut self) {
        let Some(row) = self.current_row() else {
            return;
        };
        let depth = row.depth();
        if depth == 0 {
            return;
        }
        for index in (0..self.cursor).rev() {
            if self.rows[index].depth() < depth {
                self.cursor = index;
                return;
            }
        }
    }

    /// Folds or unfolds whatever the cursor is on.
    fn toggle_fold(&mut self) {
        let Some(row) = self.current_row() else {
            return;
        };

        match row {
            Row::Section { section } => {
                if let Some(section) = self.sections.get_mut(section) {
                    section.folded = !section.folded;
                }
            }
            Row::File { section, file } => {
                if let Some(file) = self
                    .sections
                    .get_mut(section)
                    .and_then(|section| section.files.get_mut(file))
                {
                    file.folded = !file.folded;
                }
            }
            // A line has nothing to fold, so folding acts on its hunk — which
            // is also where the cursor ends up, since the line disappears.
            Row::Hunk {
                section,
                file,
                hunk,
            }
            | Row::Line {
                section,
                file,
                hunk,
                ..
            } => {
                if let Some(hunk) = self
                    .sections
                    .get_mut(section)
                    .and_then(|section| section.files.get_mut(file))
                    .and_then(|file| file.hunks.get_mut(hunk))
                {
                    hunk.folded = !hunk.folded;
                }
                if matches!(row, Row::Line { .. }) {
                    self.move_to_parent();
                }
            }
        }

        self.rebuild_rows();
    }

    /// Stages or unstages what the cursor is on, then refreshes.
    fn apply(&mut self, stage: bool) {
        self.error = None;

        let Some(row) = self.current_row() else {
            return;
        };
        let Some(selection) = self.selection_at(row) else {
            self.error = Some("Move to a file, hunk or line first".to_string());
            return;
        };

        let kind = self.sections[row.section()].kind;
        match (stage, kind) {
            (true, SectionKind::Staged) => {
                self.error = Some("Already staged — use u to unstage".to_string());
                return;
            }
            (false, SectionKind::Unstaged) => {
                self.error = Some("Not staged yet — use s to stage".to_string());
                return;
            }
            _ => {}
        }

        let Some(file) = self.file_at(row).cloned() else {
            return;
        };

        let repository = match Repository::discover(&self.workdir) {
            Ok(repository) => repository,
            Err(err) => {
                self.error = Some(err.to_string());
                return;
            }
        };

        let outcome = if stage {
            repository.stage(&file, &selection)
        } else {
            repository.unstage(&file, &selection)
        };

        if let Err(err) = outcome {
            self.error = Some(err.to_string());
            return;
        }

        // The tree just changed shape, so the view is rebuilt from git rather
        // than patched in place.
        if let Err(err) = self.reload(&repository) {
            self.error = Some(err.to_string());
        }
    }

    /// Keeps the cursor inside the visible window.
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
}

impl Component for DiffView {
    fn render(&mut self, viewport: Rect, surface: &mut Surface, cx: &mut Context) {
        let area = viewport.intersection(Rect::new(
            0,
            0,
            viewport.width,
            viewport.height.saturating_sub(1),
        ));
        let popup_style = cx.editor.theme.get("ui.popup");
        surface.clear_with(area, popup_style);

        let title = match &self.error {
            Some(error) => format!("Magit: {error}"),
            None => format!("Magit: {}", self.head),
        };
        let block = Block::bordered().title(title).border_style(popup_style);
        let inner = block.inner(area);
        block.render(area, surface);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        if self.rows.is_empty() {
            surface.set_string_truncated(
                inner.x,
                inner.y,
                "Nothing to commit, working tree clean",
                inner.width as usize,
                |_| cx.editor.theme.get("ui.virtual"),
                true,
                false,
            );
            return;
        }

        self.scroll_into_view(inner.height as usize);
        let renderer = RowRenderer::new(cx.editor);

        for (offset, row) in self
            .rows
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(inner.height as usize)
        {
            let y = inner.y + (offset - self.scroll) as u16;
            renderer.render(self, *row, offset == self.cursor, inner, y, surface);
        }
    }

    fn handle_event(&mut self, event: &Event, _cx: &mut Context) -> EventResult {
        let Event::Key(key) = event else {
            return EventResult::Consumed(None);
        };

        // The status buffer is modal: it owns the keyboard while it is open.
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('q'), KeyModifiers::NONE) => {
                return EventResult::Consumed(Some(Box::new(|compositor, _| {
                    compositor.remove(DiffView::ID);
                })));
            }
            (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => self.move_cursor(1),
            (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => self.move_cursor(-1),
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => self.move_cursor(10),
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => self.move_cursor(-10),
            (KeyCode::Char('g'), KeyModifiers::NONE) => self.cursor = 0,
            (KeyCode::Char('G'), _) => self.cursor = self.rows.len().saturating_sub(1),
            (KeyCode::Char('h') | KeyCode::Left, KeyModifiers::NONE) => self.move_to_parent(),
            (KeyCode::Tab, _) | (KeyCode::Char('l') | KeyCode::Right, KeyModifiers::NONE) => {
                self.toggle_fold()
            }
            (KeyCode::Char('s'), KeyModifiers::NONE) => self.apply(true),
            (KeyCode::Char('u'), KeyModifiers::NONE) => self.apply(false),
            _ => {}
        }

        EventResult::Consumed(None)
    }

    fn id(&self) -> Option<&'static str> {
        Some(Self::ID)
    }
}

/// Where a fragment is drawn, and how much room it has.
#[derive(Debug, Clone, Copy)]
struct Cell {
    x: u16,
    y: u16,
    width: usize,
}

/// Draws one row, with the theme's diff colours and Tree-sitter highlighting.
struct RowRenderer<'a> {
    theme: &'a helix_view::Theme,
    loader: arc_swap::Guard<std::sync::Arc<helix_core::syntax::Loader>>,
}

impl<'a> RowRenderer<'a> {
    fn new(editor: &'a Editor) -> Self {
        Self {
            theme: &editor.theme,
            loader: editor.syn_loader.load(),
        }
    }

    fn render(
        &self,
        view: &DiffView,
        row: Row,
        focused: bool,
        area: Rect,
        y: u16,
        surface: &mut Surface,
    ) {
        let cursor_style = self.theme.get("ui.cursorline.primary");
        let width = area.width as usize;
        let indent = row.depth() * 2;
        let x = area.x + indent;
        let available = width.saturating_sub(indent as usize);
        if available == 0 {
            return;
        }
        let cell = Cell {
            x,
            y,
            width: available,
        };

        // The focused row is highlighted across the full width, so the cursor
        // stays visible on a short line.
        if focused {
            surface.set_style(Rect::new(area.x, y, area.width, 1), cursor_style);
        }

        match row {
            Row::Section { section } => {
                let section = &view.sections[section];
                let text = format!(
                    "{} {} ({})",
                    fold_marker(section.folded),
                    section.kind.title(),
                    section.files.len()
                );
                self.put(
                    surface,
                    cell,
                    &text,
                    self.theme.get("ui.text.focus"),
                    focused,
                );
            }
            Row::File { .. } => {
                let Some(file) = view.file_at(row) else {
                    return;
                };
                let (added, removed) = file.stats();
                let text = format!(
                    "{} {} {}  +{added} -{removed}{}",
                    fold_marker(file.folded),
                    file.status.code(),
                    file.path.display(),
                    if file.binary { "  (binary)" } else { "" }
                );
                self.put(surface, cell, &text, self.theme.get("ui.text"), focused);
            }
            Row::Hunk { hunk, .. } => {
                let Some(file) = view.file_at(row) else {
                    return;
                };
                let Some(hunk) = file.hunks.get(hunk) else {
                    return;
                };
                let text = format!("{} {}", fold_marker(hunk.folded), hunk.header);
                self.put(
                    surface,
                    cell,
                    &text,
                    self.theme.get("ui.linenr.selected"),
                    focused,
                );
            }
            Row::Line { hunk, line, .. } => {
                let Some(file) = view.file_at(row) else {
                    return;
                };
                let Some(line) = file
                    .hunks
                    .get(hunk)
                    .and_then(|hunk| hunk.lines.get(line))
                    .cloned()
                else {
                    return;
                };

                // The git marker is drawn separately, so the code fragment
                // handed to the highlighter has no diff syntax in it.
                let base = match line.kind {
                    DiffLineKind::Addition => self.theme.get("diff.plus"),
                    DiffLineKind::Deletion => self.theme.get("diff.minus"),
                    DiffLineKind::Context => self.theme.get("ui.text"),
                };
                let base = if focused {
                    base.patch(cursor_style)
                } else {
                    base
                };

                surface.set_string_truncated(
                    x,
                    y,
                    &line.kind.prefix().to_string(),
                    1,
                    |_| base,
                    false,
                    false,
                );

                self.put_highlighted(
                    surface,
                    Cell {
                        x: x + 1,
                        width: available.saturating_sub(1),
                        ..cell
                    },
                    &line.content,
                    base,
                    &file.path,
                );
            }
        }
    }

    fn put(
        &self,
        surface: &mut Surface,
        cell: Cell,
        text: &str,
        style: helix_view::graphics::Style,
        focused: bool,
    ) {
        let style = if focused {
            style.patch(self.theme.get("ui.cursorline.primary"))
        } else {
            style
        };
        surface.set_string_truncated(cell.x, cell.y, text, cell.width, |_| style, true, false);
    }

    /// Draws a code fragment with Tree-sitter highlighting over the diff's own
    /// background.
    ///
    /// The fragment is highlighted on its own rather than as part of the whole
    /// file, so a construct spanning a hunk boundary can be coloured as if the
    /// hunk were the entire file. Reconstructing both sides of every file to
    /// avoid that would mean reading each file twice per frame.
    fn put_highlighted(
        &self,
        surface: &mut Surface,
        cell: Cell,
        content: &str,
        base: helix_view::graphics::Style,
        path: &Path,
    ) {
        let Cell { x, y, width } = cell;
        if width == 0 {
            return;
        }

        // Without a grammar the highlighter falls back to a style of its own,
        // which would override the diff's; draw the line plainly instead.
        let Some(language) = self.loader.language_for_filename(path) else {
            surface.set_string_truncated(x, y, content, width, |_| base, true, false);
            return;
        };

        let highlighted: Text = crate::ui::markdown::highlighted_code_block_for_language(
            content,
            Some(language),
            Some(self.theme),
            &self.loader,
            None,
        );

        let Some(spans) = highlighted.lines.first() else {
            surface.set_string_truncated(x, y, content, width, |_| base, true, false);
            return;
        };

        let mut cursor = x;
        let end = x + width as u16;
        for span in &spans.0 {
            if cursor >= end {
                break;
            }
            // Take only the foreground and emphasis from the syntax highlight,
            // so the diff keeps whatever background the theme gives
            // `diff.plus` / `diff.minus`. A theme that colours those with a
            // foreground only still marks the row through its `+`/`-` column,
            // which is drawn in the diff style.
            let mut style = base;
            if let Some(fg) = span.style.fg {
                style = style.fg(fg);
            }
            style = style.add_modifier(span.style.add_modifier);

            let remaining = (end - cursor) as usize;
            let (next_x, _) = surface.set_string_truncated(
                cursor,
                y,
                &span.content,
                remaining,
                |_| style,
                false,
                false,
            );
            if next_x == cursor {
                break;
            }
            cursor = next_x;
        }
    }
}

fn fold_marker(folded: bool) -> char {
    if folded {
        '▸'
    } else {
        '▾'
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_magit::diff::{DiffHunk, DiffLine, FileStatus, HunkHeader};

    fn line(kind: DiffLineKind, content: &str) -> DiffLine {
        DiffLine {
            kind,
            content: content.to_string(),
            old_line: None,
            new_line: None,
            no_newline: false,
        }
    }

    fn hunk(lines: usize) -> DiffHunk {
        DiffHunk {
            header: HunkHeader::default(),
            lines: (0..lines)
                .map(|index| line(DiffLineKind::Context, &format!("l{index}")))
                .collect(),
            folded: false,
        }
    }

    fn file(path: &str, hunks: usize, lines_per_hunk: usize) -> FileDiff {
        FileDiff {
            path: PathBuf::from(path),
            status: FileStatus::Modified,
            hunks: (0..hunks).map(|_| hunk(lines_per_hunk)).collect(),
            folded: false,
            binary: false,
        }
    }

    /// A view over a fixed model, with no repository behind it.
    fn make_view(unstaged: Vec<FileDiff>, staged: Vec<FileDiff>) -> DiffView {
        let mut view = DiffView {
            workdir: PathBuf::from("/repo"),
            sections: vec![
                Section {
                    kind: SectionKind::Unstaged,
                    files: unstaged,
                    folded: false,
                },
                Section {
                    kind: SectionKind::Staged,
                    files: staged,
                    folded: false,
                },
            ],
            rows: Vec::new(),
            cursor: 0,
            scroll: 0,
            head: "main".to_string(),
            error: None,
        };
        view.rebuild_rows();
        view
    }

    #[test]
    fn rows_flatten_the_tree_in_order() {
        let view = make_view(vec![file("a.rs", 2, 2)], Vec::new());

        // Section, file, then each hunk followed by its lines. The staged
        // section is empty, so it contributes no header.
        assert_eq!(view.rows.len(), 1 + 1 + (1 + 2) * 2);
        assert_eq!(view.rows[0], Row::Section { section: 0 });
        assert_eq!(
            view.rows[1],
            Row::File {
                section: 0,
                file: 0
            }
        );
        assert_eq!(
            view.rows[2],
            Row::Hunk {
                section: 0,
                file: 0,
                hunk: 0
            }
        );
        assert_eq!(
            view.rows[3],
            Row::Line {
                section: 0,
                file: 0,
                hunk: 0,
                line: 0
            }
        );
    }

    #[test]
    fn an_empty_section_contributes_no_rows() {
        let view = make_view(Vec::new(), Vec::new());
        assert!(view.rows.is_empty());

        let view = make_view(Vec::new(), vec![file("a.rs", 1, 1)]);
        assert_eq!(view.rows[0], Row::Section { section: 1 });
    }

    #[test]
    fn folding_a_file_hides_its_hunks() {
        let mut view = make_view(vec![file("a.rs", 2, 2)], Vec::new());
        let before = view.rows.len();

        view.cursor = 1; // the file row
        view.toggle_fold();

        assert_eq!(view.rows.len(), 2, "only the section and the file remain");
        assert!(view.sections[0].files[0].folded);

        view.toggle_fold();
        assert_eq!(view.rows.len(), before);
    }

    #[test]
    fn folding_a_hunk_hides_only_its_own_lines() {
        let mut view = make_view(vec![file("a.rs", 2, 2)], Vec::new());
        view.cursor = 2; // the first hunk
        view.toggle_fold();

        // The first hunk's two lines are gone; the second hunk keeps its own.
        assert_eq!(view.rows.len(), 1 + 1 + 1 + (1 + 2));
        assert!(view.sections[0].files[0].hunks[0].folded);
        assert!(!view.sections[0].files[0].hunks[1].folded);
    }

    #[test]
    fn folding_from_a_line_folds_its_hunk_and_moves_to_it() {
        let mut view = make_view(vec![file("a.rs", 1, 3)], Vec::new());
        view.cursor = 4; // the second line of the hunk

        view.toggle_fold();

        assert!(view.sections[0].files[0].hunks[0].folded);
        // The line the cursor was on no longer exists, so it lands on the hunk.
        assert_eq!(
            view.current_row(),
            Some(Row::Hunk {
                section: 0,
                file: 0,
                hunk: 0
            })
        );
    }

    #[test]
    fn the_cursor_maps_to_the_selection_the_action_applies_to() {
        let view = make_view(vec![file("a.rs", 2, 2)], Vec::new());

        assert_eq!(view.selection_at(view.rows[0]), None, "a section header");
        assert_eq!(view.selection_at(view.rows[1]), Some(Selection::File));
        assert_eq!(view.selection_at(view.rows[2]), Some(Selection::Hunk(0)));
        assert_eq!(
            view.selection_at(view.rows[3]),
            Some(Selection::Lines {
                hunk: 0,
                lines: vec![0]
            })
        );
        // The second hunk's index is the one within the file, not the row.
        assert_eq!(view.selection_at(view.rows[5]), Some(Selection::Hunk(1)));
    }

    #[test]
    fn moving_to_the_parent_climbs_one_level() {
        let mut view = make_view(vec![file("a.rs", 1, 2)], Vec::new());

        view.cursor = 3; // a line
        view.move_to_parent();
        assert!(matches!(view.current_row(), Some(Row::Hunk { .. })));

        view.move_to_parent();
        assert!(matches!(view.current_row(), Some(Row::File { .. })));

        view.move_to_parent();
        assert!(matches!(view.current_row(), Some(Row::Section { .. })));

        // Already at the top: staying put beats wrapping around.
        view.move_to_parent();
        assert_eq!(view.cursor, 0);
    }

    #[test]
    fn the_cursor_never_leaves_the_rows() {
        let mut view = make_view(vec![file("a.rs", 1, 1)], Vec::new());
        let last = view.rows.len() - 1;

        view.move_cursor(1000);
        assert_eq!(view.cursor, last);
        view.move_cursor(-1000);
        assert_eq!(view.cursor, 0);
    }

    #[test]
    fn scrolling_follows_the_cursor_in_both_directions() {
        let mut view = make_view(vec![file("a.rs", 4, 4)], Vec::new());

        view.cursor = view.rows.len() - 1;
        view.scroll_into_view(5);
        assert_eq!(view.scroll, view.cursor + 1 - 5);

        view.cursor = 0;
        view.scroll_into_view(5);
        assert_eq!(view.scroll, 0);
    }

    #[test]
    fn staging_from_the_staged_section_is_refused() {
        let mut view = make_view(Vec::new(), vec![file("a.rs", 1, 1)]);
        view.cursor = 1; // the staged file

        view.apply(true);
        assert!(
            view.error.as_deref().unwrap().contains("Already staged"),
            "got {:?}",
            view.error
        );
    }

    #[test]
    fn unstaging_from_the_unstaged_section_is_refused() {
        let mut view = make_view(vec![file("a.rs", 1, 1)], Vec::new());
        view.cursor = 1;

        view.apply(false);
        assert!(
            view.error.as_deref().unwrap().contains("Not staged yet"),
            "got {:?}",
            view.error
        );
    }

    #[test]
    fn acting_on_a_section_header_says_what_to_do_instead() {
        let mut view = make_view(vec![file("a.rs", 1, 1)], Vec::new());
        view.cursor = 0;

        view.apply(true);
        assert!(view.error.as_deref().unwrap().contains("Move to a file"));
    }
}
