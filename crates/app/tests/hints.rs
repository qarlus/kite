//! Shortcut tips: clicking an action in keyboard mode names its key once a day,
//! retires after the key ran the command three times, and never appears in classic
//! mode or with "Show shortcut tips" off.
mod common;
use common::{Fixture, mount};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use kite_core::model::Action;

const DONE: usize = kite::keys::cmd::DONE as usize;

fn toast_label(
    window: gpui_kit::WindowHandle<gpui_kit::base::Root>,
    cx: &mut TestAppContext,
) -> Option<String> {
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window
            .try_find("toast")
            .map(|toast| toast.label().unwrap_or_default().to_string())
    })
    .unwrap()
}

#[gpui_kit::test]
fn clicking_the_archive_button_tips_the_key_once(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("action", DONE), cx);
    })
    .unwrap();
    assert_eq!(
        toast_label(window, cx),
        Some("Tip: press E to archive".to_string())
    );

    // A second click the same day shows the action's own toast, not another tip.
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.hover(("row", 0usize), cx);
        window.render_frame(cx);
        window.click(("row-archive", 0usize), cx);
    })
    .unwrap();
    assert_eq!(toast_label(window, cx), Some("Marked done".to_string()));
    assert_eq!(fixture.last_action(), Some((vec![2], Action::Archive)));
}

#[gpui_kit::test]
fn using_the_key_three_times_retires_the_tip(cx: &mut TestAppContext) {
    let fixture = Fixture::new(4);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "e e e");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.hover(("row", 0usize), cx);
        window.render_frame(cx);
        window.click(("row-archive", 0usize), cx);
    })
    .unwrap();
    assert_eq!(toast_label(window, cx), Some("Marked done".to_string()));
    assert_eq!(fixture.last_action(), Some((vec![4], Action::Archive)));
}

#[gpui_kit::test]
fn classic_mode_never_tips(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    fixture.settings.lock().unwrap().input_mode = "classic".into();
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("action", DONE), cx);
    })
    .unwrap();
    assert_eq!(toast_label(window, cx), Some("Marked done".to_string()));
}

#[gpui_kit::test]
fn the_shortcut_tips_setting_silences_them(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    fixture.settings.lock().unwrap().shortcut_tips = false;
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("action", DONE), cx);
    })
    .unwrap();
    assert_eq!(toast_label(window, cx), Some("Marked done".to_string()));
}

#[gpui_kit::test]
fn the_switch_lives_in_settings_shortcuts(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", kite::preferences::OPEN_SHORTCUTS), cx);
        window.render_frame(cx);
        window.click("shortcut-tips", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert!(!fixture.settings.lock().unwrap().shortcut_tips);
}
