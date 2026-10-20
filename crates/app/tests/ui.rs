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
fn the_rail_the_list_and_the_window_strip_use_the_design_metrics(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, _) = mount(cx, fixture);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("row", 0usize)).bounds().size.height,
            gpui_kit::px(kite::theme::ROW_H * 2.),
            "list rows hold a sender, a subject and a two-line snippet"
        );
        let rail = window.find("sidebar").bounds();
        assert_eq!(rail.size.width, gpui_kit::px(64.), "the icon rail is 64px");
        let list = window.find("list-column").bounds();
        assert_eq!(
            list.origin.x,
            rail.right(),
            "the list starts beside the rail"
        );
        assert_eq!(
            list.size.width,
            gpui_kit::px(408.),
            "the list column is 408px"
        );
        let pane = window.find("reading-empty").bounds();
        assert_eq!(pane.origin.x, list.right(), "the reading pane follows it");
        assert_eq!(
            pane.right(),
            gpui_kit::px(1200.),
            "the pane takes the rest of the window"
        );
        let strip = window.find("window-strip").bounds();
        assert_eq!(
            strip.size.height,
            gpui_kit::px(34.),
            "the window strip is 34px"
        );
        assert_eq!(
            strip.origin.y,
            gpui_kit::px(0.),
            "it is the top of the window"
        );
        assert_eq!(
            strip.right(),
            gpui_kit::px(1200.),
            "it reaches the right edge"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_reading_pane_invites_a_conversation_when_nothing_is_open(cx: &mut TestAppContext) {
    let (window, _) = mount(cx, Fixture::new(3));
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("reading-empty").label(),
            Some("No conversation selected")
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
            Some("Sign in to Proton or Outlook to start syncing your mail.")
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
fn the_compact_window_keeps_the_list_search_and_shortcuts_in_place(cx: &mut TestAppContext) {
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
        let list = window.find("list-column").bounds();
        let search = window.find("search").bounds();
        assert!(search.size.width > gpui_kit::px(60.));
        assert!(
            search.right() <= list.right(),
            "search stays inside the list column"
        );
        // Keyboard help lives on the rail, so it never competes with search for width.
        let shortcuts = window.find("shortcut-help").bounds();
        assert!(shortcuts.right() <= list.origin.x);
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
        assert!(
            window.find("reading-empty").bounds().right() <= gpui_kit::px(720.),
            "the reading pane fits too"
        );
        window.click("shortcut-help", cx);
        window.render_frame(cx);
        assert!(window.try_find("help").is_some());
    })
    .unwrap();
}
