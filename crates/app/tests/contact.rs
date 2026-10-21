//! The contact sidebar: the card beside an open conversation, its actions and its toggle.
mod common;
use common::{Fixture, mount, update_content};
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{AppContext, TestAppContext};
use kite_core::model::Addr;
use std::time::Duration;

/// Waits until the card the background `Mail::contact` built is on screen.
async fn card_up(window: gpui_kit::WindowHandle<gpui_kit::base::Root>, cx: &mut TestAppContext) {
    cx.wait_for(window.into(), Duration::from_secs(5), |window, _| {
        window.try_find(("contact-stat", 0usize)).is_some()
    })
    .await;
}

#[gpui_kit::test]
async fn opening_a_thread_shows_the_correspondent_card(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "enter");
    card_up(window, cx).await;
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("contact-name").label(),
            Some("Sender 1"),
            "the latest non-me sender"
        );
        assert_eq!(
            window.find("contact-email").label(),
            Some("sender1@example.com, click to copy")
        );
        assert_eq!(
            window.find(("contact-stat", 2usize)).label(),
            Some("1"),
            "only this sender's own conversation counts"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
async fn i_toggles_the_panel_and_the_choice_is_remembered(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    card_up(window, cx).await;
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("contact-panel").is_some());
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "i");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("contact-panel").is_none(),
            "i hides the card"
        );
    })
    .unwrap();
    assert!(
        !fixture.settings.lock().unwrap().contact_panel,
        "the toggle persisted to settings"
    );
    cx.simulate_keystrokes(window.into(), "i");
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("contact-panel").is_some(),
            "i brings it back"
        );
    })
    .unwrap();
    assert!(fixture.settings.lock().unwrap().contact_panel);
    // The strip's own button does the same thing.
    cx.update_window(window.into(), |_, window, cx| {
        window.click("toggle-contact-panel", cx);
        window.render_frame(cx);
        assert!(window.try_find("contact-panel").is_none());
    })
    .unwrap();
    let _ = kite;
}

#[gpui_kit::test]
async fn a_recent_conversation_opens_on_click(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    // Give Sender 1 a second conversation, so the card has a recent row to open.
    fixture.set_message_from(
        2,
        Addr {
            name: "Sender 1".into(),
            email: "sender1@example.com".into(),
        },
    );
    let (window, kite) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "enter");
    card_up(window, cx).await;
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let row = window.find(("contact-thread", 0usize));
        assert_eq!(row.label(), Some("Thread 2"), "the other thread is listed");
        window.click(("contact-thread", 0usize), cx);
        window.render_frame(cx);
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(
            kite.open_id(),
            Some(2),
            "the click opened that conversation"
        );
    });
}

#[gpui_kit::test]
async fn search_all_mail_runs_a_from_query(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, kite) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "enter");
    card_up(window, cx).await;
    cx.update_window(window.into(), |_, window, cx| {
        window.click(("contact-action", 1usize), cx);
        window.render_frame(cx);
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(
            kite.view().query.as_deref(),
            Some("from:sender1@example.com"),
            "the button searched their mail"
        );
    });
}

#[gpui_kit::test]
async fn a_narrow_window_hides_the_panel(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (window, _) = common::open_window(
        cx,
        Some(gpui_kit::size(gpui_kit::px(900.), gpui_kit::px(800.))),
        |window, cx| cx.new(|cx| kite::app::Kite::new(Fixture::new(2), window, cx)),
    );
    cx.simulate_keystrokes(window.into(), "enter");
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("contact-panel").is_none(),
            "900px keeps the reading pane for mail"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
async fn mail_from_my_other_account_shows_that_account(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    fixture
        .accounts
        .lock()
        .unwrap()
        .push(kite_core::model::Account {
            id: 2,
            provider: kite_core::model::Provider::Microsoft,
            email: "karl@work.example".into(),
            name: "Karl".into(),
        });
    fixture.set_message_from(
        1,
        Addr {
            name: "Karl at work".into(),
            email: "karl@work.example".into(),
        },
    );
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "enter");
    card_up(window, cx).await;
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("contact-name").label(),
            Some("Karl at work"),
            "only the account holding the conversation counts as me"
        );
    })
    .unwrap();
}
