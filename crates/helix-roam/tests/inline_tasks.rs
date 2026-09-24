//! Inline tasks: fifteen stars or more, a task inside an entry's body that
//! does not end the entry.

use helix_roam::restructure::{cycle_todo, log_entry};
use helix_roam::FileSettings;

const NOTE: &str = "\
* TODO Write the chapter
Some prose.
*************** TODO Check the quote
It may be misattributed.
*************** END
More prose, still the chapter's.
*************** WAIT No END line on this one
Final paragraph.
* Next
";

fn cycled(line: usize) -> String {
    cycle_todo(NOTE, line, &FileSettings::scan(NOTE), true).unwrap()
}

#[test]
fn the_task_at_the_cursor_is_the_inline_one() {
    let out = cycled(2);
    assert!(
        out.contains("*************** DONE Check the quote\n"),
        "{out}"
    );
    assert!(out.starts_with("* TODO Write the chapter\n"), "{out}");
}

#[test]
fn inside_an_inline_task_s_body_the_entry_is_the_task() {
    let out = cycled(3);
    assert!(
        out.contains("*************** DONE Check the quote\n"),
        "{out}"
    );
}

#[test]
fn after_the_end_line_the_entry_is_the_headline_again() {
    let out = cycled(5);
    assert!(out.starts_with("* DONE Write the chapter\n"), "{out}");
    assert!(
        out.contains("*************** TODO Check the quote\n"),
        "{out}"
    );
}

#[test]
fn an_inline_task_without_end_is_one_line() {
    // The line after it belongs to the chapter.
    let out = cycled(7);
    assert!(out.starts_with("* DONE Write the chapter\n"), "{out}");
}

#[test]
fn the_parent_s_logbook_goes_under_the_parent() {
    let out = log_entry(NOTE, 5, "- noted");
    assert!(
        out.starts_with("* TODO Write the chapter\n:LOGBOOK:\n- noted\n:END:\nSome prose."),
        "{out}"
    );
}

#[test]
fn inline_tasks_are_not_headings_of_the_outline() {
    let titles: Vec<String> = helix_roam::outline::headings(NOTE)
        .into_iter()
        .map(|entry| entry.title)
        .collect();
    assert_eq!(titles, ["Write the chapter", "Next"]);
}

#[test]
fn the_end_line_is_never_a_headline() {
    let file = helix_roam::parse_org(NOTE, "n.org");
    assert!(file.nodes.iter().all(|node| node.title != "END"));
}

#[test]
fn export_draws_an_inline_task_apart_from_the_body() {
    let out = helix_roam::export::export(
        &format!("#+OPTIONS: toc:nil num:nil\n{NOTE}"),
        std::path::Path::new("/n/x.org"),
        helix_roam::export::Backend::Html,
        &helix_roam::export::NoResolve,
    )
    .content;
    assert!(
        out.contains("<div class=\"inlinetask\">\n<p>\n<b>TODO Check the quote</b>\n</p>\n<p>\nIt may be misattributed.\n</p>\n</div>"),
        "{out}"
    );
    assert!(
        out.contains("<p>\nMore prose, still the chapter&#x27;s.\n</p>")
            || out.contains("More prose, still the chapter's."),
        "{out}"
    );
    // One section, not three.
    assert_eq!(out.matches("<h2").count(), 2, "{out}");
}
