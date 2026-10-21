mod common;
use common::{Fixture, mount};
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{AppContext, TestAppContext};
use kite_core::{
    api::Mail,
    model::{Addr, Draft, Mailbox},
};
use std::time::Duration;

#[gpui_kit::test]
fn waiting_navigation_labels_done_and_undo(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    fixture
        .locations
        .lock()
        .unwrap()
        .insert(1, (Mailbox::Waiting, None));
    fixture.set_thread_date(1, chrono::Local::now().timestamp_millis() - 3 * 86_400_000);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "g w");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("row-time", 0usize)).label(),
            Some("Waiting 3 days")
        );
        window.click(("done-waiting", 0usize), cx);
        window.render_frame(cx);
        assert_eq!(window.find("inbox-zero").label(), Some("Nothing waiting"));
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "z");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("row-time", 0usize)).label(),
            Some("Waiting 3 days")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
async fn scheduled_chord_picker_reschedule_and_cancel_restore_draft(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    let at = chrono::Local::now() + chrono::Duration::hours(1);
    fixture
        .send(Draft {
            account_id: 1,
            to: vec![Addr {
                name: String::new(),
                email: "bob@example.com".into(),
            }],
            subject: "Later".into(),
            body: "Keep this text".into(),
            send_at: Some(at.timestamp_millis()),
            ..Default::default()
        })
        .unwrap();
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "g l");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(("row-subject", 0usize)).label(), Some("Later"));
        assert!(
            window
                .find(("row-time", 0usize))
                .label()
                .unwrap()
                .contains(&at.format("%H:%M").to_string())
        );
        window.click(("reschedule", 0usize), cx);
        window.render_frame(cx);
        assert_eq!(window.find("snooze").label(), Some("Send later"));
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "2");
    cx.wait_for(window.into(), Duration::from_secs(4), |_, _| {
        fixture.drafts().len() == 2
    })
    .await;
    cx.simulate_keystrokes(window.into(), "z");
    cx.wait_for(window.into(), Duration::from_secs(4), |_, _| {
        fixture.drafts().len() == 3
    })
    .await;
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find(("row-time", 0usize))
                .label()
                .unwrap()
                .contains(&at.format("%H:%M").to_string())
        );
    })
    .unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("cancel-schedule", 0usize), cx);
    })
    .unwrap();
    cx.wait_for(window.into(), Duration::from_secs(4), |_, _| {
        fixture
            .threads(
                &kite_core::model::View {
                    account_id: None,
                    mailbox: Mailbox::Scheduled,
                    split: None,
                    query: None,
                    folder: None,
                },
                0,
                10,
            )
            .unwrap()
            .is_empty()
    })
    .await;
    cx.simulate_keystrokes(window.into(), "g d");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(("row-subject", 0usize)).label(), Some("Later"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn mailbox_picker_has_waiting_and_scheduled(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    let (window, _) = mount(cx, fixture);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("mailbox-picker", cx);
        window.render_frame(cx);
        window.click(("mailbox", Mailbox::Scheduled as usize), cx);
        window.render_frame(cx);
        assert_eq!(window.find("inbox-zero").label(), Some("Nothing scheduled"));
        window.click("mailbox-picker", cx);
        window.render_frame(cx);
        window.click(("mailbox", Mailbox::Waiting as usize), cx);
        window.render_frame(cx);
        assert_eq!(window.find("inbox-zero").label(), Some("Nothing waiting"));
    })
    .unwrap();
}

#[gpui_kit::test]
async fn opening_a_schedule_cancels_then_opens_the_editable_draft(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    fixture
        .send(Draft {
            account_id: 1,
            to: vec![Addr {
                name: String::new(),
                email: "bob@example.com".into(),
            }],
            subject: "Editable later".into(),
            body: "Original scheduled body".into(),
            send_at: Some(chrono::Local::now().timestamp_millis() + 600_000),
            ..Default::default()
        })
        .unwrap();
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "g l");
    cx.simulate_keystrokes(window.into(), "e");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("row-subject", 0usize)).label(),
            Some("Editable later")
        );
    })
    .unwrap();
    assert!(
        fixture.actions().is_empty(),
        "an outbox id cannot act on an unrelated conversation with the same id"
    );
    cx.simulate_keystrokes(window.into(), "enter");
    cx.wait_for(window.into(), Duration::from_secs(4), |window, _| {
        window.try_find("compose").is_some()
    })
    .await;
    common::update_content(window, &kite, cx, |kite, _, cx| {
        assert_eq!(kite.compose_body(cx), "Original scheduled body");
        assert_eq!(kite.compose_to(cx), "bob@example.com");
    });
    assert!(
        fixture
            .threads(
                &kite_core::model::View {
                    account_id: None,
                    mailbox: Mailbox::Scheduled,
                    split: None,
                    query: None,
                    folder: None
                },
                0,
                10
            )
            .unwrap()
            .is_empty()
    );
    assert_eq!(fixture.saved_drafts().last().unwrap().send_at, None);
}
