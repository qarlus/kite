//! Snooze: presets, keys 1-4, and the natural-language field.
mod common;
use chrono::{Duration, Local};
use common::{Fixture, mount};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use kite::snooze::parse_at;
use kite_core::model::Action;

#[gpui_kit::test]
fn h_opens_presets_and_1_snoozes_until_later_today(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "h");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("snooze").is_some(), "h opens the picker");
        assert_eq!(window.find(("preset", 0usize)).label(), Some("Later today"));
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "1");
    let now = Local::now();
    match fixture.last_action() {
        Some((ids, Action::Snooze { until })) => {
            assert_eq!(ids, vec![1]);
            let delta = until - now.timestamp_millis();
            assert!(
                (Duration::hours(3).num_milliseconds() - delta).abs() < 5_000,
                "later today is about three hours away, got {delta} ms"
            );
        }
        other => panic!("expected a snooze, got {other:?}"),
    }
}

#[gpui_kit::test]
fn typing_tomorrow_9am_snoozes_until_then(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "h");
    for key in [
        "t", "o", "m", "o", "r", "r", "o", "w", "space", "9", "a", "m",
    ] {
        cx.simulate_keystrokes(window.into(), key);
    }
    cx.simulate_keystrokes(window.into(), "enter");
    let expected = parse_at("tomorrow 9am", Local::now())
        .expect("parses")
        .timestamp_millis();
    match fixture.last_action() {
        Some((_, Action::Snooze { until })) => assert!(
            (until - expected).abs() < 5_000,
            "snoozed until {until}, expected about {expected}"
        ),
        other => panic!("expected a snooze, got {other:?}"),
    }
}

#[gpui_kit::test]
fn escape_closes_the_picker_without_snoozing(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "h");
    cx.simulate_keystrokes(window.into(), "escape");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("snooze").is_none());
    })
    .unwrap();
    assert!(fixture.actions().is_empty(), "nothing reached the engine");
}

#[gpui_kit::test]
fn arrow_keys_move_the_preset_selection(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "h");
    cx.simulate_keystrokes(window.into(), "down");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(("preset", 1usize)).selected(), Some(true));
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "enter");
    let tomorrow = kite::snooze::parse_at("tomorrow 8am", Local::now())
        .expect("parses")
        .timestamp_millis();
    match fixture.last_action() {
        Some((_, Action::Snooze { until })) => assert!((until - tomorrow).abs() < 5_000),
        other => panic!("expected a snooze, got {other:?}"),
    }
}

#[gpui_kit::test]
fn clicking_a_preset_snoozes(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "h");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("preset", 1usize), cx);
        window.render_frame(cx);
        assert!(window.try_find("snooze").is_none(), "the picker closes");
    })
    .unwrap();
    let tomorrow = parse_at("tomorrow 8am", Local::now())
        .expect("parses")
        .timestamp_millis();
    match fixture.last_action() {
        Some((ids, Action::Snooze { until })) => {
            assert_eq!(ids, vec![1]);
            assert!((until - tomorrow).abs() < 5_000);
        }
        other => panic!("expected a snooze, got {other:?}"),
    }
}

#[gpui_kit::test]
fn send_later_button_then_a_clicked_preset_schedules_the_send(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-to", cx);
        window.input("jane@example.com", cx);
        window.click("compose-send-later", cx);
        window.render_frame(cx);
        assert_eq!(window.find("snooze").label(), Some("Send later"));
        window.click(("preset", 0usize), cx);
        window.render_frame(cx);
        assert!(window.try_find("snooze").is_none(), "the picker closes");
        assert!(
            window.try_find("compose").is_none(),
            "compose closes once scheduled"
        );
    })
    .unwrap();
    let sent = fixture.drafts();
    assert_eq!(sent.len(), 1);
    let delta = sent[0].send_at.expect("scheduled") - Local::now().timestamp_millis();
    assert!(
        (Duration::hours(3).num_milliseconds() - delta).abs() < 5_000,
        "later today is about three hours away, got {delta} ms"
    );
}
