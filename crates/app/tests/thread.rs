//! The thread view: HTML bodies, text bodies, collapsing, and side effects.
mod common;
use common::{Fixture, HTML, mount, update_content};
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{AppContext, TestAppContext};
use kite_core::api::Mail as _;
use kite_core::model::{Action, AiRequest, Event, PRIVATE_TAG};
use std::time::Duration;

/// A fixture with AI on and three scripted reply suggestions.
fn ai_on() -> std::sync::Arc<Fixture> {
    let fixture = Fixture::new(3);
    let mut settings = fixture.settings.lock().unwrap();
    settings.ai_enabled = true;
    settings.ai_base_url = "https://ai.example.com/v1".into();
    settings.ai_model = "kite-test".into();
    drop(settings);
    *fixture.ai_reply.lock().unwrap() =
        "1. Sounds good to me.\n2. Can we move it to Friday?\n3. Thanks, but I'll pass.".into();
    fixture
}

#[gpui_kit::test]
fn suggest_replies_offers_three_chips_that_seed_a_reply(cx: &mut TestAppContext) {
    let fixture = ai_on();
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("suggest-replies", cx);
        window.render_frame(cx);
        // While the stream warms up, three quiet placeholders hold the spot.
        assert!(window.try_find(("suggestion-skeleton", 0usize)).is_some());
    })
    .unwrap();
    let requests = fixture.ai_requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1, "the button asks on click, never before");
    assert!(
        matches!(requests[0].1, AiRequest::Replies { thread: 1 }),
        "the open conversation is the source: {:?}",
        requests[0].1
    );
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("suggestion", 0usize)).label(),
            Some("Sounds good to me.")
        );
        assert_eq!(
            window.find(("suggestion", 1usize)).label(),
            Some("Can we move it to Friday?")
        );
        assert_eq!(
            window.find(("suggestion", 2usize)).label(),
            Some("Thanks, but I'll pass.")
        );
        window.click(("suggestion", 1usize), cx);
        window.render_frame(cx);
        assert!(window.try_find("compose").is_some(), "a chip opens a reply");
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, cx| {
        let body = kite.compose_body(cx);
        assert!(
            body.starts_with("Can we move it to Friday?\n\n"),
            "the suggestion sits above the quote, editable: {body:?}"
        );
        assert!(
            body.contains("Plain body for thread 1"),
            "the quote follows"
        );
        assert_eq!(
            kite.compose_subject(cx),
            "Re: Subject 1",
            "it is the reply the buttons would open"
        );
    });
}

#[gpui_kit::test]
fn a_private_thread_never_offers_suggestions(cx: &mut TestAppContext) {
    let fixture = ai_on();
    fixture
        .act(
            &[1],
            &Action::Label {
                tag: PRIVATE_TAG.into(),
            },
        )
        .unwrap();
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("suggest-replies").is_none(),
            "a Private conversation does not offer AI"
        );
        assert!(
            window
                .try_find(("inline-action", kite::keys::cmd::REPLY as usize))
                .is_some(),
            "the human verbs stay"
        );
    })
    .unwrap();
    assert!(fixture.ai_requests.lock().unwrap().is_empty());
}

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
        // No card, no padding: the body is the render, at the render's own size.
        let scale = window.scale_factor();
        let expected = kite::html::render(HTML, body.bounds().size.width.as_f32(), scale, true);
        assert!(
            (body.bounds().size.width.as_f32() - expected.width).abs() <= 1.,
            "the body is displayed at its rendered width ({} vs {})",
            body.bounds().size.width.as_f32(),
            expected.width
        );
        assert!(
            (body.bounds().size.height.as_f32() - expected.height).abs() <= 1.,
            "the body is displayed at its rendered height ({} vs {})",
            body.bounds().size.height.as_f32(),
            expected.height
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
        // The body is laid out for the pane's own column, edge to edge, so authored mail is
        // never clipped.
        let expected = gpui_kit::px(720. - list.right().as_f32() - 2. * kite::thread::PANE_PAD);
        assert_eq!(body.size.width, expected);
        assert!(body.origin.x >= list.right());
        assert!(body.right() <= gpui_kit::px(720.));
    })
    .unwrap();
}

/// An HTML body is the reading pane's own column: it starts where the sender's avatar does and
/// runs to the pane's right edge, so authored mail is never squeezed. A text body is unchanged,
/// still hanging under the sender's name.
#[gpui_kit::test]
async fn an_html_body_starts_at_the_avatar_column_and_spans_the_pane(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "j enter");
    cx.wait_for(window.into(), Duration::from_millis(5_000), |window, _| {
        window.try_find("html-22").is_some()
    })
    .await;
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let avatar = window.find(("avatar", 1usize)).bounds();
        let body = window.find("html-22").bounds();
        assert_eq!(
            body.origin.x, avatar.origin.x,
            "the body lines up with the message header, not indented past it"
        );
        assert!(
            body.origin.x > window.find("list-column").bounds().right(),
            "the body stays inside the reading pane"
        );
        // The older message is plain text: expanding it shows the indent is kept there.
        window.click("msg-0", cx);
        window.render_frame(cx);
        let older = window.find(("avatar", 0usize)).bounds();
        let text = window.find("text-21").bounds();
        assert_eq!(
            text.origin.x,
            older.origin.x + gpui_kit::px(kite::thread::BODY_INDENT),
            "a text body still hangs under the sender's name"
        );
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
        assert!(
            window
                .find(("action", kite::keys::cmd::UNSUBSCRIBE as usize))
                .label()
                .is_some_and(|label| label.starts_with("Unsubscribe")),
            "unsubscribe is a button too, not only a shortcut"
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

/// A collapsed message is one line: who, what, when. The snippet takes whatever the name leaves
/// and the date sits at the far right, in the same column as an expanded message's date.
#[gpui_kit::test]
fn a_collapsed_message_lines_up_with_the_expanded_one(cx: &mut TestAppContext) {
    let (window, _) = mount(cx, Fixture::new(3));
    cx.simulate_keystrokes(window.into(), "j enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        // The older message is collapsed, the latest one is open.
        let collapsed = window.find("message-date-0").bounds();
        let expanded = window.find("message-date-1").bounds();
        assert_eq!(
            collapsed.right(),
            expanded.right(),
            "both dates end at the same right edge, in one column"
        );
        assert!(collapsed.origin.y < expanded.origin.y);
        let sender = window.find("message-sender-0").bounds();
        let snippet = window.find("message-snippet-0").bounds();
        assert!(
            snippet.origin.x >= sender.right(),
            "the snippet starts after the sender's name: {snippet:?} after {sender:?}"
        );
        assert!(
            snippet.right() <= collapsed.origin.x,
            "and it stops before the date instead of running under it: {snippet:?} / {collapsed:?}"
        );
        assert!(
            snippet.size.width > gpui_kit::px(100.),
            "the snippet takes the room the name leaves: {:?}",
            snippet.size.width
        );
    })
    .unwrap();
}

/// A bare resize narrows or widens the reading pane's column, so a painted body has to be laid
/// out again at the new width instead of keeping the frame it was rendered for.
#[gpui_kit::test]
async fn resizing_the_window_re_renders_a_body_at_its_new_width(cx: &mut TestAppContext) {
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
    let narrow = cx
        .update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.find("html-22").bounds().size.width
        })
        .unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.resize(gpui_kit::size(gpui_kit::px(960.), gpui_kit::px(800.)));
        window.bounds_changed(cx);
        window.render_frame(cx);
    })
    .unwrap();
    // The new render lands in the background, so the body widens within a frame or two.
    cx.wait_for(window.into(), Duration::from_secs(5), move |window, _| {
        window.find("html-22").bounds().size.width > narrow
    })
    .await;
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let list = window.find("list-column").bounds();
        let body = window.find("html-22").bounds();
        let expected = gpui_kit::px(960. - list.right().as_f32() - 2. * kite::thread::PANE_PAD);
        assert_eq!(
            body.size.width, expected,
            "the body is laid out for the pane's new column (was {narrow:?})"
        );
    })
    .unwrap();
}
