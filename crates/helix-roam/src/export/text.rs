//! Plain text, as Org's `ascii` backend writes it: headlines underlined,
//! paragraphs filled to 72 columns, tables drawn, links spelled out. With
//! `utf8`, the bullets, quotes, checkboxes and rules are Unicode's.

use super::*;

/// The column paragraphs are filled to.
const WIDTH: usize = 72;

/// What differs between ASCII and UTF-8.
struct Charset {
    utf8: bool,
}

impl Charset {
    fn bullet(&self, depth: usize) -> &'static str {
        match (self.utf8, depth % 3) {
            (true, 0) => "•",
            (true, 1) => "◦",
            (true, _) => "‣",
            (false, 0) => "-",
            (false, 1) => "*",
            (false, _) => "+",
        }
    }

    fn checkbox(&self, mark: char) -> &'static str {
        match (self.utf8, mark) {
            (true, 'X') => "☑",
            (true, '-') => "☐",
            (true, _) => "☐",
            (false, 'X') => "[X]",
            (false, '-') => "[-]",
            (false, _) => "[ ]",
        }
    }

    fn underline(&self, level: usize) -> char {
        match (self.utf8, level) {
            (true, 1) => '═',
            (true, _) => '─',
            (false, 1) => '=',
            (false, _) => '-',
        }
    }

    fn quoted(&self, text: &str) -> String {
        if self.utf8 {
            format!("‘{text}’")
        } else {
            format!("`{text}'")
        }
    }
}

fn width(text: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(text)
}

/// Fills `text` to `WIDTH - indent` columns, every line but the first
/// indented by `indent`.
fn fill(text: &str, indent: usize) -> String {
    let room = WIDTH.saturating_sub(indent).max(20);
    let mut lines: Vec<String> = Vec::new();
    // A hard line break stays one.
    for paragraph_line in text.split('\n') {
        let mut line = String::new();
        for word in paragraph_line.split_whitespace() {
            if !line.is_empty() && width(&line) + 1 + width(word) > room {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    let pad = " ".repeat(indent);
    lines.join(&format!("\n{pad}"))
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

fn inlines(items: &[Inline], set: &Charset, cx: &mut Context) -> String {
    items.iter().map(|inline| one(inline, set, cx)).collect()
}

fn one(inline: &Inline, set: &Charset, cx: &mut Context) -> String {
    match inline {
        Inline::Text(text) => text.clone(),
        Inline::Bold(inner) => format!("*{}*", inlines(inner, set, cx)),
        Inline::Italic(inner) => format!("/{}/", inlines(inner, set, cx)),
        Inline::Underline(inner) => format!("_{}_", inlines(inner, set, cx)),
        Inline::Strike(inner) => format!("+{}+", inlines(inner, set, cx)),
        Inline::Verbatim(text) | Inline::Code(text) => set.quoted(text),
        Inline::Link {
            target,
            description,
        } => {
            let label = match description {
                Some(inner) => inlines(inner, set, cx),
                None => cx.default_label(target),
            };
            match cx.href(target, description.is_some()) {
                // An anchor in the same text leads nowhere on paper.
                Href::Link(href) if href.starts_with('#') => label,
                Href::Link(href) if description.is_none() && href == label => format!("<{href}>"),
                Href::Link(href) => format!("{label} <{href}>"),
                Href::Image(src) => format!("[{src}]"),
                Href::None => label,
            }
        }
        Inline::Footnote(label) => format!("[{}]", cx.footnote_number(label)),
        Inline::LineBreak => "\n".to_string(),
        Inline::Citation {
            style,
            cites,
            prefix,
            suffix,
        } => cx.basic_citation(style.as_deref(), cites, prefix, suffix, &|_, body| body),
    }
}

/// A table drawn with `+`, `-` and `|`, or with box drawing in UTF-8.
fn table(rows: &[Vec<Vec<Inline>>], header: usize, set: &Charset, cx: &mut Context) -> String {
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| row.iter().map(|cell| inlines(cell, set, cx)).collect())
        .collect();
    let columns = cells.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|at| {
            cells
                .iter()
                .filter_map(|row| row.get(at))
                .map(|cell| width(cell))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let (h, v, [tl, tm, tr], [ml, mm, mr], [bl, bm, br]) = if set.utf8 {
        ('─', '│', ['┌', '┬', '┐'], ['├', '┼', '┤'], ['└', '┴', '┘'])
    } else {
        ('-', '|', ['+', '+', '+'], ['+', '+', '+'], ['+', '+', '+'])
    };
    let rule = |left: char, middle: char, right: char| {
        let parts: Vec<String> = widths.iter().map(|w| h.to_string().repeat(w + 2)).collect();
        format!("{left}{}{right}", parts.join(&middle.to_string()))
    };
    let mut out = vec![rule(tl, tm, tr)];
    for (index, row) in cells.iter().enumerate() {
        let parts: Vec<String> = widths
            .iter()
            .enumerate()
            .map(|(at, w)| {
                let cell = row.get(at).map(String::as_str).unwrap_or("");
                format!(" {cell}{} ", " ".repeat(w - width(cell)))
            })
            .collect();
        out.push(format!("{v}{}{v}", parts.join(&v.to_string())));
        if header > 0 && index + 1 == header && index + 1 < cells.len() {
            out.push(rule(ml, mm, mr));
        }
    }
    out.push(rule(bl, bm, br));
    out.join("\n")
}

fn elements(
    items: &[Element],
    depth: usize,
    numbers: &HashMap<String, String>,
    document: &Document,
    set: &Charset,
    cx: &mut Context,
) -> Vec<String> {
    items
        .iter()
        .map(|element| element_text(element, depth, numbers, document, set, cx))
        .filter(|text| !text.is_empty())
        .collect()
}

fn element_text(
    element: &Element,
    depth: usize,
    numbers: &HashMap<String, String>,
    document: &Document,
    set: &Charset,
    cx: &mut Context,
) -> String {
    let options = &document.options;
    match element {
        Element::Heading(heading) => {
            let mut text = String::new();
            if let Some(number) = numbers.get(&heading.anchor) {
                text.push_str(number);
                text.push(' ');
            }
            if let (true, Some((keyword, _))) = (options.todo, &heading.todo) {
                text.push_str(keyword);
                text.push(' ');
            }
            if let (true, Some(priority)) = (options.priority, heading.priority) {
                text.push_str(&format!("[#{priority}] "));
            }
            text.push_str(&inlines(&heading.title, set, cx));
            if options.tags && !heading.tags.is_empty() {
                text.push_str(&format!("  :{}:", heading.tags.join(":")));
            }
            // Deeper than the outline's sections, a headline is an item.
            let levels = options.headline_levels.unwrap_or(3);
            if heading.level > levels.min(2) {
                return format!("{} {text}", set.bullet(heading.level));
            }
            let rule: String =
                std::iter::repeat_n(set.underline(heading.level), width(&text)).collect();
            format!("\n{text}\n{rule}")
        }
        Element::Paragraph(items) => fill(&inlines(items, set, cx), 0),
        Element::List(kind, items) => {
            let mut out = Vec::new();
            for (index, item) in items.iter().enumerate() {
                let mut marker = match kind {
                    ListKind::Ordered => format!("{}.", index + 1),
                    _ => set.bullet(depth).to_string(),
                };
                if let Some(mark) = item.checkbox {
                    marker.push(' ');
                    marker.push_str(set.checkbox(mark));
                }
                if let Some(term) = &item.term {
                    marker.push(' ');
                    marker.push_str(&inlines(term, set, cx));
                    marker.push(':');
                }
                let pad = width(&marker) + 1;
                let body: Vec<String> = item
                    .content
                    .iter()
                    .map(|element| match element {
                        Element::Paragraph(items) => fill(&inlines(items, set, cx), pad),
                        other => indent(
                            &element_text(other, depth + 1, numbers, document, set, cx),
                            pad,
                        ),
                    })
                    .filter(|text| !text.is_empty())
                    .collect();
                let body = body.join("\n");
                out.push(format!("{marker} {}", body.trim_start()));
            }
            out.join("\n")
        }
        Element::Table(rows, header) => table(rows, *header, set, cx),
        Element::Code { code, .. } | Element::Example(code) => indent(code.trim_end(), 2),
        Element::Quote(inner) => indent(
            &elements(inner, depth, numbers, document, set, cx).join("\n\n"),
            2,
        ),
        Element::Verse(lines) => lines
            .iter()
            .map(|line| inlines(line, set, cx))
            .collect::<Vec<_>>()
            .join("\n"),
        Element::Center(inner) => elements(inner, depth, numbers, document, set, cx)
            .join("\n\n")
            .lines()
            .map(|line| {
                let pad = WIDTH.saturating_sub(width(line)) / 2;
                format!("{}{line}", " ".repeat(pad))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Element::Special(_, inner) => {
            elements(inner, depth, numbers, document, set, cx).join("\n\n")
        }
        Element::Export { backend, text } => {
            let ours = matches!(backend.as_str(), "ascii" | "text")
                || (set.utf8 && matches!(backend.as_str(), "utf-8" | "utf8"));
            if ours {
                text.trim_end().to_string()
            } else {
                String::new()
            }
        }
        Element::Rule => set.underline(2).to_string().repeat(WIDTH),
        Element::Bibliography => cx
            .references
            .iter()
            .map(|(_, reference)| format!("{} {}", set.bullet(0), fill(reference, 2)))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

pub(super) fn document(document: &Document, cx: &mut Context, utf8: bool) -> String {
    let set = Charset { utf8 };
    let options = &document.options;
    let mut numbers = HashMap::new();
    if options.num {
        for (heading, number) in numbered(document) {
            numbers.insert(heading.anchor.clone(), number);
        }
    }
    let mut parts = Vec::new();
    if !options.body_only {
        if let Some(title) = &options.title {
            let rule: String = std::iter::repeat_n(set.underline(1), width(title)).collect();
            parts.push(format!("{title}\n{rule}"));
        }
        let byline: Vec<&str> = [options.author.as_deref(), options.date.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        if !byline.is_empty() {
            parts.push(byline.join("\n"));
        }
        if let Some(depth) = options.toc {
            let entries: Vec<String> = numbered(document)
                .into_iter()
                .filter(|(heading, _)| heading.level <= depth)
                .map(|(heading, number)| {
                    let number = if options.num {
                        format!("{number}. ")
                    } else {
                        String::new()
                    };
                    format!(
                        "{}{number}{}",
                        "  ".repeat(heading.level - 1),
                        plain(&heading.title)
                    )
                })
                .collect();
            if !entries.is_empty() {
                let title = "Table of Contents";
                let rule: String = std::iter::repeat_n(set.underline(1), title.len()).collect();
                parts.push(format!("{title}\n{rule}\n\n{}", entries.join("\n")));
            }
        }
    }
    parts.extend(elements(
        &document.elements,
        0,
        &numbers,
        document,
        &set,
        cx,
    ));

    if !cx.footnote_order.is_empty() {
        let mut notes = vec![format!(
            "\nFootnotes\n{}\n",
            std::iter::repeat_n(set.underline(1), 9).collect::<String>()
        )];
        let mut index = 0;
        while index < cx.footnote_order.len() {
            let label = cx.footnote_order[index].clone();
            let body = footnote_definition(document, &label)
                .map(|inline| inlines(inline, &set, cx))
                .unwrap_or_default();
            let marker = format!("[{}] ", index + 1);
            notes.push(format!("{marker}{}", fill(&body, marker.len())));
            index += 1;
        }
        parts.push(notes.join("\n"));
    }

    let mut out = parts.join("\n\n").trim_start_matches('\n').to_string();
    out.push('\n');
    out
}
