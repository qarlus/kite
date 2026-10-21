//! The Attachments browser: `g f` opens it, `/` and the chips filter, Enter opens the
//! file and `o` opens the conversation holding it.
mod common;
use common::{Fixture, mount, update_content};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use kite_core::model::Mailbox;

#[gpui_kit::test]
fn g_f_lists_files_and_enter_opens_them(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let report = fixture.add_attachment(1, "report.pdf", "application/pdf", 20_000);
    fixture.add_attachment(2, "photo.jpg", "image/jpeg", 50_000);
    // A tiny image is signature art: it must not reach the browser.
    fixture.add_attachment(3, "logo.png", "image/png", 400);
    let (window, kite) = mount(cx, fixture.clone());

    cx.simulate_keystrokes(window.into(), "g f");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.view().mailbox, Mailbox::Attachments);
        assert_eq!(kite.attachment_ids().len(), 2);
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("att-name", 0usize)).label(),
            Some("report.pdf")
        );
        assert_eq!(window.find(("att-name", 1usize)).label(), Some("photo.jpg"));
    })
    .unwrap();

    // j/k move the file selection, then Enter asks the engine to open it.
    cx.simulate_keystrokes(window.into(), "j");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.sel_index(), 1)
    });
    cx.simulate_keystrokes(window.into(), "k");
    cx.simulate_keystrokes(window.into(), "enter");
    assert_eq!(
        fixture.attachments_opened.lock().unwrap().as_slice(),
        &[report]
    );
}

#[gpui_kit::test]
fn chips_and_search_filter_the_list(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    fixture.add_attachment(1, "report.pdf", "application/pdf", 20_000);
    fixture.add_attachment(2, "data.xlsx", "application/vnd.ms-excel", 5_000);
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "g f");

    // A type chip narrows the list to one kind of file.
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("att-chip-spreadsheets", cx);
        window.render_frame(cx);
        assert_eq!(window.find(("att-name", 0usize)).label(), Some("data.xlsx"));
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.attachment_ids().len(), 1)
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.click("att-chip-all", cx);
        window.render_frame(cx);
        assert_eq!(window.find(("att-name", 1usize)).label(), Some("data.xlsx"));
    })
    .unwrap();

    // Nothing in the fixture is from me, so the toggle empties the list.
    cx.update_window(window.into(), |_, window, cx| {
        window.click("att-chip-from-me", cx);
        window.render_frame(cx);
        assert_eq!(window.find("inbox-zero").label(), Some("No attachments"));
        window.click("att-chip-from-me", cx);
    })
    .unwrap();

    // The search box matches the file name.
    cx.simulate_keystrokes(window.into(), "/");
    cx.update_window(window.into(), |_, window, cx| window.input("report", cx))
        .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.attachment_ids().len(), 1)
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("att-name", 0usize)).label(),
            Some("report.pdf")
        );
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "escape");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.attachment_ids().len(), 2)
    });
}

#[gpui_kit::test]
fn o_opens_the_conversation_holding_the_file(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    fixture.add_attachment(2, "report.pdf", "application/pdf", 20_000);
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "g f");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("att-name", 0usize)).label(),
            Some("report.pdf")
        );
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "o");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.open_id(), Some(2), "o opened the file's conversation")
    });
}
