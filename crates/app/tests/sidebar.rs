//! Real view navigation across accounts, mailboxes and custom folders.
mod common;
use common::{Fixture, mount, update_content};
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{AppContext, TestAppContext};
use kite_core::model::{Action, Mailbox};
use std::time::Duration;

#[gpui_kit::test]
fn account_scope_survives_splits_and_mailboxes_and_combines_again(cx: &mut TestAppContext) {
    let (window, kite) = mount(cx, Fixture::showcase());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("row-account", 0usize)).label(),
            Some("karl@proton.me")
        );
        window.click("account-switcher", cx);
        window.render_frame(cx);
        window.click("account-2", cx);
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_ids(), vec![2, 4, 6, 8, 10, 12]);
    });
    cx.simulate_keystrokes(window.into(), "tab tab g e");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.view().account_id, Some(2));
        assert_eq!(kite.view().mailbox, Mailbox::Archive);
        assert_eq!(kite.row_count(), 0);
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("account-switcher", cx);
        window.render_frame(cx);
        window.click("account-0", cx);
        window.render_frame(cx);
        assert_eq!(window.find("inbox-header").label(), Some("Inbox"));
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_count(), 12)
    });
}

#[gpui_kit::test]
fn a_single_click_opens_with_full_headers_and_the_sidebar_remains(cx: &mut TestAppContext) {
    let (window, kite) = mount(cx, Fixture::showcase());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("select-row", 0usize), cx);
        window.render_frame(cx);
        assert!(window.try_find("thread-subject").is_none());
        window.click(("select-row", 0usize), cx);
        window.click(("row", 0usize), cx);
        window.render_frame(cx);
        assert_eq!(window.find("sidebar").label(), Some("Mail navigation"));
        assert_eq!(
            window.find("thread-subject").label(),
            Some("A few thoughts on the next chapter")
        );
        assert_eq!(
            window.find("thread-account").label(),
            Some("karl@proton.me")
        );
        assert_eq!(
            window.find("message-from-1").label(),
            Some("From: Alice Chen <sender1@example.com>")
        );
        assert_eq!(
            window.find("message-to-1").label(),
            Some("To: Karl <karl@proton.me>")
        );
        assert_eq!(
            window.find("message-cc-1").label(),
            Some("Cc: Sofia Reyes <sofia@example.com>")
        );
        let date = window.find("message-date-1");
        let label = date.label().unwrap();
        assert!(label.contains(&chrono::Local::now().format("%Y").to_string()));
        assert!(label.contains(':'));
        assert_eq!(
            window.find("attachment-501").label(),
            Some("Next chapter.pdf (34567 bytes)")
        );
        window.click("account-switcher", cx);
        window.render_frame(cx);
        window.click("account-2", cx);
        window.render_frame(cx);
        assert!(window.try_find("thread-subject").is_none());
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_ids(), vec![2, 4, 6, 8, 10, 12])
    });
}

#[gpui_kit::test]
fn moving_to_a_custom_folder_and_undoing_changes_the_visible_mail(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "l down down down enter");
    update_content(window, &kite, cx, |kite, _, _| {
        assert!(!kite.row_ids().contains(&1))
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("folder-1-work", cx);
        window.render_frame(cx);
        assert_eq!(window.find("inbox-header").label(), Some("Work"));
        assert_eq!(
            window.find(("row", 0usize)).label(),
            Some("Alice Chen: A few thoughts on the next chapter")
        );
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.view().account_id, Some(1));
        assert_eq!(kite.row_ids(), vec![1]);
    });
    cx.simulate_keystrokes(window.into(), "z");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_count(), 0)
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("inbox-zero").label(), Some("No conversations"));
        window.click(("mailbox", Mailbox::Inbox as usize), cx);
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert!(kite.row_ids().contains(&1))
    });
}

#[gpui_kit::test]
fn identical_provider_folder_ids_in_two_accounts_do_not_mix_conversations(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    fixture
        .folders
        .lock()
        .unwrap()
        .push(kite_core::model::Folder {
            id: "work".into(),
            account_id: 2,
            name: "Work".into(),
        });
    fixture.locations.lock().unwrap().extend([
        (1, (Mailbox::Archive, Some("work".into()))),
        (2, (Mailbox::Archive, Some("work".into()))),
    ]);
    let (window, kite) = mount(cx, fixture);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("account-switcher", cx);
        window.render_frame(cx);
        window.click("account-2", cx);
        window.render_frame(cx);
        window.click("folder-2-work", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find(("row", 0usize)).label(),
            Some("Daniel, Sofia: Re: A place to get away")
        );
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_ids(), vec![2])
    });
}

#[gpui_kit::test]
fn mixed_account_selection_cannot_move_to_another_accounts_custom_folder(cx: &mut TestAppContext) {
    let (window, _) = mount(cx, Fixture::showcase());
    cx.simulate_keystrokes(window.into(), "x x l");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("move-target", 3usize)).is_none());
        assert_eq!(window.find(("move-target", 0usize)).label(), Some("Inbox"));
    })
    .unwrap();
}

#[gpui_kit::test]
async fn creating_a_folder_in_the_chosen_account_then_moving_mail_into_it(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    let (window, kite) = mount(cx, fixture.clone());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("new-folder", cx);
        window.render_frame(cx);
        window.input("Travel", cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "down enter");
    cx.wait_for(window.into(), Duration::from_secs(5), |window, _| {
        window.try_find("folder-2-folder-3").is_some()
    })
    .await;
    cx.update_window(window.into(), |_, window, cx| {
        window.click("account-switcher", cx);
        window.render_frame(cx);
        window.click("account-2", cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "l down down down down enter");
    assert_eq!(
        fixture.last_action(),
        Some((
            vec![2],
            Action::Move {
                folder: "folder-3".into()
            }
        ))
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("folder-2-folder-3", cx);
        window.render_frame(cx);
        assert_eq!(window.find("inbox-header").label(), Some("Travel"));
        assert_eq!(
            window.find(("row", 0usize)).label(),
            Some("Daniel, Sofia: Re: A place to get away")
        );
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_ids(), vec![2])
    });
}

#[gpui_kit::test]
async fn creation_errors_are_visible_and_do_not_add_a_fake_folder(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    *fixture.folder_error.lock().unwrap() = Some("Account is offline".into());
    let (window, _) = mount(cx, fixture.clone());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("new-folder", cx);
        window.render_frame(cx);
        window.input("Receipts", cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "enter");
    cx.wait_for(window.into(), Duration::from_secs(5), |window, _| {
        window.try_find("folder-error").is_some()
    })
    .await;
    cx.update_window(window.into(), |_, window, _| {
        assert_eq!(
            window.find("folder-error").label(),
            Some("Account is offline")
        );
    })
    .unwrap();
    assert_eq!(fixture.folders.lock().unwrap().len(), 2);
    cx.simulate_keystrokes(window.into(), "escape j enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("thread-subject").label(),
            Some("Re: A place to get away")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn standard_mailboxes_show_moved_mail_and_preserve_account_scope(cx: &mut TestAppContext) {
    let (window, kite) = mount(cx, Fixture::showcase());
    cx.simulate_keystrokes(window.into(), "e g e");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_ids(), vec![1])
    });
    cx.simulate_keystrokes(window.into(), "shift-3 g b");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_ids(), vec![1])
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("account-switcher", cx);
        window.render_frame(cx);
        window.click("account-2", cx);
        window.render_frame(cx);
        assert_eq!(window.find("inbox-header").label(), Some("Trash"));
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_count(), 0)
    });
}

#[gpui_kit::test]
fn incoming_mail_does_not_retarget_a_move_already_being_chosen(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "l");
    fixture.push_thread();
    fixture.emit(kite_core::model::Event::Changed(vec![13]));
    cx.run_until_parked();
    cx.simulate_keystrokes(window.into(), "down down down enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("folder-1-work", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find(("row", 0usize)).label(),
            Some("Alice Chen: A few thoughts on the next chapter")
        );
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_ids(), vec![1])
    });
}

#[gpui_kit::test]
fn opening_a_different_conversation_clears_the_old_multi_selection(cx: &mut TestAppContext) {
    let (window, kite) = mount(cx, Fixture::showcase());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("select-row", 0usize), cx);
        window.click(("row", 1usize), cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("thread-account").label(),
            Some("karl@outlook.com")
        );
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "l down down down enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("folder-2-receipts", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find(("row", 0usize)).label(),
            Some("Daniel, Sofia: Re: A place to get away")
        );
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_ids(), vec![2])
    });
}

#[gpui_kit::test]
fn duplicate_folder_names_show_validation_without_creating_another(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture.clone());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("new-folder", cx);
        window.render_frame(cx);
        window.input("work", cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("folder-error").label(),
            Some("A folder with this name already exists in that account.")
        );
        assert!(window.try_find("folder-name").is_some());
    })
    .unwrap();
    assert_eq!(fixture.folders.lock().unwrap().len(), 2);
}

#[gpui_kit::test]
fn a_long_folder_list_is_paged_and_clickable_in_a_compact_window(cx: &mut TestAppContext) {
    let fixture = Fixture::showcase();
    fixture
        .folders
        .lock()
        .unwrap()
        .extend((0..30).map(|i| kite_core::model::Folder {
            id: format!("custom-{i}"),
            account_id: 1,
            name: format!("Project {i}"),
        }));
    cx.update(gpui_kit::init);
    let (window, kite) = common::open_window(
        cx,
        Some(gpui_kit::size(gpui_kit::px(720.), gpui_kit::px(480.))),
        |window, cx| cx.new(|cx| kite::app::Kite::new(fixture, window, cx)),
    );
    cx.simulate_keystrokes(window.into(), "l");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let panel = window.find("folder-dialog").bounds();
        assert!(panel.bottom() < gpui_kit::px(480.));
        window.click("move-next", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find(("move-target", 4usize)).label(),
            Some("Project 0")
        );
        window.click(("move-target", 4usize), cx);
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert!(!kite.row_ids().contains(&1))
    });
}

#[gpui_kit::test]
async fn attachment_click_opens_the_selected_attachment_and_keeps_the_thread(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::showcase();
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("attachment-501", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(*fixture.attachments_opened.lock().unwrap(), vec![501]);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("thread-subject").label(),
            Some("A few thoughts on the next chapter")
        );
        assert_eq!(
            window.find("attachment-501").label(),
            Some("Next chapter.pdf (34567 bytes)")
        );
    })
    .unwrap();
}
