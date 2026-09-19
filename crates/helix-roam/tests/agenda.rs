//! What lands on which day.

use helix_roam::agenda::{agenda, todo_list, Reason};
use helix_roam::{parse_org, Date, RepeaterKind, RepeaterUnit};

fn day(year: i32, month: u32, day: u32) -> Date {
    Date { year, month, day }
}

fn nodes(text: &str) -> Vec<helix_roam::Node> {
    parse_org(text, "n.org").nodes
}

const TASKS: &str = "\
* TODO Scheduled today
SCHEDULED: <2026-09-19 Sat>
:PROPERTIES:
:ID: a
:END:
* TODO Due in three days
DEADLINE: <2026-09-22 Tue>
:PROPERTIES:
:ID: b
:END:
* DONE Finished but scheduled
SCHEDULED: <2026-09-19 Sat>
:PROPERTIES:
:ID: c
:END:
* TODO No dates at all
:PROPERTIES:
:ID: d
:END:
";

#[test]
fn a_day_view_shows_what_falls_on_that_day() {
    let nodes = nodes(TASKS);
    let entries = agenda(&nodes, day(2026, 9, 19), 1);

    let titles: Vec<&str> = entries.iter().map(|e| e.node.title.as_str()).collect();
    // The scheduled one, and the deadline inside its warning period.
    assert_eq!(titles, ["Due in three days", "Scheduled today"]);
    // A deadline sorts before a scheduled item on the same day.
    assert_eq!(entries[0].reason, Reason::Deadline);
    assert_eq!(entries[0].days_left, Some(3));
}

#[test]
fn a_finished_task_is_not_pending_whatever_its_dates_say() {
    let nodes = nodes(TASKS);
    let entries = agenda(&nodes, day(2026, 9, 19), 7);

    assert!(
        !entries
            .iter()
            .any(|e| e.node.title == "Finished but scheduled"),
        "a DONE task should not be on the agenda"
    );
}

#[test]
fn a_task_with_no_dates_is_not_on_the_agenda_but_is_on_the_todo_list() {
    let nodes = nodes(TASKS);
    let entries = agenda(&nodes, day(2026, 9, 19), 7);
    assert!(!entries.iter().any(|e| e.node.title == "No dates at all"));

    let todos: Vec<&str> = todo_list(&nodes).iter().map(|n| n.title.as_str()).collect();
    assert!(todos.contains(&"No dates at all"), "{todos:?}");
    // And the DONE one is not a todo.
    assert!(!todos.contains(&"Finished but scheduled"));
}

#[test]
fn a_week_view_spreads_entries_over_their_days() {
    let nodes = nodes(TASKS);
    let entries = agenda(&nodes, day(2026, 9, 19), 7);

    let deadline = entries
        .iter()
        .find(|e| e.node.title == "Due in three days" && e.reason == Reason::Deadline)
        .unwrap();
    assert_eq!(deadline.day, day(2026, 9, 22));
}

#[test]
fn an_overdue_deadline_is_carried_onto_today_rather_than_disappearing() {
    let nodes = nodes("* TODO Late\nDEADLINE: <2026-09-10 Thu>\n:PROPERTIES:\n:ID: a\n:END:\n");
    let entries = agenda(&nodes, day(2026, 9, 19), 1);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].day, day(2026, 9, 19), "it shows today");
    assert_eq!(entries[0].days_left, Some(-9), "and says how late it is");
}

#[test]
fn a_deadline_beyond_the_warning_period_waits_its_turn() {
    let nodes = nodes("* TODO Far off\nDEADLINE: <2026-12-25 Fri>\n:PROPERTIES:\n:ID: a\n:END:\n");
    assert!(agenda(&nodes, day(2026, 9, 19), 1).is_empty());
}

#[test]
fn a_repeating_task_lands_on_every_occurrence_in_the_window() {
    let nodes = nodes(
        "* TODO Weekly standup\nSCHEDULED: <2026-09-21 Mon +1w>\n:PROPERTIES:\n:ID: a\n:END:\n",
    );
    // Four weeks from the Saturday before the first Monday.
    let entries = agenda(&nodes, day(2026, 9, 19), 28);

    let days: Vec<Date> = entries.iter().map(|e| e.day).collect();
    assert_eq!(
        days,
        [
            day(2026, 9, 21),
            day(2026, 9, 28),
            day(2026, 10, 5),
            day(2026, 10, 12)
        ]
    );
}

#[test]
fn a_repeater_set_up_long_ago_still_lands_on_the_right_weekday() {
    let nodes =
        nodes("* TODO Old weekly\nSCHEDULED: <2020-01-06 Mon +1w>\n:PROPERTIES:\n:ID: a\n:END:\n");
    let entries = agenda(&nodes, day(2026, 9, 19), 7);

    // 2020-01-06 was a Monday; every occurrence since is a Monday.
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].day, day(2026, 9, 21));
    assert_eq!(entries[0].day.weekday(), "Mon");
}

#[test]
fn repeaters_are_read_off_the_timestamp() {
    let nodes =
        nodes("* TODO Monthly\nSCHEDULED: <2026-01-31 Sat ++1m>\n:PROPERTIES:\n:ID: a\n:END:\n");
    let repeater = nodes[0].scheduled.unwrap().repeater.unwrap();

    assert_eq!(repeater.kind, RepeaterKind::CatchUp);
    assert_eq!(repeater.count, 1);
    assert_eq!(repeater.unit, RepeaterUnit::Month);
}

#[test]
fn a_monthly_repeater_clamps_to_the_end_of_a_short_month() {
    let nodes =
        nodes("* TODO Month end\nSCHEDULED: <2026-01-31 Sat +1m>\n:PROPERTIES:\n:ID: a\n:END:\n");
    let entries = agenda(&nodes, day(2026, 2, 1), 60);

    // January's 31st plus a month is February's 28th, not a date that does
    // not exist.
    assert_eq!(entries[0].day, day(2026, 2, 28));
    assert_eq!(entries[1].day, day(2026, 3, 28));
}

#[test]
fn a_warning_period_is_not_mistaken_for_a_repeater() {
    let nodes =
        nodes("* TODO Warned\nDEADLINE: <2026-09-22 Tue -3d>\n:PROPERTIES:\n:ID: a\n:END:\n");
    assert_eq!(nodes[0].deadline.unwrap().repeater, None);
}

#[test]
fn an_inactive_timestamp_stays_out_of_the_agenda() {
    let nodes =
        nodes("* TODO Recorded\nSCHEDULED: [2026-09-19 Sat]\n:PROPERTIES:\n:ID: a\n:END:\n");
    assert!(agenda(&nodes, day(2026, 9, 19), 1).is_empty());
}

#[test]
fn the_todo_list_puts_priorities_first() {
    let nodes = nodes(
        "* TODO [#C] Low\n:PROPERTIES:\n:ID: a\n:END:\n\
         * TODO [#A] High\n:PROPERTIES:\n:ID: b\n:END:\n\
         * TODO Unranked\n:PROPERTIES:\n:ID: c\n:END:\n",
    );
    let titles: Vec<&str> = todo_list(&nodes).iter().map(|n| n.title.as_str()).collect();

    assert_eq!(titles, ["High", "Low", "Unranked"]);
}
