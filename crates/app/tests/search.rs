//! Instant search: `/` focuses the field, each keystroke re-queries locally, Esc restores.
mod common;
use common::{Fixture, mount, update_content};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};

#[gpui_kit::test]
fn search_narrows_the_list_and_escape_restores_it(cx: &mut TestAppContext) {
    let fixture = Fixture::new(5);
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "/");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("search").focused(),
            Some(true),
            "/ focuses the search field"
        );
        window.input("thread 4", cx);
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_ids(), vec![4], "only the matching thread remains");
        assert_eq!(kite.view().query.as_deref(), Some("thread 4"));
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("row", 0usize)).label(),
            Some("Sender 4: Thread 4")
        );
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "escape");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_count(), 5, "escape restores the whole list");
        assert_eq!(kite.view().query, None);
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("search").value(), Some(""));
    })
    .unwrap();
}

#[gpui_kit::test]
fn enter_opens_the_first_result(cx: &mut TestAppContext) {
    let fixture = Fixture::new(5);
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "/");
    cx.update_window(window.into(), |_, window, cx| window.input("thread 3", cx))
        .unwrap();
    cx.simulate_keystrokes(window.into(), "enter");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.open_id(), Some(3))
    });
}

#[gpui_kit::test]
fn a_search_with_no_results_says_so(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "/");
    cx.update_window(window.into(), |_, window, cx| {
        window.input("nothing matches this", cx)
    })
    .unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("inbox-zero").label(), Some("No results"));
    })
    .unwrap();
}
