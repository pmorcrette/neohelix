//! Org tables.

use helix_roam::table::{
    align, column_at, delete_column, delete_row, insert_column, insert_row, insert_separator,
    parse_table, Row,
};

const RAGGED: &str = "\
| Name | Qty |
|--+--|
| bread | 2 |
| milk | 10 |
";

#[test]
fn a_table_is_read_into_rows_and_cells() {
    let table = parse_table(RAGGED, 0).unwrap();

    assert_eq!(table.rows.len(), 4);
    assert_eq!(table.columns(), 2);
    assert_eq!(table.rows[1], Row::Separator);
    assert_eq!(
        table.rows[2],
        Row::Cells(vec!["bread".to_string(), "2".to_string()])
    );
}

#[test]
fn aligning_pads_every_column_to_its_widest_cell() {
    let out = align(RAGGED, 0).unwrap();

    assert_eq!(
        out,
        "\
| Name  | Qty |
|-------+-----|
| bread |   2 |
| milk  |  10 |
"
    );
}

#[test]
fn a_column_of_numbers_is_right_aligned() {
    // Org's rule, and the reason figures line up on their digits.
    let out = align(RAGGED, 0).unwrap();
    assert!(out.contains("|   2 |"), "{out}");
    assert!(out.contains("|  10 |"), "{out}");
    // While a column of words is not.
    assert!(out.contains("| bread |"), "{out}");
}

#[test]
fn a_column_mixing_numbers_and_words_is_left_aligned() {
    let mixed = "| a | 1 |\n| b | n/a |\n";
    let out = align(mixed, 0).unwrap();

    assert_eq!(out, "| a | 1   |\n| b | n/a |\n");
}

#[test]
fn aligning_is_stable_once_done() {
    let once = align(RAGGED, 0).unwrap();
    assert_eq!(align(&once, 0).unwrap(), once);
}

#[test]
fn an_indented_table_keeps_its_indentation() {
    let indented = "  | a | b |\n  | c | d |\n";
    let out = align(indented, 0).unwrap();

    assert!(out.lines().all(|line| line.starts_with("  |")), "{out}");
}

#[test]
fn a_row_is_inserted_below_the_cursor_and_the_table_realigned() {
    let out = insert_row(RAGGED, 2).unwrap();

    assert_eq!(
        out,
        "\
| Name  | Qty |
|-------+-----|
| bread |   2 |
|       |     |
| milk  |  10 |
"
    );
}

#[test]
fn a_separator_can_be_inserted_too() {
    let out = insert_separator("| a | b |\n| c | d |\n", 0).unwrap();
    assert_eq!(out, "| a | b |\n|---+---|\n| c | d |\n");
}

#[test]
fn a_row_can_be_removed() {
    let out = delete_row(RAGGED, 2).unwrap();
    assert!(!out.contains("bread"), "{out}");
    assert!(out.contains("milk"), "{out}");
}

#[test]
fn removing_the_last_row_removes_the_table_rather_than_leaving_a_frame() {
    let one = "| only |\n";
    assert_eq!(delete_row(one, 0).unwrap(), "");

    // A table left with nothing but separators is not a table.
    let with_rule = "| only |\n|------|\n";
    assert_eq!(delete_row(with_rule, 0).unwrap(), "");
}

#[test]
fn a_column_is_inserted_into_every_row() {
    let out = insert_column(RAGGED, 0, 1).unwrap();

    assert_eq!(
        out,
        "\
| Name  |  | Qty |
|-------+--+-----|
| bread |  |   2 |
| milk  |  |  10 |
"
    );
}

#[test]
fn a_column_is_removed_from_every_row() {
    let out = delete_column(RAGGED, 0, 1).unwrap();

    assert_eq!(out, "| Name  |\n|-------|\n| bread |\n| milk  |\n");
}

#[test]
fn a_column_that_is_not_there_is_refused() {
    assert_eq!(delete_column(RAGGED, 0, 9), None);
}

#[test]
fn the_cursors_column_is_worked_out_from_the_pipes() {
    let line = "| bread | 2 | extra |";
    //          0123456789...
    assert_eq!(column_at(line, 3), 0);
    assert_eq!(column_at(line, 10), 1);
    assert_eq!(column_at(line, 15), 2);
    // Before the first pipe there is no cell yet.
    assert_eq!(column_at(line, 0), 0);
}

#[test]
fn a_line_that_is_not_a_table_has_no_table() {
    assert!(parse_table("just prose\n", 0).is_none());
    assert!(align("just prose\n", 0).is_none());
}

#[test]
fn a_wide_character_counts_as_the_width_it_takes() {
    // Two columns of the same character count but different display width.
    let wide = "| ab |\n| 漢 |\n";
    let out = align(wide, 0).unwrap();

    // Both rows must end at the same column, or the table looks broken in a
    // terminal even though the strings are the same length.
    let widths: Vec<usize> = out
        .lines()
        .map(unicode_width::UnicodeWidthStr::width)
        .collect();
    assert_eq!(widths[0], widths[1], "{out}");
}

#[test]
fn tables_behind_comments_keep_their_comment() {
    use helix_roam::table::align;
    let text = "fn main() {}\n// |a|bb|\n//  |---|\n// | ccc | 1 |\n# | other |\n";
    let aligned = align(text, 1).unwrap();
    assert_eq!(
        aligned,
        "fn main() {}\n// | a   | bb |\n// |-----+----|\n// | ccc |  1 |\n# | other |\n"
    );
    // Formulas behind the same comment.
    let text = "# | 1 | 2 |   |\n# #+TBLFM: $3=$1+$2\n";
    let done = helix_roam::formula::recalculate(text, 0).unwrap();
    assert_eq!(done.text, "# | 1 | 2 | 3 |\n# #+TBLFM: $3=$1+$2\n");
}

#[test]
fn csv_and_tsv_become_tables() {
    use helix_roam::table::{create, parse_delimited, table_text};
    let csv = "name,note\nann,\"likes, commas\"\r\nbob,\"says \"\"hi\"\"\nand more\"\n";
    let rows = parse_delimited(csv, None);
    assert_eq!(rows[1], ["ann", "likes, commas"]);
    assert_eq!(rows[2], ["bob", "says \"hi\"\nand more"]);
    assert_eq!(
        table_text(&rows, ""),
        "| name | note               |\n| ann  | likes, commas      |\n| bob  | says \"hi\" and more |\n"
    );
    assert_eq!(parse_delimited("a\tb|c\n1\t2\n", None)[0], ["a", "b|c"]);
    assert!(table_text(&parse_delimited("a\tb|c\n", None), "").contains("b\\vert{}c"));
    assert_eq!(parse_delimited("x  y\n1 2\n", None)[1], ["1", "2"]);
    assert_eq!(parse_delimited("a;b\n", Some(';'))[0], ["a", "b"]);
    assert_eq!(create(2, 2, ""), "|  |  |\n|--+--|\n|  |  |\n");
}

#[test]
fn a_field_edited_elsewhere_goes_back_to_its_row() {
    use helix_roam::table::{field_at, find_field, set_field};
    let text = "| a | note |\n|---+------|\n| 1 | one  |\n| 2 | old  |\n";
    assert_eq!(field_at(text, 3, 1).as_deref(), Some("old"));
    assert_eq!(field_at(text, 1, 1), None);
    // A line added above the table: the field is found one row down.
    let moved = format!("intro\n{text}");
    assert_eq!(find_field(&moved, 3, 1, "old"), Some(4));
    let set = set_field(text, 3, 1, "a longer\nnote | with a pipe\n").unwrap();
    assert_eq!(
        set,
        "| a | note                              |\n|---+-----------------------------------|\n\
         | 1 | one                               |\n| 2 | a longer note \\vert{} with a pipe |\n"
    );
}
