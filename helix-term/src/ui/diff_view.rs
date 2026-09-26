//! The Magit-style status buffer: a diff you navigate and stage from.
//!
//! The model is a tree — section, file, hunk, line — but it is rendered and
//! navigated as a flat list of rows rebuilt whenever something folds or the
//! repository changes. That keeps "what is under the cursor", "what does a
//! keypress act on" and "what is on screen" one and the same question.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use helix_magit::diff::DiffOptions;
use helix_magit::diff::{DiffLineKind, FileDiff};
use helix_magit::status::Overview;
use helix_magit::{AskKind, Plan, Repository, Selection, Unmerged};
use helix_view::graphics::Rect;
use helix_view::input::{KeyCode, KeyModifiers};
use helix_view::Editor;
use tui::buffer::Buffer as Surface;
use tui::text::Text;
use tui::widgets::{Block, Widget};

use crate::compositor::{Component, Compositor, Context, Event, EventResult};
use crate::ui::confirm::Confirm;
use crate::ui::margin::{self, Margin, Stamp, STATUS_MARGIN};
use crate::ui::transient::TransientOverlay;
use helix_magit::transient::{MagitCommand, MenuKind};

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
    /// The files a shown commit changed.
    Commit,
    /// The other worktrees of the repository.
    Worktrees,
    Submodules,
    /// The refs view's three lists.
    LocalBranches,
    RemoteBranches,
    Tags,
    /// The cherries view: commits one branch has that another lacks.
    Cherries,
    /// The repository list.
    Repositories,
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
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Item {
    /// The abbreviated hash, or `stash@{0}`.
    label: String,
    text: String,
    /// A commit's author and date, for the margin.
    stamp: Option<Stamp>,
    /// A listed repository's directory.
    path: Option<PathBuf>,
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

    // Which part of the tree is there, when not all of it is.
    if let Some(directories) = &overview.sparse {
        let text = if directories.is_empty() {
            "top-level files only".to_string()
        } else {
            directories.join(" ")
        };
        lines.push(HeaderLine {
            label: "Sparse:",
            parts: vec![
                (text, Tone::Emphasis),
                ("  > to change".to_string(), Tone::Dim),
            ],
        });
    }

    if let Some(state) = &overview.in_progress {
        lines.push(HeaderLine {
            label: "State:",
            parts: vec![(state.description.clone(), Tone::Emphasis)],
        });
        // Each operation's menu has what gets out of it.
        use helix_magit::status::Operation;
        let hint = match state.operation {
            Operation::Rebase => "C to continue; r then s to skip, z to abort",
            Operation::Merge => "C to commit the merge; m then z to abort",
            Operation::CherryPick => "C to continue; A then s to skip, z to abort",
            Operation::Revert => "C to continue; V then s to skip, z to abort",
            Operation::Am => "C to continue; w then s to skip, z to abort",
            Operation::Bisect => "B then g good, b bad, s skip, r to end",
        }
        .to_string();
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
                stamp: Some(Stamp {
                    author: commit.author.clone(),
                    time: commit.time,
                    date: commit.date.clone(),
                }),
                path: None,
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
            "Unmerged paths — e to resolve, s once resolved".to_string(),
            unmerged
                .into_iter()
                .map(|path| Item {
                    label: path.state.to_string(),
                    text: path.path.display().to_string(),
                    ..Item::default()
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
                    ..Item::default()
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
        items(
            SectionKind::Worktrees,
            "Worktrees".to_string(),
            overview
                .worktrees
                .iter()
                .map(|tree| Item {
                    label: tree
                        .branch
                        .clone()
                        .unwrap_or_else(|| format!("(detached at {})", tree.head)),
                    text: tree.path.display().to_string(),
                    ..Item::default()
                })
                .collect(),
        ),
        items(
            SectionKind::Submodules,
            "Submodules".to_string(),
            overview
                .submodules
                .iter()
                .map(|module| Item {
                    label: module.path.clone(),
                    text: format!("{} {}", module.hash, module.state)
                        .trim_end()
                        .to_string(),
                    ..Item::default()
                })
                .collect(),
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
    /// Set when this shows a commit rather than the status: its hash.
    commit: Option<String>,
    /// What the view lists.
    kind: ViewKind,
    /// How the diffs are computed and shown.
    options: DiffOptions,
    /// With word marking on: the changed words of each changed line that
    /// has a counterpart, by section, file, hunk and line.
    word_ranges: HashMap<(usize, usize, usize, usize), Vec<Range<usize>>>,
    /// What the margin of commit lines shows.
    margin: Margin,
}

/// The views built on the status buffer's rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ViewKind {
    Status,
    /// The status of one file: its unstaged and staged changes.
    File(PathBuf),
    /// Between two revisions, or a revision and the working tree.
    Range {
        from: String,
        to: Option<String>,
    },
    Commit,
    Refs,
    Cherries {
        upstream: String,
        head: Option<String>,
    },
    /// The repositories under some directories, searched so deep.
    Repositories {
        roots: Vec<PathBuf>,
        depth: usize,
    },
}

impl DiffView {
    pub const ID: &'static str = "magit-status";
    /// A commit view, which opens over the status or the log.
    pub const COMMIT_ID: &'static str = "magit-commit";

    /// Shows a commit — or a stash — with the status buffer's rendering:
    /// its message above, its files, hunks and lines below.
    pub fn commit(workdir: &Path, rev: &str) -> Result<Self, String> {
        Self::commit_with(workdir, rev, DiffOptions::default())
    }

    fn commit_with(workdir: &Path, rev: &str, options: DiffOptions) -> Result<Self, String> {
        let details = helix_magit::log::show_with(workdir, rev, &options)?;
        // Looked at: on the revision stack, to refer to it in a message.
        crate::magit::remember_revision(workdir, rev);
        let mut header = vec![
            HeaderLine {
                label: "Commit:",
                parts: vec![(details.hash.clone(), Tone::Emphasis)],
            },
            HeaderLine {
                label: "Author:",
                parts: vec![(details.author.clone(), Tone::Plain)],
            },
            HeaderLine {
                label: "Date:",
                parts: vec![(details.date.clone(), Tone::Plain)],
            },
        ];
        if !details.refs.is_empty() {
            header.push(HeaderLine {
                label: "Refs:",
                parts: vec![(details.refs.join(", "), Tone::Emphasis)],
            });
        }
        for (index, line) in details.message.lines().enumerate() {
            header.push(HeaderLine {
                label: "",
                parts: vec![(
                    line.to_string(),
                    if index == 0 {
                        Tone::Emphasis
                    } else {
                        Tone::Plain
                    },
                )],
            });
        }

        let mut view = Self {
            workdir: workdir.to_path_buf(),
            header,
            sections: Vec::new(),
            rows: Vec::new(),
            cursor: 0,
            scroll: 0,
            head: if details.hash.starts_with(rev) {
                details.short.clone()
            } else {
                format!("{rev} ({})", details.short)
            },
            error: None,
            commit: Some(details.hash),
            kind: ViewKind::Commit,
            options,
            word_ranges: HashMap::new(),
            margin: STATUS_MARGIN.get(),
        };
        view.replace_sections(vec![Section {
            kind: SectionKind::Commit,
            title: "Changes".to_string(),
            files: details.files,
            items: Vec::new(),
            folded: false,
        }]);
        Ok(view)
    }

    /// The refs view, the cherries view and a range's diff.
    pub const REFS_ID: &'static str = "magit-refs";
    pub const CHERRIES_ID: &'static str = "magit-cherries";
    pub const RANGE_ID: &'static str = "magit-range";
    /// The repository list, under the status it opens.
    pub const REPOSITORIES_ID: &'static str = "magit-repositories";

    /// The diff between `from` and `to`, or between `from` and the working
    /// tree: Magit's `d r`.
    pub fn range(workdir: &Path, from: String, to: Option<String>) -> Self {
        let title = format!("Diff {from}..{}", to.as_deref().unwrap_or("working tree"));
        let mut view = Self::empty(workdir, title, ViewKind::Range { from, to });
        view.read_range();
        view
    }

    fn read_range(&mut self) {
        let ViewKind::Range { from, to } = &self.kind else {
            return;
        };
        match helix_magit::log::diff_range(&self.workdir, from, to.as_deref(), &self.options) {
            Ok(files) => {
                self.error = None;
                self.header = vec![HeaderLine {
                    label: "Diff:",
                    parts: vec![(
                        format!("{from} → {}", to.as_deref().unwrap_or("the working tree")),
                        Tone::Emphasis,
                    )],
                }];
                self.replace_sections(vec![Section {
                    kind: SectionKind::Commit,
                    title: "Changes".to_string(),
                    files,
                    items: Vec::new(),
                    folded: false,
                }]);
            }
            Err(err) => self.error = Some(err),
        }
    }

    /// The repository, with this view's diff options.
    fn repository(&self) -> Result<Repository, helix_magit::repository::Error> {
        Ok(Repository::discover(&self.workdir)?.with_diff_options(self.options.clone()))
    }

    /// Changes how the diffs are computed and shown, and reads them again.
    pub fn set_options(&mut self, options: DiffOptions) {
        self.options = options;
        match &self.kind {
            ViewKind::Commit => {
                let Some(hash) = self.commit.clone() else {
                    return;
                };
                match Self::commit_with(&self.workdir, &hash, self.options.clone()) {
                    Ok(view) => {
                        let (cursor, scroll, head) = (self.cursor, self.scroll, self.head.clone());
                        *self = view;
                        self.head = head;
                        self.cursor = cursor.min(self.rows.len().saturating_sub(1));
                        self.scroll = scroll;
                    }
                    Err(err) => self.error = Some(err),
                }
            }
            _ => match self.repository() {
                Ok(repository) => {
                    if let Err(err) = self.reload(&repository) {
                        self.error = Some(err.to_string());
                    }
                }
                Err(err) => self.error = Some(err.to_string()),
            },
        }
        self.rebuild_rows();
    }

    /// `+` / `-`: more or less context, read again.
    fn change_context(&mut self, delta: i32) {
        let mut options = self.options.clone();
        options.context = (options.context as i32 + delta).clamp(0, 1000) as u32;
        self.set_options(options);
    }

    fn view_id(&self) -> &'static str {
        match self.kind {
            // A file's diff answers to the status buffer's refreshes.
            ViewKind::Status | ViewKind::File(_) => Self::ID,
            ViewKind::Commit => Self::COMMIT_ID,
            ViewKind::Refs => Self::REFS_ID,
            ViewKind::Range { .. } => Self::RANGE_ID,
            ViewKind::Cherries { .. } => Self::CHERRIES_ID,
            ViewKind::Repositories { .. } => Self::REPOSITORIES_ID,
        }
    }

    fn empty(workdir: &Path, head: String, kind: ViewKind) -> Self {
        Self {
            workdir: workdir.to_path_buf(),
            header: Vec::new(),
            sections: Vec::new(),
            rows: Vec::new(),
            cursor: 0,
            scroll: 0,
            head,
            error: None,
            commit: None,
            kind,
            options: DiffOptions::default(),
            word_ranges: HashMap::new(),
            margin: STATUS_MARGIN.get(),
        }
    }

    /// Every branch, remote branch and tag, with where each stands against
    /// HEAD: Magit's `y`.
    pub fn refs(workdir: &Path) -> Self {
        let mut view = Self::empty(workdir, "Refs".to_string(), ViewKind::Refs);
        view.read_refs();
        view
    }

    fn read_refs(&mut self) {
        use helix_magit::refs::RefKind;
        let refs = helix_magit::refs::read_refs(&self.workdir);
        let list = |kind: RefKind| -> Vec<Item> {
            refs.iter()
                .filter(|info| info.kind == kind)
                .map(|info| {
                    let mut text = info.hash.clone();
                    if info.is_head {
                        text.push_str(" (HEAD)");
                    }
                    if let Some(upstream) = &info.upstream {
                        text.push_str(&format!(" → {upstream}"));
                    }
                    let relation = info.relation();
                    if !relation.is_empty() {
                        text.push_str(&format!(" [{relation}]"));
                    }
                    text.push(' ');
                    text.push_str(&info.subject);
                    Item {
                        label: info.name.clone(),
                        text,
                        ..Item::default()
                    }
                })
                .collect()
        };
        let section = |kind, title: &str, items| Section {
            kind,
            title: title.to_string(),
            files: Vec::new(),
            items,
            folded: false,
        };
        self.replace_sections(vec![
            section(SectionKind::LocalBranches, "Branches", list(RefKind::Local)),
            section(
                SectionKind::RemoteBranches,
                "Remote branches",
                list(RefKind::Remote),
            ),
            section(SectionKind::Tags, "Tags", list(RefKind::Tag)),
        ]);
    }

    /// The repositories under `roots`, each with its branch, how far it is
    /// from its upstream, and whether it has uncommitted changes. `RET`
    /// opens one's status; a menu key opens that menu on it.
    pub fn repositories(roots: Vec<PathBuf>, depth: usize) -> Self {
        let workdir = roots
            .first()
            .cloned()
            .unwrap_or_else(helix_stdx::env::current_working_dir);
        let mut view = Self::empty(
            &workdir,
            "Repositories".to_string(),
            ViewKind::Repositories { roots, depth },
        );
        view.read_repositories();
        view
    }

    fn read_repositories(&mut self) {
        let ViewKind::Repositories { roots, depth } = &self.kind else {
            return;
        };
        // Each named from the directory it was found under, in a column.
        let found = helix_magit::repos::find(roots, *depth);
        let names: Vec<String> = found
            .iter()
            .map(|path| {
                let relative = roots.iter().find_map(|root| {
                    let root = root.canonicalize().unwrap_or_else(|_| root.clone());
                    path.strip_prefix(&root)
                        .ok()
                        .filter(|rest| !rest.as_os_str().is_empty())
                        .map(Path::to_path_buf)
                });
                // A root that is itself a repository goes by its name.
                relative
                    .or_else(|| path.file_name().map(PathBuf::from))
                    .unwrap_or_else(|| path.clone())
                    .display()
                    .to_string()
            })
            .collect();
        let width = names
            .iter()
            .map(|name| helix_core::unicode::width::UnicodeWidthStr::width(name.as_str()))
            .max()
            .unwrap_or(0);
        let items: Vec<Item> = found
            .iter()
            .zip(names)
            .map(|(path, name)| {
                let pad = width - helix_core::unicode::width::UnicodeWidthStr::width(name.as_str());
                Item {
                    label: format!("{name}{}", " ".repeat(pad)),
                    text: helix_magit::repos::summarize(path).describe(),
                    path: Some(path.clone()),
                    ..Item::default()
                }
            })
            .collect();
        let searched: Vec<String> = roots
            .iter()
            .map(|root| {
                helix_stdx::path::fold_home_dir(root.as_path())
                    .display()
                    .to_string()
            })
            .collect();
        self.header = vec![HeaderLine {
            label: "Searched:",
            parts: vec![
                (searched.join(", "), Tone::Emphasis),
                (format!("  {depth} levels down"), Tone::Dim),
            ],
        }];
        if items.is_empty() {
            self.header.push(HeaderLine {
                label: "",
                parts: vec![(
                    "No repository found; see [editor.magit] repository-directories".to_string(),
                    Tone::Dim,
                )],
            });
        }
        self.replace_sections(vec![Section {
            kind: SectionKind::Repositories,
            title: "Repositories".to_string(),
            files: Vec::new(),
            items,
            folded: false,
        }]);
    }

    /// The commits `head` has that `upstream` lacks: Magit's `Y`. A commit
    /// marked `-` has an equivalent change upstream already.
    pub fn cherries(workdir: &Path, upstream: String, head: Option<String>) -> Self {
        let title = format!(
            "Cherries: {} not in {upstream}",
            head.as_deref().unwrap_or("HEAD")
        );
        let mut view = Self::empty(workdir, title, ViewKind::Cherries { upstream, head });
        view.read_cherries();
        view
    }

    fn read_cherries(&mut self) {
        let ViewKind::Cherries { upstream, head } = &self.kind else {
            return;
        };
        match helix_magit::refs::cherries(&self.workdir, upstream, head.as_deref()) {
            Ok(cherries) => {
                let items = cherries
                    .into_iter()
                    .map(|cherry| Item {
                        label: cherry.hash,
                        text: if cherry.equivalent {
                            format!("- {} (an equivalent is upstream)", cherry.subject)
                        } else {
                            format!("+ {}", cherry.subject)
                        },
                        ..Item::default()
                    })
                    .collect();
                self.error = None;
                self.replace_sections(vec![Section {
                    kind: SectionKind::Cherries,
                    title: "Commits".to_string(),
                    files: Vec::new(),
                    items,
                    folded: false,
                }]);
            }
            Err(err) => self.error = Some(err),
        }
    }

    /// Opens the status of the repository containing `path`.
    pub fn new(path: &Path) -> Result<Self, helix_magit::repository::Error> {
        let repository = Repository::discover(path)?.with_diff_options(DiffOptions::default());
        let mut view = Self {
            workdir: repository.workdir().to_path_buf(),
            header: Vec::new(),
            sections: Vec::new(),
            rows: Vec::new(),
            cursor: 0,
            scroll: 0,
            head: repository.head_description(),
            error: None,
            commit: None,
            kind: ViewKind::Status,
            options: DiffOptions::default(),
            word_ranges: HashMap::new(),
            margin: STATUS_MARGIN.get(),
        };
        view.reload(&repository)?;
        Ok(view)
    }

    /// The changes to one file, staged and not, to stage and discard as in
    /// the status buffer: the file dispatch's diff.
    pub fn file(workdir: &Path, path: PathBuf) -> Result<Self, helix_magit::repository::Error> {
        let repository = Repository::discover(workdir)?;
        let mut view = Self::empty(
            repository.workdir(),
            format!("Diff: {}", path.display()),
            ViewKind::File(path),
        );
        view.reload(&repository)?;
        Ok(view)
    }

    /// Re-reads the repository after something outside the view changed it.
    ///
    /// Used after a git command runs: the index, HEAD or the working tree may
    /// all have moved, and the view has no other way to know.
    pub fn refresh(&mut self, editor: &mut Editor) {
        // The list is of several repositories, not in one.
        if matches!(self.kind, ViewKind::Repositories { .. }) {
            self.read_repositories();
            return;
        }
        match self.repository() {
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
        match self.kind {
            ViewKind::Status | ViewKind::File(_) => {}
            // A commit does not change.
            ViewKind::Commit => return Ok(()),
            ViewKind::Refs => {
                self.read_refs();
                return Ok(());
            }
            ViewKind::Cherries { .. } => {
                self.read_cherries();
                return Ok(());
            }
            ViewKind::Range { .. } => {
                self.read_range();
                return Ok(());
            }
            ViewKind::Repositories { .. } => {
                self.read_repositories();
                return Ok(());
            }
        }
        let (mut unstaged, mut untracked) = repository.worktree_diffs()?;
        let mut staged = repository.staged_diff()?;
        let mut unmerged = repository.unmerged()?;

        if let ViewKind::File(path) = &self.kind {
            unstaged.retain(|file| &file.path == path);
            untracked.retain(|file| &file.path == path);
            staged.retain(|file| &file.path == path);
            unmerged.retain(|entry| &entry.path == path);
            self.header = vec![HeaderLine {
                label: "File:",
                parts: vec![(path.display().to_string(), Tone::Emphasis)],
            }];
            if unstaged.is_empty() && untracked.is_empty() && staged.is_empty() {
                self.header.push(HeaderLine {
                    label: "",
                    parts: vec![("No changes to this file".to_string(), Tone::Dim)],
                });
            }
            let sections =
                build_sections(unmerged, untracked, unstaged, staged, &Overview::default());
            self.replace_sections(sections);
            return Ok(());
        }
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
                // The summary shows each file's size of change, no hunks.
                if file.folded || self.options.stat {
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

        self.word_ranges.clear();
        if self.options.word_diff && !self.options.stat {
            for (s, section) in self.sections.iter().enumerate() {
                for (f, file) in section.files.iter().enumerate() {
                    for (h, hunk) in file.hunks.iter().enumerate() {
                        for (line, ranges) in helix_magit::diff::refine(hunk) {
                            self.word_ranges.insert((s, f, h, line), ranges);
                        }
                    }
                }
            }
        }
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

        if section.kind.staged().is_none() {
            return Err("A commit cannot be discarded; v reverses its change".to_string());
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
        // What is about to be thrown away goes to the wip refs first, when
        // they are on; if that fails, nothing is thrown away.
        if editor.config().magit.wip {
            if let Err(err) = helix_magit::wip::save(&self.workdir, "before discarding", None) {
                self.error = Some(format!("wip save failed, nothing discarded: {err}"));
                return;
            }
        }
        let repository = match self.repository() {
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
            Ok(()) => {
                editor.set_status("Discarded");
                after_worktree_change(editor);
            }
            Err(err) => self.error = Some(err.to_string()),
        }
        if let Err(err) = self.reload(&repository) {
            self.error = Some(err.to_string());
        }
    }

    /// `S` and `U`: every tracked change staged, or everything unstaged.
    fn apply_all(&mut self, stage: bool) {
        self.error = None;
        let repository = match self.repository() {
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
        let in_commit = row
            .section()
            .and_then(|section| self.sections.get(section))
            .is_some_and(|section| section.kind == SectionKind::Commit);
        if in_commit {
            return self.apply_commit_change(true);
        }
        if staged != Some(true) {
            self.error = Some("Unstaged changes cannot be reversed — use x to discard".to_string());
            return;
        }
        let outcome = self.repository().and_then(|repository| {
            repository.reverse(&file, &selection)?;
            self.reload(&repository)
        });
        if let Err(err) = outcome {
            self.error = Some(err.to_string());
        }
    }

    /// `a` and `v` in a commit: the change under the cursor applied to the
    /// working tree, or taken back out of it.
    fn apply_commit_change(&mut self, reverse: bool) {
        self.error = None;
        let Some(row) = self.current_row() else {
            return;
        };
        let (Some(file), Some(selection)) = (self.file_at(row).cloned(), self.selection_at(row))
        else {
            self.error = Some("Move to a file, hunk or line first".to_string());
            return;
        };
        let outcome = self
            .repository()
            .and_then(|repository| repository.apply_to_worktree(&file, &selection, reverse));
        self.error = Some(match outcome {
            Ok(()) if reverse => "Reversed in the working tree".to_string(),
            Ok(()) => "Applied to the working tree".to_string(),
            Err(err) => err.to_string(),
        });
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
            (SectionKind::Worktrees | SectionKind::Submodules | SectionKind::Repositories, _) => {
                return Some(Err("RET opens its status".into()))
            }
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
        Some(Ok(Plan::new(args, summary)))
    }

    /// What the menus act on when opened here: the commit this view shows,
    /// or the commit, stash, branch, tag or file under the cursor.
    fn target_at_cursor(&self) -> Option<(String, AskKind)> {
        if let Some(commit) = &self.commit {
            return Some((commit.clone(), AskKind::Revision));
        }
        let row = self.current_row()?;
        let section = self.sections.get(row.section()?)?;
        if let Row::Item { item, .. } = row {
            let item = section.items.get(item)?;
            let kind = match section.kind {
                SectionKind::Unpushed
                | SectionKind::Unpulled
                | SectionKind::Recent
                | SectionKind::Cherries => AskKind::Revision,
                SectionKind::Stashes => AskKind::Stash,
                SectionKind::LocalBranches | SectionKind::RemoteBranches => AskKind::Branch,
                SectionKind::Tags => AskKind::Tag,
                SectionKind::Worktrees | SectionKind::Unmerged => {
                    return Some((item.text.clone(), AskKind::Path))
                }
                SectionKind::Submodules => return Some((item.label.clone(), AskKind::Path)),
                _ => return None,
            };
            return Some((item.label.clone(), kind));
        }
        self.file_at(row)
            .map(|file| (file.path.display().to_string(), AskKind::Path))
    }

    /// The revision this view is about, for `A-w`: the commit shown, the
    /// newer end of a range, or HEAD.
    fn view_revision(&self) -> Option<String> {
        let rev = match (&self.kind, &self.commit) {
            (_, Some(commit)) => commit.clone(),
            (ViewKind::Range { from, to }, _) => to.clone().unwrap_or_else(|| from.clone()),
            (ViewKind::Repositories { .. }, _) => return None,
            _ => "HEAD".to_string(),
        };
        crate::magit::short_hash(&self.workdir, &rev)
    }

    /// What `RET` shows: the commit, stash or ref under the cursor.
    fn revision_at_cursor(&self) -> Option<String> {
        let Some(Row::Item { section, item }) = self.current_row() else {
            return None;
        };
        let section = self.sections.get(section)?;
        (!matches!(
            section.kind,
            SectionKind::Unmerged
                | SectionKind::Worktrees
                | SectionKind::Submodules
                | SectionKind::Repositories
        ))
        .then(|| section.items.get(item).map(|item| item.label.clone()))?
    }

    /// Moves the cursor to a section's header. Returns false when the
    /// section is empty, and so not shown.
    pub fn jump_to(&mut self, target: helix_magit::transient::JumpTarget) -> bool {
        use helix_magit::transient::JumpTarget;
        let kind = match target {
            JumpTarget::Unmerged => SectionKind::Unmerged,
            JumpTarget::Untracked => SectionKind::Untracked,
            JumpTarget::Unstaged => SectionKind::Unstaged,
            JumpTarget::Staged => SectionKind::Staged,
            JumpTarget::Stashes => SectionKind::Stashes,
            JumpTarget::Unpulled => SectionKind::Unpulled,
            JumpTarget::Unpushed => SectionKind::Unpushed,
            JumpTarget::Recent => SectionKind::Recent,
            JumpTarget::Worktrees => SectionKind::Worktrees,
            JumpTarget::Submodules => SectionKind::Submodules,
        };
        let Some(row) = self.rows.iter().position(
            |row| matches!(row, Row::Section { section } if self.sections[*section].kind == kind),
        ) else {
            return false;
        };
        self.cursor = row;
        // Shown near the top rather than wherever scrolling left it.
        self.scroll = row;
        true
    }

    /// The conflicted path under the cursor.
    fn unmerged_at_cursor(&self) -> Option<String> {
        let Some(Row::Item { section, item }) = self.current_row() else {
            return None;
        };
        let section = self.sections.get(section)?;
        (section.kind == SectionKind::Unmerged)
            .then(|| section.items.get(item).map(|item| item.text.clone()))?
    }

    /// Runs a menu command on `path` without its menu.
    fn run_on_path(&self, command: MagitCommand, path: String) -> EventResult {
        let Some(mut plan) = helix_magit::resolve(command, &[]) else {
            return EventResult::Consumed(None);
        };
        plan.preset(&path, AskKind::Path);
        let plan = match &plan.requirement {
            helix_magit::Requirement::Ask(asks) => {
                let answers: Vec<String> = asks
                    .iter()
                    .map(|ask| ask.preset.clone().unwrap_or_default())
                    .collect();
                plan.answered(&answers)
            }
            _ => plan,
        };
        let workdir = self.workdir.clone();
        EventResult::Consumed(Some(Box::new(move |compositor, cx| {
            crate::magit::execute(compositor, cx, plan, workdir);
        })))
    }

    /// The worktree, submodule or listed repository under the cursor, whose
    /// own status `RET` opens.
    fn repository_at_cursor(&self) -> Option<PathBuf> {
        let Some(Row::Item { section, item }) = self.current_row() else {
            return None;
        };
        let section = self.sections.get(section)?;
        let item = section.items.get(item)?;
        match section.kind {
            SectionKind::Worktrees => Some(PathBuf::from(&item.text)),
            SectionKind::Submodules => Some(self.workdir.join(&item.label)),
            SectionKind::Repositories => item.path.clone(),
            _ => None,
        }
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
            (_, None) => {
                self.error = Some("A commit's change: a applies it, v reverses it".to_string());
                return;
            }
            _ => {}
        }

        let Some(file) = self.file_at(row).cloned() else {
            return;
        };

        let repository = match self.repository() {
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

    /// The most lines any one file shown here changes, which the summary's
    /// bars are scaled to.
    fn largest_change(&self) -> usize {
        self.sections
            .iter()
            .flat_map(|section| &section.files)
            .map(|file| {
                let (added, removed) = file.stats();
                added + removed
            })
            .max()
            .unwrap_or(0)
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
        // Docked, in Magit's pane; otherwise over the documents.
        let area = cx
            .editor
            .dock
            .area_of(crate::ui::dock::MAGIT)
            .unwrap_or_else(|| {
                viewport.intersection(Rect::new(
                    0,
                    0,
                    viewport.width,
                    viewport.height.saturating_sub(1),
                ))
            });
        let popup_style = cx.editor.theme.get("ui.popup");
        surface.clear_with(area, popup_style);

        let settings = self.options.describe();
        let settings = if settings.is_empty() {
            String::new()
        } else {
            format!(" [{settings}]")
        };
        let title = match (&self.error, &self.kind) {
            (Some(error), _) => format!("Magit: {error}"),
            (None, ViewKind::Commit) => format!("Commit {}", self.head),
            (None, ViewKind::Status) => format!("Magit: {}", self.head),
            (None, ViewKind::File(_)) => self.head.clone(),
            (None, _) => self.head.clone(),
        };
        let title = format!("{title}{settings}");
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

        // The margin of the visible commits, aligned among themselves.
        let visible = || {
            self.rows
                .iter()
                .skip(self.scroll)
                .take(inner.height as usize)
        };
        let stamps: Vec<Option<&Stamp>> = visible()
            .map(|row| match row {
                Row::Item { section, item } => self.sections[*section]
                    .items
                    .get(*item)
                    .and_then(|item| item.stamp.as_ref()),
                _ => None,
            })
            .collect();
        let margins = margin::column(&stamps, self.margin, margin::now());

        for ((offset, row), margin) in self
            .rows
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(inner.height as usize)
            .zip(&margins)
        {
            let y = inner.y + (offset - self.scroll) as u16;
            let line = Rect::new(inner.x, y, inner.width, 1);
            renderer.render(self, *row, offset == self.cursor, line, margin, surface);
        }
    }

    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        // Docked, the keys are Magit's only while it has the focus; `Esc`
        // gives them back to the documents and `q` still closes.
        if let Some(result) = crate::ui::dock::route(crate::ui::dock::MAGIT, event, cx.editor, true)
        {
            return result;
        }
        let Event::Key(key) = event else {
            return EventResult::Consumed(None);
        };

        // The status buffer is modal: it owns the keyboard while it is open.
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('q'), KeyModifiers::NONE) => {
                let id = self.view_id();
                return EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.remove(id);
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
                if let Some(path) = self.repository_at_cursor() {
                    match DiffView::new(&path) {
                        Ok(view) => {
                            return EventResult::Consumed(Some(Box::new(move |compositor, _| {
                                compositor.remove(DiffView::ID);
                                compositor.push(Box::new(view));
                            })))
                        }
                        Err(err) => self.error = Some(err.to_string()),
                    }
                    return EventResult::Consumed(None);
                }
                if let Some(rev) = self.revision_at_cursor() {
                    match DiffView::commit(&self.workdir, &rev) {
                        Ok(view) => {
                            return EventResult::Consumed(Some(Box::new(move |compositor, _| {
                                compositor.push(Box::new(view));
                            })))
                        }
                        Err(err) => self.error = Some(err),
                    }
                    return EventResult::Consumed(None);
                }
                match self.visit_target() {
                    Ok((path, line)) => {
                        let path = self.workdir.join(path);
                        return EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                            crate::magit::step_aside(compositor, cx.editor);
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
                    None if reverse => {
                        self.reverse_change();
                        after_worktree_change(cx.editor);
                    }
                    None if self.kind == ViewKind::Commit => {
                        self.apply_commit_change(false);
                        after_worktree_change(cx.editor);
                    }
                    None => {
                        self.error = Some(
                            "A change here is already in the working tree; a applies commits and stashes"
                                .to_string(),
                        )
                    }
                }
            }
            (KeyCode::Char('S'), _) => self.apply_all(true),
            (KeyCode::Char('U'), _) => self.apply_all(false),
            // On a conflicted path, staging is marking it resolved.
            (KeyCode::Char('s'), KeyModifiers::NONE) if self.unmerged_at_cursor().is_some() => {
                let path = self.unmerged_at_cursor().unwrap_or_default();
                return self.run_on_path(MagitCommand::ConflictMarkResolved, path);
            }
            (KeyCode::Char('e'), KeyModifiers::NONE) => match self.unmerged_at_cursor() {
                Some(path) => {
                    let overlay = TransientOverlay::new(
                        MenuKind::Resolve.menu(),
                        path.clone(),
                        self.workdir.clone(),
                    )
                    .with_target(path, AskKind::Path);
                    return EventResult::Consumed(Some(Box::new(move |compositor, _| {
                        compositor.push(Box::new(overlay));
                    })));
                }
                None => self.error = Some("e resolves a conflicted path".to_string()),
            },
            (KeyCode::Char('C'), _) => {
                let plan = helix_magit::resolve(MagitCommand::Continue, &[]);
                let workdir = self.workdir.clone();
                if let Some(plan) = plan {
                    return EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                        crate::magit::execute(compositor, cx, plan, workdir);
                    })));
                }
            }
            (KeyCode::Char('s'), KeyModifiers::NONE) => self.apply(true),
            (KeyCode::Char('u'), KeyModifiers::NONE) => self.apply(false),
            // Magit's diff buffer keys: more or less context, and back.
            (KeyCode::Char('+'), _) => self.change_context(1),
            (KeyCode::Char('-'), _) => self.change_context(-1),
            (KeyCode::Char('0'), _) => {
                let options = DiffOptions {
                    context: DiffOptions::default().context,
                    ..self.options.clone()
                };
                self.set_options(options);
            }
            // Copy the value under the cursor (Magit's C-w), or the view's
            // revision (M-w).
            (KeyCode::Char('w'), KeyModifiers::CONTROL) => match self.target_at_cursor() {
                Some((value, kind)) => {
                    crate::magit::copy_value(cx.editor, &self.workdir, &value, kind)
                }
                None => cx.editor.set_error("Nothing to copy here"),
            },
            (KeyCode::Char('w'), KeyModifiers::ALT) => match self.view_revision() {
                Some(hash) => {
                    crate::magit::copy_value(cx.editor, &self.workdir, &hash, AskKind::Revision)
                }
                None => cx.editor.set_error("This view has no revision"),
            },
            (KeyCode::Char('Z'), _) => {
                self.margin = self.margin.next();
                STATUS_MARGIN.set(self.margin);
                cx.editor.set_status(self.margin.describe());
            }
            // The settings menu shows this view's settings as they are.
            (KeyCode::Char('D'), _) => {
                let overlay = TransientOverlay::new(
                    helix_magit::transient::diff_settings_menu(&self.options),
                    self.head.clone(),
                    self.workdir.clone(),
                );
                return EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.push(Box::new(overlay));
                })));
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
            (KeyCode::Char('y'), KeyModifiers::NONE) => {
                let view = DiffView::refs(&self.workdir);
                return EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.push(Box::new(view));
                })));
            }
            (KeyCode::Char('Y'), _) => {
                let workdir = self.workdir.clone();
                return EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.push(Box::new(cherries_prompt(workdir)));
                })));
            }
            (KeyCode::Char('$'), _) => {
                return EventResult::Consumed(Some(Box::new(|compositor, cx| {
                    crate::magit::step_aside(compositor, cx.editor);
                    crate::magit::show_process(cx.editor);
                })));
            }
            // Magit's dispatch keys open the menus. `l` folds here, as in
            // the rest of Helix, so the log is `L`.
            (KeyCode::Char(key), _) if key == 'L' || MenuKind::for_key(key).is_some() => {
                let kind = MenuKind::for_key(key).unwrap_or(MenuKind::Log);
                // In the repository list, a menu acts on the repository
                // under the cursor.
                let (head, workdir) = match &self.kind {
                    ViewKind::Repositories { .. } => match self.repository_at_cursor() {
                        Some(path) => (path.display().to_string(), path),
                        None => return EventResult::Consumed(None),
                    },
                    _ => (self.head.clone(), self.workdir.clone()),
                };
                let mut overlay = TransientOverlay::new(kind.menu(), head, workdir);
                if let Some((value, kind)) = self.target_at_cursor() {
                    overlay = overlay.with_target(value, kind);
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
        Some(self.view_id())
    }
}

/// Reloads the open documents a change to the working tree made stale, and
/// says which could not be reloaded because they hold unsaved changes.
fn after_worktree_change(editor: &mut Editor) {
    let stale = crate::magit::refresh_documents(editor);
    if let Some(message) = crate::magit::stale_message(&stale) {
        editor.set_error(message);
    }
}

/// Asks for the revisions a diff from the diff menu needs, then opens it:
/// from and to for a range, one for the working tree against it or for a
/// commit. `start` fills the first question, from what the menu was
/// opened on.
pub fn diff_prompt(
    command: MagitCommand,
    workdir: PathBuf,
    start: Option<String>,
    editor: &Editor,
) -> crate::ui::Prompt {
    let label = match command {
        MagitCommand::DiffRange => "Diff from: ",
        MagitCommand::DiffWorktree => "Working tree against: ",
        _ => "Show commit: ",
    };
    let names = helix_magit::refs::names(&workdir, AskKind::Revision);
    let complete = move |_: &Editor, input: &str| -> Vec<crate::ui::prompt::Completion> {
        names
            .iter()
            .filter(|name| name.contains(input))
            .map(|name| (0.., name.clone().into()))
            .collect()
    };
    let prompt = crate::ui::Prompt::new(
        label.into(),
        None,
        complete.clone(),
        move |cx, input, event| {
            if event != crate::ui::PromptEvent::Validate {
                return;
            }
            let rev = input.trim().to_string();
            if let Err(err) = helix_magit::log::LogFilter::valid_range(&rev) {
                cx.editor.set_error(err);
                return;
            }
            let workdir = workdir.clone();
            let complete = complete.clone();
            cx.jobs.callback(async move {
                Ok(crate::job::Callback::EditorCompositor(Box::new(
                    move |editor: &mut Editor, compositor: &mut Compositor| match command {
                        MagitCommand::DiffRange => {
                            compositor.push(Box::new(crate::ui::Prompt::new(
                                "Diff to (empty for the working tree): ".into(),
                                None,
                                complete,
                                move |cx, input, event| {
                                    if event != crate::ui::PromptEvent::Validate {
                                        return;
                                    }
                                    let to = input.trim().to_string();
                                    if !to.is_empty() {
                                        if let Err(err) =
                                            helix_magit::log::LogFilter::valid_range(&to)
                                        {
                                            cx.editor.set_error(err);
                                            return;
                                        }
                                    }
                                    let (workdir, from) = (workdir.clone(), rev.clone());
                                    cx.jobs.callback(async move {
                                        Ok(crate::job::Callback::EditorCompositor(Box::new(
                                            move |_: &mut Editor, compositor: &mut Compositor| {
                                                let to = (!to.is_empty()).then_some(to);
                                                compositor.push(Box::new(DiffView::range(
                                                    &workdir, from, to,
                                                )));
                                            },
                                        )))
                                    });
                                },
                            )));
                        }
                        MagitCommand::DiffWorktree => {
                            compositor.push(Box::new(DiffView::range(&workdir, rev, None)));
                        }
                        _ => match DiffView::commit(&workdir, &rev) {
                            Ok(view) => compositor.push(Box::new(view)),
                            Err(err) => editor.set_error(err),
                        },
                    },
                )))
            });
        },
    );
    match start {
        Some(start) => prompt.with_line(start, editor),
        None => prompt,
    }
}

/// Asks which branch to compare against, then opens the cherries view.
pub fn cherries_prompt(workdir: PathBuf) -> crate::ui::Prompt {
    let names = helix_magit::refs::names(&workdir, AskKind::Branch);
    crate::ui::Prompt::new(
        "Cherries: commits HEAD has that are not in (empty for the upstream): ".into(),
        None,
        move |_, input| {
            names
                .iter()
                .filter(|name| name.contains(input))
                .map(|name| (0.., name.clone().into()))
                .collect()
        },
        move |cx, input, event| {
            if event != crate::ui::PromptEvent::Validate {
                return;
            }
            let upstream = match input.trim() {
                "" => "@{upstream}".to_string(),
                other => other.to_string(),
            };
            let workdir = workdir.clone();
            cx.jobs.callback(async move {
                Ok(crate::job::Callback::EditorCompositor(Box::new(
                    move |_: &mut Editor, compositor: &mut Compositor| {
                        compositor.push(Box::new(DiffView::cherries(&workdir, upstream, None)));
                    },
                )))
            });
        },
    )
}

/// Where a fragment is drawn, and how much room it has.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Cell {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) width: usize,
}

/// Draws one row, with the theme's diff colours and Tree-sitter highlighting.
pub(crate) struct RowRenderer<'a> {
    theme: &'a helix_view::Theme,
    loader: arc_swap::Guard<std::sync::Arc<helix_core::syntax::Loader>>,
}

impl<'a> RowRenderer<'a> {
    pub(crate) fn new(editor: &'a Editor) -> Self {
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
        // The row's line: the view's width, one high.
        area: Rect,
        margin: &str,
        surface: &mut Surface,
    ) {
        let y = area.y;
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
                // The margin on the right, when there is room for it.
                let margin_width = helix_core::unicode::width::UnicodeWidthStr::width(margin);
                let cell = if margin_width > 0 && margin_width * 3 < cell.width {
                    let text_width = cell.width - margin_width;
                    let style = self.theme.get("ui.virtual");
                    let style = if focused {
                        style.patch(cursor_style)
                    } else {
                        style
                    };
                    // Exactly as wide as it needs: no room kept for an ellipsis.
                    surface.set_string_truncated(
                        cell.x + text_width as u16,
                        y,
                        margin,
                        margin_width,
                        |_| style,
                        false,
                        false,
                    );
                    Cell {
                        width: text_width,
                        ..cell
                    }
                } else {
                    cell
                };
                self.put_parts(surface, cell, &parts, focused);
            }
            Row::File { .. } => {
                let Some(file) = view.file_at(row) else {
                    return;
                };
                let (added, removed) = file.stats();
                let mut text = format!(
                    "{} {} {}  +{added} -{removed}{}",
                    if view.options.stat {
                        ' '
                    } else {
                        fold_marker(file.folded)
                    },
                    file.status.code(),
                    file.path.display(),
                    if file.binary { "  (binary)" } else { "" }
                );
                if view.options.stat {
                    text.push_str("  ");
                    text.push_str(&stat_bar(added, removed, view.largest_change()));
                }
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

                // The words that changed, reversed over the line's colours.
                if let Row::Line {
                    section,
                    file: file_index,
                    hunk: hunk_index,
                    line: line_index,
                } = row
                {
                    if let Some(ranges) = view
                        .word_ranges
                        .get(&(section, file_index, hunk_index, line_index))
                    {
                        use helix_core::unicode::width::UnicodeWidthStr;
                        let start_x = x + 1;
                        let end_x = start_x + available.saturating_sub(1) as u16;
                        for range in ranges {
                            let (Some(before), Some(changed)) = (
                                line.content.get(..range.start),
                                line.content.get(range.clone()),
                            ) else {
                                continue;
                            };
                            let from = start_x.saturating_add(before.width() as u16);
                            let to = from.saturating_add(changed.width() as u16).min(end_x);
                            // Toggled rather than set: a theme whose popups are
                            // reversed already would hide a plain `REVERSED`.
                            for column in from..to {
                                let cell = &mut surface[(column, y)];
                                cell.modifier
                                    .toggle(helix_view::graphics::Modifier::REVERSED);
                                cell.modifier.insert(helix_view::graphics::Modifier::BOLD);
                            }
                        }
                    }
                }
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
    pub(crate) fn put_highlighted(
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

/// `++++---`, scaled so the file with the most changed lines fills
/// [`STAT_WIDTH`].
fn stat_bar(added: usize, removed: usize, largest: usize) -> String {
    const STAT_WIDTH: usize = 40;
    let scale = |count: usize| {
        if largest <= STAT_WIDTH {
            count
        } else {
            // At least one mark for any change at all.
            (count * STAT_WIDTH).div_ceil(largest)
        }
    };
    format!("{}{}", "+".repeat(scale(added)), "-".repeat(scale(removed)))
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
            commit: None,
            kind: ViewKind::Status,
            options: DiffOptions::default(),
            word_ranges: HashMap::new(),
            margin: STATUS_MARGIN.get(),
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
            ..Commit::default()
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
            worktrees: Vec::new(),
            submodules: Vec::new(),
            sparse: None,
        }
    }

    #[test]
    fn the_header_says_where_head_is_and_what_is_under_way() {
        let overview = Overview {
            sparse: Some(vec!["docs".into(), "src/core".into()]),
            ..overview()
        };
        let lines = header_lines(&overview);
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
                "Sparse: docs src/core  > to change",
                "State: Merging 0123456",
                " C to commit the merge; m then z to abort",
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

        // RET shows a stash or a commit; the menus act on commits only.
        view.cursor = 5;
        assert_eq!(view.revision_at_cursor().as_deref(), Some("stash@{0}"));
        assert_eq!(
            view.target_at_cursor(),
            Some(("stash@{0}".to_string(), AskKind::Stash))
        );
        view.cursor = 7;
        assert_eq!(view.revision_at_cursor().as_deref(), Some("def5678"));
        assert_eq!(
            view.target_at_cursor(),
            Some(("def5678".to_string(), AskKind::Revision))
        );
        view.cursor = 0;
        assert_eq!(view.revision_at_cursor(), None);
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
        assert!(view.sections[0].title.starts_with("Unmerged paths"));
        view.cursor = 1;
        assert_eq!(
            view.visit_target().unwrap(),
            (PathBuf::from("src/conflict.rs"), 1)
        );
        assert!(view.item_command(false).unwrap().is_err());
        assert_eq!(view.revision_at_cursor(), None);
        // `e` and `s` act on the path itself.
        assert_eq!(
            view.unmerged_at_cursor().as_deref(),
            Some("src/conflict.rs")
        );
        view.cursor = 0;
        assert_eq!(view.unmerged_at_cursor(), None);
    }

    #[test]
    fn word_marking_and_the_summary_follow_the_options() {
        let edit = parsed(
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n ctx\n-let x = 1;\n+let x = 2;\n",
        );
        let mut view = make_view(vec![edit], Vec::new());
        assert!(view.word_ranges.is_empty(), "off by default");

        view.options.word_diff = true;
        view.rebuild_rows();
        // Section 2 (unstaged), file 0, hunk 0: the `1` and the `2`.
        assert_eq!(
            view.word_ranges.get(&(2, 0, 0, 1)).map(Vec::as_slice),
            Some(std::slice::from_ref(&(8..9)))
        );
        assert_eq!(
            view.word_ranges.get(&(2, 0, 0, 2)).map(Vec::as_slice),
            Some(std::slice::from_ref(&(8..9)))
        );

        view.options.stat = true;
        view.rebuild_rows();
        assert!(
            !view.rows.iter().any(|row| matches!(row, Row::Hunk { .. })),
            "the summary shows no hunks"
        );
        assert_eq!(stat_bar(1, 1, 2), "+-");
        // Scaled down, but any change still gets a mark.
        assert_eq!(
            stat_bar(1, 99, 100),
            "+----------------------------------------"
        );
    }
}
