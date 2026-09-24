//! The Magit-style status buffer: a diff you navigate and stage from.
//!
//! The model is a tree — section, file, hunk, line — but it is rendered and
//! navigated as a flat list of rows rebuilt whenever something folds or the
//! repository changes. That keeps "what is under the cursor", "what does a
//! keypress act on" and "what is on screen" one and the same question.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use helix_magit::diff::{DiffLineKind, FileDiff};
use helix_magit::status::Overview;
use helix_magit::{Plan, Repository, Requirement, Selection, Unmerged};
use helix_view::graphics::Rect;
use helix_view::input::{KeyCode, KeyModifiers};
use helix_view::Editor;
use tui::buffer::Buffer as Surface;
use tui::text::Text;
use tui::widgets::{Block, Widget};

use crate::compositor::{Component, Compositor, Context, Event, EventResult};
use crate::ui::confirm::Confirm;
use crate::ui::transient::TransientOverlay;
use helix_magit::transient::MenuKind;

/// What a section lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SectionKind {
    /// Paths left in conflict by a merge, rebase or cherry-pick.
    Unmerged,
    /// Files git does not track yet: what `s` can add.
    Untracked,
    /// Index against working tree: what `s` can stage.
    Unstaged,
    /// HEAD against index: what `u` can unstage.
    Staged,
    Stashes,
    /// Commits the upstream has and HEAD does not.
    Unpulled,
    /// Commits HEAD has and the upstream does not.
    Unpushed,
    /// The last few commits, when nothing is unpushed.
    Recent,
}

impl SectionKind {
    /// Whether the section holds changes still to stage, already staged, or
    /// neither.
    fn staged(self) -> Option<bool> {
        match self {
            SectionKind::Untracked | SectionKind::Unstaged => Some(false),
            SectionKind::Staged => Some(true),
            _ => None,
        }
    }
}

/// A commit or a stash in a section: what `RET` shows.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    /// The abbreviated hash, or `stash@{0}`.
    label: String,
    text: String,
}

struct Section {
    kind: SectionKind,
    title: String,
    files: Vec<FileDiff>,
    items: Vec<Item>,
    folded: bool,
}

impl Section {
    fn len(&self) -> usize {
        self.files.len() + self.items.len()
    }
}

/// How a part of a header line is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    Plain,
    Emphasis,
    Dim,
}

/// A line above the sections: where HEAD is, and what is under way.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HeaderLine {
    label: &'static str,
    parts: Vec<(String, Tone)>,
}

/// The header Magit shows above its sections, from the overview.
fn header_lines(overview: &Overview) -> Vec<HeaderLine> {
    let commit_parts = |commit: &Option<helix_magit::status::Commit>| match commit {
        Some(commit) => vec![
            (commit.hash.clone(), Tone::Dim),
            (format!(" {}", commit.subject), Tone::Plain),
        ],
        None => vec![("(no commits yet)".to_string(), Tone::Dim)],
    };

    let mut lines = Vec::new();
    let mut head = vec![(
        overview
            .branch
            .clone()
            .unwrap_or_else(|| "(detached)".to_string()),
        Tone::Emphasis,
    )];
    head.push(("  ".to_string(), Tone::Plain));
    head.extend(commit_parts(&overview.head));
    lines.push(HeaderLine {
        label: "Head:",
        parts: head,
    });

    for (label, tracked) in [("Upstream:", &overview.upstream), ("Push:", &overview.push)] {
        if let Some(tracked) = tracked {
            let mut parts = vec![
                (tracked.name.clone(), Tone::Emphasis),
                ("  ".to_string(), Tone::Plain),
            ];
            match &tracked.commit {
                Some(_) => parts.extend(commit_parts(&tracked.commit)),
                None => parts.push(("(not fetched)".to_string(), Tone::Dim)),
            }
            lines.push(HeaderLine { label, parts });
        }
    }

    if let Some(state) = &overview.in_progress {
        lines.push(HeaderLine {
            label: "State:",
            parts: vec![(state.description.clone(), Tone::Emphasis)],
        });
        let hint = match state.operation {
            // The rebase menu has all three.
            helix_magit::status::Operation::Rebase => {
                "r then c to continue, s to skip, z to abort".to_string()
            }
            operation => operation.hint().to_string(),
        };
        lines.push(HeaderLine {
            label: "",
            parts: vec![(hint, Tone::Dim)],
        });
    }
    lines
}

/// Every section, in Magit's order. Empty ones are kept, and skipped when
/// the rows are built, so a section's index never depends on its content.
fn build_sections(
    unmerged: Vec<Unmerged>,
    untracked: Vec<FileDiff>,
    unstaged: Vec<FileDiff>,
    staged: Vec<FileDiff>,
    overview: &Overview,
) -> Vec<Section> {
    let commits = |commits: &[helix_magit::status::Commit]| -> Vec<Item> {
        commits
            .iter()
            .map(|commit| Item {
                label: commit.hash.clone(),
                text: commit.subject.clone(),
            })
            .collect()
    };
    let upstream = overview
        .upstream
        .as_ref()
        .map_or("upstream", |upstream| upstream.name.as_str());

    let files = |kind, title: &str, files| Section {
        kind,
        title: title.to_string(),
        files,
        items: Vec::new(),
        folded: false,
    };
    let items = |kind, title: String, items| Section {
        kind,
        title,
        files: Vec::new(),
        items,
        folded: false,
    };

    vec![
        items(
            SectionKind::Unmerged,
            "Unmerged paths".to_string(),
            unmerged
                .into_iter()
                .map(|path| Item {
                    label: path.state.to_string(),
                    text: path.path.display().to_string(),
                })
                .collect(),
        ),
        files(SectionKind::Untracked, "Untracked files", untracked),
        files(SectionKind::Unstaged, "Unstaged changes", unstaged),
        files(SectionKind::Staged, "Staged changes", staged),
        items(
            SectionKind::Stashes,
            "Stashes".to_string(),
            overview
                .stashes
                .iter()
                .map(|stash| Item {
                    label: stash.name.clone(),
                    text: stash.subject.clone(),
                })
                .collect(),
        ),
        items(
            SectionKind::Unpulled,
            format!("Unpulled from {upstream}"),
            commits(&overview.unpulled),
        ),
        items(
            SectionKind::Unpushed,
            format!("Unmerged into {upstream}"),
            commits(&overview.unpushed),
        ),
        items(
            SectionKind::Recent,
            "Recent commits".to_string(),
            commits(&overview.recent),
        ),
    ]
}

/// The new-side line number of line `index` of a hunk. A deleted line is
/// not in the file; the line now in its place is the nearest one that is.
fn new_line_near(hunk: &helix_magit::DiffHunk, index: usize) -> u32 {
    let index = index.min(hunk.lines.len());
    hunk.lines[index..]
        .iter()
        .find_map(|line| line.new_line)
        .or_else(|| {
            hunk.lines[..index]
                .iter()
                .rev()
                .find_map(|line| line.new_line)
        })
        .unwrap_or(hunk.header.new_start)
}

/// What `x` throws away, once confirmed.
enum Discard {
    Change {
        file: FileDiff,
        selection: Selection,
        staged: bool,
    },
    Untracked(PathBuf),
    Stash(String),
}

/// One rendered line, and what it stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Header {
        line: usize,
    },
    Section {
        section: usize,
    },
    Item {
        section: usize,
        item: usize,
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
            Row::Header { .. } | Row::Section { .. } => 0,
            Row::Item { .. } | Row::File { .. } => 1,
            Row::Hunk { .. } => 2,
            Row::Line { .. } => 3,
        }
    }

    fn section(self) -> Option<usize> {
        match self {
            Row::Header { .. } => None,
            Row::Section { section }
            | Row::Item { section, .. }
            | Row::File { section, .. }
            | Row::Hunk { section, .. }
            | Row::Line { section, .. } => Some(section),
        }
    }
}

/// The status buffer.
pub struct DiffView {
    workdir: PathBuf,
    header: Vec<HeaderLine>,
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
            header: Vec::new(),
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

    /// Re-reads the repository after something outside the view changed it.
    ///
    /// Used after a git command runs: the index, HEAD or the working tree may
    /// all have moved, and the view has no other way to know.
    pub fn refresh(&mut self, editor: &mut Editor) {
        match Repository::discover(&self.workdir) {
            Ok(repository) => {
                if let Err(err) = self.reload(&repository) {
                    editor.set_error(err.to_string());
                }
            }
            Err(err) => editor.set_error(err.to_string()),
        }
    }

    /// Re-reads the repository, keeping the cursor as close as it can.
    ///
    /// Staging changes the shape of the tree — a fully staged file leaves the
    /// unstaged section entirely — so the cursor is clamped rather than
    /// restored exactly.
    fn reload(&mut self, repository: &Repository) -> Result<(), helix_magit::repository::Error> {
        let (unstaged, untracked) = repository.worktree_diffs()?;
        let staged = repository.staged_diff()?;
        let unmerged = repository.unmerged()?;
        let overview = helix_magit::status::read(repository.workdir());

        self.head = repository.head_description();
        self.header = header_lines(&overview);
        let sections = build_sections(unmerged, untracked, unstaged, staged, &overview);
        self.replace_sections(sections);
        Ok(())
    }

    /// Swaps in freshly read sections, carrying the folding over: folding is
    /// a view preference, so it survives a refresh. Untracked files start
    /// folded, as Magit lists them by name.
    fn replace_sections(&mut self, mut sections: Vec<Section>) {
        let mut files: HashMap<(SectionKind, PathBuf), bool> = HashMap::new();
        let mut folded_sections: HashMap<SectionKind, bool> = HashMap::new();
        for section in &self.sections {
            folded_sections.insert(section.kind, section.folded);
            for file in &section.files {
                files.insert((section.kind, file.path.clone()), file.folded);
            }
        }

        for section in sections.iter_mut() {
            section.folded = folded_sections
                .get(&section.kind)
                .copied()
                .unwrap_or(section.folded);
            for file in section.files.iter_mut() {
                file.folded = files
                    .get(&(section.kind, file.path.clone()))
                    .copied()
                    .unwrap_or(section.kind == SectionKind::Untracked);
            }
        }

        self.sections = sections;
        self.rebuild_rows();
    }

    /// Flattens the tree into the rows currently visible.
    fn rebuild_rows(&mut self) {
        let mut rows: Vec<Row> = (0..self.header.len())
            .map(|line| Row::Header { line })
            .collect();

        for (section_index, section) in self.sections.iter().enumerate() {
            if section.len() == 0 {
                continue;
            }
            rows.push(Row::Section {
                section: section_index,
            });
            if section.folded {
                continue;
            }

            for item in 0..section.items.len() {
                rows.push(Row::Item {
                    section: section_index,
                    item,
                });
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
            Row::Header { .. } | Row::Section { .. } | Row::Item { .. } => None,
            Row::File { section, file, .. }
            | Row::Hunk { section, file, .. }
            | Row::Line { section, file, .. } => self.sections.get(section)?.files.get(file),
        }
    }

    /// Turns the cursor's position into the selection an action applies to.
    fn selection_at(&self, row: Row) -> Option<Selection> {
        match row {
            Row::Header { .. } | Row::Section { .. } | Row::Item { .. } => None,
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
            Row::Header { .. } => return,
            Row::Section { section } => {
                if let Some(section) = self.sections.get_mut(section) {
                    section.folded = !section.folded;
                }
            }
            // A commit or stash has nothing folded under it; its section
            // folds, as a file's line folds its hunk.
            Row::Item { section, .. } => {
                if let Some(section) = self.sections.get_mut(section) {
                    section.folded = true;
                }
                self.move_to_parent();
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

    /// What `x` would throw away, and how to ask about it.
    fn discard_target(&self) -> Result<(String, Discard), String> {
        let row = self
            .current_row()
            .ok_or_else(|| "Nothing here".to_string())?;
        let section = row
            .section()
            .and_then(|section| self.sections.get(section))
            .ok_or_else(|| "Move to a change or a stash first".to_string())?;

        if let Row::Item { item, .. } = row {
            let item = &section.items[item];
            return match section.kind {
                SectionKind::Stashes => Ok((
                    format!("Drop {} ({})? (y/N)", item.label, item.text),
                    Discard::Stash(item.label.clone()),
                )),
                _ => Err("Commits cannot be discarded; see the reset menu".to_string()),
            };
        }

        let file = self
            .file_at(row)
            .ok_or_else(|| "Move to a file, hunk or line first".to_string())?;
        let selection = self
            .selection_at(row)
            .ok_or_else(|| "Move to a file, hunk or line first".to_string())?;
        let path = file.path.display();

        if section.kind == SectionKind::Untracked && selection == Selection::File {
            return Ok((
                format!("Delete untracked file {path}? (y/N)"),
                Discard::Untracked(file.path.clone()),
            ));
        }
        let what = match &selection {
            Selection::File => format!("all changes to {path}"),
            Selection::Hunk(_) => format!("this hunk of {path}"),
            Selection::Lines { .. } => format!("this line of {path}"),
        };
        let staged = section.kind.staged() == Some(true);
        let question = if staged {
            format!("Discard {what}, staged and in the working tree? (y/N)")
        } else {
            format!("Discard {what}? (y/N)")
        };
        Ok((
            question,
            Discard::Change {
                file: file.clone(),
                selection,
                staged,
            },
        ))
    }

    /// Throws away what the confirmation agreed to, then refreshes.
    fn discard(&mut self, target: Discard, editor: &mut Editor) {
        self.error = None;
        let repository = match Repository::discover(&self.workdir) {
            Ok(repository) => repository,
            Err(err) => {
                self.error = Some(err.to_string());
                return;
            }
        };
        let outcome = match &target {
            Discard::Change {
                file,
                selection,
                staged,
            } => repository.discard(file, selection, *staged),
            Discard::Untracked(path) => repository.discard_untracked(path),
            Discard::Stash(name) => {
                match helix_magit::GitCommand::new(
                    &self.workdir,
                    vec!["stash".into(), "drop".into(), name.clone()],
                )
                .run()
                {
                    Ok(output) if output.success => Ok(()),
                    Ok(output) => Err(helix_magit::repository::Error::Git(output.summary())),
                    Err(err) => Err(err.into()),
                }
            }
        };
        match outcome {
            Ok(()) => editor.set_status("Discarded"),
            Err(err) => self.error = Some(err.to_string()),
        }
        if let Err(err) = self.reload(&repository) {
            self.error = Some(err.to_string());
        }
    }

    /// `S` and `U`: every tracked change staged, or everything unstaged.
    fn apply_all(&mut self, stage: bool) {
        self.error = None;
        let repository = match Repository::discover(&self.workdir) {
            Ok(repository) => repository,
            Err(err) => {
                self.error = Some(err.to_string());
                return;
            }
        };
        let outcome = if stage {
            repository.stage_all()
        } else {
            repository.unstage_all()
        };
        if let Err(err) = outcome {
            self.error = Some(err.to_string());
        }
        if let Err(err) = self.reload(&repository) {
            self.error = Some(err.to_string());
        }
    }

    /// `v`: a staged change reversed out of the working tree, the index left
    /// as it is.
    fn reverse_change(&mut self) {
        self.error = None;
        let Some(row) = self.current_row() else {
            return;
        };
        let staged = row
            .section()
            .and_then(|section| self.sections.get(section))
            .and_then(|section| section.kind.staged());
        let (Some(file), Some(selection)) = (self.file_at(row).cloned(), self.selection_at(row))
        else {
            self.error = Some("Move to a change or a commit first".to_string());
            return;
        };
        if staged != Some(true) {
            self.error = Some("Unstaged changes cannot be reversed — use x to discard".to_string());
            return;
        }
        let outcome = Repository::discover(&self.workdir).and_then(|repository| {
            repository.reverse(&file, &selection)?;
            self.reload(&repository)
        });
        if let Err(err) = outcome {
            self.error = Some(err.to_string());
        }
    }

    /// The git command `a` (apply) or `v` (reverse) runs on the commit or
    /// stash under the cursor: Magit's cherry-apply, stash-apply and revert
    /// without committing.
    fn item_command(&self, reverse: bool) -> Option<Result<Plan, String>> {
        let Some(Row::Item { section, item }) = self.current_row() else {
            return None;
        };
        let section = self.sections.get(section)?;
        let label = section.items.get(item)?.label.clone();
        let (args, summary): (&[&str], &str) = match (section.kind, reverse) {
            (SectionKind::Unmerged, _) => {
                return Some(Err(
                    "Resolve the conflict in the file (RET visits it)".into()
                ))
            }
            (SectionKind::Stashes, false) => (&["stash", "apply"], "Apply stash"),
            (SectionKind::Stashes, true) => {
                return Some(Err(
                    "Reversing a stash is not supported; apply or drop it".into()
                ))
            }
            (_, false) => (&["cherry-pick", "--no-commit"], "Apply commit"),
            (_, true) => (&["revert", "--no-commit"], "Reverse commit"),
        };
        let mut args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        args.push(label);
        Some(Ok(Plan {
            args,
            requirement: Requirement::None,
            destructive: false,
            summary: summary.to_string(),
        }))
    }

    /// Where an interactive rebase from the commit under the cursor starts:
    /// its parent, so the commit itself is in the list; `--root` for the
    /// first commit.
    fn rebase_base_at_cursor(&self) -> Option<String> {
        let Some(Row::Item { section, item }) = self.current_row() else {
            return None;
        };
        let section = self.sections.get(section)?;
        if !matches!(
            section.kind,
            SectionKind::Unpushed | SectionKind::Unpulled | SectionKind::Recent
        ) {
            return None;
        }
        let hash = &section.items.get(item)?.label;
        let parent = format!("{hash}^");
        let has_parent = helix_magit::GitCommand::new(
            &self.workdir,
            vec![
                "rev-parse".into(),
                "--verify".into(),
                "--quiet".into(),
                parent.clone(),
            ],
        )
        .run()
        .is_ok_and(|output| output.success);
        Some(if has_parent {
            parent
        } else {
            "--root".to_string()
        })
    }

    /// Where `RET` on a change goes: the file in the working tree, at the
    /// line under the cursor. A 1-based line.
    fn visit_target(&self) -> Result<(PathBuf, usize), String> {
        let row = self
            .current_row()
            .ok_or_else(|| "Nothing here".to_string())?;
        if let Row::Item { section, item } = row {
            let section = &self.sections[section];
            if section.kind == SectionKind::Unmerged {
                return Ok((PathBuf::from(&section.items[item].text), 1));
            }
        }
        let file = self
            .file_at(row)
            .ok_or_else(|| "Move to a file, hunk or line first".to_string())?;
        if !self.workdir.join(&file.path).exists() {
            return Err(format!("{} is deleted", file.path.display()));
        }
        let line = match row {
            // The first changed line, rather than the context above it.
            Row::Hunk { hunk, .. } => file.hunks.get(hunk).map(|hunk| {
                let first = hunk
                    .lines
                    .iter()
                    .position(|line| line.kind != DiffLineKind::Context)
                    .unwrap_or(0);
                new_line_near(hunk, first)
            }),
            Row::Line { hunk, line, .. } => {
                file.hunks.get(hunk).map(|hunk| new_line_near(hunk, line))
            }
            _ => file.hunks.first().map(|hunk| hunk.header.new_start),
        };
        Ok((file.path.clone(), line.unwrap_or(1).max(1) as usize))
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

        let Some(section) = row.section() else {
            return;
        };
        match (stage, self.sections[section].kind.staged()) {
            (true, Some(true)) => {
                self.error = Some("Already staged — use u to unstage".to_string());
                return;
            }
            (false, Some(false)) => {
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

    /// The git command that shows the commit or stash under the cursor.
    fn show_args(&self) -> Option<Vec<String>> {
        let Some(Row::Item { section, item }) = self.current_row() else {
            return None;
        };
        let section = self.sections.get(section)?;
        let item = section.items.get(item)?;
        let args: &[&str] = match section.kind {
            SectionKind::Unmerged => return None,
            SectionKind::Stashes => &["stash", "show", "--patch", "--stat"],
            _ => &["show", "--stat", "--patch"],
        };
        let mut args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        args.push(item.label.clone());
        Some(args)
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
            (KeyCode::Enter, _) => {
                if let Some(args) = self.show_args() {
                    let workdir = self.workdir.clone();
                    return EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                        compositor.remove(DiffView::ID);
                        crate::magit::show(cx, workdir, args);
                    })));
                }
                match self.visit_target() {
                    Ok((path, line)) => {
                        let path = self.workdir.join(path);
                        return EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                            compositor.remove(DiffView::ID);
                            crate::roam::open_at(cx.editor, &path, line - 1);
                        })));
                    }
                    Err(err) => self.error = Some(err),
                }
            }
            (KeyCode::Char('x'), KeyModifiers::NONE) => match self.discard_target() {
                Ok((question, target)) => {
                    self.error = None;
                    return EventResult::Consumed(Some(Box::new(move |compositor, _| {
                        compositor.push(Box::new(Confirm::new(
                            question,
                            "This cannot be undone",
                            move |cx| {
                                cx.jobs.callback(async move {
                                    Ok(crate::job::Callback::EditorCompositor(Box::new(
                                        move |editor: &mut Editor, compositor: &mut Compositor| {
                                            if let Some(view) =
                                                compositor.find_id::<DiffView>(DiffView::ID)
                                            {
                                                view.discard(target, editor);
                                            }
                                        },
                                    )))
                                });
                            },
                        )));
                    })));
                }
                Err(err) => self.error = Some(err),
            },
            (KeyCode::Char('a' | 'v'), KeyModifiers::NONE) => {
                let reverse = key.code == KeyCode::Char('v');
                match self.item_command(reverse) {
                    Some(Ok(plan)) => {
                        let workdir = self.workdir.clone();
                        return EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                            crate::magit::execute(compositor, cx, plan, workdir);
                        })));
                    }
                    Some(Err(err)) => self.error = Some(err),
                    None if reverse => self.reverse_change(),
                    None => {
                        self.error = Some(
                            "A change here is already in the working tree; a applies commits and stashes"
                                .to_string(),
                        )
                    }
                }
            }
            // Magit's status buffer opens the menus by their dispatch keys.
            (KeyCode::Char(key @ ('c' | 'r' | 'P' | 'F' | 'b' | '?')), _) => {
                let kind = match key {
                    'c' => MenuKind::Commit,
                    'r' => MenuKind::Rebase,
                    'P' => MenuKind::Push,
                    'F' => MenuKind::Pull,
                    'b' => MenuKind::Branch,
                    _ => MenuKind::Main,
                };
                let mut overlay =
                    TransientOverlay::new(kind.menu(), self.head.clone(), self.workdir.clone());
                if kind == MenuKind::Rebase {
                    if let Some(base) = self.rebase_base_at_cursor() {
                        overlay = overlay.with_rebase_base(base);
                    }
                }
                return EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.push(Box::new(overlay));
                })));
            }
            (KeyCode::Char('S'), _) => self.apply_all(true),
            (KeyCode::Char('U'), _) => self.apply_all(false),
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
            Row::Header { line } => {
                let Some(line) = view.header.get(line) else {
                    return;
                };
                let mut parts = vec![(format!("{:<10}", line.label), Tone::Emphasis)];
                parts.extend(line.parts.iter().cloned());
                self.put_parts(surface, cell, &parts, focused);
            }
            Row::Section { section } => {
                let section = &view.sections[section];
                let text = format!(
                    "{} {} ({})",
                    fold_marker(section.folded),
                    section.title,
                    section.len()
                );
                self.put(
                    surface,
                    cell,
                    &text,
                    self.theme.get("ui.text.focus"),
                    focused,
                );
            }
            Row::Item { section, item } => {
                let Some(item) = view.sections[section].items.get(item) else {
                    return;
                };
                let parts = [
                    (item.label.clone(), Tone::Dim),
                    (format!(" {}", item.text), Tone::Plain),
                ];
                self.put_parts(surface, cell, &parts, focused);
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

    /// Draws a line made of differently styled parts, left to right.
    fn put_parts(
        &self,
        surface: &mut Surface,
        cell: Cell,
        parts: &[(String, Tone)],
        focused: bool,
    ) {
        let mut x = cell.x;
        let end = cell.x + cell.width as u16;
        for (text, tone) in parts {
            if x >= end {
                break;
            }
            let style = match tone {
                Tone::Plain => self.theme.get("ui.text"),
                Tone::Emphasis => self.theme.get("ui.text.focus"),
                Tone::Dim => self.theme.get("ui.virtual"),
            };
            let cell = Cell {
                x,
                width: (end - x) as usize,
                ..cell
            };
            self.put(surface, cell, text, style, focused);
            x += helix_core::unicode::width::UnicodeWidthStr::width(text.as_str()) as u16;
        }
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
        make_full_view(
            Vec::new(),
            Vec::new(),
            unstaged,
            staged,
            &Overview::default(),
        )
    }

    fn make_full_view(
        unmerged: Vec<Unmerged>,
        untracked: Vec<FileDiff>,
        unstaged: Vec<FileDiff>,
        staged: Vec<FileDiff>,
        overview: &Overview,
    ) -> DiffView {
        let mut view = DiffView {
            workdir: PathBuf::from("/repo"),
            header: Vec::new(),
            sections: Vec::new(),
            rows: Vec::new(),
            cursor: 0,
            scroll: 0,
            head: "main".to_string(),
            error: None,
        };
        view.replace_sections(build_sections(
            unmerged, untracked, unstaged, staged, overview,
        ));
        view
    }

    #[test]
    fn rows_flatten_the_tree_in_order() {
        let view = make_view(vec![file("a.rs", 2, 2)], Vec::new());

        // Section, file, then each hunk followed by its lines. The staged
        // section is empty, so it contributes no header.
        assert_eq!(view.rows.len(), 1 + 1 + (1 + 2) * 2);
        assert_eq!(view.rows[0], Row::Section { section: 2 });
        assert_eq!(
            view.rows[1],
            Row::File {
                section: 2,
                file: 0
            }
        );
        assert_eq!(
            view.rows[2],
            Row::Hunk {
                section: 2,
                file: 0,
                hunk: 0
            }
        );
        assert_eq!(
            view.rows[3],
            Row::Line {
                section: 2,
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
        assert_eq!(view.rows[0], Row::Section { section: 3 });
    }

    #[test]
    fn folding_a_file_hides_its_hunks() {
        let mut view = make_view(vec![file("a.rs", 2, 2)], Vec::new());
        let before = view.rows.len();

        view.cursor = 1; // the file row
        view.toggle_fold();

        assert_eq!(view.rows.len(), 2, "only the section and the file remain");
        assert!(view.sections[2].files[0].folded);

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
        assert!(view.sections[2].files[0].hunks[0].folded);
        assert!(!view.sections[2].files[0].hunks[1].folded);
    }

    #[test]
    fn folding_from_a_line_folds_its_hunk_and_moves_to_it() {
        let mut view = make_view(vec![file("a.rs", 1, 3)], Vec::new());
        view.cursor = 4; // the second line of the hunk

        view.toggle_fold();

        assert!(view.sections[2].files[0].hunks[0].folded);
        // The line the cursor was on no longer exists, so it lands on the hunk.
        assert_eq!(
            view.current_row(),
            Some(Row::Hunk {
                section: 2,
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

    fn overview() -> Overview {
        use helix_magit::status::{Commit, InProgress, Operation, Stash, Tracked};
        let commit = |hash: &str, subject: &str| Commit {
            hash: hash.into(),
            subject: subject.into(),
        };
        Overview {
            branch: Some("main".into()),
            head: Some(commit("abc1234", "Latest")),
            upstream: Some(Tracked {
                name: "origin/main".into(),
                commit: Some(commit("def5678", "Theirs")),
            }),
            push: None,
            in_progress: Some(InProgress {
                operation: Operation::Merge,
                description: "Merging 0123456".into(),
            }),
            unpulled: vec![commit("def5678", "Theirs")],
            unpushed: vec![commit("abc1234", "Latest")],
            recent: Vec::new(),
            stashes: vec![Stash {
                name: "stash@{0}".into(),
                subject: "WIP on main".into(),
            }],
        }
    }

    #[test]
    fn the_header_says_where_head_is_and_what_is_under_way() {
        let lines = header_lines(&overview());
        let text: Vec<String> = lines
            .iter()
            .map(|line| {
                let parts: String = line.parts.iter().map(|(text, _)| text.as_str()).collect();
                format!("{} {parts}", line.label)
            })
            .collect();
        assert_eq!(
            text,
            [
                "Head: main  abc1234 Latest",
                "Upstream: origin/main  def5678 Theirs",
                "State: Merging 0123456",
                " git merge --continue, or git merge --abort",
            ]
        );

        let detached = header_lines(&Overview::default());
        assert_eq!(detached.len(), 1);
        assert_eq!(detached[0].parts[0].0, "(detached)");
        assert_eq!(detached[0].parts[2].0, "(no commits yet)");
    }

    #[test]
    fn commits_and_stashes_have_sections_of_their_own() {
        let overview = overview();
        let mut view = make_full_view(Vec::new(), Vec::new(), Vec::new(), Vec::new(), &overview);
        view.header = header_lines(&overview);
        view.rebuild_rows();

        let titles: Vec<&str> = view
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Section { section } => Some(view.sections[*section].title.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            titles,
            [
                "Stashes",
                "Unpulled from origin/main",
                "Unmerged into origin/main"
            ]
        );
        // Four header lines, then each section and its one entry.
        assert_eq!(view.rows.len(), 4 + 3 * 2);
        assert!(matches!(view.rows[0], Row::Header { line: 0 }));

        // RET on a stash shows it with `git stash show`, on a commit with
        // `git show`; anywhere else it shows nothing.
        view.cursor = 5;
        assert_eq!(
            view.show_args().unwrap(),
            ["stash", "show", "--patch", "--stat", "stash@{0}"]
        );
        view.cursor = 7;
        assert_eq!(
            view.show_args().unwrap(),
            ["show", "--stat", "--patch", "def5678"]
        );
        view.cursor = 0;
        assert_eq!(view.show_args(), None);
    }

    #[test]
    fn untracked_files_start_folded_and_stage_like_unstaged_ones() {
        let mut view = make_full_view(
            Vec::new(),
            vec![file("new.rs", 1, 2)],
            Vec::new(),
            Vec::new(),
            &Overview::default(),
        );
        assert_eq!(view.rows.len(), 2, "the section and the file name");
        assert_eq!(view.sections[1].title, "Untracked files");

        view.cursor = 1;
        view.apply(false);
        assert!(view.error.as_deref().unwrap().contains("Not staged yet"));
    }

    #[test]
    fn folding_survives_a_refresh() {
        let mut view = make_view(vec![file("a.rs", 1, 1)], vec![file("b.rs", 1, 1)]);
        view.cursor = 0; // the unstaged section
        view.toggle_fold();
        let staged_file = view
            .rows
            .iter()
            .position(|row| matches!(row, Row::File { section: 3, .. }))
            .unwrap();
        view.cursor = staged_file;
        view.toggle_fold();

        view.replace_sections(build_sections(
            Vec::new(),
            vec![file("new.rs", 1, 1)],
            vec![file("a.rs", 1, 1)],
            vec![file("b.rs", 1, 1)],
            &Overview::default(),
        ));
        assert!(view.sections[2].folded);
        assert!(view.sections[3].files[0].folded);
        assert!(view.sections[1].files[0].folded, "a new untracked file");
    }

    fn parsed(text: &str) -> FileDiff {
        helix_magit::parse_unified_diff(text).remove(0)
    }

    const EDIT: &str = "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -10,4 +10,4 @@\n ctx\n-old\n+new\n more\n";

    #[test]
    fn discarding_asks_about_exactly_what_it_will_throw_away() {
        let mut view = make_view(vec![parsed(EDIT)], vec![parsed(EDIT)]);
        let question = |view: &DiffView| view.discard_target().map(|(question, _)| question);

        view.cursor = 1; // the unstaged file
        assert_eq!(
            question(&view).unwrap(),
            "Discard all changes to f.txt? (y/N)"
        );
        view.cursor = 2;
        assert_eq!(
            question(&view).unwrap(),
            "Discard this hunk of f.txt? (y/N)"
        );
        let staged_hunk = view
            .rows
            .iter()
            .position(|row| matches!(row, Row::Hunk { section: 3, .. }))
            .unwrap();
        view.cursor = staged_hunk;
        assert_eq!(
            question(&view).unwrap(),
            "Discard this hunk of f.txt, staged and in the working tree? (y/N)"
        );
        view.cursor = 0;
        assert!(question(&view).is_err(), "a section header");

        let untracked = make_full_view(
            vec![],
            vec![file("junk.txt", 1, 1)],
            vec![],
            vec![],
            &Overview::default(),
        );
        let mut untracked = untracked;
        untracked.cursor = 1;
        assert!(matches!(
            untracked.discard_target().unwrap().1,
            Discard::Untracked(_)
        ));
    }

    #[test]
    fn visiting_lands_on_the_line_under_the_cursor() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "x\n").unwrap();
        let mut view = make_view(vec![parsed(EDIT)], Vec::new());
        view.workdir = dir.path().to_path_buf();

        let at = |view: &mut DiffView, cursor| {
            view.cursor = cursor;
            view.visit_target().unwrap().1
        };
        assert_eq!(at(&mut view, 1), 10, "the file: its first hunk");
        assert_eq!(at(&mut view, 2), 11, "the hunk: its first change");
        assert_eq!(at(&mut view, 3), 10, "a context line");
        assert_eq!(at(&mut view, 4), 11, "a deleted line: what took its place");
        assert_eq!(at(&mut view, 5), 11, "an added line");
        assert_eq!(at(&mut view, 6), 12);

        std::fs::remove_file(dir.path().join("f.txt")).unwrap();
        view.cursor = 1;
        assert!(view.visit_target().unwrap_err().contains("deleted"));
    }

    #[test]
    fn a_and_v_apply_and_reverse_commits_and_stashes() {
        let overview = overview();
        let mut view = make_full_view(vec![], vec![], vec![], vec![], &overview);
        let row_of = |view: &DiffView, kind| {
            view.rows
                .iter()
                .position(|row| matches!(row, Row::Item { section, .. } if view.sections[*section].kind == kind))
                .unwrap()
        };

        view.cursor = row_of(&view, SectionKind::Stashes);
        assert_eq!(
            view.item_command(false).unwrap().unwrap().args,
            ["stash", "apply", "stash@{0}"]
        );
        assert!(view.item_command(true).unwrap().is_err());

        view.cursor = row_of(&view, SectionKind::Unpushed);
        assert_eq!(
            view.item_command(false).unwrap().unwrap().args,
            ["cherry-pick", "--no-commit", "abc1234"]
        );
        assert_eq!(
            view.item_command(true).unwrap().unwrap().args,
            ["revert", "--no-commit", "abc1234"]
        );
    }

    #[test]
    fn unstaged_changes_cannot_be_reversed() {
        let mut view = make_view(vec![parsed(EDIT)], Vec::new());
        view.cursor = 2;
        assert_eq!(view.item_command(true), None);
        view.reverse_change();
        assert!(view.error.as_deref().unwrap().contains("use x to discard"));
    }

    #[test]
    fn an_unmerged_path_is_visited_not_applied() {
        let mut view = make_full_view(
            vec![Unmerged {
                path: PathBuf::from("src/conflict.rs"),
                state: "both modified",
            }],
            vec![],
            vec![],
            vec![],
            &Overview::default(),
        );
        assert_eq!(view.sections[0].title, "Unmerged paths");
        view.cursor = 1;
        assert_eq!(
            view.visit_target().unwrap(),
            (PathBuf::from("src/conflict.rs"), 1)
        );
        assert!(view.item_command(false).unwrap().is_err());
        assert_eq!(view.show_args(), None);
    }
}
