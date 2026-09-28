//! OpenDocument Text: `content.xml`, `styles.xml` and the rest, zipped.
//!
//! The zip is written here, stored rather than deflated: an `.odt` is
//! read the same either way, and it keeps the export free of a
//! compression library. The `mimetype` entry comes first and plain, as the
//! format asks, so a tool can tell what the file is from its first bytes.

use super::*;

/// Escapes XML's special characters.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// Text in a preformatted paragraph: spaces kept with `text:s`, tabs with
/// `text:tab`, lines broken with `text:line-break`.
fn preformatted(text: &str) -> String {
    let mut out = String::new();
    for (index, line) in text.trim_end_matches('\n').split('\n').enumerate() {
        if index > 0 {
            out.push_str("<text:line-break/>");
        }
        let chars: Vec<char> = line.chars().collect();
        let mut at = 0;
        while at < chars.len() {
            match chars[at] {
                ' ' => {
                    let run = chars[at..].iter().take_while(|c| **c == ' ').count();
                    // One space between words is a space; any other run, or
                    // spaces opening the line, are counted.
                    if at == 0 {
                        out.push_str(&format!("<text:s text:c=\"{run}\"/>"));
                    } else if run == 1 {
                        out.push(' ');
                    } else {
                        out.push_str(&format!(" <text:s text:c=\"{}\"/>", run - 1));
                    }
                    at += run;
                }
                '\t' => {
                    out.push_str("<text:tab/>");
                    at += 1;
                }
                c => {
                    out.push_str(&escape(&c.to_string()));
                    at += 1;
                }
            }
        }
    }
    out
}

fn inlines(items: &[Inline], document: &Document, cx: &mut Context) -> String {
    items
        .iter()
        .map(|inline| one(inline, document, cx))
        .collect()
}

fn span(style: &str, inner: String) -> String {
    format!("<text:span text:style-name=\"{style}\">{inner}</text:span>")
}

fn one(inline: &Inline, document: &Document, cx: &mut Context) -> String {
    match inline {
        Inline::Text(text) => escape(text),
        Inline::Bold(inner) => span("Bold", inlines(inner, document, cx)),
        Inline::Italic(inner) => span("Italic", inlines(inner, document, cx)),
        Inline::Underline(inner) => span("Underline", inlines(inner, document, cx)),
        Inline::Strike(inner) => span("Strike", inlines(inner, document, cx)),
        Inline::Verbatim(text) | Inline::Code(text) => span("Code", escape(text)),
        Inline::Link {
            target,
            description,
        } => {
            let label = match description {
                Some(inner) => inlines(inner, document, cx),
                None => escape(&cx.default_label(target)),
            };
            match cx.href(target, description.is_some()) {
                Href::Link(href) => format!(
                    "<text:a xlink:type=\"simple\" xlink:href=\"{}\">{label}</text:a>",
                    escape(&href)
                ),
                // The picture is not carried into the archive; its path is.
                Href::Image(src) => format!("[{}]", escape(&src)),
                Href::None => label,
            }
        }
        Inline::Footnote(label) => {
            let number = cx.footnote_number(label);
            let body = footnote_definition(document, label)
                .map(|inline| inlines(inline, document, cx))
                .unwrap_or_default();
            format!(
                "<text:note text:id=\"ftn{number}\" text:note-class=\"footnote\"><text:note-citation>{number}</text:note-citation><text:note-body><text:p text:style-name=\"Footnote\">{body}</text:p></text:note-body></text:note>"
            )
        }
        Inline::LineBreak => "<text:line-break/>".to_string(),
        Inline::Citation {
            style,
            cites,
            prefix,
            suffix,
        } => escape(&cx.basic_citation(style.as_deref(), cites, prefix, suffix, &|_, body| body)),
    }
}

fn paragraph(style: &str, inner: String) -> String {
    format!("<text:p text:style-name=\"{style}\">{inner}</text:p>")
}

fn elements(items: &[Element], document: &Document, cx: &mut Context) -> String {
    items
        .iter()
        .map(|element| element_odt(element, document, cx))
        .collect()
}

fn element_odt(element: &Element, document: &Document, cx: &mut Context) -> String {
    let options = &document.options;
    match element {
        Element::Heading(heading) => {
            let mut text = String::new();
            if let (true, Some((keyword, _))) = (options.todo, &heading.todo) {
                text.push_str(&span("Bold", escape(keyword)));
                text.push(' ');
            }
            text.push_str(&inlines(&heading.title, document, cx));
            if options.tags && !heading.tags.is_empty() {
                text.push_str(&format!(" <text:tab/>{}", escape(&heading.tags.join(":"))));
            }
            let level = heading.level.min(6);
            format!(
                "<text:h text:style-name=\"Heading_20_{level}\" text:outline-level=\"{level}\"><text:bookmark text:name=\"{}\"/>{text}</text:h>",
                escape(&heading.anchor)
            )
        }
        Element::Paragraph(items) => paragraph("Text_20_body", inlines(items, document, cx)),
        Element::List(kind, items) => {
            let style = match kind {
                ListKind::Ordered => "Numbering",
                _ => "Bullets",
            };
            let mut out = format!("<text:list text:style-name=\"{style}\">");
            for item in items {
                let mut head = String::new();
                if let Some(mark) = item.checkbox {
                    head.push_str(if mark == ' ' { "☐ " } else { "☑ " });
                }
                if let Some(term) = &item.term {
                    head.push_str(&span("Bold", inlines(term, document, cx)));
                    head.push_str(": ");
                }
                out.push_str("<text:list-item>");
                let mut first = true;
                for element in &item.content {
                    match element {
                        Element::Paragraph(items) if first => {
                            out.push_str(&paragraph(
                                "List_20_Contents",
                                format!("{head}{}", inlines(items, document, cx)),
                            ));
                        }
                        other => {
                            if first && !head.is_empty() {
                                out.push_str(&paragraph("List_20_Contents", head.clone()));
                            }
                            out.push_str(&element_odt(other, document, cx));
                        }
                    }
                    first = false;
                }
                if item.content.is_empty() {
                    out.push_str(&paragraph("List_20_Contents", head));
                }
                out.push_str("</text:list-item>");
            }
            out.push_str("</text:list>");
            out
        }
        Element::Table(rows, header) => {
            let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
            let mut out = format!(
                "<table:table table:style-name=\"Table\"><table:table-column table:number-columns-repeated=\"{columns}\"/>"
            );
            for (index, row) in rows.iter().enumerate() {
                let style = if index < *header {
                    "Table_20_Heading"
                } else {
                    "Table_20_Contents"
                };
                out.push_str("<table:table-row>");
                for at in 0..columns {
                    let cell = row
                        .get(at)
                        .map(|cell| inlines(cell, document, cx))
                        .unwrap_or_default();
                    out.push_str(&format!(
                        "<table:table-cell table:style-name=\"Cell\" office:value-type=\"string\">{}</table:table-cell>",
                        paragraph(style, cell)
                    ));
                }
                out.push_str("</table:table-row>");
            }
            out.push_str("</table:table>");
            out
        }
        Element::Code { code, .. } | Element::Example(code) => {
            paragraph("Preformatted_20_Text", preformatted(code))
        }
        Element::Quote(inner) => elements(inner, document, cx).replace(
            "text:style-name=\"Text_20_body\"",
            "text:style-name=\"Quotations\"",
        ),
        Element::Verse(lines) => paragraph(
            "Quotations",
            lines
                .iter()
                .map(|line| inlines(line, document, cx))
                .collect::<Vec<_>>()
                .join("<text:line-break/>"),
        ),
        Element::Center(inner) => elements(inner, document, cx).replace(
            "text:style-name=\"Text_20_body\"",
            "text:style-name=\"Center\"",
        ),
        Element::Special(_, inner) => elements(inner, document, cx),
        Element::Export { backend, text } => {
            if backend == "odt" {
                text.clone()
            } else {
                String::new()
            }
        }
        Element::Rule => paragraph("Horizontal_20_Line", String::new()),
        Element::Bibliography => cx
            .references
            .iter()
            .map(|(_, reference)| paragraph("Text_20_body", escape(reference)))
            .collect(),
    }
}

const NAMESPACES: &str = "xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" \
    xmlns:style=\"urn:oasis:names:tc:opendocument:xmlns:style:1.0\" \
    xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\" \
    xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\" \
    xmlns:fo=\"urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0\" \
    xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
    xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
    xmlns:meta=\"urn:oasis:names:tc:opendocument:xmlns:meta:1.0\" \
    xmlns:svg=\"urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0\" \
    office:version=\"1.3\"";

/// The styles the content names, so that the document looks the same in
/// any office suite.
fn styles() -> String {
    let mut headings = String::new();
    for (level, size) in [
        (1, "150%"),
        (2, "130%"),
        (3, "115%"),
        (4, "105%"),
        (5, "100%"),
        (6, "100%"),
    ] {
        headings.push_str(&format!(
            "<style:style style:name=\"Heading_20_{level}\" style:display-name=\"Heading {level}\" style:family=\"paragraph\" style:parent-style-name=\"Heading\" style:next-style-name=\"Text_20_body\" style:default-outline-level=\"{level}\"><style:text-properties fo:font-size=\"{size}\" fo:font-weight=\"bold\"/></style:style>"
        ));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<office:document-styles {NAMESPACES}>\
<office:font-face-decls><style:font-face style:name=\"Monospace\" svg:font-family=\"monospace\" style:font-pitch=\"fixed\"/></office:font-face-decls>\
<office:styles>\
<style:style style:name=\"Standard\" style:family=\"paragraph\"><style:text-properties fo:font-size=\"11pt\"/></style:style>\
<style:style style:name=\"Heading\" style:family=\"paragraph\" style:parent-style-name=\"Standard\"><style:paragraph-properties fo:margin-top=\"0.3cm\" fo:margin-bottom=\"0.15cm\" fo:keep-with-next=\"always\"/></style:style>\
{headings}\
<style:style style:name=\"Text_20_body\" style:display-name=\"Text body\" style:family=\"paragraph\" style:parent-style-name=\"Standard\"><style:paragraph-properties fo:margin-bottom=\"0.2cm\"/></style:style>\
<style:style style:name=\"Title\" style:family=\"paragraph\" style:parent-style-name=\"Heading\"><style:paragraph-properties fo:text-align=\"center\"/><style:text-properties fo:font-size=\"180%\" fo:font-weight=\"bold\"/></style:style>\
<style:style style:name=\"Subtitle\" style:family=\"paragraph\" style:parent-style-name=\"Standard\"><style:paragraph-properties fo:text-align=\"center\"/></style:style>\
<style:style style:name=\"Center\" style:family=\"paragraph\" style:parent-style-name=\"Text_20_body\"><style:paragraph-properties fo:text-align=\"center\"/></style:style>\
<style:style style:name=\"Quotations\" style:family=\"paragraph\" style:parent-style-name=\"Text_20_body\"><style:paragraph-properties fo:margin-left=\"1cm\" fo:margin-right=\"1cm\"/></style:style>\
<style:style style:name=\"Preformatted_20_Text\" style:display-name=\"Preformatted Text\" style:family=\"paragraph\" style:parent-style-name=\"Standard\"><style:paragraph-properties fo:margin-bottom=\"0.2cm\"/><style:text-properties style:font-name=\"Monospace\" fo:font-size=\"10pt\"/></style:style>\
<style:style style:name=\"List_20_Contents\" style:display-name=\"List Contents\" style:family=\"paragraph\" style:parent-style-name=\"Standard\"/>\
<style:style style:name=\"Table_20_Contents\" style:display-name=\"Table Contents\" style:family=\"paragraph\" style:parent-style-name=\"Standard\"/>\
<style:style style:name=\"Table_20_Heading\" style:display-name=\"Table Heading\" style:family=\"paragraph\" style:parent-style-name=\"Table_20_Contents\"><style:text-properties fo:font-weight=\"bold\"/></style:style>\
<style:style style:name=\"Footnote\" style:family=\"paragraph\" style:parent-style-name=\"Standard\"><style:text-properties fo:font-size=\"9pt\"/></style:style>\
<style:style style:name=\"Horizontal_20_Line\" style:display-name=\"Horizontal Line\" style:family=\"paragraph\" style:parent-style-name=\"Standard\"><style:paragraph-properties fo:border-bottom=\"0.5pt solid #000000\" fo:margin-bottom=\"0.3cm\"/></style:style>\
<text:list-style style:name=\"Bullets\">\
<text:list-level-style-bullet text:level=\"1\" text:bullet-char=\"•\"><style:list-level-properties text:space-before=\"0.4cm\" text:min-label-width=\"0.4cm\"/></text:list-level-style-bullet>\
<text:list-level-style-bullet text:level=\"2\" text:bullet-char=\"◦\"><style:list-level-properties text:space-before=\"0.8cm\" text:min-label-width=\"0.4cm\"/></text:list-level-style-bullet>\
<text:list-level-style-bullet text:level=\"3\" text:bullet-char=\"▪\"><style:list-level-properties text:space-before=\"1.2cm\" text:min-label-width=\"0.4cm\"/></text:list-level-style-bullet>\
</text:list-style>\
<text:list-style style:name=\"Numbering\">\
<text:list-level-style-number text:level=\"1\" style:num-suffix=\".\" style:num-format=\"1\"><style:list-level-properties text:space-before=\"0.4cm\" text:min-label-width=\"0.6cm\"/></text:list-level-style-number>\
<text:list-level-style-number text:level=\"2\" style:num-suffix=\".\" style:num-format=\"a\"><style:list-level-properties text:space-before=\"1cm\" text:min-label-width=\"0.6cm\"/></text:list-level-style-number>\
</text:list-style>\
</office:styles>\
</office:document-styles>\n"
    )
}

/// The automatic styles the content's spans and tables use.
const AUTOMATIC: &str = "<office:automatic-styles>\
<style:style style:name=\"Bold\" style:family=\"text\"><style:text-properties fo:font-weight=\"bold\"/></style:style>\
<style:style style:name=\"Italic\" style:family=\"text\"><style:text-properties fo:font-style=\"italic\"/></style:style>\
<style:style style:name=\"Underline\" style:family=\"text\"><style:text-properties style:text-underline-style=\"solid\" style:text-underline-width=\"auto\" style:text-underline-color=\"font-color\"/></style:style>\
<style:style style:name=\"Strike\" style:family=\"text\"><style:text-properties style:text-line-through-style=\"solid\"/></style:style>\
<style:style style:name=\"Code\" style:family=\"text\"><style:text-properties style:font-name=\"Monospace\"/></style:style>\
<style:style style:name=\"Table\" style:family=\"table\"><style:table-properties table:border-model=\"collapsing\"/></style:style>\
<style:style style:name=\"Cell\" style:family=\"table-cell\"><style:table-cell-properties fo:padding=\"0.1cm\" fo:border=\"0.5pt solid #000000\"/></style:style>\
</office:automatic-styles>";

fn meta(document: &Document) -> String {
    let options = &document.options;
    let mut fields = String::from("<meta:generator>neohelix</meta:generator>");
    if let Some(title) = &options.title {
        fields.push_str(&format!("<dc:title>{}</dc:title>", escape(title)));
    }
    if let Some(author) = &options.author {
        fields.push_str(&format!("<dc:creator>{}</dc:creator>", escape(author)));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<office:document-meta {NAMESPACES}><office:meta>{fields}</office:meta></office:document-meta>\n"
    )
}

const MANIFEST: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<manifest:manifest xmlns:manifest=\"urn:oasis:names:tc:opendocument:xmlns:manifest:1.0\" manifest:version=\"1.3\">\
<manifest:file-entry manifest:full-path=\"/\" manifest:version=\"1.3\" manifest:media-type=\"application/vnd.oasis.opendocument.text\"/>\
<manifest:file-entry manifest:full-path=\"content.xml\" manifest:media-type=\"text/xml\"/>\
<manifest:file-entry manifest:full-path=\"styles.xml\" manifest:media-type=\"text/xml\"/>\
<manifest:file-entry manifest:full-path=\"meta.xml\" manifest:media-type=\"text/xml\"/>\
</manifest:manifest>\n";

/// The document's `content.xml`.
fn content(document: &Document, cx: &mut Context) -> String {
    let options = &document.options;
    let mut body = String::new();
    if !options.body_only {
        if let Some(title) = &options.title {
            body.push_str(&paragraph("Title", escape(title)));
        }
        let byline: Vec<String> = [options.author.as_deref(), options.date.as_deref()]
            .into_iter()
            .flatten()
            .map(escape)
            .collect();
        if !byline.is_empty() {
            body.push_str(&paragraph("Subtitle", byline.join(" — ")));
        }
        if options.toc.is_some() {
            body.push_str(
                "<text:table-of-content text:name=\"Contents\"><text:table-of-content-source text:outline-level=\"10\"><text:index-title-template>Table of Contents</text:index-title-template></text:table-of-content-source><text:index-body/></text:table-of-content>",
            );
        }
    }
    body.push_str(&elements(&document.elements, document, cx));
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<office:document-content {NAMESPACES}>{AUTOMATIC}<office:body><office:text>{body}</office:text></office:body></office:document-content>\n"
    )
}

pub(super) fn document(document: &Document, cx: &mut Context) -> Vec<u8> {
    let content = content(document, cx);
    zip(&[
        (
            "mimetype",
            b"application/vnd.oasis.opendocument.text".as_slice(),
        ),
        ("META-INF/manifest.xml", MANIFEST.as_bytes()),
        ("content.xml", content.as_bytes()),
        ("styles.xml", styles().as_bytes()),
        ("meta.xml", meta(document).as_bytes()),
    ])
}

/// CRC-32, as zip wants it (the IEEE polynomial, reflected).
pub(super) fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (n, slot) in table.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *slot = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc = table[((crc ^ byte as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

/// A zip archive of `files`, stored, in order.
pub(super) fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    // 1980-01-01 00:00, zip's earliest: the export needs no clock.
    let (time, date): (u16, u16) = (0, (1 << 5) | 1);
    for (name, data) in files {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let size = data.len() as u32;
        let name = name.as_bytes();

        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // stored
        out.extend_from_slice(&time.to_le_bytes());
        out.extend_from_slice(&date.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // no extra field
        out.extend_from_slice(name);
        out.extend_from_slice(data);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // made by
        central.extend_from_slice(&20u16.to_le_bytes()); // needed
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&time.to_le_bytes());
        central.extend_from_slice(&date.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        central.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
    }
    let central_offset = out.len() as u32;
    let central_size = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&central_size.to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}
