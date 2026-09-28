//! Beamer slides. The headlines at the frame level (`H:` in `#+OPTIONS:`,
//! or `#+BEAMER_FRAME_LEVEL:`, 1 by default as in Org) are frames; the
//! ones above it are sections, the ones below it blocks. Everything else
//! is LaTeX, written by the LaTeX backend.

use super::*;

/// What is open while the elements are written.
enum Open {
    Frame,
    Block(usize),
}

fn heading_title(heading: &Heading, document: &Document, cx: &mut Context) -> String {
    let mut title = String::new();
    if let (true, Some((keyword, _))) = (document.options.todo, &heading.todo) {
        title.push_str(&format!("\\textbf{{{keyword}}} "));
    }
    title.push_str(&latex::inlines(&heading.title, document, cx));
    title
}

/// Closes what is open down to (and including) blocks at `level` or deeper,
/// and the frame too when `frame` says so.
fn close(open: &mut Vec<Open>, out: &mut Vec<String>, level: usize, frame: bool) {
    while let Some(last) = open.last() {
        match last {
            Open::Block(at) if *at >= level => out.push("\\end{block}".to_string()),
            Open::Frame if frame => out.push("\\end{frame}".to_string()),
            _ => break,
        }
        open.pop();
    }
}

pub(super) fn document(document: &Document, cx: &mut Context) -> String {
    let options = &document.options;
    let frame_level = options.headline_levels.unwrap_or(1).max(1);
    let mut body = Vec::new();
    let mut open: Vec<Open> = Vec::new();

    for element in &document.elements {
        match element {
            Element::Heading(heading) if heading.level < frame_level => {
                close(&mut open, &mut body, 0, true);
                let command = match heading.level {
                    1 => "section",
                    2 => "subsection",
                    _ => "subsubsection",
                };
                body.push(format!(
                    "\\{command}{{{}}}",
                    heading_title(heading, document, cx)
                ));
            }
            Element::Heading(heading) if heading.level == frame_level => {
                close(&mut open, &mut body, 0, true);
                // `fragile`, so that verbatim text in the frame works.
                body.push(format!(
                    "\\begin{{frame}}[fragile]{{{}}}\n\\label{{{}}}",
                    heading_title(heading, document, cx),
                    heading.anchor
                ));
                open.push(Open::Frame);
            }
            Element::Heading(heading) => {
                close(&mut open, &mut body, heading.level, false);
                if open.is_empty() {
                    body.push("\\begin{frame}[fragile]".to_string());
                    open.push(Open::Frame);
                }
                body.push(format!(
                    "\\begin{{block}}{{{}}}",
                    heading_title(heading, document, cx)
                ));
                open.push(Open::Block(heading.level));
            }
            other => {
                let text = latex::elements(std::slice::from_ref(other), document, cx);
                if text.is_empty() {
                    continue;
                }
                // Text outside any frame gets one: a slide shows nothing else.
                if open.is_empty() {
                    body.push("\\begin{frame}[fragile]".to_string());
                    open.push(Open::Frame);
                }
                body.push(text);
            }
        }
    }
    close(&mut open, &mut body, 0, true);

    if options.body_only {
        let mut text = body.join("\n");
        text.push('\n');
        return text;
    }

    let mut out = vec![
        "\\documentclass{beamer}".to_string(),
        "\\usepackage[utf8]{inputenc}".to_string(),
        "\\usepackage[T1]{fontenc}".to_string(),
        "\\usepackage{graphicx}".to_string(),
        "\\usepackage[normalem]{ulem}".to_string(),
        "\\usepackage{amsmath}".to_string(),
        "\\usepackage{amssymb}".to_string(),
        "\\usepackage{hyperref}".to_string(),
        format!(
            "\\usetheme{{{}}}",
            options.beamer_theme.as_deref().unwrap_or("default")
        ),
    ];
    if let Some(author) = &options.author {
        out.push(format!("\\author{{{}}}", latex::escape(author)));
    }
    out.push(format!(
        "\\date{{{}}}",
        options
            .date
            .as_deref()
            .map_or_else(|| "\\today".to_string(), latex::escape)
    ));
    let title = options
        .title
        .as_deref()
        .map(latex::escape)
        .unwrap_or_default();
    out.push(format!("\\title{{{title}}}"));
    out.push("\\begin{document}".to_string());
    out.push(String::new());
    if options.title.is_some() {
        out.push("\\maketitle".to_string());
    }
    if options.toc.is_some()
        && document
            .elements
            .iter()
            .any(|e| matches!(e, Element::Heading(h) if h.level < frame_level))
    {
        out.push("\\begin{frame}{Outline}\n\\tableofcontents\n\\end{frame}".to_string());
    }
    out.extend(body);
    out.push("\\end{document}".to_string());
    let mut text = out.join("\n");
    text.push('\n');
    text
}
