//! Snooze: presets, keys 1-5, the natural-language field and the calendar panel.
mod common;
use chrono::{Duration, Local, NaiveDate, NaiveTime, TimeZone, Timelike};
use common::{Fixture, mount};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{App, AppContext, TestAppContext, Window, point, px};
use kite::snooze::parse_at;
use kite_core::model::Action;

/// Clicks the calendar day cell for `date`; the id carries its week row, so scan.
fn click_day(window: &mut Window, cx: &mut App, date: NaiveDate) {
    for week in 0..6 {
        let id = format!("calendar-{date}-0-{week}");
        if window.try_find(id.clone()).is_some() {
            window.click(id, cx);
            return;
        }
    }
    panic!("no calendar cell for {date}");
}

/// The ms the sub-panel confirms for `date` at the default next-whole-hour.
fn custom_ms(date: NaiveDate, now: chrono::DateTime<Local>) -> i64 {
    let hour = (now + Duration::hours(1)).hour();
    let naive = date.and_time(NaiveTime::from_hms_opt(hour, 0, 0).unwrap());
    Local
        .from_local_datetime(&naive)
        .earliest()
        .unwrap()
        .timestamp_millis()
}

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

#[gpui_kit::test]
fn pick_date_and_time_snoozes_at_that_time(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "h");
    let day = (Local::now() + Duration::days(2)).date_naive();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("snooze-custom", cx);
        window.render_frame(cx);
        assert!(window.try_find("snooze-calendar").is_some());
        click_day(window, cx, day);
        window.render_frame(cx);
        window.click("snooze-confirm", cx);
        window.render_frame(cx);
        assert!(window.try_find("snooze").is_none(), "the picker closes");
    })
    .unwrap();
    let expected = custom_ms(day, Local::now());
    match fixture.last_action() {
        Some((ids, Action::Snooze { until })) => {
            assert_eq!(ids, vec![1]);
            assert_eq!(until, expected, "snoozed at the picked date and time");
        }
        other => panic!("expected a snooze, got {other:?}"),
    }
}

#[gpui_kit::test]
fn send_later_pick_date_and_time_schedules_the_send(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "c");
    let day = (Local::now() + Duration::days(3)).date_naive();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-to", cx);
        window.input("jane@example.com", cx);
        window.click("compose-send-later", cx);
        window.render_frame(cx);
        assert_eq!(window.find("snooze").label(), Some("Send later"));
        window.click("snooze-custom", cx);
        window.render_frame(cx);
        click_day(window, cx, day);
        window.render_frame(cx);
        window.click("snooze-confirm", cx);
        window.render_frame(cx);
        assert!(window.try_find("snooze").is_none(), "the picker closes");
        assert!(window.try_find("compose").is_none(), "compose closes");
    })
    .unwrap();
    let sent = fixture.drafts();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].send_at, Some(custom_ms(day, Local::now())));
}

#[gpui_kit::test]
fn a_past_time_cannot_be_confirmed(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "h");
    let today = Local::now().date_naive();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("snooze-custom", cx);
        window.render_frame(cx);
        click_day(window, cx, today);
        window.render_frame(cx);
        // Click the hour segment to focus the field, then type 00:00 today —
        // a time that has already passed.
        window.click("hour", cx);
        window.press("0", cx);
        window.press("0", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("snooze-custom-error").is_some(),
            "the reason is shown"
        );
        window.click("snooze-confirm", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("snooze").is_some(),
            "a past time keeps the picker open"
        );
    })
    .unwrap();
    assert!(fixture.actions().is_empty(), "nothing reached the engine");
}

#[gpui_kit::test]
fn clicking_the_parsed_preview_confirms(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "h");
    for key in [
        "t", "o", "m", "o", "r", "r", "o", "w", "space", "9", "a", "m",
    ] {
        cx.simulate_keystrokes(window.into(), key);
    }
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("snooze-parsed", cx);
        window.render_frame(cx);
        assert!(window.try_find("snooze").is_none(), "the picker closes");
    })
    .unwrap();
    let expected = parse_at("tomorrow 9am", Local::now())
        .expect("parses")
        .timestamp_millis();
    match fixture.last_action() {
        Some((_, Action::Snooze { until })) => assert!((until - expected).abs() < 5_000),
        other => panic!("expected a snooze, got {other:?}"),
    }
}

#[gpui_kit::test]
fn clicking_the_backdrop_closes_without_snoozing(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "h");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("snooze").is_some());
        window.click_at("overlay", point(px(10.), px(10.)), cx);
        window.render_frame(cx);
        assert!(window.try_find("snooze").is_none(), "the picker closes");
    })
    .unwrap();
    assert!(fixture.actions().is_empty(), "nothing reached the engine");
}
