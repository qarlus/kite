//! Optimistic actions: the row leaves the list, the selection moves and the
//! engine is told only afterwards. `z` undoes the last action.
mod common;
use common::{Fixture, mount, update_content};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use kite_core::model::Action;

#[gpui_kit::test]
fn archive_removes_the_row_selects_the_next_and_reaches_the_mail_trait(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, kite) = mount(cx, fixture.clone());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("row", 0usize)).label(),
            Some("Sender 1: Thread 1")
        );
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "e");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(
            kite.row_count(),
            2,
            "the row left the list on the same frame"
        );
        assert_eq!(kite.sel_index(), 0, "the next row is selected");
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("row", 0usize)).label(),
            Some("Sender 2: Thread 2"),
            "the next thread is now the first row"
        );
        assert_eq!(window.find("toast").label(), Some("Marked done"));
    })
    .unwrap();
    assert_eq!(fixture.last_action(), Some((vec![1], Action::Archive)));
}

#[gpui_kit::test]
fn undo_restores_the_archived_thread(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "e");
    assert_eq!(fixture.thread_count(), 2);
    cx.simulate_keystrokes(window.into(), "z");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_count(), 3, "undo brings the thread back")
    });
    assert_eq!(
        fixture.undos.lock().unwrap().len(),
        1,
        "undo reached the engine"
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("toast").label(), Some("Undone"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn trash_and_inbox_move_the_thread(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "shift-3");
    assert_eq!(fixture.last_action(), Some((vec![1], Action::Trash)));
    cx.simulate_keystrokes(window.into(), "shift-e");
    assert_eq!(fixture.last_action(), Some((vec![2], Action::Inbox)));
}

#[gpui_kit::test]
fn star_and_read_edit_the_row_in_place(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "s");
    assert_eq!(fixture.last_action(), Some((vec![1], Action::Star)));
    cx.simulate_keystrokes(window.into(), "shift-u");
    assert_eq!(
        fixture.last_action(),
        Some((vec![1], Action::Read)),
        "an unread thread becomes read"
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("toast").label(), Some("Marked read"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn x_selects_and_the_next_action_applies_to_the_selection(cx: &mut TestAppContext) {
    let fixture = Fixture::new(4);
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "x x e");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_count(), 2, "both selected threads left")
    });
    assert_eq!(
        fixture.last_action(),
        Some((vec![1, 2], Action::Archive)),
        "the action applies to the multi-selection"
    );
}

#[gpui_kit::test]
fn toolbar_icon_runs_the_same_action(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, _) = mount(cx, fixture.clone());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window
                .find(("action", kite::keys::cmd::DONE as usize))
                .label(),
            Some("Mark done (e)")
        );
        window.click(("action", kite::keys::cmd::DONE as usize), cx);
    })
    .unwrap();
    assert_eq!(fixture.last_action(), Some((vec![1], Action::Archive)));
}
