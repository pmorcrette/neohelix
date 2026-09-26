//! Sorting entries, list items and table rows.

use helix_roam::restructure::Error;
use helix_roam::sort::{sort_entries, sort_list, sort_table, SortKey};

const TASKS: &str = "\
* Inbox
Some preamble that belongs to Inbox.
** TODO Zebra
   DEADLINE: <2026-10-01 Thu>
   :PROPERTIES:
   :EFFORT:   3
   :END:
** DONE Apple
   DEADLINE: <2026-09-20 Sun>
   :PROPERTIES:
   :EFFORT:   10
   :END:
** TODO Mango
   :PROPERTIES:
   :EFFORT:   1
   :END:
";

fn titles(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| line.starts_with("** "))
        .map(|line| line.trim_start_matches('*').trim().to_string())
        .collect()
}

#[test]
fn sorting_children_leaves_the_parent_and_its_body_alone() {
    let out = sort_entries(TASKS, 0, &SortKey::Alphabetical, false).unwrap();

    assert!(out.starts_with("* Inbox\nSome preamble that belongs to Inbox.\n** DONE Apple\n"));
}

#[test]
fn an_entry_carries_its_drawer_and_planning_with_it() {
    let out = sort_entries(TASKS, 0, &SortKey::Alphabetical, false).unwrap();

    assert!(out.contains(
        "** DONE Apple\n   DEADLINE: <2026-09-20 Sun>\n   :PROPERTIES:\n   :EFFORT:   10\n"
    ));
}

#[test]
fn alphabetical_ignores_the_keyword_and_the_case() {
    let out = sort_entries(TASKS, 0, &SortKey::Alphabetical, false).unwrap();

    assert_eq!(titles(&out), ["DONE Apple", "TODO Mango", "TODO Zebra"]);
}

#[test]
fn reversing_flips_the_order() {
    let out = sort_entries(TASKS, 0, &SortKey::Alphabetical, true).unwrap();

    assert_eq!(titles(&out), ["TODO Zebra", "TODO Mango", "DONE Apple"]);
}

#[test]
fn todo_sorts_by_the_order_the_file_declares_its_keywords_in() {
    let out = sort_entries(TASKS, 0, &SortKey::Todo, false).unwrap();

    // TODO comes before DONE because the default sequence puts it there, not
    // because it sorts earlier as a word.
    assert_eq!(titles(&out), ["TODO Zebra", "TODO Mango", "DONE Apple"]);
}

#[test]
fn an_entry_without_a_deadline_stays_at_the_bottom_even_reversed() {
    let ascending = sort_entries(TASKS, 0, &SortKey::Deadline, false).unwrap();
    assert_eq!(
        titles(&ascending),
        ["DONE Apple", "TODO Zebra", "TODO Mango"]
    );

    let descending = sort_entries(TASKS, 0, &SortKey::Deadline, true).unwrap();
    assert_eq!(
        titles(&descending),
        ["TODO Zebra", "DONE Apple", "TODO Mango"]
    );
}

#[test]
fn a_property_that_holds_a_number_sorts_as_one() {
    let out = sort_entries(TASKS, 0, &SortKey::Property("EFFORT".into()), false).unwrap();

    // As text, "10" would come before "3".
    assert_eq!(titles(&out), ["TODO Mango", "TODO Zebra", "DONE Apple"]);
}

#[test]
fn above_any_headline_the_top_level_entries_are_sorted() {
    let source = "#+title: Notes\n* Beta\n* Alpha\n";
    let out = sort_entries(source, 0, &SortKey::Alphabetical, false).unwrap();

    assert_eq!(out, "#+title: Notes\n* Alpha\n* Beta\n");
}

#[test]
fn an_entry_with_one_child_has_nothing_to_sort() {
    assert_eq!(
        sort_entries("* Only\n** Child\n", 0, &SortKey::Alphabetical, false),
        Err(Error::NoSibling)
    );
}

const LIST: &str = "\
- pear
  a note about pears
- apple
  - granny smith
- fig
";

#[test]
fn a_list_item_carries_its_notes_and_sub_items() {
    let out = sort_list(LIST, 0, &SortKey::Alphabetical, false).unwrap();

    assert_eq!(
        out,
        "\
- apple
  - granny smith
- fig
- pear
  a note about pears
"
    );
}

#[test]
fn an_ordered_list_is_renumbered_after_sorting() {
    let source = "1. pear\n2. apple\n3. fig\n";
    let out = sort_list(source, 0, &SortKey::Alphabetical, false).unwrap();

    assert_eq!(out, "1. apple\n2. fig\n3. pear\n");
}

#[test]
fn sorting_a_sub_list_leaves_its_parent_where_it_is() {
    let source = "- fruit\n  - pear\n  - apple\n- veg\n";
    let out = sort_list(source, 1, &SortKey::Alphabetical, false).unwrap();

    assert_eq!(out, "- fruit\n  - apple\n  - pear\n- veg\n");
}

#[test]
fn a_blank_line_between_two_items_does_not_end_the_list() {
    let source = "- pear\n\n- apple\n";
    let out = sort_list(source, 0, &SortKey::Alphabetical, false).unwrap();

    assert!(out.starts_with("- apple"));
}

#[test]
fn a_numeric_sort_reads_the_number_in_the_item() {
    let source = "- 10 apples\n- 3 pears\n";
    let out = sort_list(source, 0, &SortKey::Numeric, false).unwrap();

    assert_eq!(out, "- 3 pears\n- 10 apples\n");
}

const TABLE: &str = "\
| Name  | Qty |
|-------+-----|
| pear  |  10 |
| apple |   3 |
| fig   |   7 |
";

#[test]
fn a_table_sorts_the_section_the_cursor_is_in() {
    let out = sort_table(TABLE, 2, 0, &SortKey::Alphabetical, false).unwrap();

    assert_eq!(
        out,
        "\
| Name  | Qty |
|-------+-----|
| apple |   3 |
| fig   |   7 |
| pear  |  10 |
"
    );
}

#[test]
fn the_header_above_the_separator_is_a_different_section_and_stays_put() {
    let out = sort_table(TABLE, 2, 1, &SortKey::Numeric, false).unwrap();

    assert!(out.starts_with("| Name  | Qty |\n|-------+-----|\n| apple |   3 |\n"));
}

#[test]
fn there_is_nothing_to_sort_from_a_separator() {
    assert_eq!(sort_table(TABLE, 1, 0, &SortKey::Alphabetical, false), None);
}

#[test]
fn sorting_a_table_realigns_it() {
    let ragged = "| b | 1 |\n| aaaa | 2 |\n";
    let out = sort_table(ragged, 0, 0, &SortKey::Alphabetical, false).unwrap();

    assert_eq!(out, "| aaaa | 2 |\n| b    | 1 |\n");
}

#[test]
fn a_key_is_read_from_what_the_user_typed() {
    assert_eq!(SortKey::parse("Alpha"), Some(SortKey::Alphabetical));
    assert_eq!(SortKey::parse("deadline"), Some(SortKey::Deadline));
    assert_eq!(
        SortKey::parse("property:EFFORT"),
        Some(SortKey::Property("EFFORT".into()))
    );
    assert_eq!(SortKey::parse("EFFORT"), None);
}
