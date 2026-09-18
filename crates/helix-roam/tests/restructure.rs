//! Turning nodes between shapes.

use helix_roam::restructure::{
    demote_buffer, extract_subtree, promote_buffer, replace_roam_links, Error,
};
use uuid::Uuid;

fn id(n: u8) -> Uuid {
    Uuid::from_bytes([n; 16])
}

#[test]
fn promoting_turns_the_root_heading_into_the_files_title() {
    let text = "\
* The only heading  :work:urgent:
:PROPERTIES:
:ID:       aaaa
:END:
Body text.
** A child
Child body.
";
    let out = promote_buffer(text).unwrap();

    assert_eq!(
        out,
        "\
#+title: The only heading
#+filetags: :work:urgent:
:PROPERTIES:
:ID:       aaaa
:END:
Body text.
* A child
Child body.
"
    );
}

#[test]
fn promoting_refuses_what_it_cannot_represent() {
    // Two roots: the file node could only be one of them.
    let two = "* One\n* Two\n";
    assert!(matches!(promote_buffer(two), Err(Error::NotPromotable(_))));

    // Text above the heading would have nowhere to live.
    let stray = "Some loose prose.\n* One\n";
    assert!(matches!(
        promote_buffer(stray),
        Err(Error::NotPromotable(_))
    ));

    // Keywords and a preamble drawer are not stray text.
    let fine = "#+startup: overview\n:PROPERTIES:\n:CUSTOM: x\n:END:\n* One\n";
    assert!(promote_buffer(fine).is_ok());

    let none = "Just prose, no heading.\n";
    assert!(matches!(promote_buffer(none), Err(Error::NotPromotable(_))));
}

#[test]
fn demoting_is_the_other_direction() {
    let text = "\
:PROPERTIES:
:ID:       aaaa
:END:
#+title: The note
#+filetags: :work:urgent:
Body text.
* A heading
";
    let out = demote_buffer(text).unwrap();

    // The heading goes above the drawer, which therefore becomes its own —
    // this is why the drawer never has to be moved explicitly.
    assert_eq!(
        out,
        "\
* The note  :work:urgent:
:PROPERTIES:
:ID:       aaaa
:END:
Body text.
** A heading
"
    );
}

#[test]
fn demoting_needs_a_title_to_turn_into_a_heading() {
    assert_eq!(demote_buffer("Just prose.\n"), Err(Error::NoTitle));
}

#[test]
fn promote_and_demote_are_inverses() {
    let text = "\
:PROPERTIES:
:ID:       aaaa
:END:
#+title: Round trip
#+filetags: :one:two:
Body.
* Child
** Grandchild
";
    let round_tripped = promote_buffer(&demote_buffer(text).unwrap()).unwrap();

    // The keyword order is normalised, so compare the parts that must survive.
    assert!(round_tripped.contains("#+title: Round trip"));
    assert!(round_tripped.contains("#+filetags: :one:two:"));
    assert!(round_tripped.contains("* Child"));
    assert!(round_tripped.contains("** Grandchild"));
    assert!(round_tripped.contains(":ID:       aaaa"));
}

const NESTED: &str = "\
#+title: Source
* Keep me
** Extract me  :tag:
:PROPERTIES:
:ID:       6ba7b810-9dad-11d1-80b4-00c04fd430c8
:END:
The body of the extracted node.
*** A child of it
Deeper body.
* Keep me too
";

#[test]
fn extracting_cuts_the_subtree_and_leaves_nothing_behind() {
    // Line 2 is the `** Extract me` headline.
    let out = extract_subtree(NESTED, 2, id(1)).unwrap();

    // Upstream leaves no link where the subtree was: the id travels with it,
    // so anything already pointing at the node still resolves.
    assert_eq!(out.remaining, "#+title: Source\n* Keep me\n* Keep me too\n");
    assert_eq!(out.title, "Extract me");
    assert_eq!(
        out.id,
        Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap()
    );
}

#[test]
fn the_extracted_subtree_becomes_a_file_node() {
    let out = extract_subtree(NESTED, 2, id(1)).unwrap();

    assert_eq!(
        out.extracted,
        "\
#+title: Extract me
#+filetags: :tag:
:PROPERTIES:
:ID:       6ba7b810-9dad-11d1-80b4-00c04fd430c8
:END:
The body of the extracted node.
* A child of it
Deeper body.
"
    );
}

#[test]
fn extracting_from_anywhere_inside_the_subtree_finds_its_headline() {
    // Line 6 is body text well inside the subtree.
    let from_body = extract_subtree(NESTED, 6, id(1)).unwrap();
    let from_headline = extract_subtree(NESTED, 2, id(1)).unwrap();
    assert_eq!(from_body, from_headline);
}

#[test]
fn a_subtree_without_an_id_is_given_one() {
    let text = "#+title: Source\n* No id here\nBody.\n";
    let out = extract_subtree(text, 1, id(7)).unwrap();

    assert_eq!(out.id, id(7));
    assert!(
        out.extracted.contains(&format!(":ID:       {}", id(7))),
        "{}",
        out.extracted
    );
    assert_eq!(out.remaining, "#+title: Source\n");
}

#[test]
fn extracting_needs_a_headline() {
    let text = "#+title: Only a preamble\nSome body.\n";
    assert_eq!(extract_subtree(text, 1, id(1)), Err(Error::NoSubtree));
}

#[test]
fn legacy_roam_links_become_id_links_when_the_title_is_known() {
    let known = id(3);
    let resolve = |title: &str| (title == "Rust").then_some(known);

    // A bare link keeps the title as its description.
    assert_eq!(
        replace_roam_links("See [[roam:Rust]] here.", resolve),
        format!("See [[id:{known}][Rust]] here.")
    );

    // An existing description is preserved.
    assert_eq!(
        replace_roam_links("See [[roam:Rust][the language]].", resolve),
        format!("See [[id:{known}][the language]].")
    );
}

#[test]
fn an_unknown_title_is_left_alone_rather_than_broken() {
    let resolve = |_: &str| None;
    let text = "See [[roam:Nothing Here]] and [[id:abc][a real one]].";

    // Rewriting it would produce a link pointing at nothing.
    assert_eq!(replace_roam_links(text, resolve), text);
}

#[test]
fn several_links_on_one_line_are_all_rewritten() {
    let a = id(1);
    let b = id(2);
    let resolve = |title: &str| match title {
        "A" => Some(a),
        "B" => Some(b),
        _ => None,
    };

    assert_eq!(
        replace_roam_links("[[roam:A]] then [[roam:B]] then [[roam:C]]", resolve),
        format!("[[id:{a}][A]] then [[id:{b}][B]] then [[roam:C]]")
    );
}

#[test]
fn an_unterminated_link_does_not_lose_the_rest_of_the_text() {
    let resolve = |_: &str| Some(id(1));
    let text = "A broken [[roam:thing with no close";
    assert_eq!(replace_roam_links(text, resolve), text);
}

const TARGET: &str = "\
#+title: Target file
* A node here
:PROPERTIES:
:ID:       6ba7b899-9dad-11d1-80b4-00c04fd430c8
:END:
Existing body.
** An existing child
* Another node
";

fn target_id() -> Uuid {
    Uuid::parse_str("6ba7b899-9dad-11d1-80b4-00c04fd430c8").unwrap()
}

#[test]
fn refiling_moves_the_subtree_beneath_the_target() {
    use helix_roam::restructure::refile_subtree;

    // Line 2 of NESTED is `** Extract me`.
    let out = refile_subtree(NESTED, 2, TARGET, target_id()).unwrap();

    assert_eq!(out.title, "Extract me");
    assert_eq!(out.source, "#+title: Source\n* Keep me\n* Keep me too\n");

    // It lands after everything already under the target, re-levelled to be
    // its direct child, and its own children keep their relative depth.
    assert_eq!(
        out.target,
        "\
#+title: Target file
* A node here
:PROPERTIES:
:ID:       6ba7b899-9dad-11d1-80b4-00c04fd430c8
:END:
Existing body.
** An existing child
** Extract me  :tag:
:PROPERTIES:
:ID:       6ba7b810-9dad-11d1-80b4-00c04fd430c8
:END:
The body of the extracted node.
*** A child of it
Deeper body.
* Another node
"
    );
}

#[test]
fn refiling_under_a_file_level_node_appends_at_level_one() {
    use helix_roam::restructure::refile_subtree;

    let file_node = "\
:PROPERTIES:
:ID:       6ba7b899-9dad-11d1-80b4-00c04fd430c8
:END:
#+title: A file node
Body.
";
    let out = refile_subtree(NESTED, 2, file_node, target_id()).unwrap();

    // No headline owns the id, so the target is the file itself: level 1.
    assert!(
        out.target.ends_with(
            "\
* Extract me  :tag:
:PROPERTIES:
:ID:       6ba7b810-9dad-11d1-80b4-00c04fd430c8
:END:
The body of the extracted node.
** A child of it
Deeper body.
"
        ),
        "{}",
        out.target
    );
}

#[test]
fn refiling_to_a_target_that_is_not_there_fails_without_touching_anything() {
    use helix_roam::restructure::refile_subtree;

    let missing = Uuid::from_bytes([9; 16]);
    assert_eq!(
        refile_subtree(NESTED, 2, TARGET, missing),
        Err(Error::NoTarget)
    );
}

#[test]
fn refiling_twice_appends_rather_than_interleaving() {
    use helix_roam::restructure::refile_subtree;

    let first = refile_subtree(NESTED, 2, TARGET, target_id()).unwrap();
    let source_again = "#+title: Second\n* Another subtree\nIts body.\n";
    let second = refile_subtree(source_again, 1, &first.target, target_id()).unwrap();

    let at_first = second.target.find("Extract me").unwrap();
    let at_second = second.target.find("Another subtree").unwrap();
    assert!(
        at_first < at_second,
        "the second refiling should follow the first:\n{}",
        second.target
    );
}
