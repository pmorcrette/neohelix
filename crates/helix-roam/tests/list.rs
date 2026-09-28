//! Plain lists, checkboxes and their cookies.

use helix_roam::list::{
    insert_item, parse_item, renumber, shift_item, toggle_checkbox, update_cookies, Bullet,
    Checkbox,
};

#[test]
fn each_bullet_form_is_recognised() {
    assert_eq!(parse_item("- a").unwrap().bullet, Bullet::Unordered('-'));
    assert_eq!(parse_item("+ a").unwrap().bullet, Bullet::Unordered('+'));
    assert_eq!(parse_item("1. a").unwrap().bullet, Bullet::Ordered(1, '.'));
    assert_eq!(
        parse_item("12) a").unwrap().bullet,
        Bullet::Ordered(12, ')')
    );

    // `* a` at column zero is a headline, not a list item.
    assert!(parse_item("* a").is_none());
    // Indented, it is a list item.
    assert_eq!(parse_item("  * a").unwrap().bullet, Bullet::Unordered('*'));

    assert!(parse_item("just prose").is_none());
    assert!(parse_item("1.no space").is_none());
}

#[test]
fn a_checkbox_is_read_apart_from_the_content() {
    let item = parse_item("- [X] done thing").unwrap();
    assert_eq!(item.checkbox, Some(Checkbox::Done));
    assert_eq!(item.content, "done thing");

    // Lowercase is Org's too.
    assert_eq!(
        parse_item("- [x] a").unwrap().checkbox,
        Some(Checkbox::Done)
    );
    assert_eq!(
        parse_item("- [-] a").unwrap().checkbox,
        Some(Checkbox::Partial)
    );
    assert_eq!(parse_item("- a").unwrap().checkbox, None);
}

const LIST: &str = "\
1. first
2. second
3. third
";

#[test]
fn inserting_an_item_renumbers_what_follows() {
    let (out, at) = insert_item(LIST, 0).unwrap();

    assert_eq!(at, 1);
    // The new item carries no trailing space: the file stays clean and the
    // editor is what puts the cursor where typing continues.
    assert_eq!(out, "1. first\n2.\n3. second\n4. third\n");
}

#[test]
fn an_item_is_inserted_after_its_children_not_inside_them() {
    let nested = "- parent\n  - child\n- next\n";
    let (out, at) = insert_item(nested, 0).unwrap();

    assert_eq!(at, 2, "after the child");
    assert_eq!(out, "- parent\n  - child\n-\n- next\n");
}

#[test]
fn a_new_item_in_a_checklist_is_itself_tickable() {
    let checklist = "- [X] one\n- [ ] two\n";
    let (out, _) = insert_item(checklist, 0).unwrap();

    // Empty rather than copying the state of the item it followed.
    assert_eq!(out, "- [X] one\n- [ ]\n- [ ] two\n");
}

#[test]
fn renumbering_fixes_a_list_that_was_edited_by_hand() {
    let messy = "1. first\n1. second\n7. third\n";
    assert_eq!(renumber(messy, 0), "1. first\n2. second\n3. third\n");
}

#[test]
fn renumbering_leaves_nested_items_and_bullets_alone() {
    let mixed = "1. first\n   1. inner\n   5. inner two\n9. second\n";
    let out = renumber(mixed, 0);

    // The outer list is renumbered; the inner one keeps its own sequence.
    assert_eq!(out, "1. first\n   1. inner\n   5. inner two\n2. second\n");
}

#[test]
fn an_item_moves_with_its_children() {
    let nested = "- parent\n- child\n  - grandchild\n";
    let out = shift_item(nested, 1, true).unwrap();

    assert_eq!(out, "- parent\n  - child\n    - grandchild\n");
}

#[test]
fn a_top_level_item_cannot_move_out_further() {
    assert!(shift_item("- a\n", 0, false).is_none());
}

#[test]
fn toggling_ticks_and_unticks() {
    let out = toggle_checkbox("- [ ] a\n", 0).unwrap();
    assert_eq!(out, "- [X] a\n");

    let back = toggle_checkbox(&out, 0).unwrap();
    assert_eq!(back, "- [ ] a\n");
}

#[test]
fn toggling_an_item_without_a_checkbox_gives_it_one() {
    // Asking to tick something untickable plainly means "make it tickable".
    assert_eq!(toggle_checkbox("- a\n", 0).unwrap(), "- [X] a\n");
}

const COOKIE_LIST: &str = "\
- Shopping [0/3]
  - [ ] bread
  - [X] milk
  - [ ] eggs
";

#[test]
fn a_cookie_counts_the_children_below_it() {
    let out = update_cookies(COOKIE_LIST);
    assert!(out.starts_with("- Shopping [1/3]"), "{out}");
}

#[test]
fn a_percentage_cookie_keeps_its_form() {
    let text = "- Shopping [0%]\n  - [X] bread\n  - [ ] milk\n";
    let out = update_cookies(text);
    assert!(out.starts_with("- Shopping [50%]"), "{out}");
}

#[test]
fn toggling_updates_the_parents_cookie() {
    // Ticking `bread` takes the count from 1 to 2.
    let out = toggle_checkbox(COOKIE_LIST, 1).unwrap();
    assert!(out.starts_with("- Shopping [2/3]"), "{out}");
}

#[test]
fn a_cookie_counts_one_level_only() {
    let deep = "\
- Top [0/2]
  - [ ] one
    - [X] nested
    - [X] nested two
  - [ ] two
";
    // The nested ticks belong to `one`, not to `Top`.
    let out = update_cookies(deep);
    assert!(out.starts_with("- Top [0/2]"), "{out}");
}

#[test]
fn a_headline_can_carry_a_cookie_for_the_list_under_it() {
    let text = "* Tasks [0/2]\n- [X] one\n- [ ] two\n* Next heading\n";
    let out = update_cookies(text);

    assert!(out.starts_with("* Tasks [1/2]"), "{out}");
    // And the next headline is not counted into it.
    assert!(out.contains("* Next heading"));
}

#[test]
fn a_line_with_no_cookie_is_left_alone() {
    let text = "- Shopping\n  - [X] bread\n";
    assert_eq!(update_cookies(text), text);
}

#[test]
fn a_link_is_not_mistaken_for_a_cookie() {
    // `[[id:…]]` and `[#A]` both use brackets and are not counts.
    let text = "- See [[id:abc][a node]] and [#A]\n  - [X] one\n";
    assert_eq!(update_cookies(text), text);
}
