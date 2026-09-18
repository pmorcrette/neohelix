use std::path::{Path, PathBuf};

use uuid::Uuid;

/// A headline's TODO state.
///
/// Carries whether it means "done" because only the declaring file knows, and
/// the graph spans files.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TodoState {
    /// The keyword as written, e.g. `NEXT`.
    pub keyword: String,
    /// Whether the file puts this keyword after the `|`.
    pub done: bool,
}

/// An Org timestamp, reduced to the parts a query needs.
///
/// Field order makes the derived ordering chronological. Repeaters, warning
/// periods and ranges are not modelled here — a timestamp carrying one is
/// still read for its date, and the full model belongs to the agenda work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    /// `None` for a date without a time of day.
    pub hour: Option<u32>,
    pub minute: Option<u32>,
    /// `<…>` is active and appears in the agenda; `[…]` is not.
    pub active: bool,
}

impl Timestamp {
    /// The date alone, for comparing days rather than instants.
    pub fn date(&self) -> (i32, u32, u32) {
        (self.year, self.month, self.day)
    }
}

/// A single Org-Roam node.
///
/// In Org-Roam v2 a node is any headline — or the file-level preamble — that
/// carries an `:ID:` property. `title` is the headline text (or the
/// `#+title:` keyword for a file-level node), `tags` are the `:tag:` markers
/// attached to it, and `aliases` come from the `:ROAM_ALIASES:` property.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// The value of the node's `:ID:` property.
    pub id: Uuid,
    /// The headline text, or the `#+title:` keyword for a file-level node.
    pub title: String,
    /// Path of the Org file the node was parsed from.
    pub file_path: PathBuf,
    /// Tags attached to the node, without their surrounding colons.
    pub tags: Vec<String>,
    /// Alternative titles declared through `:ROAM_ALIASES:`.
    pub aliases: Vec<String>,
    /// Outline depth: 1 for a top-level headline, 0 for the file-level node.
    pub level: usize,
    /// The node's TODO state, when its headline carries one.
    pub todo: Option<TodoState>,
    /// The priority letter, when the headline carries a cookie the file
    /// declares.
    pub priority: Option<char>,
    /// `SCHEDULED:` from the planning line under the headline.
    pub scheduled: Option<Timestamp>,
    /// `DEADLINE:` from the same line.
    pub deadline: Option<Timestamp>,
    /// Titles of the headlines above this one, outermost first.
    ///
    /// Complete regardless of which ancestors are themselves nodes: a
    /// headline without an `:ID:` still names a level of the outline.
    pub outline_path: Vec<String>,
    /// Properties of the node's drawer, keys lowercased.
    ///
    /// Excludes the ones with fields of their own — `:ID:`,
    /// `:ROAM_ALIASES:` and `:ROAM_REFS:` — so nothing is stored twice.
    pub properties: Vec<(String, String)>,
    /// Zero-based line of the node's `:ID:` property within `file_path`.
    ///
    /// This is what the node picker jumps to, so it points at the `:ID:`
    /// itself rather than at the headline above it.
    pub line: usize,
}

impl Node {
    /// Creates a node with no tags and no aliases.
    pub fn new(id: Uuid, title: impl Into<String>, file_path: impl Into<PathBuf>) -> Self {
        Self {
            id,
            title: title.into(),
            file_path: file_path.into(),
            tags: Vec::new(),
            aliases: Vec::new(),
            level: 0,
            todo: None,
            priority: None,
            scheduled: None,
            deadline: None,
            outline_path: Vec::new(),
            properties: Vec::new(),
            line: 0,
        }
    }

    /// Builder-style setter for [`Node::tags`].
    pub fn with_tags<T: Into<String>>(mut self, tags: impl IntoIterator<Item = T>) -> Self {
        self.tags = tags.into_iter().map(Into::into).collect();
        self
    }

    /// Builder-style setter for [`Node::aliases`].
    pub fn with_aliases<T: Into<String>>(mut self, aliases: impl IntoIterator<Item = T>) -> Self {
        self.aliases = aliases.into_iter().map(Into::into).collect();
        self
    }

    /// Builder-style setter for [`Node::line`].
    pub fn with_line(mut self, line: usize) -> Self {
        self.line = line;
        self
    }

    /// The path of the Org file this node was parsed from.
    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    /// Whether `title` matches the node's title or any of its aliases.
    ///
    /// Org-Roam resolves `[[roam:...]]` descriptions against both, so lookups
    /// by name have to consider aliases as well.
    pub fn matches_title(&self, title: &str) -> bool {
        self.title == title || self.aliases.iter().any(|alias| alias == title)
    }
}
