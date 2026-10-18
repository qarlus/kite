//! Empty states.
mod common;
use common::{Fixture, mount};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};

#[gpui_kit::test]
fn empty_inbox_shows_inbox_zero(cx: &mut TestAppContext) {
    let fixture = Fixture::new(0);
    let (window, _) = mount(cx, fixture);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("inbox-zero").label(), Some("Inbox Zero"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn list_and_bar_use_the_design_metrics(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, _) = mount(cx, fixture);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("row", 0usize)).bounds().size.height,
            gpui_kit::px(44.),
            "list rows are 44px"
        );
        assert_eq!(
            window.find("title-bar").bounds().size.height,
            gpui_kit::px(44.),
            "the split bar doubles as the titlebar, 44px"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn an_unconnected_app_does_not_claim_the_inbox_is_cleared(cx: &mut TestAppContext) {
    let fixture = Fixture::new(0);
    fixture.accounts.lock().unwrap().clear();
    let (window, _) = mount(cx, fixture);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("inbox-zero").label(), Some("Welcome to Kite"));
        assert_eq!(
            window.find("empty-note").label(),
            Some("Sign-in isn't available in this build yet, so nothing has synced.")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn an_empty_archive_names_the_archive_rather_than_the_inbox(cx: &mut TestAppContext) {
    let (window, _) = mount(cx, Fixture::new(3));
    cx.simulate_keystrokes(window.into(), "g e");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("inbox-header").label(), Some("Archive"));
        assert_eq!(window.find("inbox-zero").label(), Some("Nothing archived"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn an_empty_starred_view_explains_what_belongs_there(cx: &mut TestAppContext) {
    let (window, _) = mount(cx, Fixture::new(3));
    cx.simulate_keystrokes(window.into(), "g s");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("inbox-header").label(), Some("Starred"));
        assert_eq!(window.find("inbox-zero").label(), Some("No starred mail"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_compact_window_keeps_search_and_shortcuts_separate(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (window, _) = common::open_window(
        cx,
        Some(gpui_kit::size(gpui_kit::px(720.), gpui_kit::px(480.))),
        |window, cx| {
            use gpui_kit::AppContext;
            cx.new(|cx| kite::app::Kite::new(Fixture::showcase(), window, cx))
        },
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let search = window.find("search").bounds();
        let shortcuts = window.find("shortcut-help").bounds();
        assert!(search.size.width > gpui_kit::px(60.));
        assert!(shortcuts.origin.x >= search.right());
        let row = window.find(("row", 0usize)).bounds();
        let timestamp = window.find(("row-time", 0usize)).bounds();
        assert!(
            timestamp.right() <= row.right(),
            "timestamps stay inside long rows"
        );
        assert!(
            row.right() <= gpui_kit::px(720.),
            "rows fit the compact window"
        );
        window.click("shortcut-help", cx);
        window.render_frame(cx);
        assert!(window.try_find("help").is_some());
    })
    .unwrap();
}
