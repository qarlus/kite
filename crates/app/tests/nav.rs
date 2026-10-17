//! Keyboard navigation, the go-to sequences and the command palette.
mod common;
use common::{Fixture, mount, update_content};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext, px};
use kite_core::model::Mailbox;

#[gpui_kit::test]
fn j_k_move_the_selection_and_enter_opens(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, kite) = mount(cx, fixture.clone());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(("row", 0usize)).selected(), Some(true));
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "j");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.sel_index(), 1, "j moves down one row")
    });
    cx.simulate_keystrokes(window.into(), "k");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.sel_index(), 0, "k moves back up")
    });
    cx.simulate_keystrokes(window.into(), "j enter");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.open_id(), Some(2), "enter opens the selected thread")
    });
}

#[gpui_kit::test]
fn escape_leaves_the_thread_view_and_j_stays_inside_it(cx: &mut TestAppContext) {
    let fixture = Fixture::new(4);
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.open_id(), Some(1))
    });
    cx.simulate_keystrokes(window.into(), "j");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(
            kite.open_id(),
            Some(2),
            "j moves to the next thread in thread view"
        )
    });
    cx.simulate_keystrokes(window.into(), "u");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.open_id(), None)
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find(("row", 0usize)).is_some(),
            "back in the list"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn go_sequences_switch_mailbox(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "g i");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.view().mailbox, Mailbox::Inbox)
    });
    cx.simulate_keystrokes(window.into(), "g s");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.view().mailbox, Mailbox::Starred)
    });
    cx.simulate_keystrokes(window.into(), "g t");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.view().mailbox, Mailbox::Sent)
    });
    cx.simulate_keystrokes(window.into(), "secondary-1");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(
            kite.view().account_id,
            Some(1),
            "cmd/ctrl-1 switches account"
        )
    });
}

#[gpui_kit::test]
fn tab_cycles_splits(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, kite) = mount(cx, fixture.clone());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(("tab", 0usize)).selected(), Some(true));
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "tab");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.view().split, Some(common::SPLITS[1]))
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(("tab", 1usize)).selected(), Some(true));
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "shift-tab");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.view().split, Some(common::SPLITS[0]))
    });
}

#[gpui_kit::test]
fn palette_opens_filters_and_runs_the_selected_command(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, kite) = mount(cx, fixture.clone());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let hint = window.find("palette-hint");
        assert_eq!(hint.label(), Some("Command palette"));
        assert!(
            hint.bounds().size.width > gpui_kit::px(0.),
            "the bar paints the palette shortcut pill"
        );
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "secondary-k");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("palette").is_some(),
            "cmd/ctrl-k opens the palette"
        );
    })
    .unwrap();
    // "done" matches "Mark done" and nothing above it.
    cx.simulate_keystrokes(window.into(), "d o n e");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("palette-row", 0usize)).label(),
            Some("Mark done"),
            "fuzzy match ranks the archive command first"
        );
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("palette").is_none(),
            "enter closes the palette"
        );
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_count(), 2, "the command removed the selected row")
    });
    assert!(
        matches!(
            fixture.last_action(),
            Some((_, kite_core::model::Action::Archive))
        ),
        "the palette command reached the fixture: {:?}",
        fixture.last_action()
    );
}

#[gpui_kit::test]
fn question_mark_opens_the_cheat_sheet(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "shift-/");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("help").is_some());
        assert_eq!(window.find("help").label(), Some("Keyboard shortcuts"));
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "escape");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("help").is_none());
    })
    .unwrap();
    let _ = px(1.);
}

#[gpui_kit::test]
fn clicking_a_palette_command_changes_the_visible_inbox(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, kite) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "secondary-k d o n e");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("palette-row", 0usize), cx);
        window.render_frame(cx);
        assert!(window.try_find("palette").is_none());
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_count(), 2);
        assert!(!kite.row_ids().contains(&1));
    });
}

#[gpui_kit::test]
fn palette_navigation_keeps_the_selected_command_visible(cx: &mut TestAppContext) {
    let (window, _) = mount(cx, Fixture::new(3));
    cx.simulate_keystrokes(
        window.into(),
        "secondary-k down down down down down down down down down down",
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let row = window.find(("palette-row", 10usize));
        assert_eq!(row.selected(), Some(true));
        assert!(row.bounds().bottom() <= window.find("palette").bounds().bottom());
    })
    .unwrap();
}
