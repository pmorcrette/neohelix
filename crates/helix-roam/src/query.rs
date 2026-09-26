//! Filtering the graph by what a node *is*, not only by what it links to.
//!
//! Before the node carried a state, a priority and its dates, the graph could
//! answer what points at what and nothing else. These are the questions those
//! fields exist for.

use crate::node::{Node, Timestamp};

/// A conjunction of filters: a node matches when every set field matches.
///
/// Every field left unset is a filter not applied, so the default query
/// matches everything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeQuery {
    /// Node carries this tag.
    pub tag: Option<String>,
    /// Node's TODO keyword is exactly this.
    pub todo: Option<String>,
    /// Node has a TODO state, and its doneness is this.
    ///
    /// `Some(false)` is how "unfinished" is asked for, and it excludes a node
    /// with no state at all — a plain heading is not an unfinished task.
    pub done: Option<bool>,
    /// Node has any TODO state at all.
    pub has_todo: Option<bool>,
    /// Node's priority is exactly this letter.
    pub priority: Option<char>,
    /// Node is scheduled on or before this day.
    pub scheduled_by: Option<Timestamp>,
    /// Node is due on or before this day.
    pub due_by: Option<Timestamp>,
    /// Node's title or one of its aliases contains this, ignoring case.
    pub title_contains: Option<String>,
    /// Node's drawer carries this property with this value.
    pub property: Option<(String, String)>,
}

impl NodeQuery {
    /// Whether `node` satisfies every filter this query sets.
    pub fn matches(&self, node: &Node) -> bool {
        self.tag_matches(node)
            && self.todo_matches(node)
            && self.priority.is_none_or(|p| node.priority == Some(p))
            && self
                .scheduled_by
                .is_none_or(|by| node.scheduled.is_some_and(|at| at.date() <= by.date()))
            && self
                .due_by
                .is_none_or(|by| node.deadline.is_some_and(|at| at.date() <= by.date()))
            && self.title_matches(node)
            && self.property_matches(node)
    }

    fn tag_matches(&self, node: &Node) -> bool {
        self.tag
            .as_ref()
            .is_none_or(|wanted| node.tags.iter().any(|tag| tag == wanted))
    }

    fn todo_matches(&self, node: &Node) -> bool {
        if let Some(wanted) = &self.todo {
            if node.todo.as_ref().map(|s| &s.keyword) != Some(wanted) {
                return false;
            }
        }
        if let Some(wanted) = self.done {
            // A node without a state is neither done nor unfinished.
            if node.todo.as_ref().map(|s| s.done) != Some(wanted) {
                return false;
            }
        }
        if let Some(wanted) = self.has_todo {
            if node.todo.is_some() != wanted {
                return false;
            }
        }
        true
    }

    fn title_matches(&self, node: &Node) -> bool {
        let Some(needle) = &self.title_contains else {
            return true;
        };
        let needle = needle.to_lowercase();

        node.title.to_lowercase().contains(&needle)
            || node
                .aliases
                .iter()
                .any(|alias| alias.to_lowercase().contains(&needle))
    }

    fn property_matches(&self, node: &Node) -> bool {
        let Some((key, value)) = &self.property else {
            return true;
        };
        let key = key.to_lowercase();

        // Inherited values count: a query that ignored a file-wide
        // `#+PROPERTY:` would miss what the file plainly says.
        node.property(&key)
            .is_some_and(|found| found.trim() == value.trim())
    }

    /// Unfinished nodes, the query a task list is built from.
    pub fn unfinished() -> Self {
        Self {
            done: Some(false),
            ..Self::default()
        }
    }

    /// Builder-style setter for [`NodeQuery::tag`].
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tag = Some(tag.into());
        self
    }

    /// Builder-style setter for [`NodeQuery::due_by`].
    pub fn due_by(mut self, when: Timestamp) -> Self {
        self.due_by = Some(when);
        self
    }
}
