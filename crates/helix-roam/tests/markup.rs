//! Emphasis, blocks, footnotes and citations.

use helix_roam::markup::{
    bib_keys, citation_at, footnote_at, footnote_definition, footnote_labels, footnote_reference,
    insert_block, insert_citation, insert_footnote, renumber_footnotes, toggle_emphasis,
    wrap_block, Emphasis,
};

#[test]
fn toggling_wraps_the_selection_in_its_marker() {
    // "world" is chars 6..11.
    assert_eq!(
        toggle_emphasis("hello world!", 6, 11, Emphasis::Bold).unwrap(),
        "hello *world*!"
    );
    assert_eq!(
        toggle_emphasis("hello world!", 6, 11, Emphasis::Code).unwrap(),
        "hello ~world~!"
    );
}

#[test]
fn toggling_again_takes_the_marker_off_however_it_was_selected() {
    let bold = "hello *world*!";

    // The selection holds the stars.
    assert_eq!(
        toggle_emphasis(bold, 6, 13, Emphasis::Bold).unwrap(),
        "hello world!"
    );
    // The selection is the word and the stars sit outside it.
    assert_eq!(
        toggle_emphasis(bold, 7, 12, Emphasis::Bold).unwrap(),
        "hello world!"
    );
}

#[test]
fn a_different_marker_nests_rather_than_removing_the_first() {
    assert_eq!(
        toggle_emphasis("hello *world*!", 7, 12, Emphasis::Italic).unwrap(),
        "hello */world/*!"
    );
}

#[test]
fn an_empty_selection_has_nothing_to_emphasise() {
    assert!(toggle_emphasis("hello", 2, 2, Emphasis::Bold).is_none());
    // A selection of nothing but spaces is empty once they are left out.
    assert!(toggle_emphasis("a   b", 1, 4, Emphasis::Bold).is_none());
}

#[test]
fn whitespace_at_the_edges_stays_outside_the_markers() {
    // Selecting a word takes its trailing space in most editors, and Org will
    // not render `*word *` — it shows the stars instead.
    assert_eq!(
        toggle_emphasis("Literate programming", 0, 9, Emphasis::Bold).unwrap(),
        "*Literate* programming"
    );
}

#[test]
fn every_marker_is_reachable_by_name() {
    assert_eq!(Emphasis::parse("Bold"), Some(Emphasis::Bold));
    assert_eq!(Emphasis::parse("s"), Some(Emphasis::Strike));
    assert_eq!(Emphasis::parse("shouty"), None);
    assert_eq!(Emphasis::Verbatim.marker(), '=');
}

const NOTE: &str = "\
* Heading
first line
second line
";

#[test]
fn a_block_is_inserted_after_the_line_with_the_cursor_on_its_inside() {
    let (out, line) = insert_block(NOTE, 1, "src", Some("rust"));

    assert_eq!(
        out,
        "\
* Heading
first line
#+BEGIN_SRC rust

#+END_SRC
second line
"
    );
    // The blank line between the delimiters, which is where you type.
    assert_eq!(line, 3);
}

#[test]
fn a_block_without_an_argument_has_no_trailing_space() {
    let (out, _) = insert_block(NOTE, 0, "quote", None);

    assert!(out.contains("#+BEGIN_QUOTE\n"));
    assert!(!out.contains("#+BEGIN_QUOTE \n"));
}

#[test]
fn wrapping_puts_the_delimiters_around_the_lines() {
    let (out, line) = wrap_block(NOTE, 1, 2, "example", None);

    assert_eq!(
        out,
        "\
* Heading
#+BEGIN_EXAMPLE
first line
second line
#+END_EXAMPLE
"
    );
    assert_eq!(line, 2);
}

const FOOTNOTES: &str = "\
Some text[fn:1] and more[fn:2].
A named one[fn:why] too.

[fn:1] the first note
[fn:2] the second
[fn:why] because
";

#[test]
fn a_label_is_read_from_either_end_of_a_footnote() {
    // Inside the reference on the first line.
    assert_eq!(footnote_at(FOOTNOTES, 12).as_deref(), Some("1"));
    // Inside the definition on the fourth.
    let definition = FOOTNOTES.find("[fn:why] because").unwrap();
    assert_eq!(
        footnote_at(FOOTNOTES, definition + 3).as_deref(),
        Some("why")
    );
}

#[test]
fn a_footnote_is_found_from_both_sides() {
    assert_eq!(footnote_definition(FOOTNOTES, "2"), Some(4));
    // The reference, not the definition: jumping from a definition to itself
    // is not jumping anywhere.
    assert_eq!(footnote_reference(FOOTNOTES, "2"), Some(0));
    assert_eq!(footnote_reference(FOOTNOTES, "why"), Some(1));
    assert_eq!(footnote_definition(FOOTNOTES, "nope"), None);
}

#[test]
fn labels_come_back_in_the_order_they_are_referred_to() {
    assert_eq!(footnote_labels(FOOTNOTES), ["1", "2", "why"]);
}

#[test]
fn a_new_footnote_takes_the_first_free_number() {
    let (out, at) = insert_footnote("Some text[fn:1] here.\n\n[fn:1] a note\n", 20);

    assert!(out.contains("Some text[fn:1] here[fn:2].\n"));
    assert!(out.ends_with("\n[fn:2] "));
    // The cursor lands where the note is written, not where it is referred to.
    assert_eq!(out.chars().count() - at, 0);
}

#[test]
fn renumbering_follows_the_order_the_references_appear_in() {
    let jumbled = "first[fn:3] second[fn:1].\n\n[fn:3] three\n[fn:1] one\n";
    let out = renumber_footnotes(jumbled);

    assert_eq!(
        out,
        "first[fn:1] second[fn:2].\n\n[fn:1] three\n[fn:2] one\n"
    );
}

#[test]
fn renumbering_leaves_a_named_footnote_named() {
    let out = renumber_footnotes(FOOTNOTES);

    assert!(out.contains("[fn:why]"));
    assert_eq!(out, FOOTNOTES);
}

const CITED: &str = "As shown [cite:@knuth1984] and [cite:@lamport1986;@knuth1984].\n";

#[test]
fn a_citation_key_is_read_from_under_the_cursor() {
    let at = CITED.find("knuth1984").unwrap();
    assert_eq!(citation_at(CITED, at + 2).as_deref(), Some("knuth1984"));

    let second = CITED.find("lamport1986").unwrap();
    assert_eq!(
        citation_at(CITED, second + 2).as_deref(),
        Some("lamport1986")
    );
}

#[test]
fn a_cursor_outside_a_citation_reads_none() {
    assert_eq!(citation_at(CITED, 2), None);
}

#[test]
fn inserting_a_citation_writes_the_whole_form() {
    assert_eq!(
        insert_citation("See .\n", 4, "knuth1984"),
        "See [cite:@knuth1984].\n"
    );
    // A key given with its sigil is not given two.
    assert_eq!(
        insert_citation("See .\n", 4, "@knuth1984"),
        "See [cite:@knuth1984].\n"
    );
}

#[test]
fn a_bibliography_gives_up_its_keys_and_nothing_else() {
    let bib = "\
@article{knuth1984,
  title = {Literate Programming},
}
@string{acm = {ACM}}
@book{lamport1986,
  title = {LaTeX},
}
";

    assert_eq!(bib_keys(bib), ["knuth1984", "lamport1986"]);
}
