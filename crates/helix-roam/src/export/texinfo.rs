//! Texinfo: a node for each headline down to the fourth level, chaptered
//! and sectioned as numbered (or not), with the menus `makeinfo` needs to
//! find its way from `Top` down.

use super::*;

/// Escapes Texinfo's `@`, `{` and `}`.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '@' | '{' | '}') {
            out.push('@');
        }
        out.push(c);
    }
    out
}

/// The deepest headline that gets a node of its own.
const NODE_LEVELS: usize = 4;

/// Node names by anchor: a headline's title without what a node name
/// cannot hold, made unique.
fn node_names(document: &Document) -> HashMap<String, String> {
    let mut names = HashMap::new();
    let mut taken: Vec<String> = vec!["Top".to_string()];
    for element in &document.elements {
        let Element::Heading(heading) = element else {
            continue;
        };
        if heading.level > NODE_LEVELS {
            continue;
        }
        let base: String = plain(&heading.title)
            .chars()
            .filter(|c| !matches!(c, ',' | ':' | '.' | '(' | ')' | '@' | '{' | '}'))
            .collect::<String>()
            .trim()
            .to_string();
        let base = if base.is_empty() {
            "Section".to_string()
        } else {
            base
        };
        let mut name = base.clone();
        let mut n = 2;
        while taken.contains(&name) {
            name = format!("{base} {n}");
            n += 1;
        }
        taken.push(name.clone());
        names.insert(heading.anchor.clone(), name);
    }
    names
}

struct Nodes<'a> {
    names: &'a HashMap<String, String>,
}

fn inlines(items: &[Inline], nodes: &Nodes, document: &Document, cx: &mut Context) -> String {
    items
        .iter()
        .map(|inline| one(inline, nodes, document, cx))
        .collect()
}

fn one(inline: &Inline, nodes: &Nodes, document: &Document, cx: &mut Context) -> String {
    match inline {
        Inline::Text(text) => escape(text),
        Inline::Bold(inner) => format!("@strong{{{}}}", inlines(inner, nodes, document, cx)),
        Inline::Italic(inner) | Inline::Underline(inner) => {
            format!("@emph{{{}}}", inlines(inner, nodes, document, cx))
        }
        Inline::Strike(inner) => inlines(inner, nodes, document, cx),
        Inline::Verbatim(text) => format!("@samp{{{}}}", escape(text)),
        Inline::Code(text) => format!("@code{{{}}}", escape(text)),
        Inline::Link {
            target,
            description,
        } => {
            let label = match description {
                Some(inner) => inlines(inner, nodes, document, cx),
                None => escape(&cx.default_label(target)),
            };
            match cx.href(target, description.is_some()) {
                Href::Link(href) if href.starts_with('#') => match nodes.names.get(&href[1..]) {
                    Some(node) => format!("@ref{{{node},,{label}}}"),
                    None => label,
                },
                Href::Link(href) => format!(
                    "@uref{{{},{label}}}",
                    escape(&href).replace(',', "@comma{}")
                ),
                Href::Image(src) => {
                    let stem = Path::new(&src).with_extension("");
                    format!("@image{{{}}}", escape(&stem.to_string_lossy()))
                }
                Href::None => label,
            }
        }
        // Texinfo places the note itself.
        Inline::Footnote(label) => {
            let body = footnote_definition(document, label)
                .map(|inline| inlines(inline, nodes, document, cx))
                .unwrap_or_default();
            format!("@footnote{{{body}}}")
        }
        Inline::LineBreak => "@*\n".to_string(),
        Inline::Citation {
            style,
            cites,
            prefix,
            suffix,
        } => escape(&cx.basic_citation(style.as_deref(), cites, prefix, suffix, &|_, body| body)),
    }
}

fn sectioning(level: usize, numbered: bool) -> &'static str {
    match (level, numbered) {
        (1, true) => "@chapter",
        (2, true) => "@section",
        (3, true) => "@subsection",
        (4, true) => "@subsubsection",
        (1, false) => "@unnumbered",
        (2, false) => "@unnumberedsec",
        (3, false) => "@unnumberedsubsec",
        (4, false) => "@unnumberedsubsubsec",
        _ => "@subsubheading",
    }
}

fn menu(entries: &[&String]) -> String {
    let mut out = vec!["@menu".to_string()];
    out.extend(entries.iter().map(|name| format!("* {name}::")));
    out.push("@end menu".to_string());
    out.join("\n")
}

fn element_texi(element: &Element, nodes: &Nodes, document: &Document, cx: &mut Context) -> String {
    let options = &document.options;
    match element {
        Element::Heading(heading) => {
            let mut title = String::new();
            if let (true, Some((keyword, _))) = (options.todo, &heading.todo) {
                title.push_str(&escape(keyword));
                title.push(' ');
            }
            title.push_str(&inlines(&heading.title, nodes, document, cx));
            let command = sectioning(heading.level, options.num);
            match nodes.names.get(&heading.anchor) {
                Some(node) => format!("@node {node}\n{command} {title}"),
                None => format!("{command} {title}"),
            }
        }
        Element::Paragraph(items) => inlines(items, nodes, document, cx),
        Element::List(kind, items) => {
            let (begin, end) = match kind {
                ListKind::Unordered => ("@itemize @bullet", "@end itemize"),
                ListKind::Ordered => ("@enumerate", "@end enumerate"),
                ListKind::Description => ("@table @asis", "@end table"),
            };
            let mut out = vec![begin.to_string()];
            for item in items {
                let mut head = match &item.term {
                    Some(term) => format!("@item {}\n", inlines(term, nodes, document, cx)),
                    None => "@item\n".to_string(),
                };
                if let Some(mark) = item.checkbox {
                    head.push_str(if mark == ' ' { "[ ] " } else { "[X] " });
                }
                let body: Vec<String> = item
                    .content
                    .iter()
                    .map(|element| element_texi(element, nodes, document, cx))
                    .collect();
                out.push(format!("{head}{}", body.join("\n\n")));
            }
            out.push(end.to_string());
            out.join("\n")
        }
        Element::Table(rows, header) => {
            let columns = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
            let fraction = format!("{:.2}", 1.0 / columns as f64);
            let mut out = vec![format!(
                "@multitable @columnfractions {}",
                vec![fraction.as_str(); columns].join(" ")
            )];
            for (index, row) in rows.iter().enumerate() {
                let cells: Vec<String> = row
                    .iter()
                    .map(|cell| inlines(cell, nodes, document, cx))
                    .collect();
                let command = if index < *header {
                    "@headitem"
                } else {
                    "@item"
                };
                out.push(format!("{command} {}", cells.join(" @tab ")));
            }
            out.push("@end multitable".to_string());
            out.join("\n")
        }
        Element::Code { code, .. } => {
            format!("@example\n{}\n@end example", escape(code.trim_end()))
        }
        Element::Example(text) => format!("@example\n{}\n@end example", escape(text.trim_end())),
        Element::Quote(inner) => format!(
            "@quotation\n{}\n@end quotation",
            elements(inner, nodes, document, cx).join("\n\n")
        ),
        Element::Verse(lines) => format!(
            "@display\n{}\n@end display",
            lines
                .iter()
                .map(|line| inlines(line, nodes, document, cx))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        Element::Center(inner) => elements(inner, nodes, document, cx)
            .join("\n\n")
            .lines()
            .map(|line| format!("@center {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
        Element::Special(_, inner) => elements(inner, nodes, document, cx).join("\n\n"),
        Element::Export { backend, text } => {
            if backend == "texinfo" {
                text.trim_end().to_string()
            } else {
                String::new()
            }
        }
        Element::Rule => "@sp 1".to_string(),
        Element::Bibliography => {
            let mut out = vec!["@itemize @bullet".to_string()];
            for (_, reference) in &cx.references {
                out.push(format!("@item\n{}", escape(reference)));
            }
            out.push("@end itemize".to_string());
            out.join("\n")
        }
    }
}

fn elements(
    items: &[Element],
    nodes: &Nodes,
    document: &Document,
    cx: &mut Context,
) -> Vec<String> {
    items
        .iter()
        .map(|element| element_texi(element, nodes, document, cx))
        .filter(|text| !text.is_empty())
        .collect()
}

pub(super) fn document(document: &Document, cx: &mut Context, source: &Path) -> String {
    let options = &document.options;
    let names = node_names(document);
    let nodes = Nodes { names: &names };
    let stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let title = options.title.clone().unwrap_or_else(|| stem.clone());

    // Each node's children, for the menus: Top's are the first level's.
    let headings: Vec<&Heading> = document
        .elements
        .iter()
        .filter_map(|element| match element {
            Element::Heading(heading) if names.contains_key(&heading.anchor) => Some(heading),
            _ => None,
        })
        .collect();
    let children_of = |at: Option<usize>| -> Vec<&String> {
        let (from, level) = match at {
            Some(at) => (at + 1, headings[at].level),
            None => (0, 0),
        };
        headings[from..]
            .iter()
            .take_while(|heading| heading.level > level)
            .filter(|heading| heading.level == level + 1)
            .filter_map(|heading| names.get(&heading.anchor))
            .collect()
    };

    let mut out = Vec::new();
    if !options.body_only {
        out.push("\\input texinfo".to_string());
        out.push(format!("@setfilename {stem}.info"));
        out.push(format!("@settitle {}", escape(&title)));
        out.push("@documentencoding UTF-8".to_string());
        out.push(String::new());
        out.push("@titlepage".to_string());
        out.push(format!("@title {}", escape(&title)));
        if let Some(author) = &options.author {
            out.push(format!("@author {}", escape(author)));
        }
        out.push("@end titlepage".to_string());
        out.push(String::new());
        if options.toc.is_some() {
            out.push("@contents".to_string());
            out.push(String::new());
        }
        out.push("@ifnottex".to_string());
        out.push("@node Top".to_string());
        out.push(format!("@top {}", escape(&title)));
        out.push("@end ifnottex".to_string());
        out.push(String::new());
    }

    // A parent's menu goes just before its first child.
    let mut heading_at = 0;
    let mut menus_done: Vec<Option<usize>> = Vec::new();
    for element in &document.elements {
        if let Element::Heading(heading) = element {
            if names.contains_key(&heading.anchor) {
                let parent = (0..heading_at)
                    .rev()
                    .find(|&at| headings[at].level < heading.level);
                let parent = match parent {
                    Some(at) if headings[at].level + 1 == heading.level => Some(at),
                    None if heading.level == 1 => None,
                    _ => {
                        heading_at += 1;
                        let text = element_texi(element, &nodes, document, cx);
                        out.push(text);
                        continue;
                    }
                };
                // A body alone has no Top node to hang the first menu on.
                let has_menu = parent.is_some() || !options.body_only;
                if has_menu && !menus_done.contains(&parent) {
                    menus_done.push(parent);
                    out.push(menu(&children_of(parent)));
                    out.push(String::new());
                }
                heading_at += 1;
            }
        }
        let text = element_texi(element, &nodes, document, cx);
        if !text.is_empty() {
            out.push(text);
            out.push(String::new());
        }
    }

    if !options.body_only {
        out.push("@bye".to_string());
    }
    let mut text = out.join("\n").trim_end().to_string();
    text.push('\n');
    text
}
