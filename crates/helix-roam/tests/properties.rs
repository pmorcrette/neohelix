//! Editing properties, drawers and the logbook.

use helix_roam::restructure::{
    insert_drawer, log_entry, property_keys, property_value, remove_property, set_property, Error,
};

const ENTRY: &str = "\
* A task
:PROPERTIES:
:ID:       6ba7b810-9dad-11d1-80b4-00c04fd430c8
:EFFORT:   1:00
:END:
Body.
";

#[test]
fn setting_a_property_replaces_rather_than_appends() {
    let out = set_property(ENTRY, 5, "EFFORT", "2:00").unwrap();

    assert!(out.contains(":EFFORT: 2:00"), "{out}");
    assert!(
        !out.contains("1:00"),
        "the old value should be gone:\n{out}"
    );
    // Exactly one EFFORT line, not two.
    assert_eq!(out.matches(":EFFORT:").count(), 1);
}

#[test]
fn a_property_that_is_not_there_is_added_inside_the_drawer() {
    let out = set_property(ENTRY, 5, "CATEGORY", "work").unwrap();

    let category = out.find(":CATEGORY:").unwrap();
    let end = out.find(":END:").unwrap();
    assert!(category < end, "it must land inside the drawer:\n{out}");
}

#[test]
fn removing_reports_whether_there_was_anything_to_remove() {
    let out = remove_property(ENTRY, 5, "EFFORT").unwrap().unwrap();
    assert!(!out.contains("EFFORT"), "{out}");
    // The rest of the drawer survives.
    assert!(out.contains(":ID:       6ba7b810-9dad-11d1-80b4-00c04fd430c8"));

    assert_eq!(remove_property(ENTRY, 5, "ABSENT").unwrap(), None);
}

#[test]
fn a_value_can_be_read_back() {
    assert_eq!(property_value(ENTRY, 5, "EFFORT").as_deref(), Some("1:00"));
    // Keys are matched without regard to case, as Org matches them.
    assert_eq!(property_value(ENTRY, 5, "effort").as_deref(), Some("1:00"));
    assert_eq!(property_value(ENTRY, 5, "ABSENT"), None);
}

#[test]
fn an_entry_without_a_drawer_says_so_rather_than_inventing_one() {
    let bare = "* No drawer here\nBody.\n";
    assert_eq!(
        set_property(bare, 1, "EFFORT", "1:00"),
        Err(Error::NoDrawer)
    );
}

#[test]
fn the_keys_in_use_are_offered_for_completion() {
    let text = "\
* One
:PROPERTIES:
:ID:       a
:EFFORT:   1:00
:END:
* Two
:PROPERTIES:
:ID:       b
:CATEGORY: work
:END:
";
    // Deduplicated, upper-cased and sorted, so the list reads the same way
    // every time.
    assert_eq!(property_keys(text), ["CATEGORY", "EFFORT", "ID"]);
}

#[test]
fn a_drawer_is_inserted_below_the_properties_rather_than_inside_them() {
    let out = insert_drawer(ENTRY, 5, "LOGBOOK");

    let properties_end = out.find(":END:").unwrap();
    let logbook = out.find(":LOGBOOK:").unwrap();
    assert!(logbook > properties_end, "{out}");
    // Colons and case in the name are the caller's mistake to forgive.
    assert!(insert_drawer(ENTRY, 5, ":notes:").contains(":NOTES:"));
}

#[test]
fn a_logbook_is_created_when_there_is_none() {
    let out = log_entry(ENTRY, 5, "- Note taken on [2026-09-18 Fri 09:05]");

    assert!(
        out.contains(":LOGBOOK:\n- Note taken on [2026-09-18 Fri 09:05]\n:END:"),
        "{out}"
    );
    // And it sits under the entry's own drawer.
    assert!(out.find(":LOGBOOK:").unwrap() > out.find(":EFFORT:").unwrap());
}

#[test]
fn later_entries_go_on_top_of_earlier_ones() {
    let first = log_entry(ENTRY, 5, "- first");
    let second = log_entry(&first, 5, "- second");

    let at_second = second.find("- second").unwrap();
    let at_first = second.find("- first").unwrap();
    assert!(
        at_second < at_first,
        "newest first is how Org writes them:\n{second}"
    );
    // Still one logbook, not two.
    assert_eq!(second.matches(":LOGBOOK:").count(), 1);
}

#[test]
fn a_logbook_belonging_to_another_entry_is_not_reused() {
    let two = "\
* First
:PROPERTIES:
:ID:       a
:END:
Body.
* Second
:PROPERTIES:
:ID:       b
:END:
:LOGBOOK:
- belongs to Second
:END:
";
    // Logging on the first entry must not append to the second's logbook.
    let out = log_entry(two, 4, "- belongs to First");
    assert_eq!(out.matches(":LOGBOOK:").count(), 2, "{out}");
    assert!(out.find("- belongs to First").unwrap() < out.find("* Second").unwrap());
}

#[test]
fn a_child_inherits_its_parents_properties() {
    use helix_roam::parse_org;

    let text = "\
* Parent
:PROPERTIES:
:ID:       6ba7b810-9dad-11d1-80b4-00c04fd430c8
:CATEGORY: work
:END:
** Child
:PROPERTIES:
:ID:       6ba7b811-9dad-11d1-80b4-00c04fd430c8
:EFFORT:   1:00
:END:
";
    let file = parse_org(text, "n.org");
    let child = file.nodes.iter().find(|n| n.title == "Child").unwrap();

    // Its own, and the parent's.
    assert_eq!(child.property("EFFORT"), Some("1:00"));
    assert_eq!(child.property("CATEGORY"), Some("work"));
    // The two are kept apart, so a caller can still ask what the entry itself
    // declares.
    assert!(child.properties.iter().any(|(k, _)| k == "effort"));
    assert!(child
        .inherited_properties
        .iter()
        .any(|(k, _)| k == "category"));
}

#[test]
fn a_child_can_override_what_it_would_inherit() {
    use helix_roam::parse_org;

    let text = "\
* Parent
:PROPERTIES:
:ID:       a
:CATEGORY: work
:END:
** Child
:PROPERTIES:
:ID:       b
:CATEGORY: personal
:END:
";
    let file = parse_org(text, "n.org");
    let child = file.nodes.iter().find(|n| n.title == "Child").unwrap();

    assert_eq!(child.property("CATEGORY"), Some("personal"));
    // The overridden value is not also carried as inherited.
    assert!(!child
        .inherited_properties
        .iter()
        .any(|(k, _)| k == "category"));
}

#[test]
fn file_level_properties_reach_every_entry() {
    use helix_roam::parse_org;

    let text = "\
#+PROPERTY: Category work
* A heading
:PROPERTIES:
:ID:       a
:END:
";
    let file = parse_org(text, "n.org");
    let node = file.nodes.iter().find(|n| n.title == "A heading").unwrap();

    assert_eq!(node.property("category"), Some("work"));
}

#[test]
fn a_query_matches_an_inherited_value() {
    use helix_roam::{scan_directory, NodeQuery};

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("n.org"),
        "#+PROPERTY: Category work\n* A heading\n:PROPERTIES:\n:ID:       6ba7b810-9dad-11d1-80b4-00c04fd430c8\n:END:\n",
    )
    .unwrap();
    let (graph, _) = scan_directory(dir.path());

    let query = NodeQuery {
        property: Some(("CATEGORY".into(), "work".into())),
        ..NodeQuery::default()
    };
    // A query that ignored the file-wide default would find nothing here.
    assert_eq!(graph.query(&query).len(), 1);
}

/// Org requires the planning line directly under the headline, so every
/// drawer comes after it. Each of these used to assume the drawer came first.
mod under_a_planning_line {
    use helix_roam::restructure::{
        ensure_id, insert_drawer, log_entry, property_value, set_planning, set_property, IdOutcome,
        Planning,
    };
    use helix_roam::Uuid;

    const PLANNED: &str = "\
* DONE A task
CLOSED: [2026-09-20 Sun 10:00] SCHEDULED: <2026-09-20 Sun>
:PROPERTIES:
:EFFORT: 1:00
:END:
Body.
";

    #[test]
    fn the_property_drawer_is_still_found() {
        assert_eq!(
            property_value(PLANNED, 0, "EFFORT").as_deref(),
            Some("1:00")
        );
        let out = set_property(PLANNED, 0, "EFFORT", "2:00").unwrap();
        assert!(out.contains(":EFFORT: 2:00"), "{out}");
    }

    #[test]
    fn setting_one_planning_stamp_keeps_the_others() {
        let out = set_planning(PLANNED, 0, Planning::Deadline, Some("<2026-09-30 Wed>")).unwrap();
        assert!(
            out.contains(
                "CLOSED: [2026-09-20 Sun 10:00] DEADLINE: <2026-09-30 Wed> SCHEDULED: <2026-09-20 Sun>"
            ),
            "{out}"
        );
    }

    #[test]
    fn the_logbook_goes_below_the_planning_line_and_the_properties() {
        let out = log_entry(PLANNED, 0, "- note");
        assert_eq!(
            out,
            "* DONE A task\nCLOSED: [2026-09-20 Sun 10:00] SCHEDULED: <2026-09-20 Sun>\n\
             :PROPERTIES:\n:EFFORT: 1:00\n:END:\n:LOGBOOK:\n- note\n:END:\nBody.\n"
        );
    }

    #[test]
    fn new_drawers_go_below_the_planning_line() {
        let planned = "* TODO A task\nSCHEDULED: <2026-09-20 Sun>\nBody.\n";

        let out = insert_drawer(planned, 0, "notes");
        assert!(
            out.starts_with("* TODO A task\nSCHEDULED: <2026-09-20 Sun>\n:NOTES:"),
            "{out}"
        );

        let out = log_entry(planned, 0, "- note");
        assert!(
            out.starts_with("* TODO A task\nSCHEDULED: <2026-09-20 Sun>\n:LOGBOOK:"),
            "{out}"
        );

        let id: Uuid = "6ba7b810-9dad-11d1-80b4-00c04fd430c8".parse().unwrap();
        let IdOutcome::Created { text, .. } = ensure_id(planned, 0, id).unwrap() else {
            panic!("the entry had no id");
        };
        assert!(
            text.starts_with("* TODO A task\nSCHEDULED: <2026-09-20 Sun>\n:PROPERTIES:"),
            "{text}"
        );
    }
}

#[test]
fn an_id_goes_into_the_drawer_the_entry_already_has() {
    use helix_roam::restructure::{ensure_id, IdOutcome};
    let id: helix_roam::Uuid = "6ba7b810-9dad-11d1-80b4-00c04fd430c8".parse().unwrap();
    let text = "* Task\nSCHEDULED: <2026-09-24 Thu>\n:PROPERTIES:\n:Effort: 1:30\n:END:\nBody.\n";
    let IdOutcome::Created { text, .. } = ensure_id(text, 0, id).unwrap() else {
        panic!("the entry had no id");
    };
    assert_eq!(
        text,
        "* Task\nSCHEDULED: <2026-09-24 Thu>\n:PROPERTIES:\n:Effort: 1:30\n\
         :ID:       6ba7b810-9dad-11d1-80b4-00c04fd430c8\n:END:\nBody.\n"
    );
}
