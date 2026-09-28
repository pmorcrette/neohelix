//! The export backends beyond Markdown, HTML and LaTeX.

use std::path::Path;

use helix_roam::export::{export, export_with, Backend, NoResolve};

const DOC: &str = "\
#+TITLE: Guide
#+AUTHOR: Ann
#+OPTIONS: toc:nil
Intro with *bold* and =code= and a [[https://example.com][link]].[fn:1]

* TODO First
- one
- [X] two
** Details
| a | b |
|---+---|
| 1 | 22 |
#+begin_src sh
echo  hi
#+end_src
* Second :noexport:
Hidden.
* Third
Last.

[fn:1] A note.
";

fn run(backend: Backend) -> String {
    export(DOC, Path::new("/notes/guide.org"), backend, &NoResolve).content
}

#[test]
fn ascii_underlines_numbers_and_draws() {
    let out = run(Backend::Ascii);
    assert!(out.starts_with("Guide\n=====\n\nAnn\n"), "{out}");
    assert!(out.contains("1 TODO First\n============"), "{out}");
    assert!(out.contains("1.1 Details\n-----------"), "{out}");
    assert!(out.contains("- one\n- [X] two"), "{out}");
    assert!(
        out.contains("+---+----+\n| a | b  |\n+---+----+\n| 1 | 22 |\n+---+----+"),
        "{out}"
    );
    assert!(
        out.contains("*bold* and `code' and a link <https://example.com>.[1]"),
        "{out}"
    );
    assert!(out.contains("  echo  hi"), "{out}");
    assert!(out.contains("[1] A note."), "{out}");
    assert!(!out.contains("Hidden"), "{out}");
}

#[test]
fn utf8_uses_unicode() {
    let out = run(Backend::Utf8);
    assert!(out.contains("1 TODO First\n════════════"), "{out}");
    assert!(out.contains("• one\n• ☑ two"), "{out}");
    assert!(out.contains("┌───┬────┐"), "{out}");
    assert!(out.contains("‘code’"), "{out}");
}

#[test]
fn man_pages_have_sections_and_escapes() {
    let out = run(Backend::Man);
    assert!(out.starts_with(".TH \"GUIDE\" \"1\""), "{out}");
    assert!(out.contains(".SH \"FIRST\""), "{out}");
    assert!(out.contains(".SS \"Details\""), "{out}");
    assert!(out.contains("\\fBbold\\fP"), "{out}");
    assert!(out.contains(".IP \\(bu 4\none"), "{out}");
    assert!(out.contains(".TS\n"), "{out}");
    assert!(out.contains(".nf\necho  hi\n.fi"), "{out}");
    assert!(out.contains(".SH \"NOTES\""), "{out}");
    assert!(out.contains(".SH \"AUTHOR\""), "{out}");
}

#[test]
fn texinfo_has_nodes_menus_and_bye() {
    let out = run(Backend::Texinfo);
    assert!(
        out.starts_with("\\input texinfo\n@setfilename guide.info"),
        "{out}"
    );
    assert!(out.contains("@node Top\n@top Guide"), "{out}");
    // Top's menu, then the chapter and its own menu before its section.
    assert!(
        out.contains("@menu\n* First::\n* Third::\n@end menu"),
        "{out}"
    );
    assert!(out.contains("@node First\n@chapter TODO First"), "{out}");
    assert!(
        out.contains("@menu\n* Details::\n@end menu\n\n@node Details\n@section Details"),
        "{out}"
    );
    assert!(out.contains("@strong{bold}"), "{out}");
    assert!(out.contains("@footnote{A note.}"), "{out}");
    assert!(out.contains("@multitable"), "{out}");
    assert!(out.trim_end().ends_with("@bye"), "{out}");
}

#[test]
fn beamer_makes_frames_at_the_frame_level() {
    let out = export(
        "#+TITLE: Talk\n#+OPTIONS: H:2\n* Part\n** Slide\nText.\n*** Aside\nNote.\n** Other\nMore.\n",
        Path::new("/notes/talk.org"),
        Backend::Beamer,
        &NoResolve,
    )
    .content;
    assert!(out.contains("\\documentclass{beamer}"), "{out}");
    assert!(out.contains("\\section{Part}"), "{out}");
    assert!(
        out.contains(
            "\\begin{frame}[fragile]{Slide}\n\\label{slide}\nText.\n\\begin{block}{Aside}\nNote.\n\\end{block}\n\\end{frame}"
        ),
        "{out}"
    );
    assert!(out.contains("\\begin{frame}[fragile]{Other}"), "{out}");
}

#[test]
fn odt_is_a_zip_with_the_mimetype_first() {
    let exported = export(DOC, Path::new("/notes/guide.org"), Backend::Odt, &NoResolve);
    let bytes = exported.bytes();
    assert_eq!(&bytes[..4], b"PK\x03\x04");
    // The first entry is `mimetype`, stored, so its content is at a fixed
    // place: what tools look at to know the file.
    assert_eq!(&bytes[30..38], b"mimetype");
    assert_eq!(&bytes[38..77], b"application/vnd.oasis.opendocument.text");
    let text = String::from_utf8_lossy(bytes);
    assert!(
        text.contains("<text:h text:style-name=\"Heading_20_1\""),
        "{text}"
    );
    assert!(text.contains("<text:span text:style-name=\"Bold\">bold</text:span>"));
    assert!(text.contains("echo <text:s text:c=\"1\"/>hi"));
    assert!(text.contains("<text:note "));
    assert!(!text.contains("Hidden"));
}

#[test]
fn org_resolves_includes_and_drops_what_is_not_exported() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("part.org"), "* Included\nFrom elsewhere.\n").unwrap();
    let source = dir.path().join("main.org");
    let text = "#+TITLE: Main\n# a comment\n#+INCLUDE: \"part.org\"\n* Kept\n* COMMENT Draft\nNo.\n* Private :noexport:\n** Deep\n* After\n";
    let out = export(text, &source, Backend::Org, &NoResolve).content;
    assert_eq!(
        out,
        "#+TITLE: Main\n* Included\nFrom elsewhere.\n* Kept\n* After\n"
    );
}

#[test]
fn body_only_leaves_the_document_around_out() {
    let html = export_with(
        DOC,
        Path::new("/notes/guide.org"),
        Backend::Html,
        &NoResolve,
        true,
    )
    .content;
    assert!(!html.contains("<html"), "{html}");
    assert!(html.contains("<p>\nIntro with"), "{html}");
    assert!(html.contains("<div id=\"footnotes\">"), "{html}");
    let latex = export_with(
        DOC,
        Path::new("/notes/guide.org"),
        Backend::Latex,
        &NoResolve,
        true,
    )
    .content;
    assert!(!latex.contains("\\documentclass"), "{latex}");
    assert!(latex.contains("\\section"), "{latex}");
    let text = export_with(
        DOC,
        Path::new("/notes/guide.org"),
        Backend::Ascii,
        &NoResolve,
        true,
    )
    .content;
    assert!(!text.starts_with("Guide"), "{text}");
}

#[test]
fn a_subtree_is_a_document_of_its_own() {
    use helix_roam::export::subtree;

    let text = "#+TITLE: Whole\n#+OPTIONS: toc:nil\n* Before\n** Chapter\nSCHEDULED: <2026-10-01 Thu>\n:PROPERTIES:\n:EXPORT_FILE_NAME: chapter-out\n:END:\nIntro.\n*** Part\nBody.\n** After\n";
    let found = subtree(text, 8).unwrap();
    assert_eq!(found.title, "Chapter");
    assert_eq!(found.file_name.as_deref(), Some("chapter-out"));
    assert_eq!(found.anchor, "chapter");
    assert_eq!(
        found.text,
        "#+OPTIONS: toc:nil\n#+TITLE: Chapter\nIntro.\n* Part\nBody.\n"
    );
    let html = export(
        &found.text,
        Path::new("/notes/a.org"),
        Backend::Html,
        &NoResolve,
    )
    .content;
    assert!(html.contains("<title>Chapter</title>"), "{html}");
    assert!(!html.contains("After"), "{html}");
}
