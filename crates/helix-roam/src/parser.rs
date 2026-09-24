//! A line-oriented parser for the subset of Org syntax Org-Roam cares about.
//!
//! This deliberately does not go through Tree-sitter: indexing runs over every
//! file in a notes directory, and the properties Org-Roam needs — `:ID:`,
//! `#+title:`, `#+filetags:`, aliases, refs and `[[id:…]]` links — all sit in
//! syntax that is unambiguous line by line.

use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::node::{Repeater, RepeaterKind, RepeaterUnit, Timestamp, TodoState};
use crate::Node;

/// Namespace for Org-Roam ids that are not themselves UUIDs.
///
/// `org-id-method` can be set to `ts`, which produces ids like
/// `20230101T120000.000000`. Hashing those into a fixed namespace keeps
/// [`Node::id`] a `Uuid` while resolving links correctly, because a node's
/// `:ID:` and the `[[id:…]]` links pointing at it hash to the same value.
const ORG_ID_NAMESPACE: Uuid = Uuid::from_bytes([
    0x6b, 0xa7, 0xb8, 0x11, 0x9d, 0xad, 0x11, 0xd1, 0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8,
]);

/// What a `[[…]]` link points at, before the graph has resolved it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// An `[[id:<id>]]` link.
    Id(Uuid),
    /// Any other link target, which resolves only if some node claims it
    /// through `:ROAM_REFS:`.
    Ref(String),
}

/// A link found in a file, with the node whose section contains it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLink {
    pub source: Uuid,
    pub target: LinkTarget,
}

/// Everything one Org file contributes to the graph.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedFile {
    pub nodes: Vec<Node>,
    pub links: Vec<ParsedLink>,
    /// `:ROAM_REFS:` entries, each mapped to the node that claims it.
    pub refs: Vec<(String, Uuid)>,
    /// `[cite:@key]` occurrences, each mapped to the node citing it.
    pub citations: Vec<(String, Uuid)>,
    /// What the file declared about how it should be read.
    pub settings: FileSettings,
}

/// What a file declares about how it should be read.
///
/// Org lets a file redefine its own TODO keywords, priority range and tag set.
/// Reading a file without them does not lose a feature — it produces a wrong
/// result: a headline's keyword ends up inside its title, and every consumer
/// of that title inherits the mistake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSettings {
    /// TODO keywords meaning "not done", in the order declared.
    pub todo_keywords: Vec<String>,
    /// TODO keywords meaning "done".
    pub done_keywords: Vec<String>,
    /// Priority letters the file recognises, highest first.
    pub priorities: Vec<char>,
    /// `#+CATEGORY:`, which the agenda groups by.
    pub category: Option<String>,
    /// `#+ARCHIVE:`, where archiving sends a subtree.
    pub archive: Option<String>,
    /// Bibliography files the file declares, for completing a citation over.
    pub bibliography: Vec<String>,
    /// Tags offered by `#+TAGS:`, for completion rather than for parsing.
    pub declared_tags: Vec<String>,
    /// Drawer names declared through `#+DRAWERS:`.
    pub drawers: Vec<String>,
    /// `#+STARTUP:` options, in the order given.
    pub startup: Vec<String>,
    /// What entering or leaving a keyword records, from `TODO(t!)` and
    /// `DONE(d@/!)`. Only keywords that ask for something appear.
    pub todo_logging: Vec<(String, crate::logging::KeywordLog)>,
    /// `#+PROPERTY:` defaults, which every entry in the file inherits.
    pub properties: Vec<(String, String)>,
    /// `#+LINK:` abbreviations, as `(name, expansion)`.
    ///
    /// Per file, so `[[gh:owner/repo]]` can mean different things in two
    /// files — which is why following a link has to consult these rather than
    /// a global table.
    pub link_abbreviations: Vec<(String, String)>,
}

impl Default for FileSettings {
    /// Org's own defaults, used by a file that declares nothing.
    fn default() -> Self {
        Self {
            todo_keywords: vec!["TODO".to_string()],
            done_keywords: vec!["DONE".to_string()],
            priorities: vec!['A', 'B', 'C'],
            category: None,
            archive: None,
            bibliography: Vec::new(),
            declared_tags: Vec::new(),
            drawers: Vec::new(),
            startup: Vec::new(),
            todo_logging: Vec::new(),
            properties: Vec::new(),
            link_abbreviations: Vec::new(),
        }
    }
}

impl FileSettings {
    /// Whether `word` is a TODO keyword in this file, done or not.
    pub fn is_todo_keyword(&self, word: &str) -> bool {
        self.todo_state(word).is_some()
    }

    /// The state `word` names in this file, if it names one.
    pub fn todo_state(&self, word: &str) -> Option<TodoState> {
        let done = if self.todo_keywords.iter().any(|kw| kw == word) {
            false
        } else if self.done_keywords.iter().any(|kw| kw == word) {
            true
        } else {
            return None;
        };

        Some(TodoState {
            keyword: word.to_string(),
            done,
        })
    }

    /// What entering or leaving `keyword` records.
    pub fn keyword_log(&self, keyword: &str) -> crate::logging::KeywordLog {
        self.todo_logging
            .iter()
            .find(|(name, _)| name == keyword)
            .map(|(_, log)| *log)
            .unwrap_or_default()
    }

    /// Reads every setting a file declares.
    ///
    /// A separate pass, because these keywords are not required to precede the
    /// headlines they govern: a sequential reader would apply `#+TODO:` only
    /// to what follows it. The scan is a `#+` test per line, which costs far
    /// less than the link scanning the main pass already does.
    pub fn scan(text: &str) -> Self {
        let mut settings = Self::default();
        let mut declared_todo = false;

        for line in text.lines() {
            let trimmed = line.trim_start();
            if !trimmed.starts_with("#+") {
                continue;
            }
            let Some((keyword, value)) = parse_keyword(trimmed) else {
                continue;
            };

            match keyword.as_str() {
                // All three spellings declare a sequence; Org distinguishes
                // them only by how the keywords are meant to be cycled.
                "todo" | "seq_todo" | "typ_todo" => {
                    let (active, done) = parse_todo_sequence(value);
                    if !declared_todo {
                        // The first declaration replaces Org's defaults; later
                        // ones add sequences rather than replacing them.
                        settings.todo_keywords.clear();
                        settings.done_keywords.clear();
                        declared_todo = true;
                    }
                    settings.todo_logging.extend(parse_todo_logging(value));
                    settings.todo_keywords.extend(active);
                    settings.done_keywords.extend(done);
                }
                "priorities" => {
                    if let Some(range) = parse_priorities(value) {
                        settings.priorities = range;
                    }
                }
                "category" => settings.category = Some(value.to_string()),
                "archive" => settings.archive = Some(value.to_string()),
                // Org allows several, one keyword each.
                "bibliography" => settings.bibliography.push(value.to_string()),
                "tags" => settings.declared_tags.extend(parse_tag_declaration(value)),
                "drawers" => settings
                    .drawers
                    .extend(value.split_whitespace().map(str::to_string)),
                "startup" => settings
                    .startup
                    .extend(value.split_whitespace().map(str::to_string)),
                // `#+PROPERTY: Effort_ALL 0 1:00 2:00`
                "property" => {
                    if let Some((key, value)) = value.split_once(char::is_whitespace) {
                        settings
                            .properties
                            .push((key.trim().to_lowercase(), value.trim().to_string()));
                    }
                }
                // `#+LINK: gh https://github.com/%s`
                "link" => {
                    if let Some((name, expansion)) = value.split_once(char::is_whitespace) {
                        settings
                            .link_abbreviations
                            .push((name.trim().to_string(), expansion.trim().to_string()));
                    }
                }
                _ => {}
            }
        }

        settings
    }
}

/// Splits `TODO NEXT | DONE` into its not-done and done keywords.
///
/// Without a bar, Org treats the last keyword as the done state, so a bare
/// `#+TODO: TODO DONE` still means what it looks like.
fn parse_todo_sequence(value: &str) -> (Vec<String>, Vec<String>) {
    let (active, done) = match value.split_once('|') {
        Some((active, done)) => (strip_fast_keys(active), strip_fast_keys(done)),
        None => {
            let mut all = strip_fast_keys(value);
            let last = all.pop();
            (all, last.into_iter().collect())
        }
    };

    (active, done)
}

/// The logging a sequence asks for: `WAIT(w@/!)` takes a note on entering
/// `WAIT` and records the time on leaving it.
fn parse_todo_logging(value: &str) -> Vec<(String, crate::logging::KeywordLog)> {
    value
        .split_whitespace()
        .filter_map(|word| {
            let (name, rest) = word.split_once('(')?;
            let log = crate::logging::KeywordLog::parse(rest.strip_suffix(')')?);
            (!name.is_empty() && log != Default::default()).then(|| (name.to_string(), log))
        })
        .collect()
}

/// Drops the `(t)` fast-access keys Org allows after a keyword or a tag.
fn strip_fast_keys(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .filter(|word| !matches!(*word, "{" | "}"))
        .map(|word| word.split_once('(').map_or(word, |(name, _)| name))
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// `#+TAGS: @work(w) { laptop(l) pc(p) }` — the names, without keys or groups.
fn parse_tag_declaration(value: &str) -> Vec<String> {
    strip_fast_keys(value)
        .into_iter()
        .filter(|tag| tag != ":")
        .collect()
}

/// `#+PRIORITIES: A C B` — highest, lowest, and the default in between.
fn parse_priorities(value: &str) -> Option<Vec<char>> {
    let mut letters = value.split_whitespace();
    let highest = letters.next()?.chars().next()?;
    let lowest = letters.next()?.chars().next()?;

    (highest <= lowest).then(|| (highest..=lowest).collect())
}

/// Every `:ID:` the text declares, whatever it declares them on.
///
/// Cheaper and blunter than parsing the file into nodes: this answers "which
/// ids live here", which is all an id-location cache needs to know.
pub fn ids_in(text: &str) -> Vec<Uuid> {
    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let rest = starts_with_ignore_case(trimmed, ":ID:").then(|| &trimmed[4..])?;
            rest.trim().parse().ok()
        })
        .collect()
}

/// Reads `<2026-09-18 Fri 10:30>` or its inactive `[…]` form.
///
/// Anything after the date — a day name, a repeater, a warning period — is
/// ignored rather than rejected, so a timestamp the agenda work will model
/// fully is still usable for its date now.
pub fn parse_timestamp(text: &str) -> Option<Timestamp> {
    let (open, close, active) = if text.starts_with('<') {
        ('<', '>', true)
    } else if text.starts_with('[') {
        ('[', ']', false)
    } else {
        return None;
    };

    let inner = text.strip_prefix(open)?.split(close).next()?;
    let mut parts = inner.split_whitespace();

    let mut date = parts.next()?.split('-');
    let year = date.next()?.parse().ok()?;
    let month = date.next()?.parse().ok()?;
    let day = date.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    // The day name is optional, so a time may be in either remaining slot.
    let (hour, minute) = parts
        .find_map(|part| {
            let (hour, minute) = part.split_once(':')?;
            Some((hour.parse().ok()?, minute.parse().ok()?))
        })
        .map_or((None, None), |(h, m)| (Some(h), Some(m)));

    // A repeater may sit anywhere after the date, so the whole tail is
    // searched rather than a fixed slot.
    let repeater = inner.split_whitespace().find_map(parse_repeater);

    // `<a>--<b>` is one timestamp spanning days, not two timestamps.
    let range_end = text
        .split_once(close)
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.strip_prefix("--"))
        .and_then(parse_timestamp)
        .map(|end| end.day());

    Some(Timestamp {
        year,
        month,
        day,
        hour,
        minute,
        active,
        repeater,
        range_end,
    })
}

/// Reads `+1w`, `++2m` or `.+3d`.
pub fn parse_repeater(token: &str) -> Option<Repeater> {
    let (kind, rest) = if let Some(rest) = token.strip_prefix("++") {
        (RepeaterKind::CatchUp, rest)
    } else if let Some(rest) = token.strip_prefix(".+") {
        (RepeaterKind::Restart, rest)
    } else if let Some(rest) = token.strip_prefix('+') {
        (RepeaterKind::Cumulate, rest)
    } else {
        // `-2d` is a warning period, not a repeater, and is not one here.
        return None;
    };

    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let count: i64 = digits.parse().ok()?;
    let unit = match rest[digits.len()..].chars().next()? {
        'h' => RepeaterUnit::Hour,
        'd' => RepeaterUnit::Day,
        'w' => RepeaterUnit::Week,
        'm' => RepeaterUnit::Month,
        'y' => RepeaterUnit::Year,
        _ => return None,
    };

    Some(Repeater { kind, count, unit })
}

/// Reads `SCHEDULED:` and `DEADLINE:` from the planning line under a headline.
pub(crate) fn parse_planning(trimmed: &str) -> Option<(Option<Timestamp>, Option<Timestamp>)> {
    const LABELS: [&str; 3] = ["SCHEDULED:", "DEADLINE:", "CLOSED:"];

    // Reject without allocating: this runs on every line of every file, and
    // almost none of them are planning lines.
    if !LABELS
        .iter()
        .any(|label| starts_with_ignore_case(trimmed, label))
    {
        return None;
    }

    let read = |label: &str| {
        let at = find_ignore_case(trimmed, label)?;
        parse_timestamp(trimmed[at + label.len()..].trim_start())
    };

    Some((read("SCHEDULED:"), read("DEADLINE:")))
}

/// `haystack` begins with `needle`, comparing ASCII case-insensitively.
pub(crate) fn starts_with_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack.len() >= needle.len() && haystack[..needle.len()].eq_ignore_ascii_case(needle)
}

/// Byte offset of `needle` in `haystack`, comparing ASCII case-insensitively.
pub(crate) fn find_ignore_case(haystack: &str, needle: &str) -> Option<usize> {
    (0..=haystack.len().checked_sub(needle.len())?).find(|&at| {
        haystack.is_char_boundary(at) && starts_with_ignore_case(&haystack[at..], needle)
    })
}

/// Every `@key` inside a `[cite…:…]` bracket on a line.
///
/// Org's citation syntax allows styles (`[cite/t:…]`) and several keys
/// separated by semicolons; both are handled by looking only for the keys.
fn citation_keys(line: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;

    while let Some(open) = find_from(bytes, i, b"[cite") {
        let Some(close) = find_from(bytes, open, b"]") else {
            break;
        };
        for token in line[open..close].split('@').skip(1) {
            let key: String = token
                .chars()
                .take_while(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
                .collect();
            if !key.is_empty() {
                keys.push(key);
            }
        }
        i = close + 1;
    }

    keys
}

/// Turns an Org-Roam id into a [`Uuid`], hashing ids that are not UUIDs.
pub fn parse_id(raw: &str) -> Uuid {
    Uuid::parse_str(raw).unwrap_or_else(|_| Uuid::new_v5(&ORG_ID_NAMESPACE, raw.as_bytes()))
}

/// Parses `text` as the Org file stored at `path`.
///
/// Everything a node carries is read in this one pass, eagerly. That was a
/// decision rather than an assumption, and it was measured: on 800 files and
/// 113,600 lines shaped like real notes — roughly a tenth of lines carrying a
/// link, a fiftieth a citation — reading the states, priorities, planning
/// lines, outline paths, properties and citations took the full scan from
/// 19 ms to 27 ms. That is 42% more for about 34 µs per file, which is
/// invisible against the save that triggers a reindex, and still under half a
/// second for a directory ten times this size.
///
/// A first attempt cost three times the original instead, because detecting a
/// planning line uppercased every line of every file to test three prefixes.
/// Rejecting without allocating is what makes the figure above affordable; a
/// change here that allocates per line will not be.
pub fn parse_org(text: &str, path: impl Into<PathBuf>) -> ParsedFile {
    Parser::new(path.into()).run(text)
}

/// The node a link belongs to: the innermost headline with an `:ID:`, falling
/// back to the file-level node.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Scope {
    id: Uuid,
    /// This node's properties plus the ones it inherited, which is what its
    /// own descendants inherit in turn.
    effective: Vec<(String, String)>,
    /// Headline depth that opened this scope; 0 for the file-level node.
    level: usize,
}

struct Parser {
    path: PathBuf,
    file: ParsedFile,
    /// Node being built from the drawer/keywords currently being read.
    pending: Option<PendingNode>,
    /// Ancestor nodes of the current line, innermost last.
    ///
    /// A headline without an `:ID:` is not a node, so its links belong to the
    /// nearest ancestor that is one — ultimately the file-level node, which
    /// sits at the bottom of the stack and is never popped.
    scopes: Vec<Scope>,
    /// Keywords apply to the file-level node, which may be discovered after
    /// them, so they are collected separately.
    file_title: Option<String>,
    file_tags: Vec<String>,
    file_aliases: Vec<String>,
    file_node: Option<usize>,
    /// What the file says about how to read it, scanned before the main pass.
    settings: FileSettings,
    /// `(level, title)` of every enclosing headline, for the outline path.
    headline_path: Vec<(usize, String)>,
}

/// A headline (or the file preamble) whose property drawer is being read.
struct PendingNode {
    level: usize,
    title: String,
    tags: Vec<String>,
    in_drawer: bool,
    id: Option<(Uuid, usize)>,
    aliases: Vec<String>,
    refs: Vec<String>,
    todo: Option<TodoState>,
    priority: Option<char>,
    scheduled: Option<Timestamp>,
    deadline: Option<Timestamp>,
    /// Properties other than the ones with fields of their own.
    properties: Vec<(String, String)>,
}

impl Parser {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            file: ParsedFile::default(),
            pending: None,
            scopes: Vec::new(),
            file_title: None,
            file_tags: Vec::new(),
            file_aliases: Vec::new(),
            file_node: None,
            settings: FileSettings::default(),
            headline_path: Vec::new(),
        }
    }

    fn run(mut self, text: &str) -> ParsedFile {
        // Read before anything else: these govern headlines that may precede
        // the declaration itself.
        self.settings = FileSettings::scan(text);

        // The preamble behaves like a level-0 headline: it can carry a
        // property drawer, and its node owns every link before the first
        // headline.
        self.pending = Some(PendingNode::preamble());

        for (line_number, line) in text.lines().enumerate() {
            let trimmed = line.trim();

            if let Some(pending) = &mut self.pending {
                if pending.in_drawer {
                    if trimmed.eq_ignore_ascii_case(":END:") {
                        pending.in_drawer = false;
                        // The node exists from here on, so links in its own
                        // section belong to it rather than to its parent.
                        self.finish_pending();
                    } else {
                        pending.read_property(trimmed, line_number);
                    }
                    continue;
                }
            }

            if let Some(headline) = parse_headline(line, &self.settings) {
                // A headline without an `:ID:` never became a node.
                self.pending = None;
                self.close_scopes(headline.level);

                // Every headline names a level of the outline, whether or not
                // it is a node itself.
                self.headline_path
                    .retain(|(level, _)| *level < headline.level);
                self.headline_path
                    .push((headline.level, headline.title.clone()));

                self.pending = Some(PendingNode::headline(headline));
                continue;
            }

            // A planning line sits between the headline and its drawer.
            if let Some((scheduled, deadline)) = parse_planning(trimmed) {
                if let Some(pending) = &mut self.pending {
                    pending.scheduled = pending.scheduled.or(scheduled);
                    pending.deadline = pending.deadline.or(deadline);
                }
                continue;
            }

            if trimmed.eq_ignore_ascii_case(":PROPERTIES:") {
                if let Some(pending) = &mut self.pending {
                    pending.in_drawer = true;
                }
                continue;
            }

            if let Some((keyword, value)) = parse_keyword(trimmed) {
                self.read_keyword(&keyword, value);
            }

            self.collect_links(line);
        }

        // Tolerate a property drawer that was never closed with `:END:`.
        self.finish_pending();
        self.apply_file_keywords();

        self.file.settings = self.settings;
        self.file
    }

    /// Emits the node being built, if its drawer supplied an `:ID:`.
    fn finish_pending(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let Some((id, line)) = pending.id else {
            return;
        };

        self.close_scopes(pending.level);

        if pending.level == 0 {
            self.file_node = Some(self.file.nodes.len());
        }

        // What this node inherits: the enclosing node's effective properties,
        // falling back to the file's `#+PROPERTY:` defaults at the top.
        let from_ancestors = self
            .scopes
            .last()
            .map(|scope| scope.effective.clone())
            .unwrap_or_else(|| self.settings.properties.clone());
        let inherited: Vec<(String, String)> = from_ancestors
            .iter()
            .filter(|(key, _)| !pending.properties.iter().any(|(own, _)| own == key))
            .cloned()
            .collect();

        // What this node's own descendants will inherit in turn.
        let mut effective = pending.properties.clone();
        effective.extend(inherited.iter().cloned());

        self.file.nodes.push(Node {
            id,
            title: pending.title,
            file_path: self.path.clone(),
            tags: pending.tags,
            aliases: pending.aliases,
            level: pending.level,
            todo: pending.todo,
            priority: pending.priority,
            scheduled: pending.scheduled,
            deadline: pending.deadline,
            // The path of the headlines above this one, which the stack holds
            // for every headline rather than only for the ones that are nodes.
            outline_path: self
                .headline_path
                .iter()
                .filter(|(level, _)| *level < pending.level.max(1))
                .map(|(_, title)| title.clone())
                .collect(),
            properties: pending.properties,
            inherited_properties: inherited,
            line,
        });
        self.file
            .refs
            .extend(pending.refs.into_iter().map(|key| (key, id)));
        self.scopes.push(Scope {
            id,
            effective,
            level: pending.level,
        });
    }

    /// Leaves every scope a headline at depth `level` closes.
    fn close_scopes(&mut self, level: usize) {
        while self.scopes.last().is_some_and(|scope| scope.level >= level) {
            self.scopes.pop();
        }
    }

    fn read_keyword(&mut self, keyword: &str, value: &str) {
        match keyword {
            "title" => self.file_title = Some(value.to_string()),
            "filetags" => self.file_tags.extend(parse_tags(value)),
            // `#+roam_alias:` is Org-Roam v1 syntax; v2 uses the
            // `:ROAM_ALIASES:` property. Both are accepted.
            "roam_alias" | "roam_aliases" => self.file_aliases.extend(parse_quoted_list(value)),
            _ => {}
        }
    }

    /// Applies file-level keywords to the file node, which the preamble
    /// drawer may only have produced after they were read.
    fn apply_file_keywords(&mut self) {
        let Some(index) = self.file_node else {
            return;
        };
        let node = &mut self.file.nodes[index];

        if let Some(title) = self.file_title.take() {
            node.title = title;
        } else if node.title.is_empty() {
            // Org-Roam falls back to the file name when there is no `#+title:`.
            node.title = self
                .path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
        }

        node.tags.append(&mut self.file_tags);
        node.aliases.append(&mut self.file_aliases);
    }

    fn collect_links(&mut self, line: &str) {
        let Some(scope) = self.scopes.last() else {
            return;
        };
        let source = scope.id;

        for target in find_link_targets(line) {
            self.file.links.push(ParsedLink { source, target });
        }

        // A citation is a relation to a bibliography key rather than to a
        // node, so it is collected separately from the links.
        for key in citation_keys(line) {
            self.file.citations.push((key, source));
        }
    }
}

impl PendingNode {
    fn preamble() -> Self {
        Self::new(0, String::new(), Vec::new())
    }

    fn headline(headline: Headline) -> Self {
        let mut pending = Self::new(headline.level, headline.title, headline.tags);
        pending.todo = headline.todo;
        pending.priority = headline.priority;
        pending
    }

    fn new(level: usize, title: String, tags: Vec<String>) -> Self {
        Self {
            level,
            title,
            tags,
            in_drawer: false,
            id: None,
            aliases: Vec::new(),
            refs: Vec::new(),
            todo: None,
            priority: None,
            scheduled: None,
            deadline: None,
            properties: Vec::new(),
        }
    }

    fn read_property(&mut self, trimmed: &str, line_number: usize) {
        let Some((key, value)) = parse_property(trimmed) else {
            return;
        };

        match key.as_str() {
            "id" => {
                let value = value.trim();
                if !value.is_empty() {
                    self.id = Some((parse_id(value), line_number));
                }
            }
            "roam_aliases" | "roam_alias" => self.aliases.extend(parse_quoted_list(value)),
            "roam_refs" => self.refs.extend(parse_quoted_list(value)),
            // Everything else is kept as written: the file may declare any
            // property, and a query has no other way to reach it.
            _ => self.properties.push((key, value.to_string())),
        }
    }
}

/// A headline's title, without its stars, keyword, priority or tags.
pub fn parse_headline_title(line: &str, settings: &FileSettings) -> Option<String> {
    parse_headline(line, settings).map(|headline| headline.title)
}

/// Splits `* TODO [#A] Headline  :tag1:tag2:` into depth, title and tags.
///
/// The keyword and the priority are metadata rather than title, but only the
/// file can say which words are keywords and which letters are priorities, so
/// both come from its [`FileSettings`].
pub(crate) fn parse_headline(line: &str, settings: &FileSettings) -> Option<Headline> {
    let stars = line.bytes().take_while(|&b| b == b'*').count();
    if stars == 0 {
        return None;
    }

    let rest = &line[stars..];
    // `**bold**` at the start of a line is not a headline: stars must be
    // followed by whitespace (or end the line).
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }

    let mut title = rest.trim();
    let mut tags = Vec::new();

    // Trailing `:tag1:tag2:` belongs to the headline, not the title.
    if let Some(start) = trailing_tag_start(title) {
        tags = parse_tags(&title[start..]);
        title = title[..start].trim_end();
    }

    // Org's order is keyword, then priority, then title.
    let mut todo = None;
    if let Some((first, rest)) = title.split_once(char::is_whitespace) {
        if let Some(state) = settings.todo_state(first) {
            todo = Some(state);
            title = rest.trim_start();
        }
    } else if let Some(state) = settings.todo_state(title) {
        // A headline that is only a keyword has no title at all.
        todo = Some(state);
        title = "";
    }

    // A cookie whose letter the file does not declare is not a cookie; it is
    // text that happens to look like one.
    let mut priority = None;
    if let Some(rest) = title.strip_prefix("[#") {
        if let Some((letter, after)) = rest.split_once(']') {
            let letter = letter.trim();
            if let Some(c) = letter.chars().next() {
                if letter.chars().count() == 1 && settings.priorities.contains(&c) {
                    priority = Some(c);
                    title = after.trim_start();
                }
            }
        }
    }

    Some(Headline {
        level: stars,
        title: title.to_string(),
        tags,
        todo,
        priority,
    })
}

/// What a headline line carries, once its metadata is separated from its text.
pub(crate) struct Headline {
    pub(crate) level: usize,
    pub(crate) title: String,
    pub(crate) tags: Vec<String>,
    pub(crate) todo: Option<TodoState>,
    pub(crate) priority: Option<char>,
}

/// Byte offset of a trailing `:tag:` run, if the line ends with one.
fn trailing_tag_start(title: &str) -> Option<usize> {
    if !title.ends_with(':') || title.len() < 2 {
        return None;
    }

    let start = title.rfind(|c: char| c.is_whitespace()).map(|i| i + 1)?;
    let candidate = &title[start..];

    let valid = candidate.starts_with(':')
        && candidate.len() > 2
        && candidate
            .trim_matches(':')
            .split(':')
            .all(|tag| !tag.is_empty() && tag.chars().all(is_tag_char));

    valid.then_some(start)
}

fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '@' | '#' | '%')
}

/// Parses `#+key: value`, lowercasing the key.
fn parse_keyword(trimmed: &str) -> Option<(String, &str)> {
    let rest = trimmed.strip_prefix("#+")?;
    let (key, value) = rest.split_once(':')?;
    Some((key.trim().to_ascii_lowercase(), value.trim()))
}

/// Parses `:KEY: value` inside a property drawer, lowercasing the key.
fn parse_property(trimmed: &str) -> Option<(String, &str)> {
    let rest = trimmed.strip_prefix(':')?;
    let (key, value) = rest.split_once(':')?;
    if key.is_empty() || key.contains(char::is_whitespace) {
        return None;
    }
    Some((key.to_ascii_lowercase(), value.trim()))
}

/// Splits `:tag1:tag2:` — or a plain whitespace-separated list — into tags.
fn parse_tags(value: &str) -> Vec<String> {
    value
        .split([':', ' ', '\t'])
        .filter(|tag| !tag.is_empty())
        .map(ToString::to_string)
        .collect()
}

/// Splits a whitespace-separated list where an entry may be `"quoted"`.
fn parse_quoted_list(value: &str) -> Vec<String> {
    let mut entries = Vec::new();
    let mut current = String::new();
    let mut quoted = false;

    for c in value.chars() {
        match c {
            '"' => {
                if quoted {
                    entries.push(std::mem::take(&mut current));
                    quoted = false;
                } else {
                    // A quote opens an entry; anything buffered before it was
                    // a separate unquoted entry.
                    if !current.trim().is_empty() {
                        entries.push(current.trim().to_string());
                    }
                    current.clear();
                    quoted = true;
                }
            }
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    entries.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }

    if !current.trim().is_empty() {
        entries.push(current.trim().to_string());
    }

    entries
}

/// Every `[[target]]` or `[[target][description]]` on a line.
fn find_link_targets(line: &str) -> Vec<LinkTarget> {
    let mut targets = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;

    while let Some(open) = find_from(bytes, i, b"[[") {
        let start = open + 2;
        let Some(end) = find_close(bytes, start) else {
            break;
        };

        let target = &line[start..end];
        if !target.is_empty() {
            targets.push(match target.strip_prefix("id:") {
                Some(id) => LinkTarget::Id(parse_id(id.trim())),
                None => LinkTarget::Ref(target.to_string()),
            });
        }

        i = end;
    }

    targets
}

pub(crate) fn find_from(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| from + offset)
}

/// End of the target part of a link: the `]` that closes it, whether the link
/// has a description (`][`) or not (`]]`).
pub(crate) fn find_close(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b']' {
            return Some(i);
        }
        if bytes[i] == b'\n' {
            return None;
        }
        i += 1;
    }
    None
}

/// Whether `path` looks like an Org file.
pub fn is_org_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("org"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this task exists for: a keyword ending up inside a title.
    #[test]
    fn a_todo_keyword_is_not_part_of_the_title() {
        let file = parse_org(
            "* TODO Write the thing\n:PROPERTIES:\n:ID: n1\n:END:\n",
            "n.org",
        );
        assert_eq!(file.nodes[0].title, "Write the thing");
    }

    #[test]
    fn a_files_own_keywords_decide_what_is_one() {
        // `NEXT` is a keyword here and `TODO` is not, so `TODO` is title text.
        let text = "#+TODO: NEXT WAITING | DONE\n* NEXT Real keyword\n:PROPERTIES:\n:ID: a\n:END:\n* TODO Not a keyword here\n:PROPERTIES:\n:ID: b\n:END:\n";
        let file = parse_org(text, "n.org");

        let titles: Vec<&str> = file.nodes.iter().map(|n| n.title.as_str()).collect();
        assert!(titles.contains(&"Real keyword"), "{titles:?}");
        assert!(titles.contains(&"TODO Not a keyword here"), "{titles:?}");
    }

    #[test]
    fn a_declaration_is_read_even_when_it_follows_the_headline() {
        // Org does not require the declaration to come first, so a purely
        // sequential reader would get this wrong.
        let text = "* NEXT Written before the declaration\n:PROPERTIES:\n:ID: a\n:END:\n#+TODO: NEXT | DONE\n";
        let file = parse_org(text, "n.org");
        assert_eq!(file.nodes[0].title, "Written before the declaration");
    }

    #[test]
    fn without_a_bar_the_last_keyword_is_the_done_state() {
        let settings = FileSettings::scan("#+TODO: TODO FEEDBACK VERIFY DONE\n");
        assert_eq!(settings.todo_keywords, ["TODO", "FEEDBACK", "VERIFY"]);
        assert_eq!(settings.done_keywords, ["DONE"]);
    }

    #[test]
    fn fast_access_keys_are_not_part_of_the_keyword() {
        let settings = FileSettings::scan("#+TODO: TODO(t) NEXT(n) | DONE(d!)\n");
        assert_eq!(settings.todo_keywords, ["TODO", "NEXT"]);
        assert_eq!(settings.done_keywords, ["DONE"]);
    }

    #[test]
    fn several_declarations_add_sequences_rather_than_replacing() {
        let settings = FileSettings::scan("#+TODO: TODO | DONE\n#+TODO: BUG | FIXED\n");
        assert_eq!(settings.todo_keywords, ["TODO", "BUG"]);
        assert_eq!(settings.done_keywords, ["DONE", "FIXED"]);
    }

    #[test]
    fn a_file_declaring_nothing_gets_orgs_defaults() {
        let settings = FileSettings::scan("* TODO nothing declared\n");
        assert_eq!(settings.todo_keywords, ["TODO"]);
        assert_eq!(settings.done_keywords, ["DONE"]);
        assert_eq!(settings.priorities, ['A', 'B', 'C']);
    }

    #[test]
    fn a_priority_cookie_outside_the_declared_range_is_title_text() {
        // Default range is A to C, so `[#Z]` is not a cookie.
        let file = parse_org(
            "* TODO [#Z] Keep me\n:PROPERTIES:\n:ID: a\n:END:\n",
            "n.org",
        );
        assert_eq!(file.nodes[0].title, "[#Z] Keep me");

        // Declaring a wider range makes it one.
        let file = parse_org(
            "#+PRIORITIES: A Z M\n* TODO [#Z] Keep me\n:PROPERTIES:\n:ID: a\n:END:\n",
            "n.org",
        );
        assert_eq!(file.nodes[0].title, "Keep me");
    }

    #[test]
    fn a_keyword_a_priority_and_tags_come_off_together() {
        let file = parse_org(
            "* TODO [#A] The title  :work:urgent:\n:PROPERTIES:\n:ID: a\n:END:\n",
            "n.org",
        );
        assert_eq!(file.nodes[0].title, "The title");
        assert_eq!(file.nodes[0].tags, ["work", "urgent"]);
    }

    #[test]
    fn a_headline_that_is_only_a_keyword_has_no_title() {
        let file = parse_org("* TODO\n:PROPERTIES:\n:ID: a\n:END:\n", "n.org");
        assert_eq!(file.nodes[0].title, "");
    }

    #[test]
    fn a_word_merely_starting_with_a_keyword_is_not_one() {
        let file = parse_org("* TODOs for later\n:PROPERTIES:\n:ID: a\n:END:\n", "n.org");
        assert_eq!(file.nodes[0].title, "TODOs for later");
    }

    #[test]
    fn the_remaining_settings_are_read() {
        let settings = FileSettings::scan(
            "#+CATEGORY: notes\n#+ARCHIVE: ::* Archived\n#+TAGS: @work(w) { laptop(l) pc(p) }\n#+DRAWERS: LOGBOOK CLOCK\n#+STARTUP: overview hidedrawers\n",
        );
        assert_eq!(settings.category.as_deref(), Some("notes"));
        assert_eq!(settings.archive.as_deref(), Some("::* Archived"));
        assert_eq!(settings.declared_tags, ["@work", "laptop", "pc"]);
        assert_eq!(settings.drawers, ["LOGBOOK", "CLOCK"]);
        assert_eq!(settings.startup, ["overview", "hidedrawers"]);
    }

    #[test]
    fn keywords_are_matched_without_regard_to_case() {
        // Org accepts `#+todo:` and `#+TODO:` alike.
        let lower = FileSettings::scan("#+todo: NEXT | DONE\n");
        let upper = FileSettings::scan("#+TODO: NEXT | DONE\n");
        assert_eq!(lower, upper);
        assert_eq!(lower.todo_keywords, ["NEXT"]);
    }

    #[test]
    fn the_settings_reach_the_parsed_file() {
        let file = parse_org("#+CATEGORY: notes\n", "n.org");
        assert_eq!(file.settings.category.as_deref(), Some("notes"));
    }

    const SAMPLE: &str = r#":PROPERTIES:
:ID:       6ba7b810-9dad-11d1-80b4-00c04fd430c8
:ROAM_ALIASES: "Rust Language" rustlang
:ROAM_REFS: https://rust-lang.org
:END:
#+title: Rust
#+filetags: :lang:systems:

Rust is a systems language, see [[id:6ba7b811-9dad-11d1-80b4-00c04fd430c8][Helix]].

* Ownership                                                    :memory:core:
:PROPERTIES:
:ID:       6ba7b812-9dad-11d1-80b4-00c04fd430c8
:END:

Borrowing is covered in [[id:6ba7b813-9dad-11d1-80b4-00c04fd430c8]].

** A subheading without an id
Still part of Ownership, linking [[id:6ba7b811-9dad-11d1-80b4-00c04fd430c8][Helix]].
"#;

    fn uuid(last: u8) -> Uuid {
        Uuid::parse_str(&format!("6ba7b8{last:02x}-9dad-11d1-80b4-00c04fd430c8")).unwrap()
    }

    #[test]
    fn parses_the_file_level_node() {
        let file = parse_org(SAMPLE, "/notes/rust.org");
        let node = &file.nodes[0];

        assert_eq!(node.id, uuid(0x10));
        assert_eq!(node.title, "Rust");
        assert_eq!(node.file_path, Path::new("/notes/rust.org"));
        assert_eq!(node.tags, ["lang", "systems"]);
        assert_eq!(node.aliases, ["Rust Language", "rustlang"]);
        // Zero-based line of the `:ID:` property, which is the second line.
        assert_eq!(node.line, 1);
    }

    #[test]
    fn parses_headline_nodes_with_tags() {
        let file = parse_org(SAMPLE, "/notes/rust.org");
        assert_eq!(file.nodes.len(), 2);

        let node = &file.nodes[1];
        assert_eq!(node.id, uuid(0x12));
        assert_eq!(node.title, "Ownership");
        assert_eq!(node.tags, ["memory", "core"]);
        assert_eq!(node.line, 12);
    }

    #[test]
    fn links_belong_to_the_enclosing_node() {
        let file = parse_org(SAMPLE, "/notes/rust.org");

        let from_file: Vec<_> = file
            .links
            .iter()
            .filter(|link| link.source == uuid(0x10))
            .map(|link| link.target.clone())
            .collect();
        assert_eq!(from_file, [LinkTarget::Id(uuid(0x11))]);

        // The id-less subheading stays inside its parent's scope.
        let from_ownership: Vec<_> = file
            .links
            .iter()
            .filter(|link| link.source == uuid(0x12))
            .map(|link| link.target.clone())
            .collect();
        assert_eq!(
            from_ownership,
            [LinkTarget::Id(uuid(0x13)), LinkTarget::Id(uuid(0x11))]
        );
    }

    #[test]
    fn collects_roam_refs() {
        let file = parse_org(SAMPLE, "/notes/rust.org");
        assert_eq!(
            file.refs,
            [("https://rust-lang.org".to_string(), uuid(0x10))]
        );
    }

    #[test]
    fn a_headline_without_an_id_is_not_a_node() {
        let file = parse_org("* Just a headline\nSome text.\n", "/notes/x.org");
        assert!(file.nodes.is_empty());
        assert!(file.links.is_empty());
    }

    #[test]
    fn a_link_outside_any_node_is_dropped() {
        // No `:ID:` anywhere, so there is no node to hang the link off.
        let file = parse_org(
            "Text with [[id:6ba7b810-9dad-11d1-80b4-00c04fd430c8]].",
            "/x.org",
        );
        assert!(file.links.is_empty());
    }

    #[test]
    fn falls_back_to_the_file_name_when_there_is_no_title() {
        let text = ":PROPERTIES:\n:ID: 6ba7b810-9dad-11d1-80b4-00c04fd430c8\n:END:\n";
        let file = parse_org(text, "/notes/my-note.org");
        assert_eq!(file.nodes[0].title, "my-note");
    }

    #[test]
    fn non_uuid_ids_hash_consistently() {
        // `org-id-method` can produce timestamps rather than UUIDs; a node and
        // the links pointing at it must still agree.
        let text = ":PROPERTIES:\n:ID: 20230101T120000.000000\n:END:\n#+title: TS\n\n[[id:20230101T120000.000000]]\n";
        let file = parse_org(text, "/notes/ts.org");

        assert_eq!(file.nodes.len(), 1);
        assert_eq!(file.links.len(), 1);
        assert_eq!(file.links[0].target, LinkTarget::Id(file.nodes[0].id));
        assert_ne!(file.nodes[0].id, Uuid::nil());
    }

    #[test]
    fn bold_text_is_not_a_headline() {
        assert!(parse_headline("**bold** at line start", &FileSettings::default()).is_none());
        assert!(parse_headline("*italic*", &FileSettings::default()).is_none());
        assert_eq!(
            parse_headline("* Real", &FileSettings::default())
                .unwrap()
                .level,
            1
        );
        assert_eq!(
            parse_headline("*** Deep", &FileSettings::default())
                .unwrap()
                .level,
            3
        );
    }

    #[test]
    fn headline_metadata_is_stripped_from_the_title() {
        let headline = parse_headline(
            "** [#A] Urgent thing   :work:urgent:",
            &FileSettings::default(),
        )
        .unwrap();
        assert_eq!(headline.title, "Urgent thing");
        assert_eq!(headline.tags, ["work", "urgent"]);
        // The cookie is now kept rather than thrown away with the text.
        assert_eq!(headline.priority, Some('A'));

        // A trailing colon that is not a tag run stays in the title.
        let headline = parse_headline("* See also:", &FileSettings::default()).unwrap();
        assert_eq!(headline.title, "See also:");
        assert!(headline.tags.is_empty());
    }

    #[test]
    fn link_targets_are_found_with_and_without_descriptions() {
        let targets = find_link_targets("a [[id:abc][desc]] b [[https://x.test]] c [[id:def]]");
        assert_eq!(
            targets,
            [
                LinkTarget::Id(parse_id("abc")),
                LinkTarget::Ref("https://x.test".to_string()),
                LinkTarget::Id(parse_id("def")),
            ]
        );
    }

    #[test]
    fn quoted_lists_keep_spaces_together() {
        assert_eq!(
            parse_quoted_list(r#""Two Words" single"#),
            ["Two Words", "single"]
        );
        assert_eq!(parse_quoted_list("a b  c"), ["a", "b", "c"]);
        assert!(parse_quoted_list("   ").is_empty());
    }

    #[test]
    fn drawer_keys_are_case_insensitive() {
        let text =
            ":properties:\n:id: 6ba7b810-9dad-11d1-80b4-00c04fd430c8\n:end:\n#+TITLE: Upper\n";
        let file = parse_org(text, "/notes/u.org");
        assert_eq!(file.nodes.len(), 1);
        assert_eq!(file.nodes[0].title, "Upper");
    }
}
