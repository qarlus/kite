//! Privacy settings: remote images in HTML mail, blocked by default and loadable per message.
mod common;
use common::{Fixture, mount};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};

/// A tracking pixel, as a newsletter writes it.
const BODY: &str =
    r#"<p>Your week</p><img src="https://track.example.com/pixel.png" width="8" height="8">"#;
/// The latest message of thread 1, which the list opens first.
const MESSAGE: usize = 12;

#[gpui_kit::test]
fn a_blocked_message_asks_before_loading_its_remote_images(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    fixture.set_message_html(1, BODY);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(format!("image-notice-{MESSAGE}")).label(),
            Some("Remote images are blocked"),
            "the message says why its images are missing"
        );
    })
    .unwrap();
    assert!(
        fixture.images_fetched.lock().unwrap().is_empty(),
        "a blocked message must never reach the network"
    );
}

#[gpui_kit::test]
fn loading_images_for_one_message_fetches_them_and_clears_the_notice(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    fixture.set_message_html(1, BODY);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(format!("load-images-{MESSAGE}"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        *fixture.images_fetched.lock().unwrap(),
        vec!["https://track.example.com/pixel.png".to_string()],
        "the button loads exactly this message's image"
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find(format!("image-notice-{MESSAGE}")).is_none(),
            "the notice goes once the images are allowed"
        );
        assert!(
            window.try_find(format!("html-{MESSAGE}")).is_some(),
            "the body is still rendered"
        );
    })
    .unwrap();
    // Nothing is remembered as a sender-wide rule: a fresh room still blocks.
    assert!(
        !fixture.settings.lock().unwrap().load_remote_images,
        "one message's choice is not a global setting"
    );
}

#[gpui_kit::test]
fn the_privacy_switch_loads_remote_images_without_asking(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    fixture.set_message_html(1, BODY);
    fixture.settings.lock().unwrap().load_remote_images = true;
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find(format!("image-notice-{MESSAGE}")).is_none(),
            "a reader who allowed images is not asked again"
        );
    })
    .unwrap();
    assert_eq!(
        *fixture.images_fetched.lock().unwrap(),
        vec!["https://track.example.com/pixel.png".to_string()],
        "with the switch on, the image is fetched without a click"
    );
}
