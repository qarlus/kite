//! Engine -> UI events re-query the visible view.
mod common;
use common::{Fixture, mount};
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{AppContext, TestAppContext};
use kite_core::model::Event;
use std::time::Duration;

#[gpui_kit::test]
async fn a_changed_event_requeries_the_list(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("row", 2usize)).is_none());
    })
    .unwrap();
    // The engine stores a new thread and tells the UI.
    let id = fixture.push_thread();
    fixture.emit(Event::Changed(vec![1]));
    cx.wait_for(
        window.into(),
        Duration::from_millis(1_000),
        move |window, _| window.try_find(("row", 2usize)).is_some(),
    )
    .await;
    let label = format!("New Sender: Thread {id}");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("row", 0usize)).label(),
            Some(label.as_str()),
            "the new thread is at the top of the list"
        );
    })
    .unwrap();
    assert!(
        fixture.queries.lock().unwrap().len() >= 2,
        "the list was queried again after the event"
    );
}

#[gpui_kit::test]
async fn an_error_event_shows_a_toast(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    let (window, _) = mount(cx, fixture.clone());
    fixture.emit(Event::Error("Auth expired".into()));
    cx.wait_for(window.into(), Duration::from_millis(1_000), |window, _| {
        window.try_find("toast").is_some()
    })
    .await;
    cx.update_window(window.into(), |_, window, _| {
        assert_eq!(window.find("toast").label(), Some("Auth expired"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn refresh_button_and_classic_shortcut_request_mail_and_show_feedback(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    fixture.settings.lock().unwrap().input_mode = "classic".into();
    let (window, _) = mount(cx, fixture.clone());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("refresh-mail", cx);
        window.render_frame(cx);
        assert_eq!(window.find("toast").label(), Some("Refresh requested"));
    })
    .unwrap();
    assert_eq!(
        fixture.refreshes.load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    cx.simulate_keystrokes(window.into(), "secondary-r");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("toast").label(), Some("Refresh requested"));
        assert!(
            window
                .find("refresh-mail")
                .label()
                .unwrap()
                .contains("Refresh mail")
        );
    })
    .unwrap();
    assert_eq!(
        fixture.refreshes.load(std::sync::atomic::Ordering::Relaxed),
        2
    );
}
