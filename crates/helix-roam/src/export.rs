//! Export to Markdown, HTML and LaTeX.
//!
//! One reader turns the Org text into a small tree of elements, and each
//! backend writes that tree out. The reader is not Org's full element
//! grammar: it covers what notes are written with — headlines, paragraphs
//! and their markup, links, lists, tables, source and example blocks,
//! quotes, footnotes — and says in [`Exported::warnings`] what it could not
//! carry across, rather than dropping it silently.
//!
//! `id:` links are what make this worth having on a notes directory. Left
//! alone they are UUIDs; here they become links to the exported file of the
//! node they point at, with the node's anchor when it is a headline, through
//! a [`Resolve`] the caller supplies from the graph.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use uuid::Uuid;

use crate::hyperlink::{find_links, FileSearch, LinkKind};
use crate::parser::FileSettings;
use crate::restructure::{headline_level, is_planning_line};
use crate::source::{common_indent, unescape};

/// The formats this can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Markdown,
    Html,
    Latex,
}

impl Backend {
    /// The extension of an exported file, which is also what a link to
    /// another exported Org file is rewritten to.
    pub fn extension(self) -> &'static str {
        match self {
            Backend::Markdown => "md",
            Backend::Html => "html",
            Backend::Latex => "tex",
        }
    }

    /// The extension a link to another exported Org file gets.
    ///
    /// For LaTeX that is the PDF the file compiles to: a link to a `.tex`
    /// file from inside a PDF leads nowhere a reader can use.
    pub fn link_extension(self) -> &'static str {
        match self {
            Backend::Latex => "pdf",
            other => other.extension(),
        }
    }

    /// Reads a backend's name as a command argument gives it.
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "md" | "markdown" => Some(Backend::Markdown),
            "html" => Some(Backend::Html),
            "latex" | "tex" => Some(Backend::Latex),
            _ => None,
        }
    }
}

/// Where an `id:` link lands, as the graph knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdTarget {
    /// The Org file the node lives in.
    pub file: PathBuf,
    /// The node's anchor, for a headline node; `None` for a file node.
    pub anchor: Option<String>,
    pub title: String,
}

/// Answers where an `id:` link points.
pub trait Resolve {
    fn id(&self, id: &Uuid) -> Option<IdTarget>;
}

/// Resolves nothing: every `id:` link falls back to its description.
pub struct NoResolve;

impl Resolve for NoResolve {
    fn id(&self, _: &Uuid) -> Option<IdTarget> {
        None
    }
}

/// An export's output, and what it could not carry across.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exported {
    pub content: String,
    pub warnings: Vec<String>,
}

/// The anchor a headline exports with: its `:CUSTOM_ID:` if it has one, or
/// its title turned into a slug the way GitHub turns a Markdown heading
/// into one — lower case, letters, digits, `-` and `_` kept, spaces become
/// dashes — so a Markdown renderer's own anchors agree with these.
pub fn anchor_for(title: &str, custom_id: Option<&str>) -> String {
    if let Some(id) = custom_id.filter(|id| !id.is_empty()) {
        return id.to_string();
    }
    title
        .trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            ' ' => Some('-'),
            _ => None,
        })
        .collect()
}

// ── The tree ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum Inline {
    Text(String),
    Bold(Vec<Inline>),
    Italic(Vec<Inline>),
    Underline(Vec<Inline>),
    Strike(Vec<Inline>),
    Verbatim(String),
    Code(String),
    Link {
        target: LinkKind,
        description: Option<Vec<Inline>>,
    },
    /// A footnote reference, by label.
    Footnote(String),
    LineBreak,
    /// `[cite/style:prefix;@key suffix;…]`.
    Citation {
        style: Option<String>,
        cites: Vec<Cite>,
        prefix: String,
        suffix: String,
    },
}

/// One reference inside a citation, with the text around it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Cite {
    key: String,
    prefix: String,
    suffix: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Heading {
    level: usize,
    todo: Option<(String, bool)>,
    priority: Option<char>,
    title: Vec<Inline>,
    tags: Vec<String>,
    anchor: String,
    /// `:CUSTOM_ID:`, which a Markdown renderer's own anchors do not know.
    custom: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListKind {
    Unordered,
    Ordered,
    Description,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    checkbox: Option<char>,
    term: Option<Vec<Inline>>,
    content: Vec<Element>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Element {
    Heading(Heading),
    Paragraph(Vec<Inline>),
    List(ListKind, Vec<Item>),
    /// Rows, and how many of them are the header.
    Table(Vec<Vec<Vec<Inline>>>, usize),
    Code {
        language: Option<String>,
        code: String,
    },
    Example(String),
    Quote(Vec<Element>),
    Verse(Vec<Vec<Inline>>),
    Center(Vec<Element>),
    Special(String, Vec<Element>),
    Export {
        backend: String,
        text: String,
    },
    Rule,
    /// Where `#+PRINT_BIBLIOGRAPHY:` asks for the references.
    Bibliography,
}

/// The export options Org reads from `#+OPTIONS:`, with Org's defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Options {
    /// `toc:`: `None` for no table of contents, or the deepest level in it.
    toc: Option<usize>,
    num: bool,
    todo: bool,
    tags: bool,
    priority: bool,
    title: Option<String>,
    author: Option<String>,
    date: Option<String>,
    exclude_tags: Vec<String>,
    /// `#+BIBLIOGRAPHY:` files, as written.
    bibliography: Vec<String>,
    /// `#+CITE_EXPORT:`'s processor: `basic` unless it says `biblatex` or
    /// `natbib`, which only LaTeX has.
    cite_export: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            toc: Some(usize::MAX),
            num: true,
            todo: true,
            tags: true,
            priority: false,
            title: None,
            author: None,
            date: None,
            exclude_tags: vec!["noexport".to_string()],
            bibliography: Vec::new(),
            cite_export: "basic".to_string(),
        }
    }
}

struct Document {
    options: Options,
    elements: Vec<Element>,
    footnotes: Vec<(String, Vec<Inline>)>,
    /// Headline titles to the anchors they export with, for `*Headline`
    /// links.
    anchors: HashMap<String, String>,
    warnings: Vec<String>,
    /// The bibliography's entries, from `#+BIBLIOGRAPHY:`.
    bib: Vec<crate::bib::Entry>,
    /// Keys in the order they are first cited.
    cited: Vec<String>,
}

// ── Reading ────────────────────────────────────────────────────────────────

struct Reader<'a> {
    settings: &'a FileSettings,
    footnotes: Vec<(String, Vec<Inline>)>,
    anonymous: usize,
    exclude_tags: Vec<String>,
    warnings: Vec<String>,
    /// `#+MACRO:` definitions and the built-in ones, by name.
    macros: HashMap<String, String>,
    /// Every `#+KEY: value`, for `{{{keyword(KEY)}}}`.
    keywords: HashMap<String, String>,
    cited: Vec<String>,
}

fn keyword<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("#+")?;
    let (key, value) = rest.split_once(':')?;
    key.eq_ignore_ascii_case(name).then_some(value.trim())
}

fn read_options(text: &str) -> Options {
    let mut options = Options::default();
    for line in text.lines() {
        if let Some(value) = keyword(line, "title") {
            options.title = Some(value.to_string());
        } else if let Some(value) = keyword(line, "author") {
            options.author = Some(value.to_string());
        } else if let Some(value) = keyword(line, "date") {
            options.date = Some(value.to_string());
        } else if let Some(value) = keyword(line, "bibliography") {
            options
                .bibliography
                .push(value.trim_matches('"').to_string());
        } else if let Some(value) = keyword(line, "cite_export") {
            options.cite_export = value
                .split_whitespace()
                .next()
                .unwrap_or("basic")
                .to_ascii_lowercase();
        } else if let Some(value) = keyword(line, "exclude_tags") {
            options.exclude_tags = value.split_whitespace().map(str::to_string).collect();
        } else if let Some(value) = keyword(line, "options") {
            for option in value.split_whitespace() {
                let Some((key, value)) = option.split_once(':') else {
                    continue;
                };
                let on = value != "nil";
                match key {
                    "toc" => {
                        options.toc = match value {
                            "nil" => None,
                            "t" => Some(usize::MAX),
                            depth => depth.parse().ok(),
                        }
                    }
                    "num" => options.num = on,
                    "todo" => options.todo = on,
                    "tags" => options.tags = on,
                    "pri" => options.priority = on,
                    _ => {}
                }
            }
        }
    }
    options
}

/// `- `, `+ `, `1. ` or `1) ` at `indent`, and where the content starts.
fn bullet(line: &str) -> Option<(usize, ListKind, usize)> {
    let indent = line.len() - line.trim_start().len();
    let rest = &line[indent..];

    let marker = if rest.starts_with("- ") || rest.starts_with("+ ") || rest == "-" || rest == "+" {
        1
    } else if rest.starts_with("* ") && indent > 0 {
        // At column 0 that is a headline.
        1
    } else {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        let after = rest.get(digits..digits + 1);
        if digits == 0 || !matches!(after, Some(".") | Some(")")) {
            return None;
        }
        if !rest[digits + 1..].is_empty() && !rest[digits + 1..].starts_with(' ') {
            return None;
        }
        return Some((indent, ListKind::Ordered, indent + digits + 1));
    };

    Some((indent, ListKind::Unordered, indent + marker))
}

impl Reader<'_> {
    /// Reads a run of lines into elements. `top` is true for the document
    /// itself, where headlines can occur.
    fn elements(&mut self, lines: &[&str], top: bool) -> Vec<Element> {
        let mut out = Vec::new();
        let mut paragraph: Vec<&str> = Vec::new();
        let mut drop_results = false;
        let mut at = 0;

        macro_rules! flush {
            () => {
                if !paragraph.is_empty() {
                    let text = paragraph.join("\n");
                    out.push(Element::Paragraph(self.inlines(&text)));
                    paragraph.clear();
                }
            };
        }

        while at < lines.len() {
            let line = lines[at];
            let trimmed = line.trim();

            if trimmed.is_empty() {
                flush!();
                at += 1;
                continue;
            }

            // Headlines.
            if let Some(level) = headline_level(line).filter(|_| top) {
                flush!();
                let end = (at + 1..lines.len())
                    .find(|&i| headline_level(lines[i]).is_some_and(|other| other <= level))
                    .unwrap_or(lines.len());
                let (heading, skip) = self.heading(lines, at, level);
                match heading {
                    Some(heading) => {
                        out.push(Element::Heading(heading));
                        at += 1 + skip;
                    }
                    // Excluded: the whole subtree goes.
                    None => at = end,
                }
                continue;
            }

            // Inline tasks: a task inside the body, drawn apart from it as
            // Org's exporters do, and not a section.
            if crate::restructure::inline_task_level(line).is_some() {
                flush!();
                let end = crate::restructure::inline_task_end(lines, at);
                let body_end =
                    if end > at + 1 && lines[end - 1].trim_start_matches('*').trim() == "END" {
                        end - 1
                    } else {
                        end
                    };
                if let Some(parsed) = crate::parser::parse_headline(line, self.settings) {
                    let mut heading = Vec::new();
                    if let Some(state) = parsed.todo {
                        heading.push(Inline::Text(format!("{} ", state.keyword)));
                    }
                    heading.extend(self.inlines(&parsed.title));
                    let mut inner = vec![Element::Paragraph(vec![Inline::Bold(heading)])];
                    inner.extend(self.elements(&lines[at + 1..body_end], false));
                    out.push(Element::Special("inlinetask".to_string(), inner));
                }
                at = end;
                continue;
            }

            // Drawers, anywhere.
            if let Some(end) = drawer_end(lines, at) {
                flush!();
                at = end + 1;
                continue;
            }

            // Blocks.
            if let Some((name, params)) = block_begin(trimmed) {
                if let Some(end) = block_end(lines, at, &name) {
                    flush!();
                    let inner = &lines[at + 1..end];
                    match name.as_str() {
                        "src" => {
                            let exports = header_arg(params, "exports").unwrap_or("code");
                            drop_results = !matches!(exports, "results" | "both");
                            if matches!(exports, "code" | "both") {
                                out.push(Element::Code {
                                    language: params
                                        .split_whitespace()
                                        .next()
                                        .filter(|l| !l.starts_with(':'))
                                        .map(str::to_string),
                                    code: raw_block(inner),
                                });
                            }
                        }
                        "example" => out.push(Element::Example(raw_block(inner))),
                        "quote" => out.push(Element::Quote(self.elements(inner, false))),
                        "center" => out.push(Element::Center(self.elements(inner, false))),
                        "verse" => out.push(Element::Verse(
                            inner.iter().map(|line| self.inlines(line.trim())).collect(),
                        )),
                        "comment" => {}
                        "export" => out.push(Element::Export {
                            backend: params.trim().to_ascii_lowercase(),
                            text: inner.join("\n"),
                        }),
                        _ => out.push(Element::Special(name.clone(), self.elements(inner, false))),
                    }
                    at = end + 1;
                    continue;
                }
            }

            // `#+RESULTS:`, kept or dropped with the block above it.
            if keyword(line, "results").is_some()
                || trimmed.to_ascii_lowercase().starts_with("#+results[")
            {
                flush!();
                at += 1;
                if drop_results {
                    while at < lines.len() && !lines[at].trim().is_empty() {
                        at += 1;
                    }
                }
                drop_results = false;
                continue;
            }

            if keyword(line, "print_bibliography").is_some() {
                flush!();
                out.push(Element::Bibliography);
                at += 1;
                continue;
            }

            // Other keywords, and comments.
            if trimmed.starts_with("#+") || trimmed == "#" || trimmed.starts_with("# ") {
                flush!();
                at += 1;
                continue;
            }

            // Fixed-width lines.
            if trimmed == ":" || trimmed.starts_with(": ") {
                flush!();
                let mut body = Vec::new();
                while at < lines.len()
                    && (lines[at].trim() == ":" || lines[at].trim_start().starts_with(": "))
                {
                    body.push(lines[at].trim_start().get(2..).unwrap_or(""));
                    at += 1;
                }
                out.push(Element::Example(body.join("\n") + "\n"));
                continue;
            }

            // Tables.
            if trimmed.starts_with('|') {
                flush!();
                let start = at;
                while at < lines.len() && lines[at].trim_start().starts_with('|') {
                    at += 1;
                }
                out.push(self.table(&lines[start..at]));
                continue;
            }

            // Horizontal rules: five dashes or more, alone.
            if trimmed.len() >= 5 && trimmed.chars().all(|c| c == '-') {
                flush!();
                out.push(Element::Rule);
                at += 1;
                continue;
            }

            // Footnote definitions, at the left margin.
            if line.starts_with("[fn:") {
                if let Some(close) = line.find(']') {
                    flush!();
                    let label = line[4..close].to_string();
                    let mut body = vec![line[close + 1..].trim()];
                    at += 1;
                    while at < lines.len()
                        && !lines[at].trim().is_empty()
                        && !lines[at].starts_with("[fn:")
                        && headline_level(lines[at]).is_none()
                    {
                        body.push(lines[at].trim());
                        at += 1;
                    }
                    let inlines = self.inlines(&body.join("\n"));
                    self.footnotes.push((label, inlines));
                    continue;
                }
            }

            // Lists.
            if let Some((indent, _, _)) = bullet(line) {
                flush!();
                let (list, next) = self.list(lines, at, indent);
                out.push(list);
                at = next;
                continue;
            }

            paragraph.push(trimmed);
            at += 1;
        }
        flush!();
        out
    }

    /// A headline, or `None` when it is excluded; and how many lines under
    /// it (planning and property drawer) belong to it rather than the body.
    fn heading(&mut self, lines: &[&str], at: usize, level: usize) -> (Option<Heading>, usize) {
        let parsed = crate::parser::parse_headline(lines[at], self.settings);

        let Some(parsed) = parsed else {
            return (None, 0);
        };
        let commented = parsed.title == "COMMENT" || parsed.title.starts_with("COMMENT ");
        if commented
            || parsed
                .tags
                .iter()
                .any(|tag| self.exclude_tags.iter().any(|excluded| excluded == tag))
        {
            return (None, 0);
        }

        let mut skip = 0;
        if lines.get(at + 1).is_some_and(|line| is_planning_line(line)) {
            skip += 1;
        }
        let mut custom_id = None;
        if lines
            .get(at + 1 + skip)
            .is_some_and(|line| line.trim().eq_ignore_ascii_case(":PROPERTIES:"))
        {
            if let Some(end) = drawer_end(lines, at + 1 + skip) {
                for line in &lines[at + 2 + skip..end] {
                    let line = line.trim();
                    if let Some(rest) = line.strip_prefix(':') {
                        if let Some((key, value)) = rest.split_once(':') {
                            if key.eq_ignore_ascii_case("CUSTOM_ID") {
                                custom_id = Some(value.trim().to_string());
                            }
                        }
                    }
                }
                skip = end - at;
            }
        }

        let title = self.inlines(&parsed.title);
        let anchor = anchor_for(&plain(&title), custom_id.as_deref());
        (
            Some(Heading {
                level,
                todo: parsed.todo.map(|state| (state.keyword, state.done)),
                priority: parsed.priority,
                title,
                tags: parsed.tags,
                anchor,
                custom: custom_id.is_some(),
            }),
            skip,
        )
    }

    fn table(&mut self, lines: &[&str]) -> Element {
        let mut rows = Vec::new();
        let mut header = 0;
        for line in lines {
            let trimmed = line.trim();
            if trimmed.starts_with("|-") {
                // The first rule marks the rows above it as the header.
                if header == 0 && !rows.is_empty() {
                    header = rows.len();
                }
                continue;
            }
            let inner = trimmed.trim_start_matches('|');
            let inner = inner.strip_suffix('|').unwrap_or(inner);
            rows.push(
                inner
                    .split('|')
                    .map(|cell| self.inlines(cell.trim()))
                    .collect(),
            );
        }
        Element::Table(rows, header)
    }

    /// A list starting at `at`, and the line after it.
    fn list(&mut self, lines: &[&str], mut at: usize, indent: usize) -> (Element, usize) {
        let mut items = Vec::new();
        let mut kind = None;

        while at < lines.len() {
            let Some((item_indent, item_kind, content)) = bullet(lines[at]) else {
                break;
            };
            if item_indent != indent {
                break;
            }

            // The item's lines: until a line at or left of the bullet, or two
            // blank lines in a row.
            let mut end = at + 1;
            let mut blanks = 0;
            while end < lines.len() {
                let line = lines[end];
                if line.trim().is_empty() {
                    blanks += 1;
                    if blanks == 2 {
                        break;
                    }
                    end += 1;
                    continue;
                }
                let line_indent = line.len() - line.trim_start().len();
                if line_indent <= indent {
                    break;
                }
                blanks = 0;
                end += 1;
            }
            // Trailing blank lines are the list's, not the item's.
            let mut last = end;
            while last > at + 1 && lines[last - 1].trim().is_empty() {
                last -= 1;
            }

            let mut first = lines[at]
                .get(content..)
                .unwrap_or("")
                .trim_start()
                .to_string();
            let mut checkbox = None;
            for (mark, value) in [("[ ] ", ' '), ("[X] ", 'X'), ("[x] ", 'X'), ("[-] ", '-')] {
                if let Some(rest) = first.strip_prefix(mark) {
                    checkbox = Some(value);
                    first = rest.to_string();
                }
            }
            let mut term = None;
            let mut this_kind = item_kind;
            if item_kind == ListKind::Unordered {
                if let Some((t, rest)) = first.split_once(" :: ") {
                    term = Some(self.inlines(t.trim()));
                    first = rest.to_string();
                    this_kind = ListKind::Description;
                } else if let Some(t) = first.strip_suffix(" ::") {
                    term = Some(self.inlines(t.trim()));
                    first = String::new();
                    this_kind = ListKind::Description;
                }
            }
            kind.get_or_insert(this_kind);

            // Continuation lines, shifted left by what they share.
            let rest: Vec<&str> = lines[at + 1..last].to_vec();
            let shift = common_indent(&rest);
            let mut body: Vec<String> = vec![first];
            body.extend(
                rest.iter()
                    .map(|line| line.get(shift..).unwrap_or("").to_string()),
            );
            let body_refs: Vec<&str> = body.iter().map(String::as_str).collect();
            let content = self.elements(&body_refs, false);

            items.push(Item {
                checkbox,
                term,
                content,
            });
            at = end;
            // A blank line between items does not end the list.
            while at < lines.len() && lines[at].trim().is_empty() && blanks < 2 {
                if lines
                    .get(at + 1)
                    .and_then(|l| bullet(l))
                    .is_some_and(|(i, _, _)| i == indent)
                {
                    at += 1;
                } else {
                    break;
                }
            }
        }

        (
            Element::List(kind.unwrap_or(ListKind::Unordered), items),
            at,
        )
    }

    /// Inline markup, with links found first so their brackets are never
    /// read as anything else.
    fn inlines(&mut self, text: &str) -> Vec<Inline> {
        let expanded = self.expand_macros(text, 0);
        let text = expanded.as_str();
        let links = find_links(text, &self.settings.link_abbreviations);
        let mut masked = String::with_capacity(text.len());
        let mut last = 0;
        let mut found = Vec::new();
        for (index, link) in links.iter().enumerate() {
            masked.push_str(&text[last..link.range.start]);
            masked.push(char::from_u32(0xF0000 + index as u32).unwrap());
            last = link.range.end;
            let description = link.description.as_deref().map(|d| self.markup(d, &[]));
            found.push(Inline::Link {
                target: link.kind.clone(),
                description,
            });
        }
        masked.push_str(&text[last..]);
        self.markup(&masked, &found)
    }

    /// `{{{name(arg, arg)}}}` replaced by the macro's body, `$1`, `$2`, …
    /// taking the arguments. A macro's body may call another, to a depth
    /// that stops a macro calling itself from looping.
    fn expand_macros(&mut self, text: &str, depth: usize) -> String {
        if !text.contains("{{{") || depth > 8 {
            return text.to_string();
        }
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(open) = rest.find("{{{") {
            out.push_str(&rest[..open]);
            let Some(close) = rest[open..].find("}}}").map(|close| open + close) else {
                out.push_str(&rest[open..]);
                return out;
            };
            let call = &rest[open + 3..close];
            let (name, args) = match call.split_once('(') {
                Some((name, args)) => (name.trim(), args.strip_suffix(')').unwrap_or(args)),
                None => (call.trim(), ""),
            };
            let args: Vec<String> = split_macro_args(args);

            let expansion = match name.to_ascii_lowercase().as_str() {
                "keyword" => self
                    .keywords
                    .get(
                        &args
                            .first()
                            .cloned()
                            .unwrap_or_default()
                            .to_ascii_lowercase(),
                    )
                    .cloned(),
                lower => self.macros.get(lower).map(|body| {
                    let mut body = body.clone();
                    for (index, arg) in args.iter().enumerate().rev() {
                        body = body.replace(&format!("${}", index + 1), arg);
                    }
                    body
                }),
            };
            match expansion {
                Some(expansion) => out.push_str(&self.expand_macros(&expansion, depth + 1)),
                None => {
                    self.warnings.push(format!(
                        "{{{{{{{name}}}}}}} is not a macro this file defines"
                    ));
                    out.push_str(&rest[open..close + 3]);
                }
            }
            rest = &rest[close + 3..];
        }
        out.push_str(rest);
        out
    }

    /// Reads `[cite/style:prefix;@key suffix;…;suffix]`, from the text after
    /// its opening bracket, returning the citation and its length.
    fn citation(&mut self, inner: &str) -> Option<Inline> {
        let rest = inner.strip_prefix("cite")?;
        let (style, body) = match rest.strip_prefix('/') {
            Some(styled) => {
                let (style, body) = styled.split_once(':')?;
                (Some(style.to_string()), body)
            }
            None => (None, rest.strip_prefix(':')?),
        };

        let parts: Vec<&str> = body.split(';').collect();
        let mut cites = Vec::new();
        let mut prefix = String::new();
        let mut suffix = String::new();
        for (index, part) in parts.iter().enumerate() {
            match part.find('@') {
                Some(at) => {
                    let key: String = part[at + 1..]
                        .chars()
                        .take_while(|c| {
                            c.is_alphanumeric() || matches!(c, '-' | '_' | ':' | '.' | '/')
                        })
                        .collect();
                    let after = part[at + 1 + key.len()..].trim().to_string();
                    if !self.cited.contains(&key) {
                        self.cited.push(key.clone());
                    }
                    cites.push(Cite {
                        key,
                        prefix: part[..at].trim().to_string(),
                        suffix: after,
                    });
                }
                None if index == 0 => prefix = part.trim().to_string(),
                None => suffix = part.trim().to_string(),
            }
        }
        (!cites.is_empty()).then_some(Inline::Citation {
            style,
            cites,
            prefix,
            suffix,
        })
    }

    /// Emphasis, footnote references, line breaks and bare URLs.
    fn markup(&mut self, text: &str, links: &[Inline]) -> Vec<Inline> {
        let chars: Vec<char> = text.chars().collect();
        let mut out = Vec::new();
        let mut plain = String::new();
        let mut at = 0;

        macro_rules! push_plain {
            () => {
                if !plain.is_empty() {
                    out.push(Inline::Text(std::mem::take(&mut plain)));
                }
            };
        }

        while at < chars.len() {
            let c = chars[at];

            // A masked link.
            if let Some(index) = (c as u32)
                .checked_sub(0xF0000)
                .filter(|&i| (i as usize) < links.len())
            {
                push_plain!();
                out.push(links[index as usize].clone());
                at += 1;
                continue;
            }

            // `\\` at the end of a line.
            if c == '\\' && chars.get(at + 1) == Some(&'\\') {
                let after: String = chars[at + 2..].iter().take_while(|c| **c != '\n').collect();
                if after.trim().is_empty() {
                    push_plain!();
                    out.push(Inline::LineBreak);
                    at += 2 + after.chars().count();
                    if chars.get(at) == Some(&'\n') {
                        at += 1;
                    }
                    continue;
                }
            }

            // `[cite:@key]` and its styled forms.
            if c == '[' && chars[at..].iter().take(5).collect::<String>() == "[cite" {
                if let Some(close) = chars[at..].iter().position(|c| *c == ']') {
                    let inner: String = chars[at + 1..at + close].iter().collect();
                    if let Some(citation) = self.citation(&inner) {
                        push_plain!();
                        out.push(citation);
                        at += close + 1;
                        continue;
                    }
                }
            }

            // `[fn:label]`, `[fn::inline definition]`, `[fn:label:definition]`.
            if c == '[' && chars[at..].iter().take(4).collect::<String>() == "[fn:" {
                if let Some(close) = chars[at..].iter().position(|c| *c == ']') {
                    let inner: String = chars[at + 4..at + close].iter().collect();
                    let (label, definition) = match inner.split_once(':') {
                        Some((label, definition)) => {
                            (label.to_string(), Some(definition.to_string()))
                        }
                        None => (inner.clone(), None),
                    };
                    let label = if label.is_empty() {
                        self.anonymous += 1;
                        format!("anon-{}", self.anonymous)
                    } else {
                        label
                    };
                    if let Some(definition) = definition {
                        let inlines = self.markup(&definition, &[]);
                        self.footnotes.push((label.clone(), inlines));
                    }
                    push_plain!();
                    out.push(Inline::Footnote(label));
                    at += close + 1;
                    continue;
                }
            }

            // Bare URLs.
            if (c == 'h' || c == 'f')
                && (at == 0 || !chars[at - 1].is_alphanumeric())
                && ["https://", "http://", "ftp://"].iter().any(|scheme| {
                    chars[at..].iter().take(scheme.len()).collect::<String>() == *scheme
                })
            {
                let len = chars[at..]
                    .iter()
                    .position(|c| c.is_whitespace() || matches!(c, '<' | '>' | '"' | ')'))
                    .unwrap_or(chars.len() - at);
                let mut url: String = chars[at..at + len].iter().collect();
                // Sentence punctuation after a URL is not part of it.
                while url.ends_with(['.', ',', ';', ':', '!', '?']) {
                    url.pop();
                }
                let taken = url.chars().count();
                push_plain!();
                out.push(Inline::Link {
                    target: LinkKind::Url(url),
                    description: None,
                });
                at += taken;
                continue;
            }

            // Emphasis.
            if let Some((end, inline)) = self.emphasis(&chars, at, links) {
                push_plain!();
                out.push(inline);
                at = end;
                continue;
            }

            plain.push(c);
            at += 1;
        }
        push_plain!();
        out
    }

    /// Org's emphasis rule: the opening marker follows whitespace, the start,
    /// or one of `-({'"`, and is followed by a non-blank; the closing marker
    /// follows a non-blank and is followed by whitespace, the end, or
    /// punctuation. The contents span at most one line break.
    fn emphasis(&mut self, chars: &[char], at: usize, links: &[Inline]) -> Option<(usize, Inline)> {
        let marker = chars[at];
        if !matches!(marker, '*' | '/' | '_' | '+' | '=' | '~') {
            return None;
        }
        let before_ok =
            at == 0 || chars[at - 1].is_whitespace() || "-({'\"".contains(chars[at - 1]);
        let first = *chars.get(at + 1)?;
        if !before_ok || first.is_whitespace() || first == marker {
            return None;
        }

        let mut newlines = 0;
        for end in at + 1..chars.len() {
            if chars[end] == '\n' {
                newlines += 1;
                if newlines > 1 {
                    return None;
                }
            }
            if chars[end] != marker || end == at + 1 {
                continue;
            }
            if chars[end - 1].is_whitespace() {
                continue;
            }
            let after_ok = chars
                .get(end + 1)
                .is_none_or(|c| c.is_whitespace() || "-.,;:!?'\")}[\\".contains(*c));
            if !after_ok {
                continue;
            }

            let inner: String = chars[at + 1..end].iter().collect();
            let inline = match marker {
                '=' => Inline::Verbatim(inner),
                '~' => Inline::Code(inner),
                _ => {
                    let children = self.markup(&inner, links);
                    match marker {
                        '*' => Inline::Bold(children),
                        '/' => Inline::Italic(children),
                        '_' => Inline::Underline(children),
                        _ => Inline::Strike(children),
                    }
                }
            };
            return Some((end + 1, inline));
        }
        None
    }
}

/// The `:END:` of a drawer opening at `at`, if the line opens one.
fn drawer_end(lines: &[&str], at: usize) -> Option<usize> {
    let name = lines[at].trim().strip_prefix(':')?.strip_suffix(':')?;
    if name.is_empty()
        || name.eq_ignore_ascii_case("END")
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    lines[at + 1..]
        .iter()
        .take_while(|line| headline_level(line).is_none())
        .position(|line| line.trim().eq_ignore_ascii_case(":END:"))
        .map(|offset| at + 1 + offset)
}

/// `#+begin_NAME params`, lower-casing the name.
fn block_begin(trimmed: &str) -> Option<(String, &str)> {
    let head = trimmed.get(..8)?;
    if !head.eq_ignore_ascii_case("#+begin_") {
        return None;
    }
    let rest = &trimmed[8..];
    let name_len = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let name = rest[..name_len].to_ascii_lowercase();
    (!name.is_empty()).then(|| (name, rest[name_len..].trim()))
}

fn block_end(lines: &[&str], at: usize, name: &str) -> Option<usize> {
    let closing = format!("#+end_{name}");
    lines[at + 1..]
        .iter()
        .position(|line| line.trim().eq_ignore_ascii_case(&closing))
        .map(|offset| at + 1 + offset)
}

fn header_arg<'a>(params: &'a str, key: &str) -> Option<&'a str> {
    let mut words = params.split_whitespace();
    while let Some(word) = words.next() {
        if word
            .strip_prefix(':')
            .is_some_and(|name| name.eq_ignore_ascii_case(key))
        {
            return words.next();
        }
    }
    None
}

/// A block's body as its code: shared indentation off, commas unescaped.
fn raw_block(lines: &[&str]) -> String {
    let indent = common_indent(lines);
    let mut body: Vec<String> = lines
        .iter()
        .map(|line| unescape(line.get(indent..).unwrap_or("")))
        .collect();
    body.push(String::new());
    body.join("\n")
}

/// The text of inlines, without markup.
fn plain(inlines: &[Inline]) -> String {
    inlines
        .iter()
        .map(|inline| match inline {
            Inline::Text(text) | Inline::Verbatim(text) | Inline::Code(text) => text.clone(),
            Inline::Bold(inner)
            | Inline::Italic(inner)
            | Inline::Underline(inner)
            | Inline::Strike(inner) => plain(inner),
            Inline::Link {
                description: Some(inner),
                ..
            } => plain(inner),
            Inline::Link { target, .. } => link_label(target),
            Inline::Footnote(_) => String::new(),
            Inline::LineBreak => " ".to_string(),
            Inline::Citation { cites, .. } => cites
                .iter()
                .map(|cite| cite.key.clone())
                .collect::<Vec<_>>()
                .join("; "),
        })
        .collect()
}

fn link_label(target: &LinkKind) -> String {
    match target {
        LinkKind::Url(url) => url.clone(),
        LinkKind::File { path, .. } => path.clone(),
        LinkKind::Mailto(who) => who.clone(),
        LinkKind::Headline(title) | LinkKind::Target(title) | LinkKind::Roam(title) => {
            title.clone()
        }
        LinkKind::CustomId(id) => id.clone(),
        LinkKind::Id(id) => id.to_string(),
        LinkKind::Other { scheme, rest } => format!("{scheme}:{rest}"),
    }
}

fn read(text: &str, source: &Path) -> Document {
    let settings = FileSettings::scan(text);
    let options = read_options(text);

    let mut keywords = HashMap::new();
    let mut macros = HashMap::new();
    for line in text.lines() {
        let Some(rest) = line.trim_start().strip_prefix("#+") else {
            continue;
        };
        let Some((key, value)) = rest.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        if key == "macro" {
            if let Some((name, body)) = value.trim().split_once(char::is_whitespace) {
                macros.insert(name.to_ascii_lowercase(), body.trim().to_string());
            } else if !value.trim().is_empty() {
                macros.insert(value.trim().to_ascii_lowercase(), String::new());
            }
        } else if !key.starts_with("begin_") && !key.starts_with("end_") {
            keywords.insert(key, value.trim().to_string());
        }
    }
    // Org's built-in macros, which a file's own definitions override.
    for (name, value) in [
        ("title", options.title.clone()),
        ("author", options.author.clone()),
        ("date", options.date.clone()),
        (
            "input-file",
            source
                .file_name()
                .map(|name| name.to_string_lossy().to_string()),
        ),
    ] {
        macros
            .entry(name.to_string())
            .or_insert(value.unwrap_or_default());
    }

    let mut reader = Reader {
        settings: &settings,
        footnotes: Vec::new(),
        anonymous: 0,
        exclude_tags: options.exclude_tags.clone(),
        warnings: Vec::new(),
        macros,
        keywords,
        cited: Vec::new(),
    };
    let lines: Vec<&str> = text.lines().collect();
    let elements = reader.elements(&lines, true);

    let mut anchors = HashMap::new();
    for element in &elements {
        if let Element::Heading(heading) = element {
            anchors
                .entry(plain(&heading.title))
                .or_insert_with(|| heading.anchor.clone());
        }
    }

    let mut warnings = reader.warnings;
    let dir = source.parent().unwrap_or(Path::new(""));
    let mut bib = Vec::new();
    for file in &options.bibliography {
        match std::fs::read_to_string(dir.join(file)) {
            Ok(text) => bib.extend(crate::bib::parse(&text)),
            Err(err) => warnings.push(format!("could not read the bibliography {file}: {err}")),
        }
    }
    for key in &reader.cited {
        if !bib.iter().any(|entry| &entry.key == key) {
            warnings.push(format!("@{key} is not in the bibliography"));
        }
    }

    Document {
        options,
        elements,
        footnotes: reader.footnotes,
        anchors,
        warnings,
        bib,
        cited: reader.cited,
    }
}

/// `a, b\, c` as `["a", "b, c"]`: commas separate arguments unless escaped.
fn split_macro_args(args: &str) -> Vec<String> {
    if args.trim().is_empty() {
        return Vec::new();
    }
    let mut out = vec![String::new()];
    let mut chars = args.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&',') => {
                out.last_mut().unwrap().push(',');
                chars.next();
            }
            ',' => out.push(String::new()),
            c => out.last_mut().unwrap().push(c),
        }
    }
    out.into_iter().map(|arg| arg.trim().to_string()).collect()
}

/// `#+INCLUDE:` lines replaced by what they include.
///
/// `#+INCLUDE: "file.org"` includes Org text, itself expanded; with `src
/// lang`, `example` or `export backend` after the file, it is wrapped in
/// that block. `:lines "5-10"` takes those lines (one-based, either end
/// open), and `:minlevel N` moves included headlines so the shallowest is
/// at level N.
fn expand_includes(text: &str, dir: &Path, depth: usize, warnings: &mut Vec<String>) -> String {
    if depth > 8 {
        warnings.push("#+INCLUDE: nested too deep; stopped".to_string());
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let Some(value) = keyword(line, "include") else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        let (file, rest) = match value.strip_prefix('"') {
            Some(quoted) => match quoted.split_once('"') {
                Some((file, rest)) => (file, rest),
                None => (quoted, ""),
            },
            None => value.split_once(char::is_whitespace).unwrap_or((value, "")),
        };
        let path = dir.join(file);
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(err) => {
                warnings.push(format!("could not include {file}: {err}"));
                continue;
            }
        };

        let words: Vec<&str> = rest.split_whitespace().collect();
        let option = |name: &str| {
            words
                .iter()
                .position(|word| *word == name)
                .and_then(|at| words.get(at + 1))
                .map(|value| value.trim_matches('"').to_string())
        };
        let mut lines: Vec<&str> = content.lines().collect();
        if let Some(range) = option(":lines") {
            let (from, to) = range
                .split_once('-')
                .unwrap_or((range.as_str(), range.as_str()));
            let from = from.parse::<usize>().unwrap_or(1).max(1) - 1;
            let to = to.parse::<usize>().unwrap_or(lines.len()).min(lines.len());
            lines = lines.get(from..to.max(from)).unwrap_or(&[]).to_vec();
        }
        let body = lines.join("\n");

        let block = words.first().filter(|word| !word.starts_with(':'));
        let escaped = || {
            body.lines()
                .map(|line| {
                    let trimmed = line.trim_start();
                    if trimmed.starts_with('*') || trimmed.starts_with("#+") {
                        format!(",{line}")
                    } else {
                        line.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        match block.map(|word| word.to_ascii_lowercase()).as_deref() {
            Some("src") => {
                let language = words
                    .get(1)
                    .filter(|word| !word.starts_with(':'))
                    .unwrap_or(&"");
                out.push_str(&format!(
                    "#+begin_src {language}\n{}\n#+end_src\n",
                    escaped()
                ));
            }
            Some("example") => {
                out.push_str(&format!("#+begin_example\n{}\n#+end_example\n", escaped()))
            }
            Some("export") => {
                let backend = words.get(1).unwrap_or(&"");
                out.push_str(&format!("#+begin_export {backend}\n{body}\n#+end_export\n"));
            }
            _ => {
                let nested_dir = path.parent().unwrap_or(dir).to_path_buf();
                let mut included = expand_includes(&body, &nested_dir, depth + 1, warnings);
                if let Some(level) =
                    option(":minlevel").and_then(|level| level.parse::<usize>().ok())
                {
                    included = shift_headlines(&included, level);
                }
                out.push_str(&included);
                if !included.ends_with('\n') {
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// Moves every headline so the shallowest one is at `level`.
fn shift_headlines(text: &str, level: usize) -> String {
    let shallowest = text.lines().filter_map(headline_level).min();
    let Some(shallowest) = shallowest else {
        return text.to_string();
    };
    text.lines()
        .map(|line| match headline_level(line) {
            Some(stars) => {
                let wanted = (stars + level).saturating_sub(shallowest).max(1);
                format!("{}{}", "*".repeat(wanted), &line[stars..])
            }
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

// ── Links ──────────────────────────────────────────────────────────────────

/// `to` relative to the directory `from_dir`, with `/` separators.
fn relative(from_dir: &Path, to: &Path) -> String {
    let from: Vec<Component> = from_dir.components().collect();
    let to_parts: Vec<Component> = to.components().collect();
    let common = from
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    if common == 0 && from_dir.is_absolute() != to.is_absolute() {
        return to.to_string_lossy().replace('\\', "/");
    }

    let mut parts: Vec<String> = from[common..].iter().map(|_| "..".to_string()).collect();
    parts.extend(
        to_parts[common..]
            .iter()
            .map(|part| part.as_os_str().to_string_lossy().to_string()),
    );
    parts.join("/")
}

const IMAGE_EXTENSIONS: [&str; 7] = ["png", "jpg", "jpeg", "gif", "svg", "webp", "bmp"];

/// Where a link goes in the exported file.
enum Href {
    Link(String),
    Image(String),
    /// Nothing to link to: the text stays, the link does not.
    None,
}

struct Context<'a> {
    source: &'a Path,
    backend: Backend,
    resolve: &'a dyn Resolve,
    anchors: &'a HashMap<String, String>,
    /// Footnote labels in order of first reference, which numbers them.
    footnote_order: Vec<String>,
    bib: &'a [crate::bib::Entry],
    /// The cited entries' keys and bibliography lines, in the order the
    /// bibliography lists them.
    references: Vec<(String, String)>,
    /// `basic`, `biblatex` or `natbib`.
    processor: String,
    warnings: Vec<String>,
}

impl Context<'_> {
    /// A citation as the `basic` processor writes it: author and year.
    ///
    /// `wrap` decorates each reference, which is how HTML makes it a link
    /// to the bibliography.
    fn basic_citation(
        &mut self,
        style: Option<&str>,
        cites: &[Cite],
        prefix: &str,
        suffix: &str,
        wrap: &dyn Fn(&str, String) -> String,
    ) -> String {
        let style = style.unwrap_or("").split('/').next().unwrap_or("");
        if style == "nocite" || style == "n" {
            return String::new();
        }
        let parts: Vec<String> = cites
            .iter()
            .map(|cite| {
                let (authors, year) = match self.bib.iter().find(|entry| entry.key == cite.key) {
                    Some(entry) => (entry.short_authors(), entry.year().unwrap_or_default()),
                    None => (cite.key.clone(), "?".to_string()),
                };
                let body = match style {
                    "t" | "text" => format!("{authors} ({year}{})", with_comma(&cite.suffix)),
                    "a" | "author" => authors,
                    "na" | "noauthor" => format!("{year}{}", with_comma(&cite.suffix)),
                    _ => format!("{authors}, {year}{}", with_comma(&cite.suffix)),
                };
                let body = if cite.prefix.is_empty() {
                    body
                } else {
                    format!("{} {body}", cite.prefix)
                };
                wrap(&cite.key, body)
            })
            .collect();
        let joined = parts.join("; ");
        let framed = match style {
            "t" | "text" | "a" | "author" => joined,
            _ => format!("({joined})"),
        };
        [prefix, framed.as_str(), suffix]
            .iter()
            .filter(|part| !part.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn href(&mut self, target: &LinkKind, described: bool) -> Href {
        let dir = self.source.parent().unwrap_or(Path::new(""));
        let ext = self.backend.link_extension();
        match target {
            LinkKind::Url(url) => {
                let is_image = !described
                    && url.rsplit('.').next().is_some_and(|e| {
                        IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str())
                    });
                if is_image {
                    Href::Image(url.clone())
                } else {
                    Href::Link(url.clone())
                }
            }
            LinkKind::Mailto(who) => Href::Link(format!("mailto:{who}")),
            LinkKind::File { path, search } => {
                let extension = Path::new(path)
                    .extension()
                    .map(|e| e.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                if !described && IMAGE_EXTENSIONS.contains(&extension.as_str()) {
                    return Href::Image(path.clone());
                }
                let mut href = if extension == "org" {
                    Path::new(path)
                        .with_extension(ext)
                        .to_string_lossy()
                        .to_string()
                } else {
                    path.clone()
                };
                if let Some(FileSearch::Headline(title)) = search {
                    href.push('#');
                    href.push_str(&anchor_for(title, None));
                }
                Href::Link(href)
            }
            LinkKind::Headline(title) | LinkKind::Target(title) => Href::Link(format!(
                "#{}",
                self.anchors
                    .get(title)
                    .cloned()
                    .unwrap_or_else(|| anchor_for(title, None))
            )),
            LinkKind::CustomId(id) => Href::Link(format!("#{id}")),
            LinkKind::Id(id) => match self.resolve.id(id) {
                Some(target) => {
                    let anchor = target.anchor.map(|a| format!("#{a}")).unwrap_or_default();
                    if target.file == self.source {
                        if anchor.is_empty() {
                            Href::Link("#".to_string())
                        } else {
                            Href::Link(anchor)
                        }
                    } else {
                        let file = target.file.with_extension(ext);
                        Href::Link(format!("{}{anchor}", relative(dir, &file)))
                    }
                }
                None => {
                    self.warnings
                        .push(format!("id:{id} is not in the index; its link was dropped"));
                    Href::None
                }
            },
            LinkKind::Roam(title) => {
                self.warnings
                    .push(format!("roam:{title} is a v1 link; its link was dropped"));
                Href::None
            }
            LinkKind::Other { scheme, rest } => Href::Link(format!("{scheme}:{rest}")),
        }
    }

    /// The label an `id:` link without a description shows: the node's
    /// title rather than its UUID.
    fn default_label(&self, target: &LinkKind) -> String {
        if let LinkKind::Id(id) = target {
            if let Some(found) = self.resolve.id(id) {
                return found.title;
            }
        }
        link_label(target)
    }

    fn footnote_number(&mut self, label: &str) -> usize {
        match self.footnote_order.iter().position(|seen| seen == label) {
            Some(index) => index + 1,
            None => {
                self.footnote_order.push(label.to_string());
                self.footnote_order.len()
            }
        }
    }
}

// ── Writing ────────────────────────────────────────────────────────────────

/// Exports `text`, the content of the Org file at `source`.
pub fn export(text: &str, source: &Path, backend: Backend, resolve: &dyn Resolve) -> Exported {
    let mut include_warnings = Vec::new();
    let dir = source.parent().unwrap_or(Path::new(""));
    let expanded = expand_includes(text, dir, 0, &mut include_warnings);
    let mut document = read(&expanded, source);
    document.warnings.splice(0..0, include_warnings);
    let mut context = Context {
        source,
        backend,
        resolve,
        anchors: &document.anchors,
        footnote_order: Vec::new(),
        bib: &document.bib,
        references: cited_entries(&document)
            .into_iter()
            .map(|entry| (entry.key.clone(), entry.reference()))
            .collect(),
        processor: document.options.cite_export.clone(),
        warnings: document.warnings.clone(),
    };

    let content = match backend {
        Backend::Markdown => markdown::document(&document, &mut context),
        Backend::Html => html::document(&document, &mut context),
        Backend::Latex => latex::document(&document, &mut context),
    };

    let mut warnings = context.warnings;
    warnings.dedup();
    Exported { content, warnings }
}

/// `, p. 5` for a non-empty suffix, nothing for an empty one.
fn with_comma(suffix: &str) -> String {
    if suffix.is_empty() {
        String::new()
    } else {
        format!(", {suffix}")
    }
}

/// The cited entries, sorted as a bibliography lists them.
fn cited_entries(document: &Document) -> Vec<&crate::bib::Entry> {
    let mut entries: Vec<&crate::bib::Entry> = document
        .bib
        .iter()
        .filter(|entry| document.cited.contains(&entry.key))
        .collect();
    entries.sort_by_key(|entry| entry.reference());
    entries
}

/// Headings in order, with their numbers when the options number them.
fn numbered(document: &Document) -> Vec<(&Heading, String)> {
    let mut counters: Vec<usize> = Vec::new();
    document
        .elements
        .iter()
        .filter_map(|element| match element {
            Element::Heading(heading) => Some(heading),
            _ => None,
        })
        .map(|heading| {
            counters.truncate(heading.level);
            while counters.len() < heading.level {
                counters.push(0);
            }
            counters[heading.level - 1] += 1;
            let number = counters
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(".");
            (heading, number)
        })
        .collect()
}

fn footnote_definition<'a>(document: &'a Document, label: &str) -> Option<&'a [Inline]> {
    document
        .footnotes
        .iter()
        .find(|(name, _)| name == label)
        .map(|(_, inlines)| inlines.as_slice())
}

mod markdown {
    use super::*;

    fn escape(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for c in text.chars() {
            if matches!(c, '\\' | '`' | '*' | '_' | '[' | ']' | '<') {
                out.push('\\');
            }
            out.push(c);
        }
        out
    }

    fn code_span(text: &str) -> String {
        let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
        let fence = "`".repeat(longest + 1);
        if text.starts_with('`') || text.ends_with('`') {
            format!("{fence} {text} {fence}")
        } else {
            format!("{fence}{text}{fence}")
        }
    }

    pub(super) fn inlines(items: &[Inline], cx: &mut Context) -> String {
        items.iter().map(|inline| one(inline, cx)).collect()
    }

    fn one(inline: &Inline, cx: &mut Context) -> String {
        match inline {
            Inline::Text(text) => escape(text),
            Inline::Bold(inner) => format!("**{}**", inlines(inner, cx)),
            Inline::Italic(inner) => format!("*{}*", inlines(inner, cx)),
            Inline::Underline(inner) => format!("<u>{}</u>", inlines(inner, cx)),
            Inline::Strike(inner) => format!("~~{}~~", inlines(inner, cx)),
            Inline::Verbatim(text) | Inline::Code(text) => code_span(text),
            Inline::Link {
                target,
                description,
            } => {
                let label = match description {
                    Some(inner) => inlines(inner, cx),
                    None => escape(&cx.default_label(target)),
                };
                match cx.href(target, description.is_some()) {
                    Href::Link(href) => format!("[{label}]({})", href.replace(' ', "%20")),
                    Href::Image(src) => format!("![]({})", src.replace(' ', "%20")),
                    Href::None => label,
                }
            }
            Inline::Footnote(label) => format!("[^{}]", cx.footnote_number(label)),
            Inline::LineBreak => "\\\n".to_string(),
            Inline::Citation {
                style,
                cites,
                prefix,
                suffix,
            } => {
                let text =
                    cx.basic_citation(style.as_deref(), cites, prefix, suffix, &|_, body| body);
                escape(&text)
            }
        }
    }

    fn fence(code: &str) -> String {
        let longest = code
            .lines()
            .map(|line| line.chars().take_while(|c| *c == '`').count())
            .max()
            .unwrap_or(0);
        "`".repeat(longest.max(2) + 1)
    }

    fn indent(text: &str, by: usize) -> String {
        let pad = " ".repeat(by);
        text.lines()
            .map(|line| {
                if line.is_empty() {
                    String::new()
                } else {
                    format!("{pad}{line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(super) fn elements(elements: &[Element], shift: usize, cx: &mut Context) -> Vec<String> {
        elements
            .iter()
            .map(|element| element_md(element, shift, cx))
            .filter(|s| !s.is_empty())
            .collect()
    }

    fn element_md(element: &Element, shift: usize, cx: &mut Context) -> String {
        match element {
            Element::Heading(heading) => {
                let level = (heading.level + shift).min(6);
                let mut text = String::new();
                if let Some((keyword, _)) = &heading.todo {
                    text.push_str(keyword);
                    text.push(' ');
                }
                let shown = format!("{text}{}", plain(&heading.title));
                text.push_str(&inlines(&heading.title, cx));
                // A renderer makes its own anchor from the text it shows; where
                // that is not the anchor links use — a custom id, or a TODO
                // keyword in front of the title — say which one it is.
                let anchor = if heading.custom || anchor_for(&shown, None) != heading.anchor {
                    format!("<a id=\"{}\"></a>\n", heading.anchor)
                } else {
                    String::new()
                };
                format!("{anchor}{} {text}", "#".repeat(level))
            }
            Element::Paragraph(items) => inlines(items, cx),
            Element::List(kind, items) => {
                let mut out = Vec::new();
                for (index, item) in items.iter().enumerate() {
                    let marker = match kind {
                        ListKind::Ordered => format!("{}.", index + 1),
                        _ => "-".to_string(),
                    };
                    let mut head = String::new();
                    if let Some(mark) = item.checkbox {
                        head.push_str(if mark == ' ' { "[ ] " } else { "[x] " });
                    }
                    if let Some(term) = &item.term {
                        head.push_str(&format!("**{}**: ", inlines(term, cx)));
                    }
                    // A sublist follows its item's text directly, which keeps
                    // a tight list tight; anything else is its own paragraph.
                    let mut body = String::new();
                    for (index, element) in item.content.iter().enumerate() {
                        let rendered = element_md(element, shift, cx);
                        if index > 0 {
                            body.push_str(if matches!(element, Element::List(..)) {
                                "\n"
                            } else {
                                "\n\n"
                            });
                        }
                        body.push_str(&rendered);
                    }
                    let width = marker.len() + 1;
                    let text = format!("{head}{body}");
                    let mut lines = text.lines();
                    let first = lines.next().unwrap_or("");
                    let rest: Vec<&str> = lines.collect();
                    let mut entry = format!("{marker} {first}");
                    if !rest.is_empty() {
                        entry.push('\n');
                        entry.push_str(&indent(&rest.join("\n"), width));
                    }
                    out.push(entry);
                }
                out.join("\n")
            }
            Element::Table(rows, header) => {
                if rows.is_empty() {
                    return String::new();
                }
                let width = rows.iter().map(Vec::len).max().unwrap_or(0);
                let render = |row: &Vec<Vec<Inline>>, cx: &mut Context| {
                    let mut cells: Vec<String> = row
                        .iter()
                        .map(|cell| inlines(cell, cx).replace('|', "\\|"))
                        .collect();
                    cells.resize(width, String::new());
                    format!("| {} |", cells.join(" | "))
                };
                // Markdown tables must have a header; a table without one
                // uses its first row. Of several header rows, the last one
                // names the columns.
                let header_at = header.saturating_sub(1);
                let mut out = vec![render(&rows[header_at], cx)];
                out.push(format!("|{}|", vec!["---"; width].join("|")));
                for row in &rows[header_at + 1..] {
                    out.push(render(row, cx));
                }
                out.join("\n")
            }
            Element::Code { language, code } => {
                let fence = fence(code);
                format!(
                    "{fence}{}\n{}{fence}",
                    language.as_deref().unwrap_or(""),
                    code
                )
            }
            Element::Example(text) => {
                let fence = fence(text);
                format!("{fence}\n{text}{fence}")
            }
            Element::Quote(inner) => elements(inner, shift, cx)
                .join("\n\n")
                .lines()
                .map(|line| {
                    if line.is_empty() {
                        ">".to_string()
                    } else {
                        format!("> {line}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Element::Verse(lines) => lines
                .iter()
                .map(|line| inlines(line, cx))
                .collect::<Vec<_>>()
                .join("\\\n"),
            Element::Center(inner) | Element::Special(_, inner) => {
                elements(inner, shift, cx).join("\n\n")
            }
            Element::Export { backend, text } => {
                if backend == "md" || backend == "markdown" {
                    text.clone()
                } else {
                    String::new()
                }
            }
            Element::Rule => "---".to_string(),
            Element::Bibliography => cx
                .references
                .iter()
                .map(|(_, reference)| format!("- {}", escape(reference)))
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    pub(super) fn document(document: &Document, cx: &mut Context) -> String {
        let mut parts = Vec::new();
        // The title is the one first-level heading, so the outline moves
        // down a level under it.
        let shift = usize::from(document.options.title.is_some());
        if let Some(title) = &document.options.title {
            parts.push(format!("# {}", escape(title)));
        }
        let options = &document.options;
        let body: Vec<Element> = document
            .elements
            .iter()
            .map(|element| match element {
                Element::Heading(heading) if !options.todo => Element::Heading(Heading {
                    todo: None,
                    ..heading.clone()
                }),
                other => other.clone(),
            })
            .collect();

        if let Some(depth) = options.toc {
            let toc: Vec<String> = numbered(document)
                .into_iter()
                .filter(|(heading, _)| heading.level <= depth)
                .map(|(heading, _)| {
                    let title = inlines(&heading.title, cx);
                    format!(
                        "{}- [{title}](#{})",
                        "  ".repeat(heading.level - 1),
                        heading.anchor
                    )
                })
                .collect();
            if !toc.is_empty() {
                parts.push(toc.join("\n"));
            }
        }

        parts.extend(elements(&body, shift, cx));

        if !cx.footnote_order.is_empty() {
            let mut notes = Vec::new();
            let mut index = 0;
            while index < cx.footnote_order.len() {
                let label = cx.footnote_order[index].clone();
                let body = footnote_definition(document, &label)
                    .map(|inline| inlines(inline, cx))
                    .unwrap_or_default();
                notes.push(format!("[^{}]: {body}", index + 1));
                index += 1;
            }
            parts.push(notes.join("\n"));
        }

        let mut out = parts.join("\n\n");
        out.push('\n');
        out
    }
}

mod html {
    use super::*;

    pub(super) fn escape(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    pub(super) fn inlines(items: &[Inline], cx: &mut Context) -> String {
        items.iter().map(|inline| one(inline, cx)).collect()
    }

    fn one(inline: &Inline, cx: &mut Context) -> String {
        match inline {
            Inline::Text(text) => escape(text),
            Inline::Bold(inner) => format!("<b>{}</b>", inlines(inner, cx)),
            Inline::Italic(inner) => format!("<i>{}</i>", inlines(inner, cx)),
            Inline::Underline(inner) => {
                format!("<span class=\"underline\">{}</span>", inlines(inner, cx))
            }
            Inline::Strike(inner) => format!("<del>{}</del>", inlines(inner, cx)),
            Inline::Verbatim(text) | Inline::Code(text) => format!("<code>{}</code>", escape(text)),
            Inline::Link {
                target,
                description,
            } => {
                let label = match description {
                    Some(inner) => inlines(inner, cx),
                    None => escape(&cx.default_label(target)),
                };
                match cx.href(target, description.is_some()) {
                    Href::Link(href) => format!("<a href=\"{}\">{label}</a>", escape(&href)),
                    Href::Image(src) => {
                        format!("<img src=\"{}\" alt=\"{}\">", escape(&src), escape(&src))
                    }
                    Href::None => label,
                }
            }
            Inline::Footnote(label) => {
                let n = cx.footnote_number(label);
                format!("<sup><a id=\"fnr.{n}\" class=\"footref\" href=\"#fn.{n}\" role=\"doc-backlink\">{n}</a></sup>")
            }
            Inline::LineBreak => "<br>\n".to_string(),
            Inline::Citation {
                style,
                cites,
                prefix,
                suffix,
            } => {
                // Linked to the bibliography when one is printed.
                let wrap = |key: &str, body: String| {
                    format!("<a href=\"#bib-{}\">{}</a>", escape(key), escape(&body))
                };
                cx.basic_citation(
                    style.as_deref(),
                    cites,
                    &escape(prefix),
                    &escape(suffix),
                    &wrap,
                )
            }
        }
    }

    fn checkbox(mark: char) -> &'static str {
        match mark {
            'X' => "<code>[X]</code> ",
            '-' => "<code>[-]</code> ",
            _ => "<code>[&#xa0;]</code> ",
        }
    }

    /// An item's content, without a paragraph wrapper when it is one line of
    /// text — which is what keeps a tight list tight.
    fn item_body(
        content: &[Element],
        numbers: &HashMap<String, String>,
        cx: &mut Context,
    ) -> String {
        match content {
            [Element::Paragraph(items)] => inlines(items, cx),
            _ => elements(content, numbers, cx),
        }
    }

    pub(super) fn elements(
        elements_: &[Element],
        numbers: &HashMap<String, String>,
        cx: &mut Context,
    ) -> String {
        elements_
            .iter()
            .map(|element| element_html(element, numbers, cx))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn element_html(
        element: &Element,
        numbers: &HashMap<String, String>,
        cx: &mut Context,
    ) -> String {
        match element {
            Element::Heading(heading) => {
                let level = (heading.level + 1).min(6);
                let mut text = String::new();
                if let Some(number) = numbers.get(&heading.anchor) {
                    text.push_str(&format!(
                        "<span class=\"section-number-{level}\">{number}</span> "
                    ));
                }
                if let Some((keyword, done)) = &heading.todo {
                    let class = if *done { "done" } else { "todo" };
                    text.push_str(&format!(
                        "<span class=\"{class} {keyword}\">{keyword}</span> "
                    ));
                }
                if let Some(priority) = heading.priority {
                    text.push_str(&format!("<span class=\"priority\">[{priority}]</span> "));
                }
                text.push_str(&inlines(&heading.title, cx));
                if !heading.tags.is_empty() {
                    let tags: Vec<String> = heading
                        .tags
                        .iter()
                        .map(|tag| format!("<span class=\"{0}\">{0}</span>", escape(tag)))
                        .collect();
                    text.push_str(&format!(
                        "&#xa0;&#xa0;&#xa0;<span class=\"tag\">{}</span>",
                        tags.join("&#xa0;")
                    ));
                }
                format!(
                    "<h{level} id=\"{}\">{text}</h{level}>",
                    escape(&heading.anchor)
                )
            }
            Element::Paragraph(items) => format!("<p>\n{}\n</p>", inlines(items, cx)),
            Element::List(kind, items) => {
                let (open, close) = match kind {
                    ListKind::Unordered => ("<ul class=\"org-ul\">", "</ul>"),
                    ListKind::Ordered => ("<ol class=\"org-ol\">", "</ol>"),
                    ListKind::Description => ("<dl class=\"org-dl\">", "</dl>"),
                };
                let mut out = vec![open.to_string()];
                for item in items {
                    let mark = item.checkbox.map(checkbox).unwrap_or("");
                    let body = item_body(&item.content, numbers, cx);
                    match (kind, &item.term) {
                        (ListKind::Description, Some(term)) => {
                            out.push(format!(
                                "<dt>{mark}{}</dt><dd>{body}</dd>",
                                inlines(term, cx)
                            ));
                        }
                        _ => out.push(format!("<li>{mark}{body}</li>")),
                    }
                }
                out.push(close.to_string());
                out.join("\n")
            }
            Element::Table(rows, header) => {
                let mut out = vec![
                    "<table border=\"2\" cellspacing=\"0\" cellpadding=\"6\" rules=\"groups\" frame=\"hsides\">"
                        .to_string(),
                ];
                if *header > 0 {
                    out.push("<thead>".to_string());
                    for row in &rows[..*header] {
                        let cells: Vec<String> = row
                            .iter()
                            .map(|c| format!("<th scope=\"col\">{}</th>", inlines(c, cx)))
                            .collect();
                        out.push(format!("<tr>{}</tr>", cells.join("")));
                    }
                    out.push("</thead>".to_string());
                }
                out.push("<tbody>".to_string());
                for row in &rows[*header..] {
                    let cells: Vec<String> = row
                        .iter()
                        .map(|c| format!("<td>{}</td>", inlines(c, cx)))
                        .collect();
                    out.push(format!("<tr>{}</tr>", cells.join("")));
                }
                out.push("</tbody>".to_string());
                out.push("</table>".to_string());
                out.join("\n")
            }
            Element::Code { language, code } => format!(
                "<div class=\"org-src-container\">\n<pre class=\"src src-{0}\">{1}</pre>\n</div>",
                escape(language.as_deref().unwrap_or("text")),
                escape(code)
            ),
            Element::Example(text) => format!("<pre class=\"example\">\n{}</pre>", escape(text)),
            Element::Quote(inner) => format!(
                "<blockquote>\n{}\n</blockquote>",
                elements(inner, numbers, cx)
            ),
            Element::Verse(lines) => format!(
                "<p class=\"verse\">\n{}\n</p>",
                lines
                    .iter()
                    .map(|line| inlines(line, cx))
                    .collect::<Vec<_>>()
                    .join("<br>\n")
            ),
            Element::Center(inner) => format!(
                "<div class=\"org-center\">\n{}\n</div>",
                elements(inner, numbers, cx)
            ),
            Element::Special(name, inner) => format!(
                "<div class=\"{}\">\n{}\n</div>",
                escape(name),
                elements(inner, numbers, cx)
            ),
            Element::Export { backend, text } => {
                if backend == "html" {
                    text.clone()
                } else {
                    String::new()
                }
            }
            Element::Rule => "<hr>".to_string(),
            Element::Bibliography => {
                let mut out = vec!["<div class=\"bibliography\">".to_string()];
                for (key, reference) in &cx.references {
                    out.push(format!(
                        "<p class=\"bib-entry\" id=\"bib-{}\">{}</p>",
                        escape(key),
                        escape(reference)
                    ));
                }
                out.push("</div>".to_string());
                out.join("\n")
            }
        }
    }

    const STYLE: &str = "body { max-width: 50em; margin: 2em auto; padding: 0 1em; font-family: sans-serif; line-height: 1.5; }
.title { text-align: center; }
.todo { color: #c00; font-weight: bold; }
.done { color: #080; font-weight: bold; }
.tag { float: right; font-size: 80%; background: #eee; padding: 0 .3em; }
.underline { text-decoration: underline; }
pre { background: #f6f6f6; padding: .6em; overflow: auto; }
table { border-collapse: collapse; }
td, th { padding: .2em .6em; }
.verse { white-space: pre-line; }
.org-center { text-align: center; }";

    pub(super) fn document(document: &Document, cx: &mut Context) -> String {
        let options = &document.options;
        let title = options.title.clone().unwrap_or_else(|| {
            cx.source
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        });

        let mut numbers = HashMap::new();
        if options.num {
            for (heading, number) in numbered(document) {
                numbers.insert(heading.anchor.clone(), number);
            }
        }
        let body: Vec<Element> = document
            .elements
            .iter()
            .map(|element| match element {
                Element::Heading(heading) => Element::Heading(Heading {
                    todo: heading.todo.clone().filter(|_| options.todo),
                    priority: heading.priority.filter(|_| options.priority),
                    tags: if options.tags {
                        heading.tags.clone()
                    } else {
                        Vec::new()
                    },
                    ..heading.clone()
                }),
                other => other.clone(),
            })
            .collect();

        let mut out = vec![
            "<!DOCTYPE html>".to_string(),
            "<html lang=\"en\">".to_string(),
            "<head>".to_string(),
            "<meta charset=\"utf-8\">".to_string(),
            "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">".to_string(),
            format!("<title>{}</title>", escape(&title)),
        ];
        if let Some(author) = &options.author {
            out.push(format!(
                "<meta name=\"author\" content=\"{}\">",
                escape(author)
            ));
        }
        out.push(format!("<style>\n{STYLE}\n</style>"));
        out.push("</head>".to_string());
        out.push("<body>".to_string());
        out.push("<div id=\"content\" class=\"content\">".to_string());
        out.push(format!("<h1 class=\"title\">{}</h1>", escape(&title)));

        if let Some(depth) = options.toc {
            let entries: Vec<(&Heading, String)> = numbered(document)
                .into_iter()
                .filter(|(heading, _)| heading.level <= depth)
                .collect();
            if !entries.is_empty() {
                out.push("<div id=\"table-of-contents\" role=\"doc-toc\">".to_string());
                out.push("<h2>Table of Contents</h2>".to_string());
                out.push("<div id=\"text-table-of-contents\" role=\"doc-toc\">".to_string());
                let mut depth_now = 0;
                for (heading, number) in entries {
                    while depth_now < heading.level {
                        out.push("<ul>".to_string());
                        depth_now += 1;
                    }
                    while depth_now > heading.level {
                        out.push("</ul>".to_string());
                        depth_now -= 1;
                    }
                    let number = if options.num {
                        format!("{number}. ")
                    } else {
                        String::new()
                    };
                    out.push(format!(
                        "<li><a href=\"#{}\">{number}{}</a></li>",
                        escape(&heading.anchor),
                        inlines(&heading.title, cx)
                    ));
                }
                while depth_now > 0 {
                    out.push("</ul>".to_string());
                    depth_now -= 1;
                }
                out.push("</div>".to_string());
                out.push("</div>".to_string());
            }
        }

        out.push(elements(&body, &numbers, cx));

        if !cx.footnote_order.is_empty() {
            out.push("<div id=\"footnotes\">".to_string());
            out.push("<h2 class=\"footnotes\">Footnotes: </h2>".to_string());
            let mut index = 0;
            while index < cx.footnote_order.len() {
                let label = cx.footnote_order[index].clone();
                let n = index + 1;
                let body = footnote_definition(document, &label)
                    .map(|inline| inlines(inline, cx))
                    .unwrap_or_default();
                out.push(format!(
                    "<div class=\"footdef\"><sup><a id=\"fn.{n}\" class=\"footnum\" href=\"#fnr.{n}\" role=\"doc-backlink\">{n}</a></sup> <div class=\"footpara\" role=\"doc-footnote\"><p class=\"footpara\">{body}</p></div></div>"
                ));
                index += 1;
            }
            out.push("</div>".to_string());
        }

        out.push("</div>".to_string());
        if options.author.is_some() || options.date.is_some() {
            out.push("<div id=\"postamble\" class=\"status\">".to_string());
            if let Some(author) = &options.author {
                out.push(format!(
                    "<p class=\"author\">Author: {}</p>",
                    escape(author)
                ));
            }
            if let Some(date) = &options.date {
                out.push(format!("<p class=\"date\">Date: {}</p>", escape(date)));
            }
            out.push("</div>".to_string());
        }
        out.push("</body>".to_string());
        out.push("</html>".to_string());

        let mut text = out.join("\n");
        text.push('\n');
        text
    }
}

mod latex {
    use super::*;

    pub(super) fn escape(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for c in text.chars() {
            match c {
                '\\' => out.push_str("\\textbackslash{}"),
                '{' | '}' | '$' | '&' | '#' | '_' | '%' => {
                    out.push('\\');
                    out.push(c);
                }
                '~' => out.push_str("\\textasciitilde{}"),
                '^' => out.push_str("\\textasciicircum{}"),
                c => out.push(c),
            }
        }
        out
    }

    pub(super) fn inlines(items: &[Inline], document: &Document, cx: &mut Context) -> String {
        items
            .iter()
            .map(|inline| one(inline, document, cx))
            .collect()
    }

    fn one(inline: &Inline, document: &Document, cx: &mut Context) -> String {
        match inline {
            Inline::Text(text) => escape(text),
            Inline::Bold(inner) => format!("\\textbf{{{}}}", inlines(inner, document, cx)),
            Inline::Italic(inner) => format!("\\emph{{{}}}", inlines(inner, document, cx)),
            Inline::Underline(inner) => format!("\\uline{{{}}}", inlines(inner, document, cx)),
            Inline::Strike(inner) => format!("\\sout{{{}}}", inlines(inner, document, cx)),
            Inline::Verbatim(text) | Inline::Code(text) => format!("\\texttt{{{}}}", escape(text)),
            Inline::Link {
                target,
                description,
            } => {
                let label = match description {
                    Some(inner) => inlines(inner, document, cx),
                    None => escape(&cx.default_label(target)),
                };
                match cx.href(target, description.is_some()) {
                    Href::Link(href) if href.starts_with('#') => {
                        format!("\\hyperref[{}]{{{label}}}", &href[1..])
                    }
                    Href::Link(href) => format!("\\href{{{}}}{{{label}}}", href.replace('%', "\\%").replace('#', "\\#")),
                    Href::Image(src) => format!(
                        "\\begin{{center}}\n\\includegraphics[width=.9\\linewidth]{{{src}}}\n\\end{{center}}"
                    ),
                    Href::None => label,
                }
            }
            // LaTeX places the note itself; there is nothing to number.
            Inline::Footnote(label) => {
                let body = footnote_definition(document, label)
                    .map(|inline| inlines(inline, document, cx))
                    .unwrap_or_default();
                format!("\\footnote{{{body}}}")
            }
            Inline::LineBreak => "\\\\\n".to_string(),
            Inline::Citation {
                style,
                cites,
                prefix,
                suffix,
            } => {
                let keys: Vec<&str> = cites.iter().map(|cite| cite.key.as_str()).collect();
                let style = style
                    .as_deref()
                    .unwrap_or("")
                    .split('/')
                    .next()
                    .unwrap_or("");
                let command = match (cx.processor.as_str(), style) {
                    (_, "nocite" | "n") if cx.processor != "basic" => "nocite",
                    ("biblatex", "t" | "text") => "textcite",
                    ("biblatex", "a" | "author") | ("natbib", "a" | "author") => "citeauthor",
                    ("biblatex", "na" | "noauthor") | ("natbib", "na" | "noauthor") => "citeyear",
                    ("biblatex", _) => "autocite",
                    ("natbib", "t" | "text") => "citet",
                    ("natbib", _) => "citep",
                    _ => {
                        let text = cx.basic_citation(
                            Some(style).filter(|s| !s.is_empty()),
                            cites,
                            prefix,
                            suffix,
                            &|_, body| body,
                        );
                        return escape(&text);
                    }
                };
                // One reference's own prefix and suffix are the command's
                // optional arguments; with several, the citation's own.
                let (pre, post) = match cites.as_slice() {
                    [one] => (
                        [prefix.as_str(), one.prefix.as_str()]
                            .iter()
                            .filter(|p| !p.is_empty())
                            .copied()
                            .collect::<Vec<_>>()
                            .join(" "),
                        [one.suffix.as_str(), suffix.as_str()]
                            .iter()
                            .filter(|p| !p.is_empty())
                            .copied()
                            .collect::<Vec<_>>()
                            .join(" "),
                    ),
                    _ => (prefix.clone(), suffix.clone()),
                };
                let options = match (pre.is_empty(), post.is_empty()) {
                    (true, true) => String::new(),
                    (true, false) => format!("[{}]", escape(&post)),
                    (false, _) => format!("[{}][{}]", escape(&pre), escape(&post)),
                };
                format!("\\{command}{options}{{{}}}", keys.join(","))
            }
        }
    }

    fn checkbox(mark: char) -> &'static str {
        match mark {
            'X' => "[{$\\boxtimes$}] ",
            '-' => "[{$\\boxminus$}] ",
            _ => "[{$\\square$}] ",
        }
    }

    pub(super) fn elements(elements_: &[Element], document: &Document, cx: &mut Context) -> String {
        elements_
            .iter()
            .map(|element| element_tex(element, document, cx))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn element_tex(element: &Element, document: &Document, cx: &mut Context) -> String {
        let options = &document.options;
        match element {
            Element::Heading(heading) => {
                let command = match heading.level {
                    1 => "section",
                    2 => "subsection",
                    3 => "subsubsection",
                    4 => "paragraph",
                    _ => "subparagraph",
                };
                let star = if options.num { "" } else { "*" };
                let mut text = String::new();
                if let (true, Some((keyword, _))) = (options.todo, &heading.todo) {
                    text.push_str(&format!("\\textbf{{{keyword}}} "));
                }
                if let (true, Some(priority)) = (options.priority, heading.priority) {
                    text.push_str(&format!("\\framebox{{\\#{priority}}} "));
                }
                text.push_str(&inlines(&heading.title, document, cx));
                if options.tags && !heading.tags.is_empty() {
                    let tags: Vec<String> = heading.tags.iter().map(|t| escape(t)).collect();
                    text.push_str(&format!("\\hfill{{}}\\textsc{{{}}}", tags.join(":")));
                }
                format!("\\{command}{star}{{{text}}}\n\\label{{{}}}", heading.anchor)
            }
            Element::Paragraph(items) => inlines(items, document, cx),
            Element::List(kind, items) => {
                let env = match kind {
                    ListKind::Unordered => "itemize",
                    ListKind::Ordered => "enumerate",
                    ListKind::Description => "description",
                };
                let mut out = vec![format!("\\begin{{{env}}}")];
                for item in items {
                    let body = elements(&item.content, document, cx);
                    let head = match (&item.term, item.checkbox) {
                        (Some(term), _) => format!("[{{{}}}] ", inlines(term, document, cx)),
                        (None, Some(mark)) => checkbox(mark).to_string(),
                        (None, None) => String::new(),
                    };
                    out.push(format!("\\item {head}{body}"));
                }
                out.push(format!("\\end{{{env}}}"));
                out.join("\n")
            }
            Element::Table(rows, header) => {
                let width = rows.iter().map(Vec::len).max().unwrap_or(0);
                let mut out = vec![
                    "\\begin{center}".to_string(),
                    format!("\\begin{{tabular}}{{{}}}", "l".repeat(width)),
                ];
                for (index, row) in rows.iter().enumerate() {
                    let cells: Vec<String> = row.iter().map(|c| inlines(c, document, cx)).collect();
                    out.push(format!("{}\\\\", cells.join(" & ")));
                    if *header > 0 && index + 1 == *header {
                        out.push("\\hline".to_string());
                    }
                }
                out.push("\\end{tabular}".to_string());
                out.push("\\end{center}".to_string());
                out.join("\n")
            }
            Element::Code { code, .. } | Element::Example(code) => {
                format!("\\begin{{verbatim}}\n{code}\\end{{verbatim}}")
            }
            Element::Quote(inner) => format!(
                "\\begin{{quote}}\n{}\n\\end{{quote}}",
                elements(inner, document, cx)
            ),
            Element::Verse(lines) => format!(
                "\\begin{{verse}}\n{}\n\\end{{verse}}",
                lines
                    .iter()
                    .map(|line| inlines(line, document, cx))
                    .collect::<Vec<_>>()
                    .join("\\\\\n")
            ),
            Element::Center(inner) => format!(
                "\\begin{{center}}\n{}\n\\end{{center}}",
                elements(inner, document, cx)
            ),
            Element::Special(_, inner) => elements(inner, document, cx),
            Element::Export { backend, text } => {
                if backend == "latex" {
                    text.clone()
                } else {
                    String::new()
                }
            }
            Element::Rule => "\\noindent\\rule{\\textwidth}{0.5pt}".to_string(),
            Element::Bibliography => match cx.processor.as_str() {
                "biblatex" => "\\printbibliography".to_string(),
                "natbib" => {
                    let files: Vec<String> = document
                        .options
                        .bibliography
                        .iter()
                        .map(|file| file.trim_end_matches(".bib").to_string())
                        .collect();
                    format!(
                        "\\bibliographystyle{{plainnat}}\n\\bibliography{{{}}}",
                        files.join(",")
                    )
                }
                _ => {
                    let mut out = vec!["\\begin{itemize}".to_string()];
                    for (_, reference) in &cx.references {
                        out.push(format!("\\item {}", escape(reference)));
                    }
                    out.push("\\end{itemize}".to_string());
                    out.join("\n")
                }
            },
        }
    }

    pub(super) fn document(document: &Document, cx: &mut Context) -> String {
        let options = &document.options;
        let mut out = vec![
            "\\documentclass[11pt]{article}".to_string(),
            "\\usepackage[utf8]{inputenc}".to_string(),
            "\\usepackage[T1]{fontenc}".to_string(),
            "\\usepackage{graphicx}".to_string(),
            "\\usepackage{longtable}".to_string(),
            "\\usepackage[normalem]{ulem}".to_string(),
            "\\usepackage{amsmath}".to_string(),
            "\\usepackage{amssymb}".to_string(),
            "\\usepackage{hyperref}".to_string(),
        ];
        match cx.processor.as_str() {
            "biblatex" => {
                out.push("\\usepackage[backend=biber]{biblatex}".to_string());
                for file in &options.bibliography {
                    out.push(format!("\\addbibresource{{{file}}}"));
                }
            }
            "natbib" => out.push("\\usepackage{natbib}".to_string()),
            _ => {}
        }
        if let Some(author) = &options.author {
            out.push(format!("\\author{{{}}}", escape(author)));
        }
        out.push(format!(
            "\\date{{{}}}",
            options
                .date
                .as_deref()
                .map_or_else(|| "\\today".to_string(), escape)
        ));
        let title = options.title.as_deref().map(escape).unwrap_or_default();
        out.push(format!("\\title{{{title}}}"));
        out.push("\\begin{document}".to_string());
        out.push(String::new());
        if options.title.is_some() {
            out.push("\\maketitle".to_string());
        }
        if let Some(depth) = options.toc {
            if depth != usize::MAX {
                out.push(format!("\\setcounter{{tocdepth}}{{{depth}}}"));
            }
            out.push("\\tableofcontents".to_string());
            out.push(String::new());
        }
        out.push(elements(&document.elements, document, cx));
        out.push("\\end{document}".to_string());

        let mut text = out.join("\n");
        text.push('\n');
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Graph(Vec<(Uuid, IdTarget)>);

    impl Resolve for Graph {
        fn id(&self, id: &Uuid) -> Option<IdTarget> {
            self.0
                .iter()
                .find(|(key, _)| key == id)
                .map(|(_, target)| target.clone())
        }
    }

    fn md(text: &str) -> String {
        export(
            text,
            Path::new("/notes/a.org"),
            Backend::Markdown,
            &NoResolve,
        )
        .content
    }

    fn body_md(text: &str) -> String {
        md(&format!("#+OPTIONS: toc:nil\n{text}"))
    }

    #[test]
    fn anchors_follow_github_s_slugs() {
        assert_eq!(anchor_for("Hello, World!", None), "hello-world");
        assert_eq!(anchor_for("Two  spaces", None), "two--spaces");
        assert_eq!(anchor_for("Any", Some("custom")), "custom");
    }

    #[test]
    fn markdown_carries_the_outline_and_its_markup() {
        let out = body_md(
            "* TODO First :work:\nSome *bold*, /italic/, =verb= and ~code~.\n** Second\nText.\n",
        );
        assert_eq!(
            out,
            // The anchor links use is `first`; the renderer's would be
            // `todo-first`, so the heading names its own.
            "<a id=\"first\"></a>\n# TODO First\n\nSome **bold**, *italic*, `verb` and `code`.\n\n## Second\n\nText.\n"
        );
    }

    #[test]
    fn a_title_becomes_the_only_first_level_heading() {
        let out = md("#+title: Notes\n#+OPTIONS: toc:nil\n* Section\n");
        assert_eq!(out, "# Notes\n\n## Section\n");
    }

    #[test]
    fn emphasis_needs_its_boundaries() {
        // `2*3*4` is arithmetic, and `a/b/c` is a path.
        assert_eq!(body_md("2*3*4 and a/b/c\n"), "2\\*3\\*4 and a/b/c\n");
        assert_eq!(body_md("(*bold*)\n"), "(**bold**)\n");
        assert_eq!(
            body_md("*nested /italic/ inside*\n"),
            "**nested *italic* inside**\n"
        );
    }

    #[test]
    fn drawers_planning_and_comments_are_not_exported() {
        let out = body_md(
            "* Task\nSCHEDULED: <2026-09-24 Thu>\n:PROPERTIES:\n:ID: x\n:END:\n# a comment\nKept.\n:LOGBOOK:\n- note\n:END:\n#+begin_comment\nhidden\n#+end_comment\n",
        );
        assert_eq!(out, "# Task\n\nKept.\n");
    }

    #[test]
    fn noexport_and_comment_subtrees_are_left_out() {
        let out = body_md(
            "* Kept\n* Private :noexport:\nsecret\n** Child\n* COMMENT Draft\nx\n* Also kept\n",
        );
        assert_eq!(out, "# Kept\n\n# Also kept\n");
    }

    #[test]
    fn lists_nest_and_keep_their_checkboxes() {
        let out = body_md(
            "- one\n- [X] two\n  - nested\nThen:\n1. first\n2. second\nAnd:\n- term :: meaning\n",
        );
        assert_eq!(
            out,
            "- one\n- [x] two\n  - nested\n\nThen:\n\n1. first\n2. second\n\nAnd:\n\n- **term**: meaning\n"
        );
    }

    #[test]
    fn tables_get_a_header_row() {
        let out = body_md("| a | b |\n|---+---|\n| 1 | 2 |\n");
        assert_eq!(out, "| a | b |\n|---|---|\n| 1 | 2 |\n");
        let out = body_md("| a | b |\n| 1 | 2 |\n");
        assert_eq!(out, "| a | b |\n|---|---|\n| 1 | 2 |\n");
    }

    #[test]
    fn source_blocks_export_their_code_and_drop_results_by_default() {
        let text =
            "#+begin_src rust\n  fn main() {}\n#+end_src\n\n#+RESULTS:\n: output\n\nAfter.\n";
        assert_eq!(body_md(text), "```rust\nfn main() {}\n```\n\nAfter.\n");

        let both = text.replace("rust\n", "rust :exports both\n");
        assert_eq!(
            body_md(&both),
            "```rust\nfn main() {}\n```\n\n```\noutput\n```\n\nAfter.\n"
        );
    }

    #[test]
    fn links_become_links() {
        let out = body_md("See [[https://example.com][the site]], [[file:other.org][other]], [[file:pic.png]] and https://x.org/a.\n");
        assert_eq!(
            out,
            "See [the site](https://example.com), [other](other.md), ![](pic.png) and [https://x.org/a](https://x.org/a).\n"
        );
    }

    #[test]
    fn internal_links_use_the_heading_anchors() {
        let out = body_md("* Target Heading\n:PROPERTIES:\n:CUSTOM_ID: tgt\n:END:\n* Other\nGo to [[*Target Heading][there]] or [[#tgt][here]].\n");
        assert!(
            out.contains("<a id=\"tgt\"></a>\n# Target Heading"),
            "{out}"
        );
        assert!(out.contains("[there](#tgt) or [here](#tgt)"), "{out}");
    }

    #[test]
    fn id_links_resolve_through_the_graph_or_fall_back_to_their_text() {
        let known: Uuid = "6ba7b810-9dad-11d1-80b4-00c04fd430c8".parse().unwrap();
        let file_node: Uuid = "6ba7b811-9dad-11d1-80b4-00c04fd430c8".parse().unwrap();
        let graph = Graph(vec![
            (
                known,
                IdTarget {
                    file: PathBuf::from("/notes/sub/b.org"),
                    anchor: Some("a-heading".to_string()),
                    title: "A heading".to_string(),
                },
            ),
            (
                file_node,
                IdTarget {
                    file: PathBuf::from("/notes/c.org"),
                    anchor: None,
                    title: "C".to_string(),
                },
            ),
        ]);
        let text = format!(
            "#+OPTIONS: toc:nil\n[[id:{known}]], [[id:{file_node}][see C]], [[id:00000000-0000-0000-0000-000000000000][gone]]\n"
        );
        let out = export(&text, Path::new("/notes/a.org"), Backend::Markdown, &graph);
        assert_eq!(
            out.content,
            "[A heading](sub/b.md#a-heading), [see C](c.md), gone\n"
        );
        assert_eq!(out.warnings.len(), 1);

        let html = export(&text, Path::new("/notes/a.org"), Backend::Html, &graph).content;
        assert!(
            html.contains("<a href=\"sub/b.html#a-heading\">A heading</a>"),
            "{html}"
        );
    }

    #[test]
    fn a_link_up_a_directory_is_relative() {
        assert_eq!(
            relative(Path::new("/notes/sub"), Path::new("/notes/c.md")),
            "../c.md"
        );
        assert_eq!(
            relative(Path::new("/notes"), Path::new("/notes/c.md")),
            "c.md"
        );
    }

    #[test]
    fn footnotes_are_numbered_by_first_reference() {
        let out = body_md(
            "Text[fn:b] and more[fn:a], inline[fn::said here].\n\n[fn:a] Note A.\n[fn:b] Note B.\n",
        );
        assert_eq!(
            out,
            "Text[^1] and more[^2], inline[^3].\n\n[^1]: Note B.\n[^2]: Note A.\n[^3]: said here\n"
        );
    }

    #[test]
    fn a_table_of_contents_is_on_unless_turned_off() {
        let out = md("* One\n** Two\n");
        assert!(
            out.starts_with("- [One](#one)\n  - [Two](#two)\n\n# One"),
            "{out}"
        );
    }

    #[test]
    fn html_escapes_numbers_and_marks_state() {
        let out = export(
            "#+title: T & C\n* TODO A <b> :tag:\n** Sub\nx < y\n",
            Path::new("/n/t.org"),
            Backend::Html,
            &NoResolve,
        )
        .content;
        assert!(out.contains("<title>T &amp; C</title>"), "{out}");
        assert!(out.contains(
            "<h2 id=\"a-b\"><span class=\"section-number-2\">1</span> <span class=\"todo TODO\">TODO</span> A &lt;b&gt;&#xa0;&#xa0;&#xa0;<span class=\"tag\"><span class=\"tag\">tag</span></span></h2>"
        ), "{out}");
        assert!(
            out.contains("<h3 id=\"sub\"><span class=\"section-number-3\">1.1</span> Sub</h3>"),
            "{out}"
        );
        assert!(out.contains("<p>\nx &lt; y\n</p>"), "{out}");
        assert!(
            out.contains("<li><a href=\"#a-b\">1. A &lt;b&gt;</a></li>"),
            "{out}"
        );
    }

    #[test]
    fn latex_escapes_and_uses_its_own_structures() {
        let out = export(
            "#+title: Costs\n#+OPTIONS: toc:nil num:nil\n* 50% of $10 & more\n- [ ] item_one\n\nSee[fn:1].\n\n[fn:1] A note.\n",
            Path::new("/n/c.org"),
            Backend::Latex,
            &NoResolve,
        )
        .content;
        assert!(out.contains("\\title{Costs}"), "{out}");
        assert!(
            out.contains("\\section*{50\\% of \\$10 \\& more}\n\\label{50-of-10--more}"),
            "{out}"
        );
        assert!(out.contains("\\item [{$\\square$}] item\\_one"), "{out}");
        assert!(out.contains("See\\footnote{A note.}."), "{out}");
        assert!(!out.contains("\\tableofcontents"), "{out}");

        let linked = export(
            "[[file:other.org][other]]\n",
            Path::new("/n/c.org"),
            Backend::Latex,
            &NoResolve,
        )
        .content;
        assert!(linked.contains("\\href{other.pdf}{other}"), "{linked}");
    }

    #[test]
    fn export_blocks_reach_only_their_backend() {
        let text = "#+OPTIONS: toc:nil\n#+begin_export html\n<div>raw</div>\n#+end_export\n";
        assert_eq!(md(text), "\n");
        let html = export(text, Path::new("/n/x.org"), Backend::Html, &NoResolve).content;
        assert!(html.contains("<div>raw</div>"), "{html}");
    }

    #[test]
    fn quotes_and_examples() {
        let out = body_md("#+begin_quote\nWise *words*.\n#+end_quote\n: fixed\n: width\n");
        assert_eq!(out, "> Wise **words**.\n\n```\nfixed\nwidth\n```\n");
    }
}
