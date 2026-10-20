//! Privacy enforcement through the production Mail API and real SQLite, without an AI service.
use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::Mutex;

use kite_core::api::Mail;
use kite_core::engine::{Engine, Options, Secrets};
use kite_core::model::*;
use kite_core::store::Store;

#[test]
fn private_conversations_and_their_replies_never_reach_the_ai_endpoint() {
    let dir = std::env::temp_dir().join(format!("kite-ai-privacy-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = Store::open(&dir.join("kite.db")).unwrap();
    let account = store
        .add_account(Provider::Proton, "me@example.com", "Me")
        .unwrap();
    let draft = Draft {
        account_id: account.id,
        subject: "Sensitive title".into(),
        body: "Sensitive body".into(),
        ..Draft::default()
    };
    let (id, _) = store
        .save_draft(&draft, "private-draft@example.com")
        .unwrap();
    let view = View {
        account_id: Some(account.id),
        mailbox: Mailbox::Drafts,
        split: None,
        query: None,
        folder: None,
    };
    let thread = store.threads(&view, 0, 10).unwrap()[0].id;
    let message = store.thread(thread).unwrap()[0].id;
    let undo = store
        .act(
            &[thread],
            &Action::Label {
                tag: PRIVATE_TAG.into(),
            },
        )
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    store
        .set_settings(Settings {
            ai_enabled: true,
            ai_base_url: format!("http://{}", listener.local_addr().unwrap()),
            ..Settings::default()
        })
        .unwrap();
    let engine = Engine::open_with(
        &dir,
        Secrets::Memory(Mutex::new(HashMap::new())),
        Options::default(),
    )
    .unwrap();
    engine.set_ai_key("test-only").unwrap();

    for request in [
        AiRequest::Summary { thread },
        AiRequest::Compose {
            draft: Draft {
                id: Some(id),
                ..draft.clone()
            },
        },
        AiRequest::Compose {
            draft: Draft {
                reply_to: Some(message),
                ..draft.clone()
            },
        },
    ] {
        let error = engine.ai(1, request).unwrap_err().to_string();
        assert!(error.to_ascii_lowercase().contains("private"), "{error}");
    }
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert!(
        engine
            .ai(2, AiRequest::Summary { thread: i64::MAX })
            .is_err()
    );

    let reply = Draft {
        reply_to: Some(message),
        subject: "Saved reply".into(),
        ..draft.clone()
    };
    let (reply_id, _) = store
        .save_draft(&reply, "private-reply@example.com")
        .unwrap();
    let reply_thread = store
        .threads(&view, 0, 10)
        .unwrap()
        .into_iter()
        .find(|row| row.subject == "Saved reply")
        .unwrap()
        .id;
    assert!(
        engine
            .ai(
                8,
                AiRequest::Summary {
                    thread: reply_thread
                }
            )
            .is_err(),
        "a saved reply opened through All mail must retain its source's privacy"
    );
    let reply_message = store.thread(reply_thread).unwrap()[0].id;
    assert!(
        engine
            .ai(
                9,
                AiRequest::Compose {
                    draft: Draft {
                        reply_to: Some(reply_message),
                        ..draft.clone()
                    }
                }
            )
            .is_err(),
        "forwarding a saved reply must follow the entire source chain"
    );
    store.discard_draft(reply_id).unwrap();

    // Editing the catalog cannot strip existing protection; only an explicit untag or undo can.
    let mut tags = engine.tags().unwrap();
    tags.retain(|t| t.name != PRIVATE_TAG);
    engine.set_tags(tags).unwrap();
    assert!(engine.ai(3, AiRequest::Summary { thread }).is_err());
    let mut tags = engine.tags().unwrap();
    tags.iter_mut()
        .find(|t| t.name == PRIVATE_TAG)
        .unwrap()
        .name = "Public".into();
    assert!(engine.set_tags(tags).is_err());
    engine.undo(undo).unwrap();
    store.check_ai_thread(thread).unwrap();

    engine
        .act(
            &[thread],
            &Action::Label {
                tag: PRIVATE_TAG.into(),
            },
        )
        .unwrap();
    let saved = Draft {
        id: Some(id),
        ..draft
    };
    let outbox = store.outbox_add(&saved, i64::MAX).unwrap();
    assert!(
        engine
            .ai(
                10,
                AiRequest::Compose {
                    draft: saved.clone()
                }
            )
            .is_err(),
        "a hidden queued draft must remain private"
    );
    store.outbox_cancel(outbox).unwrap();
    assert!(
        engine.ai(11, AiRequest::Compose { draft: saved }).is_err(),
        "undo send must restore the Private tag with the draft"
    );
    let restored = store.threads(&view, 0, 10).unwrap();
    assert!(restored[0].labels.iter().any(|name| name == PRIVATE_TAG));

    // No network request was launched by any of the rejected attempts.
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
