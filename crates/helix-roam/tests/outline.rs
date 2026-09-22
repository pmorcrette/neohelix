//! Reading a buffer as an outline.

use helix_roam::outline::{
    at, headings, narrow, next, next_sibling, outline_path, parent, previous, previous_sibling,
    sparse_tree, Filter,
};

const FILE: &str = "\
#+title: Notes
* Projects
Some prose about projects.
** TODO Alpha :work:
alpha body
*** DONE Detail
** Beta
beta body
* Archive
old stuff
";

/// What the buffer looks like once `ranges` are hidden, one line per line.
///
/// Expressed as the text that survives rather than as character offsets: the
/// offsets are what the code computes, and a test that recomputed them by hand
/// would only check the arithmetic against itself.
fn visible(text: &str, ranges: &[(usize, usize)]) -> String {
    let hidden: Vec<bool> = (0..text.chars().count())
        .map(|at| ranges.iter().any(|(from, to)| (*from..*to).contains(&at)))
        .collect();

    let mut out = String::new();
    let mut marked = false;
    for (at, c) in text.chars().enumerate() {
        if hidden[at] {
            if !marked {
                out.push('…');
                marked = true;
            }
            continue;
        }
        marked = false;
        out.push(c);
    }
    out
}

#[test]
fn every_headline_is_read_with_its_metadata() {
    let entries = headings(FILE);

    assert_eq!(entries.len(), 5);
    assert_eq!(entries[1].title, "Alpha");
    assert_eq!(entries[1].level, 2);
    assert_eq!(entries[1].line, 3);
    assert_eq!(entries[1].tags, ["work"]);
    assert_eq!(
        entries[1].todo.as_ref().map(|state| state.keyword.as_str()),
        Some("TODO")
    );
    // `#+title:` is a keyword, not a headline.
    assert_eq!(entries[0].title, "Projects");
}

#[test]
fn a_line_belongs_to_the_headline_above_it() {
    let entries = headings(FILE);

    assert_eq!(at(&entries, 0), None); // the `#+title:` line
    assert_eq!(at(&entries, 2), Some(0)); // prose under Projects
    assert_eq!(at(&entries, 4), Some(1)); // alpha body
}

#[test]
fn the_outline_path_is_what_says_where_an_entry_is() {
    let entries = headings(FILE);

    // From inside Detail, three levels deep.
    assert_eq!(outline_path(&entries, 5), ["Projects", "Alpha", "Detail"]);
    assert_eq!(outline_path(&entries, 8), ["Archive"]);
    assert!(outline_path(&entries, 0).is_empty());
}

#[test]
fn heading_motion_walks_the_file_in_order() {
    let entries = headings(FILE);

    assert_eq!(next(&entries, 3), Some(5));
    assert_eq!(previous(&entries, 5), Some(3));
    assert_eq!(next(&entries, 8), None);
    assert_eq!(previous(&entries, 1), None);
}

#[test]
fn sibling_motion_stays_inside_the_parent() {
    let entries = headings(FILE);

    // Alpha (line 3) to Beta (line 6), both under Projects.
    assert_eq!(next_sibling(&entries, 3), Some(6));
    assert_eq!(previous_sibling(&entries, 6), Some(3));

    // Beta has no next sibling: Archive is a level up, not a cousin.
    assert_eq!(next_sibling(&entries, 6), None);
    assert_eq!(previous_sibling(&entries, 3), None);
}

#[test]
fn going_up_lands_on_the_parent_not_the_previous_heading() {
    let entries = headings(FILE);

    // Detail's previous heading is Alpha, which is also its parent.
    assert_eq!(parent(&entries, 5), Some(3));
    // Beta's previous heading is Detail; its parent is Projects.
    assert_eq!(previous(&entries, 6), Some(5));
    assert_eq!(parent(&entries, 6), Some(1));
    assert_eq!(parent(&entries, 1), None);
}

#[test]
fn a_filter_reads_the_agendas_sigils_plus_a_text_search() {
    let filter = Filter::parse("TODO :work: #A /alpha");

    assert_eq!(filter.todo.keyword.as_deref(), Some("TODO"));
    assert_eq!(filter.todo.tag.as_deref(), Some("work"));
    assert_eq!(filter.todo.priority, Some('A'));
    assert_eq!(filter.text.as_deref(), Some("alpha"));
    assert!(!filter.is_empty());
    assert!(Filter::parse("").is_empty());
}

#[test]
fn a_sparse_tree_keeps_the_matches_and_the_path_to_them() {
    let out = visible(FILE, &sparse_tree(FILE, &Filter::parse("/detail")));

    // Detail matches; Projects and Alpha are how you get to it. Every body,
    // and every entry nobody asked for, is gone.
    assert_eq!(
        out,
        "\
…
* Projects…
** TODO Alpha :work:…
*** DONE Detail…
"
    );
}

#[test]
fn a_sparse_tree_by_tag_keeps_what_carries_the_tag() {
    let out = visible(FILE, &sparse_tree(FILE, &Filter::parse(":work:")));

    assert!(out.contains("** TODO Alpha :work:"));
    assert!(out.contains("* Projects"));
    assert!(!out.contains("*** DONE Detail"));
    assert!(!out.contains("* Archive"));
}

#[test]
fn a_sparse_tree_that_matches_nothing_hides_everything() {
    let out = visible(FILE, &sparse_tree(FILE, &Filter::parse("/nothing-here")));

    assert_eq!(out, "…\n");
}

#[test]
fn narrowing_shows_one_subtree_and_hides_both_sides_of_it() {
    // Line 3 is `** TODO Alpha`, whose subtree runs through Detail.
    let out = visible(FILE, &narrow(FILE, 3));

    assert_eq!(
        out,
        "\
…
** TODO Alpha :work:
alpha body
*** DONE Detail…
"
    );
}

#[test]
fn narrowing_to_the_last_subtree_hides_only_what_is_above_it() {
    let out = visible(FILE, &narrow(FILE, 8));

    assert_eq!(
        out,
        "\
…
* Archive
old stuff
"
    );
}

#[test]
fn narrowing_above_any_headline_has_nothing_to_narrow_to() {
    assert!(narrow(FILE, 0).is_empty());
}
