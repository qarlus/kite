//! Compose phase 1: new mail, reply/reply-all/forward, send + undo, draft keeping
//! and key isolation while typing.
mod common;
use common::{Fixture, mount, update_content};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use kite_core::model::Addr;

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
    assert_eq!(drafts[0].reply_to, Some(12), "threading points at the latest message");
    assert_eq!(drafts[0].to, vec![addr("Sender 1", "sender1@example.com")]);
}

#[gpui_kit::test]
fn a_replies_to_all_without_self_or_duplicates(cx: &mut TestAppContext) {
    let fixture = Fixture::new(2);
    fixture.set_message_recipients(
        1,
        vec![
            addr("Me", "me@example.com"),
            addr("Bob", "bob@example.com"),
        ],
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
    assert_eq!(drafts[0].reply_to, None, "a forward starts a new thread");
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
        assert_eq!(kite.compose_body(cx), "Half written", "the draft survives r");
        assert_eq!(kite.compose_subject(cx), "", "the reply did not replace the draft");
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
fn invalid_address_is_rejected_with_an_error_toast(cx: &mut TestAppContext) {
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
        assert_eq!(window.find("toast").label(), Some("Invalid address: not-an-email"));
    })
    .unwrap();
}
