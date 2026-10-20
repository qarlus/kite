//! Input mode: keyboard-first versus click-first, and the switch between them.
mod common;
use common::{Fixture, mount, update_content};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use kite_core::model::Action;

/// A fixture that opens in the click-first mode.
fn click_first() -> std::sync::Arc<Fixture> {
    let fixture = Fixture::new(3);
    fixture.settings.lock().unwrap().input_mode = "classic".into();
    fixture
}

#[gpui_kit::test]
fn classic_mode_leaves_single_keys_alone_and_the_row_button_does_the_work(cx: &mut TestAppContext) {
    let fixture = click_first();
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "e s # x c r a f g");
    assert!(
        fixture.actions().is_empty(),
        "typing must never run a mail action in classic mode"
    );
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_count(), 3);
        assert!(kite.open_id().is_none());
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.hover(("row", 0usize), cx);
        window.render_frame(cx);
        window.click(("row-archive", 0usize), cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(fixture.last_action(), Some((vec![1], Action::Archive)));
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_count(), 2, "the clicked row left the list");
        assert_eq!(kite.sel_index(), 0);
    });
}

#[gpui_kit::test]
fn classic_mode_still_keeps_every_modifier_shortcut(cx: &mut TestAppContext) {
    let fixture = click_first();
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "secondary-k");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("palette").is_some(),
            "the palette is a modifier shortcut and stays bound"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn arrows_move_the_selection_and_delete_trashes_in_classic_mode(cx: &mut TestAppContext) {
    let fixture = click_first();
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "down down");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.sel_index(), 2, "down moves the selection");
    });
    cx.simulate_keystrokes(window.into(), "up");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.sel_index(), 1, "up moves it back");
    });
    cx.simulate_keystrokes(window.into(), "delete");
    assert_eq!(
        fixture.last_action(),
        Some((vec![2], Action::Trash)),
        "delete trashes the selected thread"
    );
}

#[gpui_kit::test]
fn arrows_move_the_selection_and_delete_trashes_in_keyboard_mode_too(cx: &mut TestAppContext) {
    let fixture = Fixture::new(4);
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "down down up");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.sel_index(), 1);
    });
    cx.simulate_keystrokes(window.into(), "delete");
    assert_eq!(fixture.last_action(), Some((vec![2], Action::Trash)));
}

#[gpui_kit::test]
fn switching_back_to_keyboard_mode_re_enables_single_keys_without_a_restart(
    cx: &mut TestAppContext,
) {
    let fixture = click_first();
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 0usize), cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("input-mode-classic").selected(),
            Some(true),
            "General shows the mode that is in force"
        );
        window.click("input-mode-keyboard", cx);
        window.render_frame(cx);
        assert_eq!(window.find("input-mode-keyboard").selected(), Some(true));
        window.click("close-settings", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(fixture.settings.lock().unwrap().input_mode, "keyboard");
    cx.simulate_keystrokes(window.into(), "e");
    assert_eq!(
        fixture.last_action(),
        Some((vec![1], Action::Archive)),
        "e archives again as soon as the mode changes"
    );
}

/// The command ids behind the reading-pane toolbar buttons (keys::cmd).
const BACK: usize = 3;
const SNOOZE: usize = 7;
const MOVE: usize = 19;

#[gpui_kit::test]
fn classic_mode_reaches_snooze_and_move_from_the_reading_pane_toolbar(cx: &mut TestAppContext) {
    let fixture = click_first();
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    update_content(window, &kite, cx, |kite, _, _| {
        assert!(kite.open_id().is_some(), "Enter still opens a conversation");
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("action", SNOOZE), cx);
        window.render_frame(cx);
        assert!(
            window.try_find("snooze").is_some(),
            "the toolbar's snooze button opens the picker without a key"
        );
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "escape");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("action", MOVE), cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("folder-dialog").label(),
            Some("Move to folder"),
            "the toolbar's move button opens the folder picker"
        );
    })
    .unwrap();
    assert!(
        !fixture
            .actions()
            .iter()
            .any(|(_, action)| matches!(action, Action::Move { .. })),
        "opening the picker must not move anything"
    );
}

#[gpui_kit::test]
fn classic_mode_toolbar_labels_carry_no_dead_keys_and_back_returns_to_the_list(
    cx: &mut TestAppContext,
) {
    let fixture = click_first();
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        // Escape still reaches Back in either mode, so its hint stays; single-key triage does not.
        assert_eq!(window.find(("action", BACK)).label(), Some("Back (escape)"));
        for (cmd, label) in [(SNOOZE, "Snooze"), (MOVE, "Move to folder")] {
            assert_eq!(
                window.find(("action", cmd)).label(),
                Some(label),
                "a single-key command shows no key it no longer has"
            );
        }
        window.click(("action", BACK), cx);
        window.render_frame(cx);
        assert!(
            window.try_find("thread-toolbar").is_some(),
            "the conversation stays open beside the list"
        );
    })
    .unwrap();
    // Back hands the keyboard to the list: the arrows move its selection again.
    cx.simulate_keystrokes(window.into(), "down");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(("row", 1usize)).selected(), Some(true));
    })
    .unwrap();
}

#[gpui_kit::test]
fn classic_mode_composes_from_the_list_header_button(cx: &mut TestAppContext) {
    let fixture = click_first();
    let (window, _) = mount(cx, fixture);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-new", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("compose").is_some(),
            "the header's compose button is the click-first way to write"
        );
    })
    .unwrap();
}
