//! Right-click and `Shift+L` tag menus: toggling a tag, creating one, and the shared
//! triage actions, all reaching the `Mail` fixture.
mod common;
use common::{Fixture, mount, update_content};
use gpui_kit::base::Root;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext, WindowHandle};
use kite_core::model::Action;

/// The fixture's tag list order, then "New tag…", a separator, then the actions.
/// Menu item ids are their index in this order.
const TAG_NEWSLETTER: usize = 0;
const TAG_NEW: usize = 3;
const ACTION_DONE: usize = 5; // index 4 is the separator

#[gpui_kit::test]
fn private_tag_toggles_on_the_clicked_row_without_opening_it(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, _) = mount(cx, fixture.clone());
    let id: gpui_kit::ElementId = ("row", 1usize).into();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.right_click(id, cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.within("popup-menu").find(2usize).label(),
            Some("Private · exclude from AI")
        );
        window.within("popup-menu").click(2usize, cx);
        window.render_frame(cx);
        assert_eq!(
            window.find(("row-tag", 1usize)).label(),
            Some("Tag: Private")
        );
    })
    .unwrap();
    menu_click(window, ("row", 1usize), 2, cx);
    assert_eq!(
        fixture.last_action(),
        Some((
            vec![2],
            Action::Unlabel {
                tag: "Private".into()
            }
        ))
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("row-tag", 1usize)).is_none());
    })
    .unwrap();
    assert!(fixture.opened.lock().unwrap().is_empty());
}

/// Right-clicks `id`, lets the deferred menu build, and clicks the item at `item`.
fn menu_click(
    window: WindowHandle<Root>,
    id: impl Into<gpui_kit::ElementId>,
    item: usize,
    cx: &mut TestAppContext,
) {
    let id = id.into();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.right_click(id, cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.within("popup-menu").click(item, cx);
        window.render_frame(cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn right_click_opens_the_menu_and_a_tag_reaches_the_row(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, kite) = mount(cx, fixture.clone());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("popup-menu").is_none());
    })
    .unwrap();
    menu_click(window, ("row", 0usize), TAG_NEWSLETTER, cx);
    assert_eq!(
        fixture.last_action(),
        Some((
            vec![1],
            Action::Label {
                tag: "Newsletter".into()
            }
        )),
        "the menu item applies the tag through Mail::act"
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("row-tag", 0usize)).label(),
            Some("Tag: Newsletter"),
            "the row paints the chip"
        );
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.sel_index(), 0, "right-click selected the row");
    });
}

#[gpui_kit::test]
fn right_click_inside_the_multi_selection_tags_all_of_it(cx: &mut TestAppContext) {
    let fixture = Fixture::new(4);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "x x");
    // Row 2 is inside the selection made by `x x` (rows 1 and 2).
    menu_click(window, ("row", 1usize), TAG_NEWSLETTER, cx);
    assert_eq!(
        fixture.last_action(),
        Some((
            vec![1, 2],
            Action::Label {
                tag: "Newsletter".into()
            }
        )),
        "the tag applies to the whole selection, not only the clicked row"
    );
}

#[gpui_kit::test]
fn right_click_outside_the_multi_selection_tags_only_the_clicked_row(cx: &mut TestAppContext) {
    let fixture = Fixture::new(4);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "x x");
    menu_click(window, ("row", 3usize), TAG_NEWSLETTER, cx);
    assert_eq!(
        fixture.last_action(),
        Some((
            vec![4],
            Action::Label {
                tag: "Newsletter".into()
            }
        )),
        "a secondary click outside the selection replaces the action's targets"
    );
    assert!(fixture.opened.lock().unwrap().is_empty());
}

#[gpui_kit::test]
fn new_tag_creates_and_applies_a_tag(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    menu_click(window, ("row", 0usize), TAG_NEW, cx);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.find("tag-name");
        window.input("Receipts", cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "enter");
    assert!(
        fixture
            .tags
            .lock()
            .unwrap()
            .iter()
            .any(|tag| tag.name == "Receipts"),
        "the tag was created through set_tags"
    );
    assert_eq!(
        fixture.last_action(),
        Some((
            vec![1],
            Action::Label {
                tag: "Receipts".into()
            }
        )),
        "…and applied to the conversation"
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("row-tag", 0usize)).label(),
            Some("Tag: Receipts")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_menu_action_runs_the_same_command_as_the_toolbar(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, _) = mount(cx, fixture.clone());
    menu_click(window, ("row", 0usize), ACTION_DONE, cx);
    assert_eq!(fixture.last_action(), Some((vec![1], Action::Archive)));
}

#[gpui_kit::test]
fn right_click_on_the_open_conversation_offers_the_same_menu(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    menu_click(window, "thread-scroll", TAG_NEWSLETTER, cx);
    assert_eq!(
        fixture.last_action(),
        Some((
            vec![1],
            Action::Label {
                tag: "Newsletter".into()
            }
        )),
        "the conversation menu tags the open thread"
    );
}

#[gpui_kit::test]
fn row_popup_archives_its_captured_row_while_the_keyboard_is_in_another_thread(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new(3);
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    let opened = fixture.opened.lock().unwrap().clone();
    let actions = fixture.actions().len();
    menu_click(window, ("row", 1usize), ACTION_DONE, cx);
    assert_eq!(fixture.actions()[actions..], [(vec![2], Action::Archive)]);
    assert_eq!(*fixture.opened.lock().unwrap(), opened);
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.open_id(), Some(1), "the other preview stays open");
        assert_eq!(kite.row_ids(), vec![1, 3]);
    });
}

#[gpui_kit::test]
fn thread_popup_archives_its_preview_while_a_different_list_row_is_selected(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new(3);
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter escape j");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.open_id(), Some(1));
        assert_eq!(kite.sel_index(), 1);
    });
    let opened = fixture.opened.lock().unwrap().clone();
    let actions = fixture.actions().len();
    menu_click(window, "thread-scroll", ACTION_DONE, cx);
    assert_eq!(fixture.actions()[actions..], [(vec![1], Action::Archive)]);
    assert_eq!(*fixture.opened.lock().unwrap(), opened);
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.open_id(), None);
        assert_eq!(kite.row_ids(), vec![2, 3]);
    });
}

#[gpui_kit::test]
fn shift_l_opens_the_tag_menu_for_the_keyboard(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "shift-l");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let menu = window.find("tag-dialog");
        assert_eq!(menu.label(), Some("Tags and actions"));
        assert_eq!(
            window.find(("tag-item", 0usize)).label(),
            Some("Newsletter")
        );
        // Down to "To Reply" and toggle it on.
        window.click(("tag-item", 1usize), cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(
        fixture.last_action(),
        Some((
            vec![1],
            Action::Label {
                tag: "To Reply".into()
            }
        ))
    );
    // The command is also in the palette like every other command.
    cx.simulate_keystrokes(window.into(), "escape");
    cx.simulate_keystrokes(window.into(), "secondary-k");
    cx.simulate_keystrokes(window.into(), "t a g");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("palette-row", 0usize)).label(),
            Some("Tag conversation")
        );
    })
    .unwrap();
}
