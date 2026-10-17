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
