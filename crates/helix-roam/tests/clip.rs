//! Structure-aware cut, copy, paste and cloning of subtrees.

use helix_roam::clip::{clone_subtree, copy_subtree, cut_subtree, parse_shift, paste_subtree};
use helix_roam::restructure::Error;

const OUTLINE: &str = "\
* Projects
** Alpha
Some notes.
*** Detail
** Beta
* Archive
";

#[test]
fn copying_takes_the_whole_subtree_and_leaves_the_buffer_alone() {
    let clip = copy_subtree(OUTLINE, 1).unwrap();

    assert_eq!(clip.level, 2);
    assert_eq!(clip.text, "** Alpha\nSome notes.\n*** Detail");
}

#[test]
fn a_cursor_in_the_body_copies_the_entry_it_belongs_to() {
    // Line 2 is "Some notes.", which is not a headline but is inside Alpha.
    let clip = copy_subtree(OUTLINE, 2).unwrap();

    assert_eq!(clip.text, "** Alpha\nSome notes.\n*** Detail");
}

#[test]
fn a_copy_drops_the_id_but_a_cut_keeps_it() {
    // Copying makes a second entry, so it must not carry the first one's
    // identity; cutting moves the entry, so it must.
    assert!(!copy_subtree(TASK, 0).unwrap().text.contains(":ID:"));
    assert!(cut_subtree(TASK, 0).unwrap().1.text.contains(":ID:"));
}

#[test]
fn cutting_removes_the_subtree_and_keeps_its_siblings() {
    let (text, clip) = cut_subtree(OUTLINE, 1).unwrap();

    assert_eq!(text, "* Projects\n** Beta\n* Archive\n");
    assert_eq!(clip.level, 2);
}

#[test]
fn pasting_shifts_the_clip_to_the_level_it_lands_at() {
    let clip = copy_subtree(OUTLINE, 1).unwrap();
    // Line 5 is "* Archive", a level-1 entry: the level-2 clip moves up one,
    // and its level-3 child moves with it.
    let (text, at) = paste_subtree(OUTLINE, 5, &clip);

    assert_eq!(at, 6);
    assert_eq!(
        text,
        "\
* Projects
** Alpha
Some notes.
*** Detail
** Beta
* Archive
* Alpha
Some notes.
** Detail
"
    );
}

#[test]
fn pasting_deeper_moves_the_whole_tree_down_together() {
    let clip = copy_subtree(OUTLINE, 5).unwrap(); // "* Archive", level 1
    let (text, _) = paste_subtree(OUTLINE, 3, &clip); // inside "*** Detail"

    assert!(text.contains("*** Detail\n*** Archive\n"));
}

#[test]
fn pasting_above_any_headline_lands_at_the_top_level() {
    let clip = copy_subtree(OUTLINE, 3).unwrap(); // "*** Detail", level 3
    let (text, _) = paste_subtree("Just a preamble.\n", 0, &clip);

    assert_eq!(text, "Just a preamble.\n* Detail\n");
}

#[test]
fn round_tripping_a_cut_and_a_paste_at_the_same_level_changes_nothing() {
    let (cut, clip) = cut_subtree(OUTLINE, 4).unwrap(); // "** Beta"
                                                        // Line 1 is "** Alpha", whose subtree ends where Beta used to start.
    let (text, _) = paste_subtree(&cut, 1, &clip);

    assert_eq!(text, OUTLINE);
}

const TASK: &str = "\
* TODO Water the plants
  SCHEDULED: <2026-09-18 Fri>
  :PROPERTIES:
  :ID:       11111111-1111-1111-1111-111111111111
  :END:
  Noted on [2026-09-01 Tue].
* Next
";

#[test]
fn cloning_appends_copies_after_the_original() {
    let text = clone_subtree(TASK, 0, 2, None).unwrap();

    assert_eq!(text.matches("Water the plants").count(), 3);
    // The copies go after the subtree, not after the file.
    assert!(text.ends_with("* Next\n"));
}

#[test]
fn each_copy_is_shifted_from_the_original_not_from_the_one_before() {
    let text = clone_subtree(TASK, 0, 3, Some(parse_shift("+1w").unwrap())).unwrap();

    assert!(text.contains("SCHEDULED: <2026-09-25 Fri>"));
    assert!(text.contains("SCHEDULED: <2026-10-02 Fri>"));
    assert!(text.contains("SCHEDULED: <2026-10-09 Fri>"));
}

#[test]
fn a_month_shift_keeps_the_day_instead_of_drifting_through_short_months() {
    let text = clone_subtree(
        "* Rent\n  DEADLINE: <2026-01-31 Sat>\n",
        0,
        2,
        Some(parse_shift("+1m").unwrap()),
    )
    .unwrap();

    // Stepping a month at a time from the copy would give 28 February and
    // then 28 March; stepping from the original gives the 31st back.
    assert!(text.contains("<2026-02-28 Sat>"));
    assert!(text.contains("<2026-03-31 Tue>"));
}

#[test]
fn the_day_name_is_recomputed_rather_than_carried_over() {
    let text = clone_subtree(TASK, 0, 1, Some(parse_shift("+3d").unwrap())).unwrap();

    assert!(text.contains("<2026-09-21 Mon>"));
}

#[test]
fn an_inactive_timestamp_is_a_record_and_does_not_move() {
    let text = clone_subtree(TASK, 0, 1, Some(parse_shift("+1w").unwrap())).unwrap();

    assert_eq!(text.matches("[2026-09-01 Tue]").count(), 2);
}

#[test]
fn a_copy_does_not_carry_the_original_id() {
    let text = clone_subtree(TASK, 0, 2, None).unwrap();

    assert_eq!(text.matches("11111111-1111").count(), 1);
    // The drawer held nothing else, so it goes with the id.
    assert_eq!(text.matches(":PROPERTIES:").count(), 1);
}

#[test]
fn a_drawer_with_other_properties_survives_losing_its_id() {
    let source = "\
* Task
  :PROPERTIES:
  :ID:       22222222-2222-2222-2222-222222222222
  :CATEGORY: work
  :END:
";
    let text = clone_subtree(source, 0, 1, None).unwrap();

    assert_eq!(text.matches(":PROPERTIES:").count(), 2);
    assert_eq!(text.matches(":CATEGORY: work").count(), 2);
    assert_eq!(
        text.matches("22222222-2222-2222-2222-222222222222").count(),
        1
    );
}

#[test]
fn an_hour_shift_is_refused_because_a_clone_set_is_a_calendar_of_days() {
    assert_eq!(parse_shift("+2h"), Err(Error::UnsupportedShift));
    assert_eq!(parse_shift("soon"), Err(Error::UnsupportedShift));
}

#[test]
fn cloning_needs_a_subtree() {
    assert_eq!(
        clone_subtree("No headings here.\n", 0, 1, None),
        Err(Error::NoSubtree)
    );
}

#[test]
fn a_link_is_not_mistaken_for_a_timestamp() {
    let source = "* Task\n  See [[id:abc][the note]] and <not-a-date>.\n";
    let text = clone_subtree(source, 0, 1, Some(parse_shift("+1d").unwrap())).unwrap();

    assert_eq!(text.matches("[[id:abc][the note]]").count(), 2);
    assert_eq!(text.matches("<not-a-date>").count(), 2);
}
