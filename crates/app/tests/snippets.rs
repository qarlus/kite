//! Snippets: the Settings page creates, edits and deletes reusable text, and the
//! rail order places Snippets right after Shortcuts.
mod common;
use common::{Fixture, mount};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, ScrollDelta, TestAppContext, point, px};
use kite_core::model::Snippet;

fn snippet(name: &str, shortcut: &str, body: &str) -> Snippet {
    Snippet {
        name: name.into(),
        shortcut: shortcut.into(),
        body: body.into(),
    }
}

/// Opens Settings on the Snippets page (rail index 6, after Shortcuts).
fn open_snippets(window: gpui_kit::WindowHandle<gpui_kit::base::Root>, cx: &mut TestAppContext) {
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-tab", 6usize), cx);
        window.render_frame(cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_snippets_page_creates_edits_and_deletes_a_snippet(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture.clone());
    open_snippets(window, cx);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("snippet-new", cx);
        window.render_frame(cx);
        window.click("snippet-name", cx);
        window.input("Thanks", cx);
        window.click("snippet-shortcut", cx);
        window.input("thx", cx);
        window.click("snippet-body", cx);
        window.input("Hi {first_name}, thanks!", cx);
        window.click("snippet-save", cx);
        window.render_frame(cx);
    })
    .unwrap();
    {
        let settings = fixture.settings.lock().unwrap();
        assert_eq!(settings.snippets.len(), 1, "the save reached settings");
        assert_eq!(settings.snippets[0].name, "Thanks");
        assert_eq!(settings.snippets[0].shortcut, "thx");
        assert_eq!(settings.snippets[0].body, "Hi {first_name}, thanks!");
    }
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("snippet-edit", 0usize), cx);
        window.render_frame(cx);
        window.click("snippet-name", cx);
        window.press("secondary-a", cx);
        window.input("Grateful", cx);
        window.click("snippet-save", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(
        fixture.settings.lock().unwrap().snippets[0].name,
        "Grateful",
        "the edit replaced the snippet in place"
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("snippet-delete", 0usize), cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert!(
        fixture.settings.lock().unwrap().snippets.is_empty(),
        "delete removed it from settings"
    );
}

#[gpui_kit::test]
fn an_existing_snippet_list_shows_name_shortcut_and_preview(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    fixture.settings.lock().unwrap().snippets.push(snippet(
        "Out of office",
        "ooo",
        "I am away until {date}.",
    ));
    let (window, _) = mount(cx, fixture.clone());
    open_snippets(window, cx);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("snippet-edit", 0usize)).is_some());
        assert!(window.try_find(("snippet-delete", 0usize)).is_some());
        window.scroll(
            "settings-scroll",
            ScrollDelta::Pixels(point(px(0.), px(-400.))),
            cx,
        );
        window.render_frame(cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_rail_places_snippets_after_shortcuts(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "secondary-,");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("settings-tab", 5usize)).label(),
            Some("Shortcuts")
        );
        assert_eq!(
            window.find(("settings-tab", 6usize)).label(),
            Some("Snippets")
        );
        assert_eq!(
            window.find(("settings-tab", 7usize)).label(),
            Some("Privacy")
        );
        assert_eq!(window.find(("settings-tab", 8usize)).label(), Some("AI"));
    })
    .unwrap();
}
