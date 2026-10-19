//! The thread view: HTML bodies, text bodies, collapsing, and side effects.
mod common;
use common::{Fixture, HTML, mount, update_content};
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{AppContext, TestAppContext};
use kite_core::model::Action;
use std::time::Duration;

#[gpui_kit::test]
fn archiving_from_the_reading_view_returns_to_the_remaining_inbox(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, kite) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "enter e");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.open_id(), None);
        assert_eq!(kite.row_ids(), vec![2, 3]);
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("thread-subject").is_none());
        assert_eq!(
            window.find(("row", 0usize)).label(),
            Some("Sender 2: Thread 2")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
async fn opening_a_thread_renders_its_html_body_at_the_rendered_height(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, _) = mount(cx, fixture);
    // Thread 2 carries the HTML part in its latest message.
    cx.simulate_keystrokes(window.into(), "j enter");
    cx.wait_for(window.into(), Duration::from_millis(5_000), |window, _| {
        window.try_find("html-22").is_some()
    })
    .await;
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let body = window.find("html-22");
        let width = body.bounds().size.width.as_f32();
        let scale = window.scale_factor();
        let expected = kite::html::render(HTML, width, scale, true).height;
        assert!(
            (body.bounds().size.height.as_f32() - expected).abs() <= 2.,
            "the body is displayed at its rendered height ({} vs {expected})",
            body.bounds().size.height.as_f32()
        );
    })
    .unwrap();
}

#[gpui_kit::test]
async fn compact_html_uses_only_the_space_remaining_in_the_reading_pane(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (window, _) = common::open_window(
        cx,
        Some(gpui_kit::size(gpui_kit::px(720.), gpui_kit::px(800.))),
        |window, cx| cx.new(|cx| kite::app::Kite::new(Fixture::new(2), window, cx)),
    );
    cx.simulate_keystrokes(window.into(), "j enter");
    cx.wait_for(window.into(), Duration::from_secs(5), |window, _| {
        window.try_find("html-22").is_some()
    })
    .await;
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let list = window.find("list-column").bounds();
        let body = window.find("html-22").bounds();
        // The body is laid out for the pane, inside its padding, less the indent it hangs under.
        let expected = gpui_kit::px(
            720. - list.right().as_f32() - 2. * kite::thread::PANE_PAD - kite::thread::BODY_INDENT,
        );
        assert_eq!(body.size.width, expected);
        assert!(body.origin.x >= list.right());
        assert!(body.right() <= gpui_kit::px(720.));
    })
    .unwrap();
}

#[gpui_kit::test]
fn opening_an_unread_thread_renders_its_text_body_and_marks_it_read(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("text-12").label(),
            Some("Plain body for thread 1"),
            "a text-only body renders as GPUI text"
        );
        assert!(window.try_find("html-12").is_none());
    })
    .unwrap();
    assert_eq!(
        fixture.last_action(),
        Some((vec![1], Action::Read)),
        "opening an unread thread marks it read"
    );
    let _ = kite;
}

#[gpui_kit::test]
fn older_messages_collapse_until_clicked(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "j enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("text-21").is_none(),
            "the older message starts collapsed"
        );
        window.click("msg-0", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("text-21").label(),
            Some("Earlier plain text"),
            "clicking the collapsed header expands it"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_thread_toolbar_replies_by_click(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("thread-toolbar").label(),
            Some("Thread actions")
        );
        assert_eq!(
            window
                .find(("action", kite::keys::cmd::REPLY as usize))
                .label(),
            Some("Reply (r)"),
            "a toolbar button names the key that runs it"
        );
        window.click(("action", kite::keys::cmd::REPLY as usize), cx);
        window.render_frame(cx);
        assert!(
            window.try_find("compose").is_some(),
            "the reply panel opened"
        );
    })
    .unwrap();
}
