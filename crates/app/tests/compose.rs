//! Compose: new mail, reply/reply-all/forward, send + undo, persistent drafts
//! and key isolation while typing.
mod common;
use common::{Fixture, mount, update_content};
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{AppContext, TestAppContext};
use kite_core::api::Mail;
use kite_core::model::{Addr, Draft};
use std::time::Duration;

fn addr(name: &str, email: &str) -> Addr {
    Addr {
        name: name.into(),
        email: email.into(),
    }
}

#[gpui_kit::test]
fn c_opens_compose_and_ctrl_enter_sends_the_draft(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("compose").is_some(), "c opens compose");
        window.click("compose-to", cx);
        window.input("Alice <alice@example.com>", cx);
        window.click("compose-subject", cx);
        window.input("Lunch", cx);
        window.click("compose-body", cx);
        window.input("Are you free?", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "secondary-enter");
    let drafts = fixture.drafts();
    assert_eq!(drafts.len(), 1, "Ctrl+Enter sent exactly one draft");
    let draft = &drafts[0];
    assert_eq!(draft.account_id, 1);
    assert_eq!(draft.to, vec![addr("Alice", "alice@example.com")]);
    assert_eq!(draft.subject, "Lunch");
    assert_eq!(draft.body, "Are you free?");
    assert_eq!(draft.reply_to, None);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("compose").is_none(),
            "the panel closes in the same frame"
        );
        assert_eq!(window.find("toast").label(), Some("Sending…"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn r_replies_to_the_sender_and_quotes_the_message(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    cx.simulate_keystrokes(window.into(), "r");
    update_content(window, &kite, cx, |kite, _, cx| {
        assert_eq!(
            kite.compose_to(cx),
            "Sender 1 <sender1@example.com>",
            "Reply goes to the sender"
        );
        assert_eq!(kite.compose_subject(cx), "Re: Subject 1");
        let body = kite.compose_body(cx);
        assert!(body.contains("wrote:"), "one-line attribution: {body:?}");
        assert!(
            body.contains("> Plain body for thread 1"),
            "the original plain text is quoted: {body:?}"
        );
    });
    cx.simulate_keystrokes(window.into(), "secondary-enter");
    let drafts = fixture.drafts();
    assert_eq!(drafts.len(), 1);
    assert_eq!(
        drafts[0].reply_to,
        Some(12),
        "threading points at the latest message"
    );
    assert_eq!(drafts[0].to, vec![addr("Sender 1", "sender1@example.com")]);
}

#[gpui_kit::test]
fn a_replies_to_all_without_self_or_duplicates(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    fixture.set_message_recipients(
        1,
        vec![addr("Me", "me@example.com"), addr("Bob", "bob@example.com")],
        vec![
            addr("Sender 1", "sender1@example.com"),
            addr("Me", "me@example.com"),
            addr("Carol", "carol@example.com"),
        ],
    );
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    cx.simulate_keystrokes(window.into(), "a");
    update_content(window, &kite, cx, |kite, _, _| {
        assert!(kite.compose_open());
    });
    cx.simulate_keystrokes(window.into(), "secondary-enter");
    let drafts = fixture.drafts();
    assert_eq!(drafts.len(), 1);
    assert_eq!(
        drafts[0].to,
        vec![
            addr("Sender 1", "sender1@example.com"),
            addr("Bob", "bob@example.com"),
        ],
        "sender first, then original To, with self and duplicates removed"
    );
    assert_eq!(
        drafts[0].cc,
        vec![addr("Carol", "carol@example.com")],
        "original Cc recipients stay in Cc"
    );
}

#[gpui_kit::test]
fn f_forwards_with_an_empty_to_and_the_original_quote(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    cx.simulate_keystrokes(window.into(), "f");
    update_content(window, &kite, cx, |kite, _, cx| {
        assert_eq!(kite.compose_to(cx), "", "forward starts with an empty To");
        assert_eq!(kite.compose_subject(cx), "Fwd: Subject 1");
        assert!(kite.compose_body(cx).contains("> Plain body for thread 1"));
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-to", cx);
        window.input("dana@example.com", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "secondary-enter");
    let drafts = fixture.drafts();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].to, vec![addr("", "dana@example.com")]);
    assert_eq!(drafts[0].subject, "Fwd: Subject 1");
    assert_eq!(
        drafts[0].reply_to,
        Some(12),
        "a forward retains its original-message link"
    );
}

#[gpui_kit::test]
fn undo_cancels_the_send_and_reopens_the_draft(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-to", cx);
        window.input("alice@example.com", cx);
        window.click("compose-subject", cx);
        window.input("Keep me", cx);
        window.click("compose-body", cx);
        window.input("Draft body", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "secondary-enter");
    assert_eq!(fixture.drafts().len(), 1);
    cx.simulate_keystrokes(window.into(), "z");
    assert_eq!(fixture.cancelled(), vec![9001], "z cancels the outbox item");
    update_content(window, &kite, cx, |kite, _, cx| {
        assert!(kite.compose_open(), "undo reopens compose");
        assert_eq!(kite.compose_to(cx), "alice@example.com");
        assert_eq!(kite.compose_subject(cx), "Keep me");
        assert_eq!(kite.compose_body(cx), "Draft body");
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("compose").is_some());
        assert_eq!(window.find("toast").label(), Some("Send cancelled"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn escape_hides_compose_but_c_restores_the_draft(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-to", cx);
        window.input("bob@example.com", cx);
        window.click("compose-body", cx);
        window.input("Half written", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "escape");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("compose").is_none(), "Esc closes the panel");
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, cx| {
        assert!(!kite.compose_open());
        assert_eq!(kite.compose_to(cx), "bob@example.com");
        assert_eq!(kite.compose_body(cx), "Half written");
    });
    cx.simulate_keystrokes(window.into(), "c");
    update_content(window, &kite, cx, |kite, _, cx| {
        assert!(kite.compose_open());
        assert_eq!(kite.compose_to(cx), "bob@example.com");
        assert_eq!(kite.compose_body(cx), "Half written");
    });
}

#[gpui_kit::test]
fn typing_in_the_body_does_not_run_list_shortcuts(cx: &mut TestAppContext) {
    let fixture = Fixture::new(3);
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-body", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "e");
    cx.simulate_keystrokes(window.into(), "j");
    update_content(window, &kite, cx, |kite, _, cx| {
        assert_eq!(kite.compose_body(cx), "ej", "the keys reached the body");
        assert!(kite.compose_open());
        assert_eq!(kite.row_count(), 3, "e did not archive anything");
        assert_eq!(kite.open_id(), None, "j did not open a thread");
        assert!(fixture.actions().is_empty(), "nothing reached the engine");
    });
}

#[gpui_kit::test]
fn reply_keeps_a_hidden_draft_instead_of_replacing_it(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-body", cx);
        window.input("Half written", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "escape");
    cx.simulate_keystrokes(window.into(), "enter");
    cx.simulate_keystrokes(window.into(), "r");
    update_content(window, &kite, cx, |kite, _, cx| {
        assert!(kite.compose_open(), "the draft reopens");
        assert_eq!(
            kite.compose_body(cx),
            "Half written",
            "the draft survives r"
        );
        assert_eq!(
            kite.compose_subject(cx),
            "",
            "the reply did not replace the draft"
        );
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("toast").label(), Some("Draft kept"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn z_undoes_the_latest_action_not_the_send(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-to", cx);
        window.input("alice@example.com", cx);
        window.click("compose-body", cx);
        window.input("Hi", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "secondary-enter");
    assert_eq!(fixture.drafts().len(), 1, "the mail was sent");
    cx.simulate_keystrokes(window.into(), "e");
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_count(), 1, "e archived the selected thread");
    });
    cx.simulate_keystrokes(window.into(), "z");
    assert!(
        fixture.cancelled().is_empty(),
        "z must not cancel the send when the archive is more recent"
    );
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_count(), 2, "z undid the archive");
    });
}

#[gpui_kit::test]
fn reply_quotes_a_display_name_with_a_comma(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    fixture.set_message_from(1, addr("Doe, Jane", "jane@example.com"));
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    cx.simulate_keystrokes(window.into(), "r");
    update_content(window, &kite, cx, |kite, _, cx| {
        assert_eq!(
            kite.compose_to(cx),
            "\"Doe, Jane\" <jane@example.com>",
            "a comma in the display name is quoted so it stays one recipient"
        );
    });
    cx.simulate_keystrokes(window.into(), "secondary-enter");
    let drafts = fixture.drafts();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].to, vec![addr("Doe, Jane", "jane@example.com")]);
}

#[gpui_kit::test]
fn invalid_address_is_rejected_with_field_feedback(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    let (window, _kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-to", cx);
        window.input("not-an-email", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "secondary-enter");
    assert_eq!(fixture.drafts().len(), 0, "no draft is sent");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("compose-to-error").label(),
            Some("To: correct the address; use name@example.com or Name <name@example.com>.")
        );
    })
    .unwrap();
}

// ---- persistent drafts -------------------------------------------------------

#[gpui_kit::test]
async fn autosave_writes_the_draft_after_a_pause(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-to", cx);
        window.input("alice@example.com", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert!(
        fixture.saved_drafts().is_empty(),
        "nothing is written before the debounce"
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("compose-status").label(), Some("Saving…"));
    })
    .unwrap();
    cx.wait_for(window.into(), Duration::from_secs(4), |_, _| {
        !fixture.saved_drafts().is_empty()
    })
    .await;
    let saved = fixture.saved_drafts();
    assert_eq!(saved.len(), 1);
    assert!(saved[0].id.is_some(), "the saved draft carries an id");
    assert_eq!(saved[0].to, vec![addr("", "alice@example.com")]);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("compose-status").label(), Some("Saved"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn an_empty_draft_is_never_written(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "c");
    cx.simulate_keystrokes(window.into(), "escape");
    assert!(
        fixture.saved_drafts().is_empty(),
        "a draft with no content is never saved"
    );
}

#[gpui_kit::test]
fn escape_saves_the_draft(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-body", cx);
        window.input("Half written", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "escape");
    let saved = fixture.saved_drafts();
    assert_eq!(saved.len(), 1, "Esc persists the draft");
    assert_eq!(saved[0].body, "Half written");
    assert!(saved[0].id.is_some());
}

#[gpui_kit::test]
fn a_saved_draft_emptied_is_discarded(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    let id = fixture
        .save_draft(Draft {
            id: None,
            account_id: 1,
            to: vec![addr("", "alice@example.com")],
            ..Draft::default()
        })
        .unwrap();
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "g d");
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-to", cx);
        for _ in 0.."alice@example.com".len() {
            window.press("backspace", cx);
        }
        window.render_frame(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "escape");
    assert_eq!(
        fixture.discarded(),
        vec![id],
        "the emptied draft is discarded"
    );
    assert!(fixture.saved_drafts().is_empty());
}

#[gpui_kit::test]
async fn emptying_a_draft_discards_it_without_closing_the_panel(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    let id = fixture
        .save_draft(Draft {
            id: None,
            account_id: 1,
            to: vec![addr("", "alice@example.com")],
            ..Draft::default()
        })
        .unwrap();
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "g d");
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-to", cx);
        for _ in 0.."alice@example.com".len() {
            window.press("backspace", cx);
        }
        window.render_frame(cx);
    })
    .unwrap();
    cx.wait_for(window.into(), Duration::from_secs(4), |_, _| {
        fixture.discarded().contains(&id)
    })
    .await;
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("compose").is_some(),
            "the emptied editor stays open for more typing"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_discard_button_throws_the_saved_draft_away(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-to", cx);
        window.input("alice@example.com", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "escape");
    let id = fixture.saved_drafts()[0].id.unwrap();
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-discard", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("compose").is_none(),
            "discarding closes the panel"
        );
    })
    .unwrap();
    assert_eq!(fixture.discarded(), vec![id]);
    assert!(fixture.saved_drafts().is_empty());
}

#[gpui_kit::test]
fn secondary_shift_d_discards_the_draft(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-body", cx);
        window.input("Discard me", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "escape");
    let id = fixture.saved_drafts()[0].id.unwrap();
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "secondary-shift-d");
    assert_eq!(fixture.discarded(), vec![id]);
}

#[gpui_kit::test]
fn opening_a_draft_in_the_drafts_mailbox_reopens_it_in_compose(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    let id = fixture
        .save_draft(Draft {
            id: None,
            account_id: 1,
            to: vec![addr("Alice", "alice@example.com")],
            subject: "Lunch".into(),
            body: "noon?".into(),
            ..Draft::default()
        })
        .unwrap();
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "g d");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("mailbox-heading").label(), Some("Drafts"));
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("compose").is_some(),
            "a saved draft opens in compose"
        );
        assert!(
            window.try_find("thread-subject").is_none(),
            "not the reading view"
        );
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, cx| {
        assert!(kite.compose_open());
        assert_eq!(kite.compose_to(cx), "Alice <alice@example.com>");
        assert_eq!(kite.compose_subject(cx), "Lunch");
        assert_eq!(kite.compose_body(cx), "noon?");
    });
    cx.simulate_keystrokes(window.into(), "secondary-enter");
    let sent = fixture.drafts();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].id, Some(id), "the reopened draft keeps its id");
}

// ---- reply / reply-all / forward --------------------------------------------

#[gpui_kit::test]
fn replying_to_my_own_message_addresses_the_original_recipients(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    fixture.set_message_from(1, addr("Me", "me@example.com"));
    fixture.set_message_recipients(
        1,
        vec![
            addr("Bob", "bob@example.com"),
            addr("Carol", "carol@example.com"),
        ],
        Vec::new(),
    );
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter r");
    update_content(window, &kite, cx, |kite, _, cx| {
        assert_eq!(
            kite.compose_to(cx),
            "Bob <bob@example.com>, Carol <carol@example.com>",
            "a reply to my own mail goes to who I sent it to, not myself"
        );
    });
    cx.simulate_keystrokes(window.into(), "secondary-enter");
    let sent = fixture.drafts();
    assert_eq!(
        sent[0].to,
        vec![
            addr("Bob", "bob@example.com"),
            addr("Carol", "carol@example.com")
        ]
    );
}

#[gpui_kit::test]
fn reply_all_drops_my_address_case_insensitively(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    fixture.set_message_recipients(
        1,
        vec![addr("Me", "ME@example.com"), addr("Bob", "bob@example.com")],
        vec![
            addr("Me", "me@EXAMPLE.com"),
            addr("Carol", "carol@example.com"),
        ],
    );
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter a");
    cx.simulate_keystrokes(window.into(), "secondary-enter");
    let sent = fixture.drafts();
    assert_eq!(
        sent[0].to,
        vec![
            addr("Sender 1", "sender1@example.com"),
            addr("Bob", "bob@example.com")
        ],
        "my own address is skipped however it is cased"
    );
    assert_eq!(sent[0].cc, vec![addr("Carol", "carol@example.com")]);
}

#[gpui_kit::test]
fn reply_subject_never_stacks_its_marker(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    fixture.set_message_subject(1, "RE: Re: Fwd: hello");
    let (window, kite) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "enter r");
    update_content(window, &kite, cx, |kite, _, cx| {
        assert_eq!(kite.compose_subject(cx), "Re: hello");
    });
}

#[gpui_kit::test]
fn forward_subject_never_stacks_its_marker(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    fixture.set_message_subject(1, "Fw: Re: hi");
    let (window, kite) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "enter f");
    update_content(window, &kite, cx, |kite, _, cx| {
        assert_eq!(kite.compose_subject(cx), "Fwd: hi");
    });
}

#[gpui_kit::test]
fn forward_carries_the_original_headers_above_the_quote(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    let (window, kite) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "enter f");
    update_content(window, &kite, cx, |kite, _, cx| {
        let body = kite.compose_body(cx);
        for expected in [
            "From: Sender 1 <sender1@example.com>",
            "Subject: Subject 1",
            "To: Me <me@example.com>",
            "> Plain body for thread 1",
        ] {
            assert!(body.contains(expected), "missing {expected:?} in {body:?}");
        }
        assert!(
            body.contains("Date: "),
            "the forward includes the date: {body:?}"
        );
    });
}

#[gpui_kit::test]
fn reply_quotes_with_an_attribution_line(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    fixture.set_message_from(1, addr("Jane Doe", "jane@example.com"));
    let (window, kite) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "enter r");
    update_content(window, &kite, cx, |kite, _, cx| {
        let body = kite.compose_body(cx);
        assert!(
            body.contains("On ") && body.contains("Jane Doe <jane@example.com> wrote:"),
            "reply attributes the original: {body:?}"
        );
        assert!(body.contains("> Plain body for thread 1"));
    });
}

#[gpui_kit::test]
fn reply_targets_the_expanded_older_message(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    let (window, kite) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("msg-0", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "r");
    update_content(window, &kite, cx, |kite, _, cx| {
        let body = kite.compose_body(cx);
        assert!(
            body.contains("> Earlier plain text"),
            "quotes the expanded message: {body:?}"
        );
        assert!(!body.contains("> Plain body for thread 1"));
        assert_eq!(kite.compose_subject(cx), "Re: Older note");
    });
}

#[gpui_kit::test]
fn sending_with_no_recipients_keeps_the_panel(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "c");
    cx.simulate_keystrokes(window.into(), "secondary-enter");
    assert!(fixture.drafts().is_empty(), "nothing was sent");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("compose").is_some(), "the panel stays open");
        assert_eq!(
            window.find("compose-to-error").label(),
            Some("Add a recipient in To, Cc or Bcc.")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
async fn undo_send_reopens_the_saved_draft_with_its_id(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-to", cx);
        window.input("alice@example.com", cx);
        window.click("compose-body", cx);
        window.input("Keep me", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.wait_for(window.into(), Duration::from_secs(4), |_, _| {
        !fixture.saved_drafts().is_empty()
    })
    .await;
    let saved_id = fixture.saved_drafts()[0].id.unwrap();
    cx.simulate_keystrokes(window.into(), "secondary-enter");
    assert_eq!(
        fixture.drafts().last().unwrap().id,
        Some(saved_id),
        "sending carries the saved draft's id"
    );
    cx.simulate_keystrokes(window.into(), "z");
    assert_eq!(fixture.cancelled(), vec![9001]);
    cx.simulate_keystrokes(window.into(), "secondary-enter");
    let sent = fixture.drafts();
    assert_eq!(sent.len(), 2);
    assert_eq!(
        sent[1].id,
        Some(saved_id),
        "the reopened draft keeps its id through the next send"
    );
}

#[gpui_kit::test]
fn ctrl_shift_enter_schedules_the_send(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-to", cx);
        window.input("alice@example.com", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "secondary-shift-enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("snooze").is_some(),
            "a send-time picker opens"
        );
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "enter");
    let sent = fixture.drafts();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].send_at.is_some(), "the send is scheduled for later");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("compose").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn discarding_a_draft_reopened_from_drafts_removes_its_row_immediately(cx: &mut TestAppContext) {
    let fixture = Fixture::new(0);
    fixture
        .save_draft(Draft {
            account_id: 1,
            to: vec![addr("Alice", "alice@example.com")],
            subject: "Unfinished".into(),
            body: "Keep or discard".into(),
            ..Default::default()
        })
        .unwrap();
    let (window, kite) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "g d enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("compose").is_some());
        window.click("compose-discard", cx);
        window.render_frame(cx);
        assert!(window.try_find("compose").is_none());
        assert!(
            window.try_find(("row", 0usize)).is_none(),
            "the discarded draft leaves the list on this frame"
        );
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.row_count(), 0)
    });
    assert!(fixture.saved_drafts().is_empty());
}

#[gpui_kit::test]
fn opening_sent_reads_the_conversation_even_when_it_also_has_a_draft(cx: &mut TestAppContext) {
    use kite_core::model::{Mailbox, View};
    let fixture = Fixture::new(0);
    fixture
        .save_draft(Draft {
            account_id: 1,
            to: vec![addr("Alice", "alice@example.com")],
            subject: "A conversation with a draft".into(),
            body: "Unfinished reply".into(),
            ..Default::default()
        })
        .unwrap();
    let view = View {
        account_id: None,
        mailbox: Mailbox::Drafts,
        split: None,
        query: None,
        folder: None,
    };
    let thread = fixture.threads(&view, 0, 10).unwrap()[0].id;
    // This conversation remains editable from Drafts while also appearing in Sent.
    fixture
        .locations
        .lock()
        .unwrap()
        .insert(thread, (Mailbox::Sent, None));
    let (window, kite) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "g t enter");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("mailbox-heading").label(), Some("Sent"));
        assert_eq!(
            window.find("thread-subject").label(),
            Some("A conversation with a draft")
        );
        assert!(
            window.try_find("compose").is_none(),
            "Sent opens the reader"
        );
    })
    .unwrap();
    update_content(window, &kite, cx, |kite, _, _| {
        assert_eq!(kite.open_id(), Some(thread))
    });
}

#[gpui_kit::test]
fn each_invalid_recipient_field_blocks_send_and_send_later_until_corrected(
    cx: &mut TestAppContext,
) {
    for (field, show, error) in [
        ("compose-to", None, "compose-to-error"),
        ("compose-cc", Some("compose-show-cc"), "compose-cc-error"),
        ("compose-bcc", Some("compose-show-bcc"), "compose-bcc-error"),
    ] {
        let fixture = Fixture::new(1);
        let (window, _) = mount(cx, fixture.clone());
        cx.simulate_keystrokes(window.into(), "c");
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            if field != "compose-to" {
                window.click("compose-to", cx);
                window.input("valid@example.com", cx);
            }
            if let Some(show) = show {
                window.click(show, cx);
            }
            window.click(field, cx);
            window.input("Jane <jane@example.com> ignored", cx);
        })
        .unwrap();
        cx.simulate_keystrokes(window.into(), "secondary-enter");
        cx.simulate_keystrokes(window.into(), "secondary-shift-enter");
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window
                    .find(error)
                    .label()
                    .unwrap()
                    .contains("correct the address")
            );
            assert!(
                window.try_find("snooze").is_none(),
                "invalid recipients cannot schedule"
            );
            assert!(window.try_find("compose").is_some());
            window.click("compose-send", cx);
            window.click("compose-send-later", cx);
        })
        .unwrap();
        assert!(
            fixture.drafts().is_empty(),
            "every send path is blocked for {field}"
        );
        cx.update_window(window.into(), |_, window, cx| {
            window.click(field, cx);
            window.press("secondary-a", cx);
            window.input("\"Doe, Jane\" <jane+tag@example.com>", cx);
        })
        .unwrap();
        cx.simulate_keystrokes(window.into(), "secondary-shift-enter");
        cx.simulate_keystrokes(window.into(), "enter");
        let sent = fixture.drafts();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].send_at.is_some());
        let recipients = match field {
            "compose-to" => &sent[0].to,
            "compose-cc" => &sent[0].cc,
            _ => &sent[0].bcc,
        };
        assert_eq!(recipients, &vec![addr("Doe, Jane", "jane+tag@example.com")]);
    }
}

#[gpui_kit::test]
fn compose_docks_expands_and_keeps_actions_inside_a_small_reading_pane(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let fixture = Fixture::new(1);
    let (window, _) = common::open_window(
        cx,
        Some(gpui_kit::size(gpui_kit::px(720.), gpui_kit::px(480.))),
        |window, cx| cx.new(|cx| kite::app::Kite::new(fixture, window, cx)),
    );
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-show-cc", cx);
        window.click("compose-show-bcc", cx);
        let dock = window.find("compose").bounds();
        assert!(dock.origin.x >= gpui_kit::px(384.));
        assert!(dock.bottom_right().x <= gpui_kit::px(720.));
        assert!(dock.bottom_right().y <= gpui_kit::px(480.));
        for id in [
            "compose-send",
            "compose-send-later",
            "compose-discard",
            "compose-expand",
            "compose-close",
        ] {
            let bounds = window.find(id).bounds();
            assert!(
                dock.contains(&bounds.origin) && dock.contains(&bounds.bottom_right()),
                "{id} is inside compose"
            );
        }
        window.click("compose-expand", cx);
        let expanded = window.find("compose").bounds();
        assert_eq!(expanded.origin.x, gpui_kit::px(384.));
        assert_eq!(expanded.bottom_right().x, gpui_kit::px(720.));
        assert!(expanded.size.height > dock.size.height);
        assert_eq!(
            window.find("compose-expand").label(),
            Some("Collapse compose")
        );
        window.click("compose-expand", cx);
        assert_eq!(window.find("compose").bounds(), dock);
    })
    .unwrap();
}

/// The body textarea, found by the text it holds: it is the only observed input
/// whose accessibility value is the drafted message.
fn body_editor(window: &gpui_kit::Window, expected: &str) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    let matches: Vec<_> = gpui_kit::base::test_support::snapshots(window)
        .into_iter()
        .filter(|element| {
            element
                .value()
                .is_some_and(|value| value.contains(expected))
        })
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "one observed input holds the message body, found {}",
        matches.len()
    );
    matches[0].bounds()
}

#[gpui_kit::test]
fn expand_is_an_icon_as_small_as_close(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        for (label, expanded) in [("Expand compose", false), ("Collapse compose", true)] {
            let close = window.find("compose-close").bounds();
            let expand = window.find("compose-expand");
            assert_eq!(
                expand.label(),
                Some(label),
                "the icon keeps its announced action"
            );
            let bounds = expand.bounds();
            assert_eq!(
                bounds.size, close.size,
                "the expand icon is the size of the close X, not a word"
            );
            assert!(
                bounds.size.width <= gpui_kit::px(32.),
                "an icon button is ~24px wide, not a label: {bounds:?}"
            );
            assert!(
                (bounds.center().y - close.center().y).abs() <= gpui_kit::px(1.),
                "the icon sits on the close button's line"
            );
            // Both chips share one centred line inside the header band at the panel's top.
            let panel = window.find("compose").bounds();
            assert_eq!(
                bounds.center().y,
                close.center().y,
                "the header buttons are the same size and share one centre"
            );
            assert!(
                bounds.center().y > panel.top()
                    && bounds.center().y < panel.top() + gpui_kit::px(44.),
                "the icon chips are vertically centred on the header row ({bounds:?} in {panel:?})"
            );
            if !expanded {
                window.click("compose-expand", cx);
            }
        }
        window.click("compose-expand", cx);
        assert_eq!(
            window.find("compose-expand").label(),
            Some("Expand compose")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn body_fills_its_area_so_every_line_shows(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-body", cx);
        for line in 1..=12 {
            if line > 1 {
                window.press("enter", cx);
            }
            window.input(&format!("line {line}"), cx);
        }
        for expanded in [false, true] {
            let area = window.find("compose-body").bounds();
            let editor = body_editor(window, "line 12");
            // The area's own padding (py_2) is the only part the editor may leave empty.
            assert!(
                editor.size.height >= area.size.height - gpui_kit::px(16.) - gpui_kit::px(1.),
                "expanded={expanded}: the editor fills the body area ({editor:?} in {area:?})"
            );
            assert!(
                editor.size.height > 12. * window.line_height()
                    || editor.size.height >= area.size.height - gpui_kit::px(16.),
                "expanded={expanded}: 12 lines are visible or the area is full ({editor:?})"
            );
            assert!(
                editor.top() >= area.top() && editor.bottom() <= area.bottom(),
                "expanded={expanded}: the editor stays inside the body area ({editor:?} in {area:?})"
            );
            window.click("compose-expand", cx);
        }
    })
    .unwrap();
}

#[gpui_kit::test]
fn clicks_and_scroll_inside_compose_never_reach_the_list_behind(cx: &mut TestAppContext) {
    let fixture = Fixture::new(6);
    // The classic layout: the list runs under the whole window, so it sits behind the
    // docked compose panel, which is what a pass-through click reaches.
    fixture.settings.lock().unwrap().reading_pane = false;
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let panel = window.find("compose").bounds();
        let body = window.find("compose-body").bounds();
        let row = (0..6usize)
            .map(|ix| window.find(("row", ix)).bounds())
            .find(|row| panel.contains(&row.center()))
            .expect("a list row sits behind the compose panel");
        let header = panel.origin + gpui_kit::point(gpui_kit::px(24.), gpui_kit::px(14.));
        assert!(
            row.contains(&header),
            "the header point is over a row behind the panel ({header:?} in {row:?})"
        );
        for (target, offset) in [
            // The header, beside its title and the two icon buttons.
            (
                "compose",
                gpui_kit::point(gpui_kit::px(24.), gpui_kit::px(14.)),
            ),
            // The empty space below the message text.
            (
                "compose-body",
                gpui_kit::point(gpui_kit::px(24.), body.size.height - gpui_kit::px(4.)),
            ),
        ] {
            window.click_at(target, offset, cx);
            window.render_frame(cx);
            assert!(
                window.try_find("thread-subject").is_none(),
                "a click inside the compose panel ({target}) must not open a thread behind it"
            );
        }
        let rows_before: Vec<_> = (0..6usize)
            .map(|ix| window.find(("row", ix)).bounds())
            .collect();
        window.scroll(
            "compose-body",
            gpui_kit::ScrollDelta::Lines(gpui_kit::point(0., 100.)),
            cx,
        );
        window.render_frame(cx);
        let rows_after: Vec<_> = (0..6usize)
            .map(|ix| window.find(("row", ix)).bounds())
            .collect();
        assert_eq!(
            rows_before, rows_after,
            "a wheel event over compose must not scroll the list behind it"
        );
        assert!(
            fixture.opened.lock().unwrap().is_empty(),
            "nothing under the panel opened: {:?}",
            fixture.opened.lock().unwrap()
        );
        // Outside the panel the list still answers: the panel blocks only itself.
        let outside = (0..6usize)
            .map(|ix| (ix, window.find(("row", ix)).bounds()))
            .find(|(_, bounds)| !panel.contains(&bounds.center()))
            .expect("a row beside the panel");
        window.click(("row", outside.0), cx);
        window.render_frame(cx);
        assert!(
            !fixture.opened.lock().unwrap().is_empty(),
            "a click outside the panel still opens its thread"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn escaped_display_names_and_quoted_local_parts_send_without_losing_recipients(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new(1);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-to", cx);
        window.input(
            r#""Doe \"Jane\", Team" <jane+tag@example.com>; "quoted local"@example.org"#,
            cx,
        );
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "secondary-enter");
    let sent = fixture.drafts();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].to,
        vec![
            addr("Doe \"Jane\", Team", "jane+tag@example.com"),
            addr("", r#""quoted local"@example.org"#)
        ]
    );
}

#[gpui_kit::test]
fn closing_an_incomplete_draft_preserves_recipient_fields(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("compose-to", cx);
        window.input("unfinished", cx);
        window.click("compose-show-cc", cx);
        window.click("compose-cc", cx);
        window.input("cc@", cx);
        window.click("compose-show-bcc", cx);
        window.click("compose-bcc", cx);
        window.input("bcc@", cx);
    })
    .unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.click("compose-close", cx);
    })
    .unwrap();
    assert!(fixture.drafts().is_empty());
    let saved = fixture.saved_drafts();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].to, vec![addr("", "unfinished")]);
    assert_eq!(saved[0].cc, vec![addr("", "cc@")]);
    assert_eq!(saved[0].bcc, vec![addr("", "bcc@")]);
}

// ---- compose chrome ---------------------------------------------------------

#[gpui_kit::test]
fn the_cc_and_bcc_toggles_close_the_to_row_instead_of_adding_a_strip(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let to = window.find("compose-to").bounds();
        let subject = window.find("compose-subject").bounds();
        let cc = window.find("compose-show-cc").bounds();
        let bcc = window.find("compose-show-bcc").bounds();
        for (id, bounds) in [("compose-show-cc", cc), ("compose-show-bcc", bcc)] {
            assert!(
                bounds.center().y > to.origin.y && bounds.center().y < to.bottom_right().y,
                "{id} sits on the To row, not on a strip under it"
            );
            assert!(
                bounds.origin.x > to.origin.x,
                "{id} is at the end of the To row"
            );
        }
        assert!(cc.bottom_right().x < bcc.origin.x, "Cc comes before Bcc");
        let gap = subject.origin.y - to.bottom_right().y;
        assert!(
            gap <= gpui_kit::px(20.),
            "no row sits between To and Subject while the toggles are hidden (gap {gap:?})"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn clicking_cc_reveals_the_field_under_to_with_the_caret_in_it(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("compose-cc").is_none());
        window.click("compose-show-cc", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("compose-show-cc").is_none(),
            "the toggle goes once its field shows"
        );
        assert!(
            window.try_find("compose-show-bcc").is_some(),
            "Bcc stays one click away"
        );
        let to = window.find("compose-to").bounds();
        let cc = window.find("compose-cc").bounds();
        let subject = window.find("compose-subject").bounds();
        assert!(cc.origin.y >= to.bottom_right().y, "Cc sits under To");
        assert!(
            cc.bottom_right().y <= subject.origin.y,
            "Cc sits above Subject"
        );
        assert_eq!(
            window.find("compose-cc").focused(),
            Some(true),
            "the revealed field takes the caret"
        );
        window.input("carol@example.com", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("compose-cc").value(),
            Some("carol@example.com"),
            "typing goes into Cc"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn send_is_the_rightmost_action_on_the_footer_row(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let compose = window.find("compose").bounds();
        let discard = window.find("compose-discard").bounds();
        let later = window.find("compose-send-later").bounds();
        let send = window.find("compose-send").bounds();
        assert!(
            discard.bottom_right().x < later.origin.x,
            "Discard comes before Send later"
        );
        assert!(
            later.bottom_right().x < send.origin.x,
            "Send is the rightmost action"
        );
        assert!(
            send.bottom_right().x > compose.bottom_right().x - gpui_kit::px(24.),
            "Send sits at the footer's right edge"
        );
        for (id, bounds) in [("compose-discard", discard), ("compose-send-later", later)] {
            assert!(
                (bounds.center().y - send.center().y).abs() <= gpui_kit::px(1.),
                "{id} shares Send's row"
            );
            assert_eq!(
                bounds.size.height, send.size.height,
                "{id} is the same height as Send"
            );
        }
    })
    .unwrap();
}

// ---- the sending account ----------------------------------------------------

#[gpui_kit::test]
fn a_reply_starts_from_the_threads_account_and_can_send_from_another(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    let other = fixture.add_microsoft().unwrap();
    let (window, _) = mount(cx, fixture.clone());
    cx.simulate_keystrokes(window.into(), "enter");
    cx.simulate_keystrokes(window.into(), "r");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("compose-from").label(),
            Some("me@example.com"),
            "a reply is sent from the account the thread belongs to"
        );
        window.click("compose-from", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("popup-menu").is_some(),
            "two accounts make From a dropdown"
        );
        window.within("popup-menu").click(1usize, cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("compose-from").label(),
            Some(other.email.as_str()),
            "the menu switched the sending account"
        );
    })
    .unwrap();
    cx.simulate_keystrokes(window.into(), "secondary-enter");
    let sent = fixture.drafts();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].account_id, other.id,
        "the mail goes out as the chosen account"
    );
    assert_eq!(
        sent[0].to,
        vec![addr("Sender 1", "sender1@example.com")],
        "switching From leaves the recipients alone"
    );
    assert_eq!(sent[0].subject, "Re: Subject 1");
    assert!(sent[0].body.contains("> Plain body for thread 1"));
}

#[gpui_kit::test]
fn one_account_shows_the_from_row_without_a_dropdown(cx: &mut TestAppContext) {
    let fixture = Fixture::new(1);
    let (window, _) = mount(cx, fixture);
    cx.simulate_keystrokes(window.into(), "c");
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("compose-from").label(),
            Some("me@example.com"),
            "the only account is named in the From row"
        );
        window.click("compose-from", cx);
        window.render_frame(cx);
        assert!(
            window.try_find("popup-menu").is_none(),
            "one account leaves nothing to choose"
        );
    })
    .unwrap();
}
