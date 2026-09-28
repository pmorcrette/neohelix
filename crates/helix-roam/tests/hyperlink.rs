//! Reading Org's link syntax.

use helix_roam::hyperlink::{
    find_links, format_link, link_at, next_link, parse_target, previous_link, FileSearch, LinkKind,
};
use helix_roam::FileSettings;

fn none() -> Vec<(String, String)> {
    Vec::new()
}

#[test]
fn each_built_in_type_is_recognised() {
    let cases: Vec<(&str, LinkKind)> = vec![
        (
            "id:6ba7b810-9dad-11d1-80b4-00c04fd430c8",
            LinkKind::Id(uuid::Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap()),
        ),
        (
            "https://example.org/x",
            LinkKind::Url("https://example.org/x".into()),
        ),
        (
            "mailto:someone@example.org",
            LinkKind::Mailto("someone@example.org".into()),
        ),
        ("*A Headline", LinkKind::Headline("A Headline".into())),
        ("#custom-id", LinkKind::CustomId("custom-id".into())),
        ("roam:Old Title", LinkKind::Roam("Old Title".into())),
        ("a-plain-target", LinkKind::Target("a-plain-target".into())),
    ];

    for (raw, expected) in cases {
        assert_eq!(parse_target(raw, &none()), expected, "{raw}");
    }
}

#[test]
fn a_file_link_can_name_where_to_land() {
    assert_eq!(
        parse_target("file:notes.org", &none()),
        LinkKind::File {
            path: "notes.org".into(),
            search: None
        }
    );
    assert_eq!(
        parse_target("file:notes.org::42", &none()),
        LinkKind::File {
            path: "notes.org".into(),
            search: Some(FileSearch::Line(42))
        }
    );
    assert_eq!(
        parse_target("file:notes.org::*A Headline", &none()),
        LinkKind::File {
            path: "notes.org".into(),
            search: Some(FileSearch::Headline("A Headline".into()))
        }
    );
    assert_eq!(
        parse_target("file:notes.org::some text", &none()),
        LinkKind::File {
            path: "notes.org".into(),
            search: Some(FileSearch::Text("some text".into()))
        }
    );
}

#[test]
fn a_bare_path_is_a_file_link_but_a_bare_word_is_not() {
    assert!(matches!(
        parse_target("./sub/notes.org", &none()),
        LinkKind::File { .. }
    ));
    assert!(matches!(
        parse_target("notes.org", &none()),
        LinkKind::File { .. }
    ));
    // Without a slash or an extension this is an internal target.
    assert_eq!(
        parse_target("somewhere", &none()),
        LinkKind::Target("somewhere".into())
    );
}

#[test]
fn an_unknown_scheme_is_kept_whole_rather_than_guessed_at() {
    assert_eq!(
        parse_target("denote:20240101T120000", &none()),
        LinkKind::Other {
            scheme: "denote".into(),
            rest: "20240101T120000".into()
        }
    );
}

#[test]
fn abbreviations_come_from_the_file() {
    let abbreviations = vec![
        ("gh".to_string(), "https://github.com/%s".to_string()),
        ("bug".to_string(), "https://bugs.example.org/".to_string()),
    ];

    // `%s` marks where the tail goes.
    assert_eq!(
        parse_target("gh:helix-editor/helix", &abbreviations),
        LinkKind::Url("https://github.com/helix-editor/helix".into())
    );
    // Without `%s` the tail is appended, which is the common shape.
    assert_eq!(
        parse_target("bug:1234", &abbreviations),
        LinkKind::Url("https://bugs.example.org/1234".into())
    );
    // An abbreviation the file does not declare stays unexpanded.
    assert_eq!(
        parse_target("gh:x/y", &none()),
        LinkKind::Other {
            scheme: "gh".into(),
            rest: "x/y".into()
        }
    );
}

#[test]
fn the_file_declares_its_own_abbreviations() {
    let settings = FileSettings::scan("#+LINK: gh https://github.com/%s\n#+LINK: bug https://b/\n");
    assert_eq!(
        settings.link_abbreviations,
        [
            ("gh".to_string(), "https://github.com/%s".to_string()),
            ("bug".to_string(), "https://b/".to_string())
        ]
    );
}

const TEXT: &str = "Before [[id:6ba7b810-9dad-11d1-80b4-00c04fd430c8][a node]] then \
[[https://example.org]] and [[*A Headline][see there]] after.";

#[test]
fn links_are_found_with_their_descriptions_and_ranges() {
    let links = find_links(TEXT, &none());
    assert_eq!(links.len(), 3);

    assert_eq!(links[0].description.as_deref(), Some("a node"));
    assert_eq!(links[1].description, None);
    assert_eq!(links[2].description.as_deref(), Some("see there"));

    // The range covers the whole `[[…]]`, description included.
    assert_eq!(
        &TEXT[links[0].range.clone()],
        "[[id:6ba7b810-9dad-11d1-80b4-00c04fd430c8][a node]]"
    );
    assert_eq!(&TEXT[links[1].range.clone()], "[[https://example.org]]");
}

#[test]
fn the_cursor_finds_the_link_it_is_inside() {
    let links = find_links(TEXT, &none());
    let inside = links[0].range.start + 5;

    assert_eq!(link_at(TEXT, inside, &none()).unwrap(), links[0]);
    // Just before the first link there is nothing to follow.
    assert!(link_at(TEXT, 0, &none()).is_none());
}

#[test]
fn moving_between_links_goes_where_it_should() {
    let links = find_links(TEXT, &none());

    assert_eq!(next_link(TEXT, 0, &none()).unwrap(), links[0]);
    assert_eq!(
        next_link(TEXT, links[0].range.start, &none()).unwrap(),
        links[1]
    );
    assert!(next_link(TEXT, links[2].range.start, &none()).is_none());

    assert_eq!(
        previous_link(TEXT, links[2].range.start, &none()).unwrap(),
        links[1]
    );
    assert!(previous_link(TEXT, 0, &none()).is_none());
}

#[test]
fn a_label_falls_back_to_the_target_when_there_is_no_description() {
    let links = find_links(TEXT, &none());
    assert_eq!(links[0].label(), "a node");
    assert_eq!(links[1].label(), "https://example.org");
}

#[test]
fn formatting_round_trips_through_parsing() {
    let text = format_link("id:abc", Some("a node"));
    assert_eq!(text, "[[id:abc][a node]]");

    let links = find_links(&text, &none());
    assert_eq!(links[0].description.as_deref(), Some("a node"));

    // An empty description produces a bare link rather than an empty one.
    assert_eq!(format_link("id:abc", Some("")), "[[id:abc]]");
    assert_eq!(format_link("id:abc", None), "[[id:abc]]");
}

#[test]
fn an_unterminated_link_does_not_swallow_the_rest() {
    let text = "ok [[id:abc][fine]] then [[broken with no close";
    let links = find_links(text, &none());
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].label(), "fine");
}
