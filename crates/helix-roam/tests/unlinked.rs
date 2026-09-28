//! Finding a node's name written as prose.

use std::path::Path;

use helix_roam::unlinked::find_in_text;

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

#[test]
fn a_name_in_prose_is_a_reference_waiting_to_be_made() {
    let text = "Rust is a systems language.\nI like Rust a lot.\n";
    let found = find_in_text(text, Path::new("n.org"), &names(&["Rust"]));

    assert_eq!(found.len(), 2);
    assert_eq!((found[0].line, found[0].column), (0, 0));
    assert_eq!((found[1].line, found[1].column), (1, 7));
    assert_eq!(found[1].text, "I like Rust a lot.");
}

#[test]
fn a_name_inside_a_link_is_not_one() {
    // Both the target and the description belong to the link.
    let text = "See [[id:abc][Rust]] and [[roam:Rust]] here.\n";
    assert!(find_in_text(text, Path::new("n.org"), &names(&["Rust"])).is_empty());
}

#[test]
fn only_whole_words_match() {
    // "trusted" contains "rust"; a list full of those would be unusable.
    let text = "A trusted crustacean.\n";
    assert!(find_in_text(text, Path::new("n.org"), &names(&["Rust"])).is_empty());

    // But punctuation around the word is fine.
    let text = "Rust, and (Rust).\n";
    assert_eq!(
        find_in_text(text, Path::new("n.org"), &names(&["Rust"])).len(),
        2
    );
}

#[test]
fn matching_ignores_case_but_reports_the_name_that_matched() {
    let text = "rust and RUST.\n";
    let found = find_in_text(text, Path::new("n.org"), &names(&["Rust"]));

    assert_eq!(found.len(), 2);
    assert_eq!(found[0].matched, "Rust");
}

#[test]
fn aliases_are_searched_too() {
    let text = "The Rust language, also called rustlang.\n";
    let found = find_in_text(text, Path::new("n.org"), &names(&["Rust", "rustlang"]));

    let matched: Vec<&str> = found.iter().map(|r| r.matched.as_str()).collect();
    assert_eq!(matched, ["Rust", "rustlang"]);
}

#[test]
fn a_multi_word_title_is_found() {
    let text = "We discussed The Rust Book yesterday.\n";
    let found = find_in_text(text, Path::new("n.org"), &names(&["The Rust Book"]));

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].column, 13);
}

#[test]
fn results_come_back_in_reading_order() {
    let text = "b then a\na then b\n";
    let found = find_in_text(text, Path::new("n.org"), &names(&["a", "b"]));

    let order: Vec<(usize, usize)> = found.iter().map(|r| (r.line, r.column)).collect();
    let mut sorted = order.clone();
    sorted.sort_unstable();
    assert_eq!(order, sorted);
}

#[test]
fn an_empty_name_matches_nothing() {
    let text = "Some prose.\n";
    assert!(find_in_text(text, Path::new("n.org"), &names(&[""])).is_empty());
}
