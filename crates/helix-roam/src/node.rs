use std::path::{Path, PathBuf};

use uuid::Uuid;

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
