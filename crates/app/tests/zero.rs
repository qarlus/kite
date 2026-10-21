//! "Get me to zero": the bulk-archive dialog counts the current view's conversations
//! older than a preset, keeps starred and snoozed ones by default, and archives the
//! lot in one undoable action.
mod common;
use common::{Fixture, mount, update_content};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use kite_core::model::Action;

const DAY: i64 = 86_400_000;

/// Three old conversations (one starred, one snoozed) and three recent ones.
fn mixed() -> std::sync::Arc<Fixture> {
    let fixture = Fixture::new(6);
    let now = chrono::Local::now().timestamp_millis();
    for id in 1..=3 {
        fixture.set_thread_date(id, now - 2 * DAY);
    }
    fixture.set_starred(2);
    fixture.set_snoozed(3, now + DAY);
    fixture
}

fn open_dialog(window: gpui_kit::WindowHandle<gpui_kit::base::Root>, cx: &mut TestAppContext) {
    cx.simulate_keystrokes(window.into(), "secondary-k a r c h i v e space o l d e r");
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("zero-dialog").is_some(),
            "palette runs the command"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_dialog_counts_matches_and_excludes_starred_unless_toggled(cx: &mut TestAppContext) {
    let fixture = mixed();
    let (window, _) = mount(cx, fixture.clone());
    open_dialog(window, cx);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        // Thread 2 is starred, thread 3 snoozed: only thread 1 matches at 1 day.
        assert_eq!(
            window.find("zero-confirm").label(),
            Some("Archive 1 conversations")
        );
        window.click("zero-starred", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("zero-confirm").label(),
            Some("Archive 2 conversations")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn confirm_archives_in_one_step_and_z_restores_them_all(cx: &mut TestAppContext) {
    let fixture = mixed();
    let (window, kite) = mount(cx, fixture.clone());
    open_dialog(window, cx);
    cx.update_window(window.into(), |_, window, cx| {
        window.click("zero-starred", cx);
        window.render_frame(cx);
        window.click("zero-confirm", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("toast").label(),
            Some("Archived 2 conversations")
        );
    })
    .unwrap();
    assert_eq!(
        fixture.last_action(),
        Some((vec![1, 2], Action::Archive)),
        "one act call archives the whole matching set"
    );
    update_content(window, &kite, cx, |kite, _, _| {
        // The snoozed thread was never in the inbox: 5 rows minus the 2 archived.
        assert_eq!(kite.row_count(), 3);
    });
    cx.simulate_keystrokes(window.into(), "z");
    assert_eq!(
        fixture.undos.lock().unwrap().len(),
        1,
        "z reached the engine's undo"
    );
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_count(), 5, "undo brought every conversation back");
    });
}

#[gpui_kit::test]
fn the_header_button_opens_the_dialog_and_escape_closes_it(cx: &mut TestAppContext) {
    let fixture = mixed();
    let (window, _) = mount(cx, fixture.clone());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("list-actions", cx);
        window.render_frame(cx);
        assert!(window.try_find("zero-dialog").is_some());
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "escape");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("zero-dialog").is_none());
    })
    .unwrap();
    assert!(fixture.actions().is_empty(), "closing archives nothing");
}
