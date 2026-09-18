//! A line-oriented parser for the subset of Org syntax Org-Roam cares about.
//!
//! This deliberately does not go through Tree-sitter: indexing runs over every
//! file in a notes directory, and the properties Org-Roam needs — `:ID:`,
//! `#+title:`, `#+filetags:`, aliases, refs and `[[id:…]]` links — all sit in
//! syntax that is unambiguous line by line.

use std::path::{Path, PathBuf};

use uuid::Uuid;

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
}

/// Turns an Org-Roam id into a [`Uuid`], hashing ids that are not UUIDs.
pub fn parse_id(raw: &str) -> Uuid {
    Uuid::parse_str(raw).unwrap_or_else(|_| Uuid::new_v5(&ORG_ID_NAMESPACE, raw.as_bytes()))
}

/// Parses `text` as the Org file stored at `path`.
pub fn parse_org(text: &str, path: impl Into<PathBuf>) -> ParsedFile {
    Parser::new(path.into()).run(text)
}

/// The node a link belongs to: the innermost headline with an `:ID:`, falling
/// back to the file-level node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Scope {
    id: Uuid,
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
        }
    }

    fn run(mut self, text: &str) -> ParsedFile {
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

            if let Some((level, title, tags)) = parse_headline(line) {
                // A headline without an `:ID:` never became a node.
                self.pending = None;
                self.close_scopes(level);
                self.pending = Some(PendingNode::headline(level, title, tags));
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

        self.file.nodes.push(Node {
            id,
            title: pending.title,
            file_path: self.path.clone(),
            tags: pending.tags,
            aliases: pending.aliases,
            line,
        });
        self.file
            .refs
            .extend(pending.refs.into_iter().map(|key| (key, id)));
        self.scopes.push(Scope {
            id,
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
    }
}

impl PendingNode {
    fn preamble() -> Self {
        Self {
            level: 0,
            title: String::new(),
            tags: Vec::new(),
            in_drawer: false,
            id: None,
            aliases: Vec::new(),
            refs: Vec::new(),
        }
    }

    fn headline(level: usize, title: String, tags: Vec<String>) -> Self {
        Self {
            level,
            title,
            tags,
            in_drawer: false,
            id: None,
            aliases: Vec::new(),
            refs: Vec::new(),
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
            _ => {}
        }
    }
}

/// Splits `* TODO Headline  :tag1:tag2:` into depth, title and tags.
fn parse_headline(line: &str) -> Option<(usize, String, Vec<String>)> {
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

    // A priority cookie is metadata; TODO keywords are configurable in Org and
    // are deliberately left in the title rather than guessed at.
    if let Some(rest) = title.strip_prefix("[#") {
        if let Some((_, after)) = rest.split_once(']') {
            title = after.trim_start();
        }
    }

    Some((stars, title.to_string(), tags))
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

fn find_from(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| from + offset)
}

/// End of the target part of a link: the `]` that closes it, whether the link
/// has a description (`][`) or not (`]]`).
fn find_close(bytes: &[u8], start: usize) -> Option<usize> {
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
        assert!(parse_headline("**bold** at line start").is_none());
        assert!(parse_headline("*italic*").is_none());
        assert_eq!(parse_headline("* Real").unwrap().0, 1);
        assert_eq!(parse_headline("*** Deep").unwrap().0, 3);
    }

    #[test]
    fn headline_metadata_is_stripped_from_the_title() {
        let (_, title, tags) = parse_headline("** [#A] Urgent thing   :work:urgent:").unwrap();
        assert_eq!(title, "Urgent thing");
        assert_eq!(tags, ["work", "urgent"]);

        // A trailing colon that is not a tag run stays in the title.
        let (_, title, tags) = parse_headline("* See also:").unwrap();
        assert_eq!(title, "See also:");
        assert!(tags.is_empty());
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
