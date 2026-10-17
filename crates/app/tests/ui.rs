use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext, WindowOptions};

#[gpui_kit::test]
fn empty_inbox_shows_inbox_zero(cx: &mut TestAppContext) {
    let (window, _) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
            cx.new(|_| kite::app::Kite)
        })
        .expect("window")
    });
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("inbox-zero").label(), Some("Inbox Zero"));
    })
    .unwrap();
}
