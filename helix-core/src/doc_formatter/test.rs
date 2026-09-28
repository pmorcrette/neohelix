use crate::doc_formatter::{DocumentFormatter, TextFormat};
use crate::fold::{Fold, Folds};
use crate::text_annotations::{InlineAnnotation, Overlay, TextAnnotations};

impl TextFormat {
    fn new_test(softwrap: bool) -> Self {
        TextFormat {
            soft_wrap: softwrap,
            tab_width: 2,
            max_wrap: 3,
            max_indent_retain: 4,
            wrap_indicator: ".".into(),
            wrap_indicator_highlight: None,
            // use a prime number to allow lining up too often with repeat
            viewport_width: 17,
            soft_wrap_at_text_width: false,
            fold_marker: "…".into(),
        }
    }
}

impl<'t> DocumentFormatter<'t> {
    fn collect_to_str(&mut self) -> String {
        use std::fmt::Write;
        let mut res = String::new();
        let viewport_width = self.text_fmt.viewport_width;
        let soft_wrap_at_text_width = self.text_fmt.soft_wrap_at_text_width;
        let mut line = 0;

        for grapheme in self {
            if grapheme.visual_pos.row != line {
                line += 1;
                assert_eq!(grapheme.visual_pos.row, line);
                write!(res, "\n{}", ".".repeat(grapheme.visual_pos.col)).unwrap();
            }
            if !soft_wrap_at_text_width {
                assert!(
                    grapheme.visual_pos.col <= viewport_width as usize,
                    "softwrapped failed {}<={viewport_width}",
                    grapheme.visual_pos.col
                );
            }
            write!(res, "{}", grapheme.raw).unwrap();
        }

        res
    }
}

fn softwrap_text(text: &str) -> String {
    DocumentFormatter::new_at_prev_checkpoint(
        text.into(),
        &TextFormat::new_test(true),
        &TextAnnotations::default(),
        0,
    )
    .collect_to_str()
}

#[test]
fn basic_softwrap() {
    assert_eq!(
        softwrap_text(&"foo ".repeat(10)),
        "foo foo foo foo \n.foo foo foo foo \n.foo foo  "
    );
    assert_eq!(
        softwrap_text(&"fooo ".repeat(10)),
        "fooo fooo fooo \n.fooo fooo fooo \n.fooo fooo fooo \n.fooo  "
    );

    // check that we don't wrap unnecessarily
    assert_eq!(softwrap_text("\t\txxxx1xxxx2xx\n"), "    xxxx1xxxx2xx \n ");
}

#[test]
fn softwrap_indentation() {
    assert_eq!(
        softwrap_text("\t\tfoo1 foo2 foo3 foo4 foo5 foo6\n"),
        "    foo1 foo2 \n.....foo3 foo4 \n.....foo5 foo6 \n "
    );
    assert_eq!(
        softwrap_text("\t\t\tfoo1 foo2 foo3 foo4 foo5 foo6\n"),
        "      foo1 foo2 \n.foo3 foo4 foo5 \n.foo6 \n "
    );
}

#[test]
fn long_word_softwrap() {
    assert_eq!(
        softwrap_text("\t\txxxx1xxxx2xxxx3xxxx4xxxx5xxxx6xxxx7xxxx8xxxx9xxx\n"),
        "    xxxx1xxxx2xxx\n.....x3xxxx4xxxx5\n.....xxxx6xxxx7xx\n.....xx8xxxx9xxx \n "
    );
    assert_eq!(
        softwrap_text("xxxxxxxx1xxxx2xxx\n"),
        "xxxxxxxx1xxxx2xxx\n. \n "
    );
    assert_eq!(
        softwrap_text("\t\txxxx1xxxx 2xxxx3xxxx4xxxx5xxxx6xxxx7xxxx8xxxx9xxx\n"),
        "    xxxx1xxxx \n.....2xxxx3xxxx4x\n.....xxx5xxxx6xxx\n.....x7xxxx8xxxx9\n.....xxx \n "
    );
    assert_eq!(
        softwrap_text("\t\txxxx1xxx 2xxxx3xxxx4xxxx5xxxx6xxxx7xxxx8xxxx9xxx\n"),
        "    xxxx1xxx 2xxx\n.....x3xxxx4xxxx5\n.....xxxx6xxxx7xx\n.....xx8xxxx9xxx \n "
    );
}

#[test]
fn softwrap_multichar_grapheme() {
    assert_eq!(
        softwrap_text("xxxx xxxx xxx a\u{0301}bc\n"),
        "xxxx xxxx xxx \n.ábc \n "
    )
}

fn softwrap_text_at_text_width(text: &str) -> String {
    let mut text_fmt = TextFormat::new_test(true);
    text_fmt.soft_wrap_at_text_width = true;
    let annotations = TextAnnotations::default();
    let mut formatter =
        DocumentFormatter::new_at_prev_checkpoint(text.into(), &text_fmt, &annotations, 0);
    formatter.collect_to_str()
}
#[test]
fn long_word_softwrap_text_width() {
    assert_eq!(
        softwrap_text_at_text_width("xxxxxxxx1xxxx2xxx\nxxxxxxxx1xxxx2xxx"),
        "xxxxxxxx1xxxx2xxx \nxxxxxxxx1xxxx2xxx "
    );
}

fn overlay_text(text: &str, char_pos: usize, softwrap: bool, overlays: &[Overlay]) -> String {
    DocumentFormatter::new_at_prev_checkpoint(
        text.into(),
        &TextFormat::new_test(softwrap),
        TextAnnotations::default().add_overlay(overlays, None),
        char_pos,
    )
    .collect_to_str()
}

#[test]
fn overlay() {
    assert_eq!(
        overlay_text(
            "foobar",
            0,
            false,
            &[Overlay::new(0, "X"), Overlay::new(2, "\t")],
        ),
        "Xo  bar "
    );
    assert_eq!(
        overlay_text(
            &"foo ".repeat(10),
            0,
            true,
            &[
                Overlay::new(2, "\t"),
                Overlay::new(5, "\t"),
                Overlay::new(16, "X"),
            ]
        ),
        "fo   f  o foo \n.foo Xoo foo foo \n.foo foo foo  "
    );
}

fn annotate_text(text: &str, softwrap: bool, annotations: &[InlineAnnotation]) -> String {
    DocumentFormatter::new_at_prev_checkpoint(
        text.into(),
        &TextFormat::new_test(softwrap),
        TextAnnotations::default().add_inline_annotations(annotations, None),
        0,
    )
    .collect_to_str()
}

#[test]
fn annotation() {
    assert_eq!(
        annotate_text("bar", false, &[InlineAnnotation::new(0, "foo")]),
        "foobar "
    );
    assert_eq!(
        annotate_text(
            &"foo ".repeat(10),
            true,
            &[InlineAnnotation::new(0, "foo ")]
        ),
        "foo foo foo foo \n.foo foo foo foo \n.foo foo foo  "
    );
}

#[test]
fn annotation_and_overlay() {
    let annotations = [InlineAnnotation {
        char_idx: 0,
        text: "fooo".into(),
    }];
    let overlay = [Overlay {
        char_idx: 0,
        grapheme: "\t".into(),
    }];
    assert_eq!(
        DocumentFormatter::new_at_prev_checkpoint(
            "bbar".into(),
            &TextFormat::new_test(false),
            TextAnnotations::default()
                .add_inline_annotations(annotations.as_slice(), None)
                .add_overlay(overlay.as_slice(), None),
            0,
        )
        .collect_to_str(),
        "fooo  bar "
    );
}

/// Renders `text` with `folds`, one line of output per visual row, each
/// prefixed by the document line the row belongs to.
fn folded(text: &str, folds: &[(usize, usize)], from: usize) -> String {
    use std::fmt::Write;

    let folds: Folds = folds
        .iter()
        .map(|&(start, end)| Fold::new(start, end))
        .collect();
    let mut annotations = TextAnnotations::default();
    annotations.add_folds(&folds);

    let text_fmt = TextFormat::new_test(false);
    let mut out = String::new();
    let mut row = usize::MAX;

    for grapheme in
        DocumentFormatter::new_at_prev_checkpoint(text.into(), &text_fmt, &annotations, from)
    {
        if grapheme.visual_pos.row != row {
            row = grapheme.visual_pos.row;
            if !out.is_empty() {
                out.push('\n');
            }
            write!(out, "{}|", grapheme.line_idx).unwrap();
        }
        if grapheme.raw != crate::graphemes::Grapheme::Newline {
            write!(out, "{}", grapheme.raw).unwrap();
        }
    }

    out
}

const OUTLINE: &str = "\
* One
body
more
* Two
";

#[test]
fn a_fold_hides_its_text_and_leaves_a_marker() {
    // Chars 5..15 are the newline after "* One" through the end of "more",
    // so the two body lines collapse onto the headline.
    assert_eq!(folded(OUTLINE, &[(5, 15)], 0), "0|* One…\n3|* Two\n4| ");
}

#[test]
fn the_lines_after_a_fold_keep_their_numbers() {
    // "* Two" is document line 3 whether or not the body above it is folded:
    // a fold hides text, it does not remove it.
    let open = folded(OUTLINE, &[], 0);
    assert!(open.contains("3|* Two"));
    assert!(folded(OUTLINE, &[(5, 15)], 0).contains("3|* Two"));
}

#[test]
fn the_characters_after_a_fold_keep_their_positions() {
    let text_fmt = TextFormat::new_test(false);
    let folds: Folds = [Fold::new(5, 15)].into_iter().collect();
    let mut annotations = TextAnnotations::default();
    annotations.add_folds(&folds);

    let star =
        DocumentFormatter::new_at_prev_checkpoint(OUTLINE.into(), &text_fmt, &annotations, 0)
            .find(|grapheme| grapheme.line_idx == 3)
            .unwrap();

    assert_eq!(star.char_idx, OUTLINE.find("* Two").unwrap());
}

#[test]
fn starting_inside_a_fold_backs_up_to_the_line_it_opens_on() {
    // Asking to render from the middle of hidden text must not render it.
    assert_eq!(folded(OUTLINE, &[(5, 15)], 8), "0|* One…\n3|* Two\n4| ");
}

#[test]
fn two_folds_on_one_buffer_both_collapse() {
    let text = "a\nb\nc\nd\ne\n";
    // 1..3 hides "\nb" and 5..7 hides "\nd".
    assert_eq!(folded(text, &[(1, 3), (5, 7)], 0), "0|a…\n2|c…\n4|e\n5| ");
}

/// Renders `text` with `conceals`, revealing `revealed`, as `folded` does.
fn concealed(
    text: &str,
    conceals: &[(usize, usize, &str)],
    revealed: Vec<std::ops::Range<usize>>,
) -> (String, Vec<usize>) {
    let conceals = crate::conceal::Conceals::new(
        conceals
            .iter()
            .map(|&(start, end, replacement)| crate::conceal::Conceal {
                start,
                end,
                replacement: replacement.to_string(),
            })
            .collect(),
    );
    let mut annotations = TextAnnotations::default();
    annotations.add_conceals(&conceals, revealed);
    let text_fmt = TextFormat::new_test(false);
    let mut out = String::new();
    let mut positions = Vec::new();
    for grapheme in
        DocumentFormatter::new_at_prev_checkpoint(text.into(), &text_fmt, &annotations, 0)
    {
        if grapheme.raw == crate::graphemes::Grapheme::Newline {
            out.push('|');
        } else {
            out.push_str(&grapheme.raw.to_string());
        }
        positions.push(grapheme.char_idx);
    }
    (out, positions)
}

#[test]
fn a_conceal_draws_its_replacement_and_keeps_positions() {
    // `\alpha` is chars 2..8; `[[x][ab]]` is chars 13..22, drawn as `ab`.
    let text = "a \\alpha b\n  [[x][ab]]!\n";
    let (out, positions) = concealed(
        text,
        &[(2, 8, "α"), (13, 19, "a"), (19, 22, "b")],
        Vec::new(),
    );
    assert_eq!(out, "a α b|  ab!| ");
    // The grapheme after `\alpha` is still at char 8, and `!` at 22.
    assert_eq!(positions[3], 8);
    assert_eq!(positions[positions.len() - 3], 22);
}

#[test]
fn a_revealed_line_is_drawn_as_it_is() {
    let text = "\\alpha\n\\beta\n";
    let (out, _) = concealed(
        text,
        &[(0, 6, "α"), (7, 12, "β")],
        std::iter::once(7..13).collect(),
    );
    assert_eq!(out, "α|\\beta| ");
}
