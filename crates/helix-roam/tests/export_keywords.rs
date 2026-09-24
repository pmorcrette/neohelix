//! `#+MACRO:`, `#+INCLUDE:`, and citations with `#+BIBLIOGRAPHY:`,
//! `#+CITE_EXPORT:` and `#+PRINT_BIBLIOGRAPHY:` in export.

use std::path::Path;

use helix_roam::export::{export, Backend, NoResolve};

fn setup() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("refs.bib"),
        "@article{doe2020, author = {Doe, Jane and Smith, John}, title = {Things}, journal = {J}, year = 2020}\n\
         @book{solo, author = {Ann Other}, title = {Alone}, year = {2019}}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("part.org"),
        "* Included\nText from elsewhere.\n** Deeper\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("code.rs"),
        "fn a() {}\nfn b() {}\nfn c() {}\n",
    )
    .unwrap();
    dir
}

fn md(dir: &Path, text: &str) -> (String, Vec<String>) {
    let out = export(
        &format!("#+OPTIONS: toc:nil\n{text}"),
        &dir.join("main.org"),
        Backend::Markdown,
        &NoResolve,
    );
    (out.content, out.warnings)
}

#[test]
fn macros_expand_with_their_arguments_and_the_built_ins() {
    let dir = setup();
    let (out, warnings) = md(
        dir.path(),
        "#+TITLE: Notes\n#+MACRO: greet Hello, $1 and $2!\n#+VERSION: 1.2\n\
         {{{greet(Ann, Bob\\, Jr.)}}} This is {{{title}}}, v{{{keyword(VERSION)}}}, from {{{input-file}}}.\n\
         {{{nothing}}}\n",
    );
    assert!(
        out.contains("Hello, Ann and Bob, Jr.! This is Notes, v1.2, from main.org."),
        "{out}"
    );
    assert!(out.contains("{{{nothing}}}"), "{out}");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
}

#[test]
fn macros_are_not_expanded_in_code() {
    let dir = setup();
    let (out, _) = md(
        dir.path(),
        "#+MACRO: x expanded\n#+begin_src sh\necho {{{x}}}\n#+end_src\n",
    );
    assert!(out.contains("echo {{{x}}}"), "{out}");
}

#[test]
fn includes_bring_org_text_or_blocks_in() {
    let dir = setup();
    let (out, _) = md(dir.path(), "* Top\n#+INCLUDE: \"part.org\" :minlevel 2\n");
    assert!(
        out.contains("# Top\n\n## Included\n\nText from elsewhere.\n\n### Deeper"),
        "{out}"
    );

    let (out, _) = md(
        dir.path(),
        "#+INCLUDE: \"code.rs\" src rust :lines \"2-3\"\n",
    );
    assert!(out.contains("```rust\nfn b() {}\nfn c() {}\n```"), "{out}");

    let (_, warnings) = md(dir.path(), "#+INCLUDE: \"missing.org\"\n");
    assert!(warnings[0].contains("missing.org"), "{warnings:?}");
}

#[test]
fn basic_citations_name_authors_and_years() {
    let dir = setup();
    let (out, warnings) = md(
        dir.path(),
        "#+BIBLIOGRAPHY: refs.bib\nAs shown [cite:@doe2020 p. 5], and [cite/t:@solo]; also [cite:see;@doe2020;@solo].\n\
         [cite:@ghost]\n\n#+PRINT_BIBLIOGRAPHY:\n",
    );
    assert!(out.contains("As shown (Doe and Smith, 2020, p. 5), and Other (2019); also see (Doe and Smith, 2020; Other, 2019)."), "{out}");
    assert!(out.contains("(ghost, ?)"), "{out}");
    assert!(
        warnings.iter().any(|w| w.contains("@ghost")),
        "{warnings:?}"
    );
    // Sorted as a bibliography, cited entries only.
    assert!(
        out.contains("- Ann Other (2019). Alone.\n- Doe, Jane; Smith, John (2020). Things. J."),
        "{out}"
    );
}

#[test]
fn html_links_a_citation_to_its_bibliography_entry() {
    let dir = setup();
    let out = export(
        "#+BIBLIOGRAPHY: refs.bib\n[cite:@solo]\n\n#+PRINT_BIBLIOGRAPHY:\n",
        &dir.path().join("main.org"),
        Backend::Html,
        &NoResolve,
    )
    .content;
    assert!(
        out.contains("(<a href=\"#bib-solo\">Other, 2019</a>)"),
        "{out}"
    );
    assert!(
        out.contains("<p class=\"bib-entry\" id=\"bib-solo\">Ann Other (2019). Alone.</p>"),
        "{out}"
    );
}

#[test]
fn latex_uses_the_processor_the_file_names() {
    let dir = setup();
    let latex = |processor: &str| {
        export(
            &format!("#+BIBLIOGRAPHY: refs.bib\n#+CITE_EXPORT: {processor}\n[cite:@doe2020 p. 5] [cite/t:@solo]\n\n#+PRINT_BIBLIOGRAPHY:\n"),
            &dir.path().join("main.org"),
            Backend::Latex,
            &NoResolve,
        )
        .content
    };
    let biblatex = latex("biblatex");
    assert!(
        biblatex.contains("\\usepackage[backend=biber]{biblatex}\n\\addbibresource{refs.bib}"),
        "{biblatex}"
    );
    assert!(
        biblatex.contains("\\autocite[p. 5]{doe2020} \\textcite{solo}"),
        "{biblatex}"
    );
    assert!(biblatex.contains("\\printbibliography"), "{biblatex}");

    let natbib = latex("natbib");
    assert!(
        natbib.contains("\\citep[p. 5]{doe2020} \\citet{solo}"),
        "{natbib}"
    );
    assert!(natbib.contains("\\bibliography{refs}"), "{natbib}");

    let basic = latex("basic");
    assert!(
        basic.contains("(Doe and Smith, 2020, p. 5) Other (2019)"),
        "{basic}"
    );
}
