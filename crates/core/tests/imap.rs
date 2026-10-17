//! Integration tests for the Proton Bridge backend against the shared fake IMAP/SMTP server.
mod common;

use std::sync::{Arc, Mutex};

use common::{FIRST, SECOND, config, serve, upserts, world};
use kite_core::model::Mailbox;
use kite_core::sync::imap::Imap;
use kite_core::sync::{Delta, Op};

// ---- tests ----

#[tokio::test]
async fn first_sync_parses_messages() {
    let world = Arc::new(Mutex::new(world()));
    world
        .lock()
        .expect("world")
        .folder("INBOX")
        .push(FIRST, &["\\Flagged"]);
    world
        .lock()
        .expect("world")
        .folder("INBOX")
        .push(SECOND, &["\\Seen"]);
    let (imap_port, smtp_port) = serve(world.clone()).await.ports();

    let mut imap = Imap::new(config(imap_port, smtp_port));
    let (deltas, cursor) = imap.sync(None).await.expect("sync");

    let messages = upserts(&deltas);
    assert_eq!(messages.len(), 2, "two inbox messages");
    let first = messages
        .iter()
        .find(|m| m.subject == "Quarterly numbers")
        .expect("first");
    assert_eq!(first.remote_id, "INBOX/1/1");
    assert_eq!(first.folder, "INBOX");
    assert_eq!(first.mailbox, Mailbox::Inbox);
    assert_eq!(first.from.name, "Ada Lovelace");
    assert_eq!(first.from.email, "ada@example.org");
    assert_eq!(first.to.len(), 2, "two To addresses");
    assert_eq!(first.to[1].email, "bob@example.com");
    assert_eq!(first.cc[0].email, "carol@example.com");
    assert_eq!(first.date, 1_746_525_600_000);
    assert_eq!(first.message_id.as_deref(), Some("abc@example.org"));
    assert_eq!(first.in_reply_to.as_deref(), Some("parent@example.org"));
    assert_eq!(
        first.references,
        vec!["root@example.org", "parent@example.org"]
    );
    assert!(first.text.contains("plain body"));
    assert!(
        first
            .html
            .as_deref()
            .expect("html")
            .contains("<p>html body</p>")
    );
    assert!(first.unread);
    assert!(first.starred);
    assert_eq!(first.attachments.len(), 1);
    assert_eq!(first.attachments[0].name, "report.pdf");
    assert_eq!(first.attachments[0].mime, "application/pdf");
    assert_eq!(first.attachments[0].size, 8);

    let second = messages
        .iter()
        .find(|m| m.subject == "Lunch?")
        .expect("second");
    assert!(!second.unread);
    assert!(!second.starred);
    assert!(second.html.is_none());
    assert!(cursor.contains("INBOX"), "cursor remembers the folder");
}

#[tokio::test]
async fn incremental_sync_reports_new_flags_and_deletes() {
    let world = Arc::new(Mutex::new(world()));
    world
        .lock()
        .expect("world")
        .folder("INBOX")
        .push(FIRST, &[]);
    world
        .lock()
        .expect("world")
        .folder("INBOX")
        .push(SECOND, &[]);
    world
        .lock()
        .expect("world")
        .folder("INBOX")
        .push(FIRST, &[]);
    let (imap_port, smtp_port) = serve(world.clone()).await.ports();

    let mut imap = Imap::new(config(imap_port, smtp_port));
    let (_, cursor) = imap.sync(None).await.expect("first sync");

    {
        let mut world = world.lock().expect("world");
        let inbox = world.folder("INBOX");
        inbox.set_flag(1, "\\Seen", true);
        inbox.expunge(3);
        inbox.push(SECOND, &[]);
    }

    let (deltas, _) = imap.sync(Some(cursor)).await.expect("second sync");
    assert_eq!(deltas.len(), 3, "one of each delta: {deltas:#?}");
    assert!(matches!(&deltas[0], Delta::Delete { remote_id } if remote_id == "INBOX/1/3"));
    assert!(
        matches!(&deltas[1], Delta::Upsert(msg) if msg.remote_id == "INBOX/1/4" && msg.subject == "Lunch?")
    );
    assert!(
        matches!(&deltas[2], Delta::Flags { remote_id, unread, starred } if remote_id == "INBOX/1/1" && !unread && !starred)
    );
}

#[tokio::test]
async fn move_returns_the_new_remote_id() {
    let world = Arc::new(Mutex::new(world()));
    world
        .lock()
        .expect("world")
        .folder("INBOX")
        .push(FIRST, &[]);
    world
        .lock()
        .expect("world")
        .folder("INBOX")
        .push(SECOND, &[]);
    let (imap_port, smtp_port) = serve(world.clone()).await.ports();

    let mut imap = Imap::new(config(imap_port, smtp_port));
    let (deltas, _) = imap.sync(None).await.expect("sync");
    let message = upserts(&deltas)
        .into_iter()
        .find(|m| m.subject == "Lunch?")
        .expect("message")
        .remote_id
        .clone();

    let new_id = imap
        .apply(&Op::Move {
            remote_id: message.clone(),
            to: Mailbox::Archive,
        })
        .await
        .expect("move")
        .expect("copyuid");
    assert_eq!(new_id, "Archive/4/1");

    let world = world.lock().expect("world");
    let archive = world
        .folders
        .iter()
        .find(|f| f.name == "Archive")
        .expect("archive");
    assert_eq!(archive.uids(), vec![1]);
    assert_eq!(archive.msgs[0].raw, SECOND);
    let inbox = world
        .folders
        .iter()
        .find(|f| f.name == "INBOX")
        .expect("inbox");
    assert_eq!(inbox.uids(), vec![1]);
}

#[tokio::test]
async fn flag_ops_set_seen_and_flagged_on_the_server() {
    let world = Arc::new(Mutex::new(world()));
    world
        .lock()
        .expect("world")
        .folder("INBOX")
        .push(FIRST, &["\\Seen"]);
    let (imap_port, smtp_port) = serve(world.clone()).await.ports();
    let mut imap = Imap::new(config(imap_port, smtp_port));
    let (deltas, _) = imap.sync(None).await.expect("sync");
    let id = upserts(&deltas)[0].remote_id.clone();
    let flags = || {
        world.lock().expect("world").folder("INBOX").msgs[0]
            .flags
            .clone()
    };

    let op = Op::Flags {
        remote_id: id.clone(),
        unread: Some(true),
        starred: Some(true),
    };
    imap.apply(&op).await.expect("mark unread + star");
    assert_eq!(
        flags(),
        vec!["\\Flagged"],
        "unread clears \\Seen, star sets \\Flagged"
    );

    let op = Op::Flags {
        remote_id: id,
        unread: Some(false),
        starred: Some(false),
    };
    imap.apply(&op).await.expect("mark read + unstar");
    assert_eq!(flags(), vec!["\\Seen"]);
}

#[tokio::test]
async fn send_delivers_to_every_recipient_without_the_bcc_header() {
    let world = Arc::new(Mutex::new(world()));
    let (imap_port, smtp_port) = serve(world.clone()).await.ports();

    let mime = b"From: Karl <karl@example.com>\r\n\
To: Alice <alice@example.org>\r\n\
Cc: carol@example.org\r\n\
Bcc: hidden@example.org\r\n\
Subject: Hi\r\n\
Message-ID: <new@example.com>\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
first line\r\n\
.hidden line\r\n\
last line\r\n";

    let mut imap = Imap::new(config(imap_port, smtp_port));
    imap.apply(&Op::Send {
        mime: mime.to_vec(),
    })
    .await
    .expect("send");

    let world = world.lock().expect("world");
    assert_eq!(world.sent.len(), 1);
    let sent = &world.sent[0];
    let mut rcpts = sent.rcpts.clone();
    rcpts.sort();
    assert_eq!(
        rcpts,
        vec![
            "alice@example.org",
            "carol@example.org",
            "hidden@example.org"
        ]
    );

    let raw = String::from_utf8_lossy(&sent.raw).to_string();
    assert!(
        !raw.to_ascii_lowercase().contains("bcc:"),
        "bcc stripped: {raw}"
    );
    assert!(
        raw.contains("\r\n..hidden line\r\n"),
        "dot stuffed: {raw:?}"
    );
    assert!(
        raw.replace("\r\n..", "\r\n.")
            .contains("\r\n.hidden line\r\n")
    );
}

#[tokio::test]
async fn wrong_password_fails_verify() {
    let world = Arc::new(Mutex::new(world()));
    let (imap_port, smtp_port) = serve(world.clone()).await.ports();

    let mut cfg = config(imap_port, smtp_port);
    cfg.password = "wrong".into();
    let mut imap = Imap::new(cfg);
    assert!(imap.verify().await.is_err());
}

#[tokio::test]
async fn create_folder_creates_and_subscribes_the_mailbox() {
    let world = Arc::new(Mutex::new(world()));
    let (imap_port, smtp_port) = serve(world.clone()).await.ports();

    let mut imap = Imap::new(config(imap_port, smtp_port));
    let id = imap.create_folder("Receipts").await.expect("create");
    assert_eq!(id, "Folders/Receipts");

    {
        let world = world.lock().expect("world");
        assert!(world.folders.iter().any(|f| f.name == id));
        assert!(world.subscribed.contains(&id));
    }

    // A folder that already exists is an error, never a silent success.
    assert!(imap.create_folder("Receipts").await.is_err());
}

#[tokio::test]
async fn attachment_downloads_the_mime_part() {
    let world = Arc::new(Mutex::new(world()));
    world
        .lock()
        .expect("world")
        .folder("INBOX")
        .push(FIRST, &[]);
    let (imap_port, smtp_port) = serve(world.clone()).await.ports();

    let mut imap = Imap::new(config(imap_port, smtp_port));
    let (deltas, _) = imap.sync(None).await.expect("sync");
    let message = upserts(&deltas).into_iter().next().expect("message");
    let attachment = message.attachments[0].clone();

    let bytes = imap
        .attachment(&message.remote_id, &attachment.remote_id)
        .await
        .expect("attachment");
    assert_eq!(bytes, b"%PDF-1.4");
}
