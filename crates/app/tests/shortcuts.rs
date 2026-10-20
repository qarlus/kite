//! Settings › Shortcuts: viewing, rebinding, conflicts and live effect.
mod common;
use common::{Fixture, update_content};
use gpui_kit::base::Root;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, Entity, TestAppContext, WindowHandle, px};
use kite_core::api::Mail;
use kite_core::model::Action;
use std::sync::Arc;

/// A tall window, so every shortcut row is on screen without scrolling.
fn mount_tall(
    cx: &mut TestAppContext,
    mail: Arc<dyn Mail>,
) -> (WindowHandle<Root>, Entity<kite::app::Kite>) {
    cx.update(gpui_kit::init);
    common::open_window(
        cx,
        Some(gpui_kit::size(px(1200.), px(1100.))),
        |window, cx| cx.new(|cx| kite::app::Kite::new(mail, window, cx)),
    )
}

/// Opens Settings on the Shortcuts page.
fn open_shortcuts(window: WindowHandle<Root>, cx: &mut TestAppContext) {
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 7usize), cx);
        window.render_frame(cx);
    })
    .unwrap();
}

fn close_settings(window: WindowHandle<Root>, cx: &mut TestAppContext) {
    cx.update_window(window.into(), |_, window, cx| {
        window.click("close-settings", cx);
        window.render_frame(cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn rebinding_mark_done_takes_the_key_live_and_reset_restores_it(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, kite) = mount_tall(cx, fixture.clone());
    open_shortcuts(window, cx);
    cx.update_window(window.into(), |_, window, cx| {
        assert_eq!(
            window.find("shortcut-row-mark_done").label(),
            Some("Mark done: e"),
            "the page lists the key that is in force"
        );
        window.click("shortcut-change-mark_done", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("shortcut-row-mark_done").label(),
            Some("Mark done: Press a key")
        );
        window.press("d", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("shortcut-row-mark_done").label(),
            Some("Mark done: d")
        );
    })
    .unwrap();
    assert_eq!(
        fixture
            .settings
            .lock()
            .unwrap()
            .shortcuts
            .get("mark_done")
            .cloned(),
        Some(vec!["d".to_string()]),
        "the edit is saved through the Mail settings"
    );
    close_settings(window, cx);
    cx.simulate_keystrokes(window.into(), "e");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_count(), 3, "e no longer archives");
    });
    assert!(fixture.actions().is_empty());
    cx.simulate_keystrokes(window.into(), "d");
    assert_eq!(
        fixture.last_action(),
        Some((vec![1], Action::Archive)),
        "d archives now"
    );

    open_shortcuts(window, cx);
    cx.update_window(window.into(), |_, window, cx| {
        window.click("shortcut-reset-mark_done", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("shortcut-row-mark_done").label(),
            Some("Mark done: e")
        );
    })
    .unwrap();
    assert!(
        !fixture
            .settings
            .lock()
            .unwrap()
            .shortcuts
            .contains_key("mark_done"),
        "Reset removes the override"
    );
    close_settings(window, cx);
    cx.simulate_keystrokes(window.into(), "e");
    assert_eq!(fixture.last_action(), Some((vec![2], Action::Archive)));
}

#[gpui_kit::test]
fn a_keystroke_that_clashes_names_the_command_it_would_replace(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, _) = mount_tall(cx, fixture.clone());
    open_shortcuts(window, cx);
    cx.update_window(window.into(), |_, window, cx| {
        window.click("shortcut-change-mark_done", cx);
        window.render_frame(cx);
        window.press("secondary-k", cx);
        window.render_frame(cx);
        let conflict = window
            .find("shortcut-conflict")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(conflict.contains("Command palette"), "{conflict}");
    })
    .unwrap();
    assert!(
        fixture.settings.lock().unwrap().shortcuts.is_empty(),
        "a refused edit is never saved"
    );
}

#[gpui_kit::test]
fn the_cheat_sheet_shows_the_keys_that_are_actually_bound(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    fixture
        .settings
        .lock()
        .unwrap()
        .shortcuts
        .insert("mark_done".into(), vec!["d".into()]);
    let (window, _) = mount_tall(cx, fixture);
    cx.simulate_keystrokes(window.into(), "shift-/");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("help-mark_done").label(),
            Some("Mark done: d"),
            "the sheet reads the user's edit, not the defaults"
        );
        assert_eq!(
            window.find("help-trash").label(),
            Some("Trash: # shift-3 delete")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn keyboard_mode_keeps_each_shortcut_row_to_a_single_line(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, _) = mount_tall(cx, fixture);
    open_shortcuts(window, cx);
    cx.update_window(window.into(), |_, window, _| {
        let row = window.find("shortcut-row-mark_done");
        assert_eq!(row.label(), Some("Mark done: e"));
        // The key is shown once, as a pill beside the actions. A repeated key line would stack
        // title (19.5px) + note (16.5px) and outgrow the shared 35.5px Change/Reset button.
        let height = row.bounds().size.height;
        assert!(
            height <= px(36.),
            "the row is button-high, not two lines: {height}"
        );
        // The dense card trims the shared 16px row padding to 8px, so consecutive rows sit
        // closer than the roomy 67.5px they would at full padding.
        let next = window.find("shortcut-row-move_to_inbox");
        let pitch = next.bounds().origin.y - row.bounds().origin.y;
        assert!(
            pitch < px(60.),
            "shortcut rows keep the dense card spacing: {pitch}"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn classic_mode_explains_rows_that_have_no_key_instead_of_a_dead_pill(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    fixture.settings.lock().unwrap().input_mode = "classic".into();
    let (window, _) = mount_tall(cx, fixture);
    open_shortcuts(window, cx);
    cx.update_window(window.into(), |_, window, _| {
        assert_eq!(
            window.find("shortcut-row-mark_done").label(),
            Some("Mark done: Not bound in this mode"),
            "a single-key command names why it has no key"
        );
        assert_eq!(
            window.find("shortcut-row-palette").label(),
            Some("Command palette: secondary-k"),
            "a modifier chord keeps its key"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn text_fields_still_edit_after_the_keymap_is_rebuilt(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, _) = mount_tall(cx, fixture);
    open_shortcuts(window, cx);
    cx.update_window(window.into(), |_, window, cx| {
        window.click("shortcut-change-mark_done", cx);
        window.press("d", cx);
        window.render_frame(cx);
    })
    .unwrap();
    close_settings(window, cx);
    cx.simulate_keystrokes(window.into(), "/");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("search").focused(), Some(true));
        window.input("jekyll", cx);
        assert_eq!(window.find("search").value(), Some("jekyll"));
        // Backspace and select-all belong to the component's Input bindings; they only come
        // back if the rebind restored the keys gpui-kit installed.
        window.press("backspace", cx);
        assert_eq!(window.find("search").value(), Some("jekyl"));
        window.press("secondary-a", cx);
        window.press("backspace", cx);
        assert_eq!(window.find("search").value(), Some(""));
    })
    .unwrap();
}
