//! Editing Org structure and headline metadata.

use helix_roam::restructure::{
    change_priority, cycle_todo, insert_heading, move_subtree, set_planning, set_priority,
    shift_heading, shift_subtree, Error, Planning,
};
use helix_roam::FileSettings;

fn default() -> FileSettings {
    FileSettings::default()
}

const TREE: &str = "\
* First
Body of first.
** Child of first
* Second
Body of second.
* Third
";

#[test]
fn a_new_heading_lands_after_the_whole_subtree() {
    let (out, at) = insert_heading(TREE, 0);

    // Not between `* First` and its child.
    assert_eq!(at, 3);
    assert_eq!(out.lines().nth(3), Some("* "));
    assert_eq!(out.lines().nth(4), Some("* Second"));
}

#[test]
fn a_new_heading_matches_the_level_it_was_invoked_on() {
    let (out, at) = insert_heading(TREE, 2);
    assert_eq!(out.lines().nth(at), Some("** "));
}

#[test]
fn one_heading_can_move_without_its_children() {
    let out = shift_heading(TREE, 0, true).unwrap();

    assert_eq!(out.lines().next(), Some("** First"));
    // The child stayed where it was.
    assert_eq!(out.lines().nth(2), Some("** Child of first"));
}

#[test]
fn a_subtree_moves_with_its_children() {
    let out = shift_subtree(TREE, 0, true).unwrap();

    assert_eq!(out.lines().next(), Some("** First"));
    assert_eq!(out.lines().nth(2), Some("*** Child of first"));
    // And the next top-level heading is untouched.
    assert_eq!(out.lines().nth(3), Some("* Second"));
}

#[test]
fn a_top_level_heading_cannot_be_promoted_further() {
    assert_eq!(shift_heading(TREE, 0, false), Err(Error::AlreadyTopLevel));
    assert_eq!(shift_subtree(TREE, 0, false), Err(Error::AlreadyTopLevel));
}

#[test]
fn moving_a_subtree_swaps_it_with_a_sibling_children_and_all() {
    let (out, at) = move_subtree(TREE, 3, true).unwrap();

    // `Second` moved above `First`, which kept its child.
    assert_eq!(out.lines().next(), Some("* Second"));
    assert_eq!(at, 0);
    assert!(
        out.contains("* First\nBody of first.\n** Child of first"),
        "{out}"
    );
}

#[test]
fn moving_down_puts_the_sibling_first() {
    let (out, _) = move_subtree(TREE, 0, false).unwrap();
    assert_eq!(out.lines().next(), Some("* Second"));
    assert!(out.contains("* First"), "{out}");
}

#[test]
fn there_is_nothing_to_swap_with_at_either_end() {
    assert_eq!(move_subtree(TREE, 0, true), Err(Error::NoSibling));
    assert_eq!(move_subtree(TREE, 5, false), Err(Error::NoSibling));
}

#[test]
fn a_child_has_no_sibling_outside_its_parent() {
    // `** Child of first` must not swap with `* Second`.
    assert_eq!(move_subtree(TREE, 2, false), Err(Error::NoSibling));
}

#[test]
fn cycling_follows_the_files_own_keywords() {
    let settings = FileSettings::scan("#+TODO: NEXT WAITING | SHIPPED\n");
    let text = "* A task\n";

    let first = cycle_todo(text, 0, &settings, true).unwrap();
    assert_eq!(first.lines().next(), Some("* NEXT A task"));

    let second = cycle_todo(&first, 0, &settings, true).unwrap();
    assert_eq!(second.lines().next(), Some("* WAITING A task"));

    let third = cycle_todo(&second, 0, &settings, true).unwrap();
    assert_eq!(third.lines().next(), Some("* SHIPPED A task"));

    // Past the last keyword the state is cleared, as Org does.
    let fourth = cycle_todo(&third, 0, &settings, true).unwrap();
    assert_eq!(fourth.lines().next(), Some("* A task"));
}

#[test]
fn cycling_backwards_walks_the_sequence_the_other_way() {
    let settings = default();
    let text = "* A task\n";

    let back = cycle_todo(text, 0, &settings, false).unwrap();
    assert_eq!(back.lines().next(), Some("* DONE A task"));
}

#[test]
fn cycling_keeps_the_priority_and_the_tags() {
    let settings = default();
    let text = "* [#A] A task  :work:\n";

    let out = cycle_todo(text, 0, &settings, true).unwrap();
    assert_eq!(out.lines().next(), Some("* TODO [#A] A task  :work:"));
}

#[test]
fn a_priority_is_set_and_cleared_around_the_keyword() {
    let settings = default();
    let text = "* TODO A task  :work:\n";

    let with = set_priority(text, 0, &settings, Some('B')).unwrap();
    assert_eq!(with.lines().next(), Some("* TODO [#B] A task  :work:"));

    let without = set_priority(&with, 0, &settings, None).unwrap();
    assert_eq!(without.lines().next(), Some("* TODO A task  :work:"));
}

#[test]
fn a_priority_the_file_does_not_declare_is_refused() {
    let settings = default();
    assert_eq!(
        set_priority("* A task\n", 0, &settings, Some('Z')),
        Err(Error::UndeclaredPriority('Z'))
    );
}

#[test]
fn raising_and_lowering_walk_the_declared_range() {
    let settings = default();
    let text = "* A task\n";

    // From nothing, the first press gives the highest.
    let a = change_priority(text, 0, &settings, false).unwrap();
    assert_eq!(a.lines().next(), Some("* [#A] A task"));

    let b = change_priority(&a, 0, &settings, false).unwrap();
    assert_eq!(b.lines().next(), Some("* [#B] A task"));

    let back = change_priority(&b, 0, &settings, true).unwrap();
    assert_eq!(back.lines().next(), Some("* [#A] A task"));

    // Raising past the top clears it rather than sticking.
    let cleared = change_priority(&back, 0, &settings, true).unwrap();
    assert_eq!(cleared.lines().next(), Some("* A task"));
}

#[test]
fn planning_lines_are_written_the_way_org_writes_them() {
    let text = "* A task\nBody.\n";

    let scheduled = set_planning(text, 0, Planning::Scheduled, Some("<2026-09-18 Fri>")).unwrap();
    assert_eq!(
        scheduled.lines().nth(1),
        Some("SCHEDULED: <2026-09-18 Fri>")
    );

    // Adding a deadline keeps the schedule, and DEADLINE comes first.
    let both = set_planning(&scheduled, 0, Planning::Deadline, Some("<2026-09-25 Fri>")).unwrap();
    assert_eq!(
        both.lines().nth(1),
        Some("DEADLINE: <2026-09-25 Fri> SCHEDULED: <2026-09-18 Fri>")
    );

    // Removing one leaves the other.
    let only_deadline = set_planning(&both, 0, Planning::Scheduled, None).unwrap();
    assert_eq!(
        only_deadline.lines().nth(1),
        Some("DEADLINE: <2026-09-25 Fri>")
    );

    // Removing the last one removes the line.
    let none = set_planning(&only_deadline, 0, Planning::Deadline, None).unwrap();
    assert_eq!(none, "* A task\nBody.\n");
}

#[test]
fn a_planning_line_is_replaced_rather_than_stacked() {
    let text = "* A task\nSCHEDULED: <2026-09-18 Fri>\nBody.\n";
    let out = set_planning(text, 0, Planning::Scheduled, Some("<2026-10-01 Thu>")).unwrap();

    assert_eq!(out.lines().nth(1), Some("SCHEDULED: <2026-10-01 Thu>"));
    assert_eq!(out.matches("SCHEDULED:").count(), 1);
}

#[test]
fn archiving_cuts_the_subtree_and_records_where_it_came_from() {
    use helix_roam::restructure::archive_subtree;

    let out = archive_subtree(TREE, 3, &default(), "notes.org").unwrap();

    assert_eq!(
        out.remaining,
        "* First\nBody of first.\n** Child of first\n* Third\n"
    );
    // The archived copy says where it was, which a subtree out of context needs.
    assert_eq!(
        out.archived,
        "* Second\n:ARCHIVE_FILE: notes.org\nBody of second.\n"
    );
    // The default target sits beside the file.
    assert_eq!(out.target, "notes.org_archive");
}

#[test]
fn the_file_can_name_its_own_archive() {
    use helix_roam::restructure::archive_subtree;

    let settings = FileSettings::scan("#+ARCHIVE: done.org::* Archived\n");
    let out = archive_subtree(TREE, 3, &settings, "notes.org").unwrap();

    // The `::headline` half is not a file name and is dropped here.
    assert_eq!(out.target, "done.org");
}
