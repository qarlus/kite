//! The reading pane's layout: each row's own timestamp in the list, and the header, avatar,
//! attachment pills and inline reply of an open conversation.
mod common;
use chrono::{Datelike as _, TimeZone as _, Timelike as _};
use common::{Fixture, mount, update_content};
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{AppContext, TestAppContext};
use std::time::Duration;

#[gpui_kit::test]
fn every_row_carries_its_own_unambiguous_date(cx: &mut TestAppContext) {
    let fixture = Fixture::new(4);
    let now = chrono::Local::now().timestamp_millis();
    fixture.set_thread_date(1, now - 60_000);
    fixture.set_thread_date(2, now - 86_400_000);
    fixture.set_thread_date(3, now - 3 * 86_400_000);
    fixture.set_thread_date(4, now - 400 * 86_400_000);
    let (window, _) = mount(cx, fixture);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let label = |ix: usize| {
            window
                .find(("row-time", ix))
                .label()
                .unwrap_or_default()
                .to_string()
        };
        let now = chrono::Local::now();
        // The contract `list::when` paints: time today, `Yesterday`, a weekday inside the
        // week, then the date with the year once it matters.
        let expected = |ms: i64| {
            let at = chrono::Local.timestamp_millis_opt(ms).single().unwrap();
            match (now.date_naive() - at.date_naive()).num_days() {
                0 => format!("{}:{:02}", at.time().hour(), at.time().minute()),
                1 => "Yesterday".to_string(),
                2..=6 => at.format("%a").to_string(),
                _ if at.year() == now.year() => format!("{} {}", at.format("%b"), at.day()),
                _ => format!("{} {}, {}", at.format("%b"), at.day(), at.year()),
            }
        };
        assert_eq!(label(0), expected(now.timestamp_millis() - 60_000));
        assert_eq!(label(1), "Yesterday");
        assert_eq!(label(2), expected(now.timestamp_millis() - 3 * 86_400_000));
        let old = label(3);
        assert_eq!(old, expected(now.timestamp_millis() - 400 * 86_400_000));
        assert!(
            old.chars().any(|c| c.is_ascii_digit()) && old.contains(','),
            "mail from a previous year shows day, month and year: {old:?}"
        );
        assert!(
            window.try_find(("day", 0usize)).is_none(),
            "the list no longer paints a day gutter"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn an_unread_sender_is_announced_and_opening_clears_it(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("row-sender", 0usize)).label(),
            Some("Unread: Sender 1")
        );
        assert_eq!(
            window.find(("row-sender", 1usize)).label(),
            Some("Sender 2"),
            "a read conversation is announced plainly"
        );
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.simulate_keystrokes(window.into(), "escape");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("row-sender", 0usize)).label(),
            Some("Sender 1"),
            "reading a conversation clears its unread announcement"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_reading_pane_shows_the_sender_and_replies_from_the_body(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, kite) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("message-from-1").label(),
            Some("From: Sender 1 <sender1@example.com>"),
            "the open message names its sender"
        );
        assert_eq!(
            window.find(("avatar", 1usize)).label(),
            Some("S1"),
            "the avatar carries the sender's initials"
        );
        assert_eq!(
            window
                .find(("inline-action", kite::keys::cmd::REPLY as usize))
                .label(),
            Some("Reply"),
            "the conversation ends where you answer it"
        );
        window.click(("inline-action", kite::keys::cmd::REPLY as usize), cx);
        window.render_frame(cx);
        assert!(window.try_find("compose").is_some(), "the reply opened");
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, cx| {
        assert_eq!(kite.compose_to(cx), "Sender 1 <sender1@example.com>");
    });
}

#[gpui_kit::test]
fn the_pane_toggle_flips_the_preference_and_persists(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("toggle-reading-pane").selected(), Some(true));
        window.click("toggle-reading-pane", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("toggle-reading-pane").selected(),
            Some(false),
            "the list steps aside"
        );
        assert_eq!(
            window.find("thread-subject").label(),
            Some("Thread 1"),
            "the conversation keeps the whole window"
        );
        assert!(
            window.try_find("list-column").is_none(),
            "and no list is beside it"
        );
    })
    .unwrap();
    assert!(
        !fixture.settings.lock().unwrap().reading_pane,
        "the choice reached the engine"
    );
}

#[gpui_kit::test]
async fn an_attachment_is_a_pill_with_its_name_and_size(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "enter");
    cx.wait_for(window.into(), Duration::from_secs(5), |window, _| {
        window.try_find("attachment-501").is_some()
    })
    .await;
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let pill = window.find("attachment-501");
        assert_eq!(pill.label(), Some("Next chapter.pdf (34567 bytes)"));
        assert!(
            pill.bounds().size.height < gpui_kit::px(32.),
            "a chip, not a row: {:?}",
            pill.bounds().size
        );
    })
    .unwrap();
}
