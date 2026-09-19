//! What is due, and when.
//!
//! The agenda is a view over the graph rather than a store of its own: it
//! takes the nodes the indexer already holds and answers which of them land on
//! which day. Keeping it a pure function of nodes and a date range is what
//! lets the answer be tested without an editor.

use crate::node::Node;
use crate::Date;

/// Why a node appears on a day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reason {
    /// Its `DEADLINE:` falls on or before the day.
    Deadline,
    /// Its `SCHEDULED:` falls on the day.
    Scheduled,
}

/// One line of an agenda.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry<'a> {
    pub day: Date,
    pub reason: Reason,
    pub node: &'a Node,
    /// Days until the deadline, negative once it is past.
    ///
    /// `None` for a scheduled entry, which is not late, only due.
    pub days_left: Option<i64>,
}

/// How far ahead a deadline starts showing up.
///
/// Org's default warning period is 14 days, and a deadline nobody sees until
/// the morning it is due is not a deadline.
pub const DEADLINE_WARNING_DAYS: i64 = 14;

/// The agenda for `days` days starting at `from`.
///
/// Entries are ordered by day, then deadlines before scheduled items, then by
/// title — so the same set of notes always produces the same agenda.
pub fn agenda<'a>(
    nodes: impl IntoIterator<Item = &'a Node>,
    from: Date,
    days: i64,
) -> Vec<Entry<'a>> {
    let to = from.offset_by(days.max(1) - 1);
    let mut entries = Vec::new();

    for node in nodes {
        // A finished task is not pending, whatever its dates say.
        if node.todo.as_ref().is_some_and(|state| state.done) {
            continue;
        }

        if let Some(scheduled) = node.scheduled.filter(|stamp| stamp.active) {
            for day in scheduled.occurrences(from, to) {
                entries.push(Entry {
                    day,
                    reason: Reason::Scheduled,
                    node,
                    days_left: None,
                });
            }
        }

        if let Some(deadline) = node.deadline.filter(|stamp| stamp.active) {
            // A deadline announces itself before it arrives, so the window
            // reaches past the view's last day by the warning period.
            let horizon = to.offset_by(DEADLINE_WARNING_DAYS);
            let upcoming = deadline.occurrences(from, horizon);

            // Nothing upcoming and a date already gone: it is overdue, and an
            // overdue deadline is carried onto today rather than vanishing.
            if upcoming.is_empty() && deadline.repeater.is_none() && deadline.day() < from {
                entries.push(Entry {
                    day: from,
                    reason: Reason::Deadline,
                    node,
                    days_left: Some(deadline.day().to_days() - from.to_days()),
                });
            }

            for due in upcoming {
                // Inside the view it sits on its own day; beyond it, the
                // warning shows on the first day instead.
                let day = if due <= to { due } else { from };
                entries.push(Entry {
                    day,
                    reason: Reason::Deadline,
                    node,
                    days_left: Some(due.to_days() - from.to_days()),
                });
            }
        }
    }

    entries.sort_by(|a, b| {
        a.day
            .cmp(&b.day)
            .then(a.reason.cmp(&b.reason))
            .then(a.node.title.cmp(&b.node.title))
    });
    entries.dedup_by(|a, b| a.day == b.day && a.reason == b.reason && a.node.id == b.node.id);

    entries
}

/// Every unfinished node with a TODO state, whatever its dates.
///
/// This is the other half of an agenda: the things that have to happen but
/// were never given a day.
pub fn todo_list<'a>(nodes: impl IntoIterator<Item = &'a Node>) -> Vec<&'a Node> {
    let mut found: Vec<&Node> = nodes
        .into_iter()
        .filter(|node| node.todo.as_ref().is_some_and(|state| !state.done))
        .collect();

    // Priority first, then title, so the list is stable and reads usefully.
    found.sort_by(|a, b| {
        a.priority
            .unwrap_or('Z')
            .cmp(&b.priority.unwrap_or('Z'))
            .then(a.title.cmp(&b.title))
    });
    found
}
