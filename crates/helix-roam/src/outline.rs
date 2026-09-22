//! Reading a buffer as an outline: its headings, where the cursor sits among
//! them, and which parts of it to hide to see only what matters.
//!
//! Everything here works on the text rather than on the index, because the
//! buffer is usually ahead of the index and these answer questions about what
//! is on screen now. Ranges come back as character ranges rather than folds:
//! the editor owns what a fold is, and this crate does not depend on it.

use crate::agenda::TodoFilter;
use crate::parser::{parse_headline, FileSettings};

/// A headline, as a buffer shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The line the headline is on.
    pub line: usize,
    /// Its depth, counted in stars.
    pub level: usize,
    pub title: String,
    pub todo: Option<crate::TodoState>,
    pub priority: Option<char>,
    pub tags: Vec<String>,
}

/// Every headline in the buffer, in the order it appears.
pub fn headings(text: &str) -> Vec<Entry> {
    let settings = FileSettings::scan(text);

    text.lines()
        .enumerate()
        .filter_map(|(line, raw)| {
            let headline = parse_headline(raw, &settings)?;
            Some(Entry {
                line,
                level: headline.level,
                title: headline.title,
                todo: headline.todo,
                priority: headline.priority,
                tags: headline.tags,
            })
        })
        .collect()
}

/// The index of the entry `line` belongs to: the nearest headline at or above.
pub fn at(entries: &[Entry], line: usize) -> Option<usize> {
    entries.iter().rposition(|entry| entry.line <= line)
}

/// The titles from the top of the file down to the entry containing `line`.
///
/// What Org shows when a headline has scrolled off: not the entry's own title
/// alone but the path that gives it its meaning.
pub fn outline_path(entries: &[Entry], line: usize) -> Vec<String> {
    let Some(mut at) = at(entries, line) else {
        return Vec::new();
    };

    let mut path = vec![entries[at].title.clone()];
    let mut level = entries[at].level;

    while level > 1 {
        match entries[..at].iter().rposition(|entry| entry.level < level) {
            Some(parent) => {
                at = parent;
                level = entries[at].level;
                path.push(entries[at].title.clone());
            }
            None => break,
        }
    }

    path.reverse();
    path
}

/// The line of the next headline after `line`.
pub fn next(entries: &[Entry], line: usize) -> Option<usize> {
    entries
        .iter()
        .find(|entry| entry.line > line)
        .map(|entry| entry.line)
}

/// The line of the previous headline before `line`.
pub fn previous(entries: &[Entry], line: usize) -> Option<usize> {
    entries
        .iter()
        .rfind(|entry| entry.line < line)
        .map(|entry| entry.line)
}

/// The line of the next headline at the same level, without leaving the parent.
pub fn next_sibling(entries: &[Entry], line: usize) -> Option<usize> {
    let at = at(entries, line)?;
    let level = entries[at].level;

    entries[at + 1..]
        .iter()
        // A shallower headline ends the parent, so the search stops there
        // rather than finding a cousin further down the file.
        .take_while(|entry| entry.level >= level)
        .find(|entry| entry.level == level)
        .map(|entry| entry.line)
}

/// The line of the previous headline at the same level, within the parent.
pub fn previous_sibling(entries: &[Entry], line: usize) -> Option<usize> {
    let at = at(entries, line)?;
    let level = entries[at].level;

    entries[..at]
        .iter()
        .rev()
        .take_while(|entry| entry.level >= level)
        .find(|entry| entry.level == level)
        .map(|entry| entry.line)
}

/// The line of the headline one level out.
pub fn parent(entries: &[Entry], line: usize) -> Option<usize> {
    let at = at(entries, line)?;
    let level = entries[at].level;

    entries[..at]
        .iter()
        .rposition(|entry| entry.level < level)
        .map(|parent| entries[parent].line)
}

/// Which entries a sparse tree keeps.
///
/// The sigils are the agenda's, so one convention covers both: `:work:` is a
/// tag, `#A` a priority, a bare word a TODO keyword. `/text` is this filter's
/// own addition, and matches the headline's text.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub todo: TodoFilter,
    /// Lowercased, because a headline search that cared about case would miss
    /// what the writer meant more often than it would help.
    pub text: Option<String>,
}

impl Filter {
    pub fn parse(input: &str) -> Self {
        let mut text = None;
        let rest: Vec<&str> = input
            .split_whitespace()
            .filter(|token| match token.strip_prefix('/') {
                Some(needle) if !needle.is_empty() => {
                    text = Some(needle.to_lowercase());
                    false
                }
                _ => true,
            })
            .collect();

        Filter {
            todo: TodoFilter::parse(&rest.join(" ")),
            text,
        }
    }

    /// Whether every part of the filter that was given is satisfied.
    pub fn matches(&self, entry: &Entry) -> bool {
        let keyword = self.todo.keyword.as_ref().is_none_or(|wanted| {
            entry
                .todo
                .as_ref()
                .is_some_and(|state| state.keyword == *wanted)
        });
        let tag = self
            .todo
            .tag
            .as_ref()
            .is_none_or(|wanted| entry.tags.iter().any(|tag| tag == wanted));
        let priority = self
            .todo
            .priority
            .is_none_or(|wanted| entry.priority == Some(wanted));
        let text = self
            .text
            .as_ref()
            .is_none_or(|needle| entry.title.to_lowercase().contains(needle));

        keyword && tag && priority && text
    }

    pub fn is_empty(&self) -> bool {
        self.todo.keyword.is_none()
            && self.todo.tag.is_none()
            && self.todo.priority.is_none()
            && self.text.is_none()
    }
}

/// Character offset of the start of each line, with the text's length last.
fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    let mut chars = 0;

    for c in text.chars() {
        chars += 1;
        if c == '\n' {
            starts.push(chars);
        }
    }
    if *starts.last().unwrap() != chars {
        starts.push(chars);
    }

    starts
}

/// Turns the lines to hide into the character ranges that hide them.
///
/// A run that follows a visible line starts at that line's newline, so it
/// collapses onto it the way a folded subtree collapses onto its headline. A
/// run at the very top has no line to collapse onto and gets one of its own.
fn ranges_for(hidden: &[bool], starts: &[usize]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut at = 0;

    while at < hidden.len() {
        if !hidden[at] {
            at += 1;
            continue;
        }

        let from = at;
        while at < hidden.len() && hidden[at] {
            at += 1;
        }

        let start = if from == 0 { 0 } else { starts[from] - 1 };
        // Keep the newline that ends the run, or the next visible line joins
        // the marker instead of starting below it.
        let end = starts[at].saturating_sub(1).max(start);
        if start < end {
            ranges.push((start, end));
        }
    }

    ranges
}

/// The ranges to hide so that only matching headlines and their ancestors show.
///
/// A line survives only if it is the headline of an entry that matches or of
/// one on the way down to a match. Everything else — bodies, drawers, entries
/// nobody asked for — goes, which is what makes a sparse tree a tree.
pub fn sparse_tree(text: &str, filter: &Filter) -> Vec<(usize, usize)> {
    let entries = headings(text);
    let starts = line_starts(text);
    let lines = starts.len().saturating_sub(1);

    let mut keep = vec![false; entries.len()];
    for (index, entry) in entries.iter().enumerate() {
        if !filter.matches(entry) {
            continue;
        }
        keep[index] = true;

        // Light up the path down to it, or a match nested three levels deep
        // would appear with nothing to say where it is.
        let mut level = entry.level;
        for (above, ancestor) in entries[..index].iter().enumerate().rev() {
            if ancestor.level < level {
                keep[above] = true;
                level = ancestor.level;
            }
        }
    }

    let mut hidden = vec![true; lines];
    for (index, entry) in entries.iter().enumerate() {
        if keep[index] && entry.line < lines {
            hidden[entry.line] = false;
        }
    }

    ranges_for(&hidden, &starts)
}

/// The ranges to hide so that only the subtree at `line` shows.
///
/// Two ranges rather than one truncation: what is hidden is still in the
/// buffer, still saved, still edited by a command that reaches it. This is
/// narrowing as folding, which is not quite Org's — there, narrowing makes
/// the rest of the file unreachable.
pub fn narrow(text: &str, line: usize) -> Vec<(usize, usize)> {
    let entries = headings(text);
    let starts = line_starts(text);
    let lines = starts.len().saturating_sub(1);

    let Some(at) = at(&entries, line) else {
        return Vec::new();
    };
    let level = entries[at].level;
    let from = entries[at].line;
    let to = entries[at + 1..]
        .iter()
        .find(|entry| entry.level <= level)
        .map_or(lines, |entry| entry.line);

    let mut hidden = vec![true; lines];
    for line in hidden.iter_mut().take(to).skip(from) {
        *line = false;
    }

    ranges_for(&hidden, &starts)
}
