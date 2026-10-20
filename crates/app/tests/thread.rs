//! The thread view: HTML bodies, text bodies, collapsing, and side effects.
mod common;
use common::{Fixture, HTML, mount, update_content};
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{AppContext, TestAppContext};
use kite_core::model::{Action, Event};
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
        // The card is the body's column; the mail is laid out inside its padding.
        let width = body.bounds().size.width.as_f32() - 2. * kite::thread::PAPER_PAD;
        let scale = window.scale_factor();
        let expected = kite::html::render(HTML, width, scale).height + 2. * kite::thread::PAPER_PAD;
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
        // The body's card is laid out for the pane, inside its padding, less the indent it
        // hangs under; the mail itself renders inside the card's own padding.
        let expected = gpui_kit::px(
            720. - list.right().as_f32() - 2. * kite::thread::PANE_PAD - kite::thread::BODY_INDENT,
        );
        assert_eq!(body.size.width, expected);
        assert!(body.origin.x >= list.right());
        assert!(body.right() <= gpui_kit::px(720.));
    })
    .unwrap();
}

/// Once a body has painted, no frame may fall back to the "Rendering" placeholder: not when
/// the reader allows its images, not while the fetch is in flight or lands, and not when the
/// mark-read change event reloads the open conversation.
#[gpui_kit::test]
async fn a_painted_body_never_falls_back_to_the_placeholder(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    fixture.set_message_html(
        2,
        "<p>Hello there.</p><img src=\"https://cdn.example.com/logo.png\" width=\"8\" height=\"8\">",
    );
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "j enter");
    cx.wait_for(window.into(), Duration::from_secs(5), |window, _| {
        window
            .try_find("html-22")
            .is_some_and(|body| body.label() != Some("Rendering"))
    })
    .await;
    let painted = cx
        .update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let body = window.find("html-22");
            (
                body.label().unwrap_or_default().to_string(),
                body.bounds().size.height,
            )
        })
        .unwrap();
    // Allowing this message's images re-keys the render; the painted frame stays up.
    cx.update_window(window.into(), |_, window, cx| {
        window.click("load-images-22", cx);
        window.render_frame(cx);
        let body = window.find("html-22");
        assert_ne!(
            body.label(),
            Some("Rendering"),
            "a shown body must not flash the placeholder"
        );
        assert_eq!(body.bounds().size.height, painted.1);
    })
    .unwrap();
    // The fetch lands and the body re-renders with the image inlined; a store change
    // arrives on top. Every observed frame keeps the body.
    fixture.emit(Event::Changed(vec![1]));
    for _ in 0..20 {
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_ne!(
                window.find("html-22").label(),
                Some("Rendering"),
                "the placeholder came back"
            );
        })
        .unwrap();
    }
    assert_eq!(
        *fixture.images_fetched.lock().unwrap(),
        vec!["https://cdn.example.com/logo.png".to_string()],
        "the image was fetched once, through the engine"
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let body = window.find("html-22");
        assert!(
            body.label().unwrap_or_default().starts_with("HTML body"),
            "the body settled rendered: {:?}",
            body.label()
        );
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
        assert!(
            window.try_find("html-12").is_none(),
            "plain text is theme text on the pane, never HTML paper"
        );
        // It keeps the theme's own body type: 15px at a 1.7 line height.
        let line = window.find("text-12").bounds().size.height;
        assert!(
            (line - kite::theme::pick(true).px(15.) * 1.7).abs() < gpui_kit::px(1.),
            "the text body is the theme's body size: {line:?}"
        );
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

#[gpui_kit::test]
async fn a_conversation_taller_than_the_pane_scrolls(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (window, _) = common::open_window(
        cx,
        Some(gpui_kit::size(gpui_kit::px(900.), gpui_kit::px(340.))),
        |window, cx| cx.new(|cx| kite::app::Kite::new(Fixture::new(2), window, cx)),
    );
    cx.simulate_keystrokes(window.into(), "j enter");
    cx.wait_for(window.into(), Duration::from_secs(5), |window, _| {
        window.try_find("html-22").is_some()
    })
    .await;
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let before = window.find("html-22").bounds();
        let window_bottom = gpui_kit::px(340.);
        assert!(
            before.bottom() > window_bottom,
            "the fixture conversation must overflow a 340px window for this test to mean anything: body {:?}",
            before
        );
        assert!(
            window.find("thread-scroll").bounds().bottom() <= window_bottom,
            "the scroll area ends at the window instead of growing to the conversation"
        );
        window.scroll(
            "thread-scroll",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(-200.))),
            cx,
        );
        window.render_frame(cx);
        let after = window.find("html-22").bounds();
        assert!(
            after.origin.y < before.origin.y,
            "scrolling moves the body up ({:?} -> {:?})",
            before.origin.y,
            after.origin.y
        );
    })
    .unwrap();
}
