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

#[test]
fn an_entry_without_an_id_is_given_one() {
    use helix_roam::restructure::{ensure_id, IdOutcome};

    let text = "#+title: Notes\n* A heading\nBody.\n";
    let IdOutcome::Created {
        text: after,
        id: created,
    } = ensure_id(text, 1, id(5)).unwrap()
    else {
        panic!("expected a new id");
    };

    assert_eq!(created, id(5));
    assert_eq!(
        after,
        format!(
            "#+title: Notes\n* A heading\n:PROPERTIES:\n:ID:       {}\n:END:\nBody.\n",
            id(5)
        )
    );
}

#[test]
fn an_entry_that_already_has_an_id_is_left_alone() {
    use helix_roam::restructure::{ensure_id, IdOutcome};

    let text = "\
* A heading
:PROPERTIES:
:ID:       6ba7b810-9dad-11d1-80b4-00c04fd430c8
:END:
Body.
";
    assert_eq!(
        ensure_id(text, 4, id(5)).unwrap(),
        IdOutcome::Existing(Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap())
    );
}

#[test]
fn the_preamble_can_be_given_an_id_too() {
    use helix_roam::restructure::{ensure_id, IdOutcome};

    // Above any headline, the entry is the file itself.
    let text = "#+title: Notes\nSome body.\n* A heading\n";
    let IdOutcome::Created { text: after, .. } = ensure_id(text, 0, id(6)).unwrap() else {
        panic!("expected a new id");
    };

    assert!(after.starts_with(":PROPERTIES:\n"), "{after}");
    assert!(after.contains("#+title: Notes"));
}

#[test]
fn the_entry_at_a_line_is_found_from_its_headline_not_its_id_line() {
    use helix_roam::restructure::entry_at;

    let text = "\
#+title: Fresh
* A heading with an id
:PROPERTIES:
:ID:       6ba7b810-9dad-11d1-80b4-00c04fd430c8
:END:
Body.
";
    let expected = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();

    // Line 1 is the headline itself. A search keyed on the indexed node's line
    // would miss it, because that line points at the `:ID:` on line 3 — which
    // is *after* the cursor.
    assert_eq!(
        entry_at(text, 1),
        Some((expected, "A heading with an id".to_string()))
    );
    // And from anywhere else inside the entry.
    for line in 2..=5 {
        assert_eq!(entry_at(text, line).map(|(id, _)| id), Some(expected));
    }

    // Above the headline the entry is the preamble, which has no id here.
    assert_eq!(entry_at(text, 0), None);
}

#[test]
fn the_preamble_entry_is_found_when_the_file_node_has_an_id() {
    use helix_roam::restructure::entry_at;

    let text = ":PROPERTIES:\n:ID:       6ba7b899-9dad-11d1-80b4-00c04fd430c8\n:END:\n#+title: A file node\nBody.\n";
    let (id, title) = entry_at(text, 4).unwrap();

    assert_eq!(
        id,
        Uuid::parse_str("6ba7b899-9dad-11d1-80b4-00c04fd430c8").unwrap()
    );
    assert_eq!(title, "A file node");
}

const WITH_DRAWER: &str = "\
* A heading
:PROPERTIES:
:ID:       6ba7b810-9dad-11d1-80b4-00c04fd430c8
:ROAM_ALIASES: short \"a longer one\"
:END:
Body.
";

#[test]
fn adding_a_value_keeps_the_ones_already_there() {
    use helix_roam::restructure::edit_property;

    let out = edit_property(WITH_DRAWER, 5, "ROAM_ALIASES", "third", true)
        .unwrap()
        .unwrap();

    // Quoting is preserved for the value that needs it, and not added to the
    // ones that do not.
    assert!(
        out.contains(":ROAM_ALIASES: short \"a longer one\" third"),
        "{out}"
    );
}

#[test]
fn a_value_with_a_space_is_quoted_on_the_way_back_in() {
    use helix_roam::restructure::edit_property;

    let out = edit_property(WITH_DRAWER, 5, "ROAM_ALIASES", "two words", true)
        .unwrap()
        .unwrap();
    assert!(out.contains("\"two words\""), "{out}");
}

#[test]
fn a_property_that_is_not_there_yet_is_created() {
    use helix_roam::restructure::edit_property;

    let out = edit_property(WITH_DRAWER, 5, "ROAM_REFS", "https://example.org", true)
        .unwrap()
        .unwrap();

    assert!(out.contains(":ROAM_REFS: https://example.org"), "{out}");
    // And it lands inside the drawer, before its `:END:`.
    let refs = out.find(":ROAM_REFS:").unwrap();
    let end = out.find(":END:").unwrap();
    assert!(refs < end, "{out}");
}

#[test]
fn removing_the_last_value_removes_the_property_line() {
    use helix_roam::restructure::edit_property;

    let one = "* H\n:PROPERTIES:\n:ID:       a\n:ROAM_REFS: only\n:END:\n";
    let out = edit_property(one, 3, "ROAM_REFS", "only", false)
        .unwrap()
        .unwrap();

    assert!(!out.contains("ROAM_REFS"), "{out}");
    // The rest of the drawer survives.
    assert!(out.contains(":ID:       a"), "{out}");
}

#[test]
fn nothing_to_do_reports_nothing_rather_than_touching_the_buffer() {
    use helix_roam::restructure::edit_property;

    // Already present.
    assert_eq!(
        edit_property(WITH_DRAWER, 5, "ROAM_ALIASES", "short", true).unwrap(),
        None
    );
    // Not there to remove.
    assert_eq!(
        edit_property(WITH_DRAWER, 5, "ROAM_ALIASES", "absent", false).unwrap(),
        None
    );
}

#[test]
fn an_entry_without_a_drawer_says_so() {
    use helix_roam::restructure::{edit_property, Error};

    let bare = "* A heading with no drawer\nBody.\n";
    assert_eq!(
        edit_property(bare, 1, "ROAM_ALIASES", "x", true),
        Err(Error::NoDrawer)
    );
}

#[test]
fn a_headlines_tags_live_on_its_own_line() {
    use helix_roam::restructure::edit_tag;

    let text = "* A heading  :one:\nBody.\n";
    let out = edit_tag(text, 1, "two", true).unwrap().unwrap();
    assert_eq!(out, "* A heading  :one:two:\nBody.\n");

    // Removing the last tag leaves a clean headline rather than empty colons.
    let one = "* A heading  :only:\n";
    let out = edit_tag(one, 0, "only", false).unwrap().unwrap();
    assert_eq!(out, "* A heading\n");
}

#[test]
fn a_file_nodes_tags_live_in_filetags() {
    use helix_roam::restructure::edit_tag;

    // Above any headline the entry is the file, so the keyword is edited.
    let text = "#+title: A note\nBody.\n";
    let out = edit_tag(text, 1, "work", true).unwrap().unwrap();
    assert_eq!(out, "#+title: A note\n#+filetags: :work:\nBody.\n");

    // And an existing keyword is extended rather than duplicated.
    let out = edit_tag(&out, 2, "urgent", true).unwrap().unwrap();
    assert!(out.contains("#+filetags: :work:urgent:"), "{out}");
}

#[test]
fn a_tag_that_changes_nothing_reports_nothing() {
    use helix_roam::restructure::edit_tag;

    let text = "* A heading  :one:\n";
    assert_eq!(edit_tag(text, 0, "one", true).unwrap(), None);
    assert_eq!(edit_tag(text, 0, "absent", false).unwrap(), None);
}
