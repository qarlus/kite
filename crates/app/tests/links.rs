//! Links inside emails: hover cues, the status pill, phishing confirmation, mailto
//! compose, the tracker shield and URL cleaning.
mod common;
use common::{Fixture, mount};
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{AppContext, TestAppContext};
use std::time::Duration;

/// A body whose single link sits dead centre, so the harness's element-centre
/// hover/click lands on it.
const CENTERED: &str = r##"<div style="padding:40px 0;text-align:center"><a href="https://example.com/doc">the doc</a></div>"##;

/// Opens thread 2 (the fixture's HTML thread) and waits for its body to paint.
async fn open_html_thread(
    window: gpui_kit::WindowHandle<gpui_kit::base::Root>,
    cx: &mut TestAppContext,
) {
    cx.simulate_keystrokes(window.into(), "j enter");
    cx.wait_for(window.into(), Duration::from_secs(5), |window, _| {
        window.try_find("html-22").is_some()
    })
    .await;
}

#[gpui_kit::test]
async fn hovering_a_link_shows_the_pill_and_highlight(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    fixture.set_message_html(2, CENTERED);
    let (window, _) = mount(cx, fixture);
    open_html_thread(window, cx).await;
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("link-pill").is_none());
        window.hover("html-22", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("link-pill").label(),
            Some("https://example.com/doc"),
            "the pill shows where the link goes"
        );
        assert!(
            window.try_find("link-highlight").is_some(),
            "the accent wash over the link's box paints"
        );
        // Leaving the body hides both.
        window.hover("thread-subject", cx);
        window.render_frame(cx);
        assert!(window.try_find("link-pill").is_none());
        assert!(window.try_find("link-highlight").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
async fn clicking_a_link_opens_the_cleaned_url(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    fixture.set_message_html(
        2,
        r##"<div style="padding:40px 0;text-align:center"><a href="https://example.com/doc?utm_source=mail&utm_medium=x&fbclid=abc&keep=1">the doc</a></div>"##,
    );
    let (window, _) = mount(cx, fixture);
    open_html_thread(window, cx).await;
    cx.update_window(window.into(), |_, window, cx| {
        window.click("html-22", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://example.com/doc?keep=1"),
        "utm_* and fbclid are stripped from the opened URL"
    );
}

#[gpui_kit::test]
async fn a_text_and_href_mismatch_asks_before_opening(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    fixture.set_message_html(
        2,
        r##"<div style="padding:40px 0;text-align:center"><a href="https://paypa1-login.ru/x">paypal.com</a></div>"##,
    );
    let (window, _) = mount(cx, fixture);
    open_html_thread(window, cx).await;
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.hover("html-22", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("link-pill").label(),
            Some("Text says paypal.com — link goes to paypa1-login.ru"),
            "the pill warns before a click"
        );
        window.click("html-22", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("link-confirm").is_some(),
            "the click asks instead of opening"
        );
    })
    .unwrap();
    assert_eq!(cx.opened_url(), None, "nothing opens until confirmed");
    cx.update_window(window.into(), |_, window, cx| {
        window.click("link-confirm-cancel", cx);
        window.render_frame(cx);
        assert!(window.try_find("link-confirm").is_none());
    })
    .unwrap();
    assert_eq!(cx.opened_url(), None, "cancel never opens");
    // "Open anyway" opens the cleaned destination.
    cx.update_window(window.into(), |_, window, cx| {
        window.click("html-22", cx);
        window.render_frame(cx);
        window.click("link-confirm-open", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://paypa1-login.ru/x")
    );
}

#[gpui_kit::test]
async fn javascript_links_never_open(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    fixture.set_message_html(
        2,
        r##"<div style="padding:40px 0;text-align:center"><a href="javascript:alert(1)">click</a></div>"##,
    );
    let (window, _) = mount(cx, fixture);
    open_html_thread(window, cx).await;
    cx.update_window(window.into(), |_, window, cx| {
        window.click("html-22", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(cx.opened_url(), None);
}

#[gpui_kit::test]
async fn a_mailto_link_opens_compose_to_the_address(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    fixture.set_message_html(
        2,
        r##"<div style="padding:40px 0;text-align:center"><a href="mailto:sam@example.com">write to Sam</a></div>"##,
    );
    let (window, kite) = mount(cx, fixture);
    open_html_thread(window, cx).await;
    cx.update_window(window.into(), |_, window, cx| {
        window.click("html-22", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("compose").is_some(),
            "mailto opens Kite's own compose"
        );
    })
    .unwrap();
    common::update_content(window, &kite, cx, |kite, _, cx| {
        assert_eq!(kite.compose_to(cx), "sam@example.com");
    });
    assert_eq!(cx.opened_url(), None, "mailto never reaches the browser");
}

#[gpui_kit::test]
async fn tracking_pixels_are_reported_and_never_fetched(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    fixture.set_message_html(
        2,
        r##"<p>Hi</p><img src="https://t.list-manage.com/track/open.php?u=1" width="1" height="1"><img src="https://o.sendgrid.net/wf/open?upn=abc" width="12" height="12"><img src="https://cdn.example.com/real.png" width="8" height="8">"##,
    );
    // Even with remote images allowed, trackers are stripped, not fetched.
    fixture.settings.lock().unwrap().load_remote_images = true;
    let (window, _) = mount(cx, fixture.clone());
    open_html_thread(window, cx).await;
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("trackers-22").label(),
            Some("2 trackers blocked: Mailchimp, SendGrid"),
            "a 1x1 pixel and a known tracker host are counted"
        );
    })
    .unwrap();
    assert_eq!(
        *fixture.images_fetched.lock().unwrap(),
        vec!["https://cdn.example.com/real.png".to_string()],
        "the tracker URLs were never fetched, only the real image"
    );
}

#[gpui_kit::test]
async fn a_body_without_trackers_shows_no_line(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    fixture.set_message_html(2, CENTERED);
    let (window, _) = mount(cx, fixture);
    open_html_thread(window, cx).await;
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("trackers-22").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
async fn right_click_on_a_link_offers_open_and_copy(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    fixture.set_message_html(2, CENTERED);
    let (window, _) = mount(cx, fixture);
    open_html_thread(window, cx).await;
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.right_click("html-22", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.within("popup-menu").find(0usize).label(),
            Some("Open in browser")
        );
        window.within("popup-menu").click(0usize, cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(cx.opened_url().as_deref(), Some("https://example.com/doc"));
}

#[gpui_kit::test]
async fn urls_in_plain_text_bodies_are_clickable(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    // A body that is just the URL: the click lands on the link's own text.
    fixture.set_message_text(2, "https://example.com/plain");
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "j enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click_at(
            "text-22",
            gpui_kit::point(gpui_kit::px(20.), gpui_kit::px(10.)),
            cx,
        );
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://example.com/plain"),
        "a bare URL in a text body opens the browser"
    );
}
