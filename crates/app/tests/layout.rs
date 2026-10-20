//! The rail, list and reading-pane layout: accounts, the mailbox picker and the pane.
mod common;
use common::{Fixture, mount, update_content};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext, px};
use kite_core::model::Mailbox;

#[gpui_kit::test]
fn the_rail_shows_every_account_with_its_unread_count_and_scopes_the_list(cx: &mut TestAppContext) {
    let (window, kite) = mount(cx, Fixture::showcase());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("rail-account", 1usize)).label(),
            Some("karl@proton.me")
        );
        assert_eq!(
            window.find(("rail-account", 2usize)).label(),
            Some("karl@outlook.com")
        );
        // The showcase inbox holds three unread conversations: two of them in account 1.
        assert_eq!(window.find(("rail-unread", 1usize)).label(), Some("2"));
        assert_eq!(window.find(("rail-unread", 2usize)).label(), Some("1"));
        assert_eq!(
            window.find("all-inboxes").selected(),
            Some(true),
            "the combined view is active first"
        );
        window.click(("rail-account", 2usize), cx);
        window.render_frame(cx);
        assert_eq!(window.find(("rail-account", 2usize)).selected(), Some(true));
        assert_eq!(window.find("all-inboxes").selected(), Some(false));
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.view().account_id, Some(2));
        assert_eq!(kite.row_ids(), vec![2, 4, 6, 8, 10, 12]);
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.click("all-inboxes", cx);
        window.render_frame(cx);
        assert_eq!(window.find("all-inboxes").selected(), Some(true));
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.view().account_id, None);
        assert_eq!(kite.row_count(), 12, "the combined view is back");
    });
}

#[gpui_kit::test]
fn the_mailbox_picker_changes_the_list_title_and_the_go_keys_still_work(cx: &mut TestAppContext) {
    let (window, kite) = mount(cx, Fixture::showcase());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("mailbox-heading").label(), Some("Inbox"));
        window.click("mailbox-picker", cx);
        window.render_frame(cx);
        assert_eq!(window.find("mailbox-menu").label(), Some("Mailboxes"));
        window.click(("mailbox", Mailbox::Sent as usize), cx);
        window.render_frame(cx);
        assert!(
            window.try_find("mailbox-menu").is_none(),
            "picking a mailbox closes the picker"
        );
        assert_eq!(window.find("mailbox-heading").label(), Some("Sent"));
        assert_eq!(window.find("inbox-header").label(), Some("Sent"));
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.view().mailbox, Mailbox::Sent)
    });
    // The keyboard route is unchanged.
    cx.simulate_keystrokes(window.into(), "g s");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("mailbox-heading").label(), Some("Starred"));
        // Escape shuts the picker without leaving the mailbox behind.
        window.click("mailbox-picker", cx);
        window.render_frame(cx);
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(window.try_find("mailbox-menu").is_none());
        assert_eq!(window.find("mailbox-heading").label(), Some("Starred"));
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.view().mailbox, Mailbox::Starred)
    });
}

#[gpui_kit::test]
fn opening_a_conversation_fills_the_pane_beside_the_list_and_escape_returns_to_it(
    cx: &mut TestAppContext,
) {
    let (window, kite) = mount(cx, Fixture::showcase());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("reading-empty").label(),
            Some("No conversation selected")
        );
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("thread-subject").label(),
            Some("A few thoughts on the next chapter")
        );
        assert!(
            window.try_find(("row", 0usize)).is_some(),
            "the list stays beside the conversation"
        );
        assert!(window.try_find("reading-empty").is_none());
        let list = window.find("list-column").bounds();
        let subject = window.find("thread-subject").bounds();
        assert!(
            subject.origin.x >= list.right(),
            "the conversation sits in the pane, right of the list"
        );
    })
    .unwrap();
    // Escape leaves the conversation in view and gives the keyboard back to the list.
    cx.simulate_keystrokes(window.into(), "escape");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.open_id(), Some(1), "the conversation stays open")
    });
    cx.simulate_keystrokes(window.into(), "j");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.sel_index(), 1, "j moves the list selection");
        assert_eq!(kite.open_id(), Some(1), "without replacing the pane");
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(("row", 1usize)).selected(), Some(true));
        assert_eq!(
            window.find("thread-subject").label(),
            Some("A few thoughts on the next chapter")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn without_the_reading_pane_a_conversation_takes_the_whole_window(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    fixture.settings.lock().unwrap().reading_pane = false;
    let (window, kite) = mount(cx, fixture);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("reading-empty").is_none(),
            "no empty pane without a pane"
        );
        assert!(window.try_find("list-column").is_some());
        assert!(
            window.find("list-column").bounds().right() > px(1000.),
            "the list takes the width the pane would have used"
        );
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("thread-subject").label(),
            Some("A few thoughts on the next chapter")
        );
        assert!(
            window.try_find("list-column").is_none(),
            "the conversation replaces the list"
        );
        let subject = window.find("thread-subject").bounds();
        assert!(
            subject.right() > px(720.),
            "and uses the width the list gave up"
        );
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "escape");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.open_id(), None, "escape closes the conversation");
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("list-column").is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_settings_cog_opens_settings_over_the_whole_window_and_escape_restores_mail(
    cx: &mut TestAppContext,
) {
    let (window, _) = mount(cx, Fixture::showcase());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("open-settings", cx);
        window.render_frame(cx);
        assert!(window.try_find("settings-page").is_some());
        assert!(
            window.try_find("sidebar").is_none(),
            "settings takes the whole window, with no mail rail"
        );
        assert_eq!(
            window.find("settings-page").bounds().origin.x,
            px(0.),
            "settings starts at the window's left edge"
        );
        assert_eq!(window.find("open-settings").selected(), Some(true));
        assert!(
            window.try_find("list-column").is_none(),
            "the mail list gives way to settings"
        );
        assert!(window.try_find("mailbox-picker").is_none());
        assert!(window.try_find(("row", 0usize)).is_none());
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "escape");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("settings-page").is_none());
        assert!(window.try_find("sidebar").is_some());
        assert!(window.try_find("mailbox-picker").is_some());
        assert!(window.try_find(("row", 0usize)).is_some());
        assert_eq!(window.find("open-settings").selected(), Some(false));
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_settings_cog_is_centred_on_the_window_strip(cx: &mut TestAppContext) {
    let (window, _) = mount(cx, Fixture::showcase());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let strip = window.find("window-strip").bounds();
        let cog = window.find("open-settings").bounds();
        assert_eq!(
            cog.center().y,
            strip.center().y,
            "the cog shares the strip's midline, in line with the window controls"
        );
    })
    .unwrap();
}

/// The list/reading-pane toggle is a control of the window strip, at the list column's left
/// edge — not a button in the reading pane's own toolbar — and it keeps that spot when the list
/// steps aside, so the control never moves under the pointer.
#[gpui_kit::test]
fn the_pane_toggle_sits_in_the_window_strip_above_the_list(cx: &mut TestAppContext) {
    let (window, _) = mount(cx, Fixture::showcase());
    cx.simulate_keystrokes(window.into(), "j enter");
    let (strip, list, beside) = cx
        .update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let strip = window.find("window-strip").bounds();
            let list = window.find("list-column").bounds();
            let toggle = window.find("toggle-reading-pane").bounds();
            assert!(
                strip.contains(&toggle.origin)
                    && toggle.right() <= strip.right()
                    && toggle.bottom() <= strip.bottom(),
                "the toggle is in the window strip: {toggle:?} of {strip:?}"
            );
            assert!(
                toggle.origin.x >= list.origin.x && toggle.right() <= list.right(),
                "and above the list column: {toggle:?} of {list:?}"
            );
            assert!(
                toggle.center().x < list.center().x,
                "at its left edge, not floating at the right end: {toggle:?} of {list:?}"
            );
            assert_eq!(
                toggle.center().y,
                strip.center().y,
                "in line with the cog and the window buttons"
            );
            let toolbar = window.find("thread-toolbar").bounds();
            assert!(
                !toolbar.contains(&toggle.origin),
                "the reading pane's toolbar no longer carries it: {toggle:?} of {toolbar:?}"
            );
            window.click("toggle-reading-pane", cx);
            (strip, list, toggle)
        })
        .unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("list-column").is_none(),
            "the list stepped aside for one pane"
        );
        let alone = window.find("toggle-reading-pane").bounds();
        assert_eq!(
            alone.origin, beside.origin,
            "the toggle holds its place in the strip: {alone:?} vs {beside:?}"
        );
        assert!(
            alone.origin.x < list.center().x,
            "still at the list column's left edge: {alone:?} of {list:?}"
        );
        assert!(strip.contains(&alone.origin) && alone.bottom() <= strip.bottom());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_appearance_switch_turns_the_reading_pane_off_and_back_on(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture.clone());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("open-settings", cx);
        window.render_frame(cx);
        assert_eq!(window.find("reading-pane").selected(), Some(true));
        window.click("reading-pane", cx);
        window.render_frame(cx);
        assert_eq!(window.find("reading-pane").selected(), Some(false));
        window.click("close-settings", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("reading-empty").is_none(),
            "the pane is gone from the mail view"
        );
    })
    .unwrap();
    assert!(!fixture.settings.lock().unwrap().reading_pane);
}
