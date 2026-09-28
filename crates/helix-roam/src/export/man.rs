//! A man page, in groff's `man` macros: first-level headlines are `.SH`
//! sections, deeper ones `.SS`, and the rest what `man` has for them.

use super::*;

/// Escapes groff's backslash and hyphen, and a leading dot or quote that
/// would read as a request.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\e"),
            '-' => out.push_str("\\-"),
            c => out.push(c),
        }
    }
    out
}

/// Keeps a line that starts with `.` or `'` from being read as a request.
fn guard(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.starts_with('.') || line.starts_with('\'') {
                format!("\\&{line}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn inlines(items: &[Inline], cx: &mut Context) -> String {
    items.iter().map(|inline| one(inline, cx)).collect()
}

fn one(inline: &Inline, cx: &mut Context) -> String {
    match inline {
        Inline::Text(text) => escape(text),
        Inline::Bold(inner) => format!("\\fB{}\\fP", inlines(inner, cx)),
        Inline::Italic(inner) | Inline::Underline(inner) => {
            format!("\\fI{}\\fP", inlines(inner, cx))
        }
        Inline::Strike(inner) => inlines(inner, cx),
        Inline::Verbatim(text) | Inline::Code(text) => format!("\\fB{}\\fP", escape(text)),
        Inline::Link {
            target,
            description,
        } => {
            let label = match description {
                Some(inner) => inlines(inner, cx),
                None => escape(&cx.default_label(target)),
            };
            match cx.href(target, description.is_some()) {
                Href::Link(href) if href.starts_with('#') => label,
                Href::Link(href) if description.is_none() => format!("\\fI{}\\fP", escape(&href)),
                Href::Link(href) => format!("{label} (\\fI{}\\fP)", escape(&href)),
                Href::Image(src) => format!("[{}]", escape(&src)),
                Href::None => label,
            }
        }
        Inline::Footnote(label) => format!("[{}]", cx.footnote_number(label)),
        Inline::LineBreak => "\n.br\n".to_string(),
        Inline::Citation {
            style,
            cites,
            prefix,
            suffix,
        } => escape(&cx.basic_citation(style.as_deref(), cites, prefix, suffix, &|_, body| body)),
    }
}

fn verbatim(text: &str) -> String {
    format!(".RS\n.nf\n{}\n.fi\n.RE", guard(&escape(text.trim_end())))
}

fn elements(items: &[Element], document: &Document, cx: &mut Context) -> Vec<String> {
    items
        .iter()
        .map(|element| element_man(element, document, cx))
        .filter(|text| !text.is_empty())
        .collect()
}

fn element_man(element: &Element, document: &Document, cx: &mut Context) -> String {
    match element {
        Element::Heading(heading) => {
            let title = plain(&heading.title);
            if heading.level == 1 {
                format!(
                    ".SH \"{}\"",
                    escape(&title.to_uppercase()).replace('"', "\\(dq")
                )
            } else {
                format!(".SS \"{}\"", escape(&title).replace('"', "\\(dq"))
            }
        }
        Element::Paragraph(items) => format!(".PP\n{}", guard(&inlines(items, cx))),
        Element::List(kind, items) => {
            let mut out = Vec::new();
            for (index, item) in items.iter().enumerate() {
                let body: Vec<String> = item
                    .content
                    .iter()
                    .map(|element| match element {
                        Element::Paragraph(items) => guard(&inlines(items, cx)),
                        other => format!(".RS\n{}\n.RE", element_man(other, document, cx)),
                    })
                    .collect();
                let head = match (kind, &item.term) {
                    (_, Some(term)) => format!(".TP\n\\fB{}\\fP", inlines(term, cx)),
                    (ListKind::Ordered, None) => format!(".IP \"{}.\" 4", index + 1),
                    _ => ".IP \\(bu 4".to_string(),
                };
                let checkbox = match item.checkbox {
                    Some(' ') => "[ ] ",
                    Some(_) => "[X] ",
                    None => "",
                };
                out.push(format!("{head}\n{checkbox}{}", body.join("\n")));
            }
            out.join("\n")
        }
        Element::Table(rows, header) => {
            let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
            let mut out = vec![
                ".TS".to_string(),
                "tab(\t) box;".to_string(),
                format!("{}.", vec!["l"; columns].join(" ")),
            ];
            for (index, row) in rows.iter().enumerate() {
                let cells: Vec<String> = row.iter().map(|cell| inlines(cell, cx)).collect();
                out.push(cells.join("\t"));
                if *header > 0 && index + 1 == *header {
                    out.push("_".to_string());
                }
            }
            out.push(".TE".to_string());
            out.join("\n")
        }
        Element::Code { code, .. } | Element::Example(code) => verbatim(code),
        Element::Quote(inner) | Element::Center(inner) | Element::Special(_, inner) => {
            format!(".RS\n{}\n.RE", elements(inner, document, cx).join("\n"))
        }
        Element::Verse(lines) => format!(
            ".PP\n.nf\n{}\n.fi",
            guard(
                &lines
                    .iter()
                    .map(|line| inlines(line, cx))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        ),
        Element::Export { backend, text } => {
            if backend == "man" {
                text.trim_end().to_string()
            } else {
                String::new()
            }
        }
        Element::Rule => ".PP\n\\l'\\n(.lu'".to_string(),
        Element::Bibliography => cx
            .references
            .iter()
            .map(|(_, reference)| format!(".IP \\(bu 4\n{}", guard(&escape(reference))))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

pub(super) fn document(document: &Document, cx: &mut Context, source: &Path) -> String {
    let options = &document.options;
    let mut out = Vec::new();
    if !options.body_only {
        let name = options.title.clone().unwrap_or_else(|| {
            source
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        out.push(format!(
            ".TH \"{}\" \"1\" \"{}\" \"\" \"\"",
            escape(&name.to_uppercase()),
            escape(options.date.as_deref().unwrap_or(""))
        ));
    }
    out.extend(elements(&document.elements, document, cx));

    if !cx.footnote_order.is_empty() {
        out.push(".SH \"NOTES\"".to_string());
        let mut index = 0;
        while index < cx.footnote_order.len() {
            let label = cx.footnote_order[index].clone();
            let body = footnote_definition(document, &label)
                .map(|inline| inlines(inline, cx))
                .unwrap_or_default();
            out.push(format!(".IP \"[{}]\" 4\n{}", index + 1, guard(&body)));
            index += 1;
        }
    }
    if let (Some(author), false) = (&options.author, options.body_only) {
        out.push(format!(".SH \"AUTHOR\"\n.PP\n{}", guard(&escape(author))));
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}
