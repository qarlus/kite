//! End-to-end tests for the engine through the `Mail` trait: real SQLite in a temp dir, in-memory
//! secrets, and the shared fake Bridge/SMTP server. No keychain, no network.
mod common;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{FIRST, SECOND, serve};
use kite_core::api::Mail;
use kite_core::engine::{Engine, Options, Secrets};
use kite_core::model::*;

// ------------------------------------------------------------------ harness

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime")
}

fn temp_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("kite-engine-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn engine(dir: &Path) -> Arc<Engine> {
    Engine::open_with(
        dir,
        Secrets::Memory(Mutex::new(HashMap::new())),
        Options {
            proton_tls: false,
            ..Default::default()
        },
    )
    .expect("engine")
}

fn login((imap_port, smtp_port): (u16, u16)) -> ProtonLogin {
    ProtonLogin {
        email: "karl@example.com".into(),
        password: "secret".into(),
        host: Some("127.0.0.1".into()),
        imap_port: Some(imap_port),
        smtp_port: Some(smtp_port),
    }
}

fn inbox(account_id: Id) -> View {
    View {
        account_id: Some(account_id),
        mailbox: Mailbox::Inbox,
        split: None,
        query: None,
        folder: None,
    }
}

fn addr(name: &str, email: &str) -> Addr {
    Addr {
        name: name.into(),
        email: email.into(),
    }
}

/// A single consumer of `Event`s that buffers anything a predicate did not ask for.
struct Events {
    rx: std::sync::mpsc::Receiver<Event>,
    backlog: Mutex<Vec<Event>>,
}

impl Events {
    fn new(rx: async_channel::Receiver<Event>) -> Events {
        let (tx, out) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(event) = rx.recv_blocking() {
                if tx.send(event).is_err() {
                    break;
                }
            }
        });
        Events {
            rx: out,
            backlog: Mutex::new(Vec::new()),
        }
    }

    fn wait(&self, predicate: impl Fn(&Event) -> bool) -> Event {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut backlog = self.backlog.lock().expect("backlog");
            if let Some(index) = backlog.iter().position(&predicate) {
                return backlog.remove(index);
            }
            drop(backlog);
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out waiting for an event");
            match self.rx.recv_timeout(left) {
                Ok(event) if predicate(&event) => return event,
                Ok(event) => self.backlog.lock().expect("backlog").push(event),
                Err(_) => panic!("timed out waiting for an event"),
            }
        }
    }
}

/// A minimal SSE server: answers one request with the given `data:` payloads, and keeps the
/// request it received so the model and the key it was sent can be checked.
fn sse_server(chunks: &[&str]) -> (String, Arc<Mutex<Vec<u8>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind sse");
    let address = listener.local_addr().expect("addr").to_string();
    let payloads: Vec<String> = chunks.iter().map(|c| format!("data: {c}\n\n")).collect();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    std::thread::spawn(move || {
        let Ok((mut sock, _)) = listener.accept() else {
            return;
        };
        let mut request = Vec::new();
        let mut buffer = [0u8; 1024];
        let mut body_left = 0usize;
        loop {
            if let Some(head_end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                if body_left == 0 {
                    let head = String::from_utf8_lossy(&request[..head_end]).to_lowercase();
                    body_left = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0);
                }
                if request.len() >= head_end + 4 + body_left {
                    break;
                }
            }
            match sock.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => request.extend_from_slice(&buffer[..n]),
            }
        }
        *recorded.lock().expect("request") = request.clone();
        let header =
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
        let _ = sock.write_all(header.as_bytes());
        // Dribble the body out a byte at a time so multi-byte characters straddle network reads.
        for byte in payloads.concat().bytes() {
            let _ = sock.write_all(&[byte]);
            let _ = sock.flush();
            std::thread::sleep(std::time::Duration::from_micros(300));
        }
    });
    (address, seen)
}

// -------------------------------------------------------------------- tests

#[test]
fn adding_proton_syncs_the_inbox_into_the_store() {
    let rt = runtime();
    let world = Arc::new(Mutex::new(common::world()));
    world
        .lock()
        .expect("world")
        .folder("INBOX")
        .push(FIRST, &["\\Seen"]);
    world
        .lock()
        .expect("world")
        .folder("INBOX")
        .push(SECOND, &[]);
    let server = rt.block_on(serve(world.clone()));

    let dir = temp_dir();
    let engine = engine(&dir);
    let events = Events::new(engine.events());
    let account = engine.add_proton(login(server.ports())).expect("add");

    events.wait(|e| matches!(e, Event::Changed(ids) if ids.contains(&account.id)));

    let threads = engine.threads(&inbox(account.id), 0, 50).expect("threads");
    let mut subjects: Vec<&str> = threads.iter().map(|t| t.subject.as_str()).collect();
    subjects.sort_unstable();
    assert_eq!(subjects, vec!["Lunch?", "Quarterly numbers"]);
}

#[test]
fn archiving_applies_locally_before_the_network_and_reaches_the_server() {
    let rt = runtime();
    let world = Arc::new(Mutex::new(common::world()));
    world
        .lock()
        .expect("world")
        .folder("INBOX")
        .push(SECOND, &[]);
    let server = rt.block_on(serve(world.clone()));

    let dir = temp_dir();
    let engine = engine(&dir);
    let events = Events::new(engine.events());
    let account = engine.add_proton(login(server.ports())).expect("add");
    events.wait(|e| matches!(e, Event::Changed(ids) if ids.contains(&account.id)));

    let view = inbox(account.id);
    let thread = engine.threads(&view, 0, 50).expect("threads").remove(0);
    engine.act(&[thread.id], &Action::Archive).expect("act");

    // Gone from the inbox immediately, before any network round trip.
    assert!(engine.threads(&view, 0, 50).expect("threads").is_empty());

    events.wait(|e| matches!(e, Event::Changed(ids) if ids.contains(&account.id)));
    let world = world.lock().expect("world");
    let archive = world
        .folders
        .iter()
        .find(|f| f.name == "Archive")
        .expect("archive");
    assert_eq!(archive.uids(), vec![1]);
    assert_eq!(archive.msgs[0].raw, SECOND);
}

#[test]
fn sending_builds_mime_and_replies_carry_reply_headers() {
    let rt = runtime();
    let world = Arc::new(Mutex::new(common::world()));
    world
        .lock()
        .expect("world")
        .folder("INBOX")
        .push(FIRST, &["\\Seen"]);
    let server = rt.block_on(serve(world.clone()));

    let dir = temp_dir();
    let engine = engine(&dir);
    engine
        .set_settings(Settings {
            undo_send_secs: 0,
            ..Default::default()
        })
        .expect("settings");
    let events = Events::new(engine.events());
    let account = engine.add_proton(login(server.ports())).expect("add");
    events.wait(|e| matches!(e, Event::Changed(ids) if ids.contains(&account.id)));

    engine
        .send(Draft {
            account_id: account.id,
            to: vec![addr("Bob", "bob@example.org")],
            subject: "Hi there".into(),
            body: "hello".into(),
            ..Default::default()
        })
        .expect("send");
    events.wait(|e| matches!(e, Event::Changed(ids) if ids.contains(&account.id)));

    let view = inbox(account.id);
    let thread = engine.threads(&view, 0, 50).expect("threads").remove(0);
    let message = engine.thread(thread.id).expect("thread").remove(0);
    engine
        .send(Draft {
            account_id: account.id,
            to: vec![addr("Ada", "ada@example.org")],
            reply_to: Some(message.id),
            body: "thanks".into(),
            ..Default::default()
        })
        .expect("reply");
    events.wait(|e| matches!(e, Event::Changed(ids) if ids.contains(&account.id)));

    let world = world.lock().expect("world");
    assert_eq!(world.sent.len(), 2, "both drafts reached SMTP");
    let plain = String::from_utf8_lossy(&world.sent[0].raw).to_string();
    assert!(plain.contains("Subject: Hi there"), "{plain}");
    assert!(plain.contains("hello"), "{plain}");
    assert!(world.sent[0].rcpts.contains(&"bob@example.org".to_string()));

    let reply = String::from_utf8_lossy(&world.sent[1].raw).to_string();
    assert!(reply.contains("In-Reply-To: <abc@example.org>"), "{reply}");
    assert!(
        reply.contains("References: <root@example.org> <parent@example.org> <abc@example.org>"),
        "{reply}"
    );
    assert!(reply.contains("Subject: Re: Quarterly numbers"), "{reply}");
    assert!(world.sent[1].rcpts.contains(&"ada@example.org".to_string()));
}

#[test]
fn cancelling_a_queued_send_keeps_it_out_of_the_outbox() {
    let rt = runtime();
    let world = Arc::new(Mutex::new(common::world()));
    world
        .lock()
        .expect("world")
        .folder("INBOX")
        .push(FIRST, &[]);
    let server = rt.block_on(serve(world.clone()));

    let dir = temp_dir();
    let engine = engine(&dir);
    let events = Events::new(engine.events());
    let account = engine.add_proton(login(server.ports())).expect("add");
    events.wait(|e| matches!(e, Event::Changed(ids) if ids.contains(&account.id)));

    engine
        .set_settings(Settings {
            undo_send_secs: 30,
            ..Default::default()
        })
        .expect("settings");
    let held = engine
        .send(Draft {
            account_id: account.id,
            to: vec![addr("Bob", "bob@example.org")],
            subject: "Held".into(),
            body: "held".into(),
            ..Default::default()
        })
        .expect("send");
    let returned = engine.cancel_send(held).expect("cancel").expect("draft");
    assert_eq!(returned.subject, "Held");

    // A second draft due at once proves the outbox ran; only it can reach SMTP.
    engine
        .set_settings(Settings {
            undo_send_secs: 0,
            ..Default::default()
        })
        .expect("settings");
    engine
        .send(Draft {
            account_id: account.id,
            to: vec![addr("Bob", "bob@example.org")],
            subject: "Sent".into(),
            body: "sent".into(),
            ..Default::default()
        })
        .expect("send");
    events.wait(|e| matches!(e, Event::Changed(ids) if ids.contains(&account.id)));

    let world = world.lock().expect("world");
    assert_eq!(world.sent.len(), 1, "the cancelled draft never left");
    let raw = String::from_utf8_lossy(&world.sent[0].raw).to_string();
    assert!(raw.contains("Subject: Sent"), "{raw}");
    assert!(!raw.contains("Held"), "{raw}");
}

#[test]
fn a_wrong_bridge_password_leaves_no_account() {
    let rt = runtime();
    let world = Arc::new(Mutex::new(common::world()));
    let server = rt.block_on(serve(world.clone()));

    let dir = temp_dir();
    let engine = engine(&dir);
    let mut login = login(server.ports());
    login.password = "wrong".into();
    assert!(engine.add_proton(login).is_err());
    assert!(engine.accounts().expect("accounts").is_empty());
}

#[test]
fn creating_a_folder_persists_it_and_a_provider_error_persists_nothing() {
    let rt = runtime();
    let world = Arc::new(Mutex::new(common::world()));
    let server = rt.block_on(serve(world.clone()));

    let dir = temp_dir();
    let engine = engine(&dir);
    let account = engine.add_proton(login(server.ports())).expect("add");

    assert!(
        engine.folders().expect("folders").is_empty(),
        "discovery is out of scope, so nothing is cached yet"
    );

    let folder = engine
        .create_folder(account.id, "Receipts")
        .expect("create");
    assert_eq!(folder.account_id, account.id);
    assert_eq!(folder.name, "Receipts");
    assert!(folder.id.ends_with("Receipts"), "{}", folder.id);

    // The provider really accepted it, including the subscribe.
    {
        let world = world.lock().expect("world");
        assert!(world.folders.iter().any(|f| f.name == folder.id));
        assert!(world.subscribed.contains(&folder.id));
    }

    // `Mail::folders` reads the local catalog, so the folder is visible without a round trip.
    let folders = engine.folders().expect("folders");
    assert_eq!(folders.len(), 1);
    assert_eq!(folders[0].id, folder.id);
    assert_eq!(folders[0].name, "Receipts");

    // A provider failure must not leave a phantom folder behind.
    server.stop();
    assert!(engine.create_folder(account.id, "Loud").is_err());
    let folders = engine.folders().expect("folders");
    assert_eq!(folders.len(), 1, "only the first folder exists");
    assert!(folders.iter().all(|f| f.name == "Receipts"));
}

#[test]
fn ai_streams_sse_chunks_in_order() {
    let (address, request) = sse_server(&[
        r#"{"choices":[{"delta":{"content":"Hel"}}]}"#,
        r#"{"choices":[{"delta":{"content":"lo, "}}]}"#,
        r#"{"choices":[{"delta":{"content":"wörld 🙂"}}]}"#,
        "[DONE]",
    ]);
    let dir = temp_dir();
    let engine = engine(&dir);
    engine.set_ai_key("secret").expect("key");
    engine
        .set_settings(Settings {
            ai_enabled: true,
            ai_base_url: format!("http://{address}"),
            ai_model: "test-model".into(),
            ..Default::default()
        })
        .expect("settings");
    let events = Events::new(engine.events());
    engine
        .ai(
            7,
            vec![ChatMsg {
                role: "user".into(),
                content: "hi".into(),
            }],
        )
        .expect("ai");

    let mut deltas = Vec::new();
    loop {
        match events.wait(|e| matches!(e, Event::Ai { req: 7, .. })) {
            Event::Ai { delta, done, .. } => {
                if done {
                    break;
                }
                deltas.push(delta);
            }
            other => panic!("unexpected event {other:?}"),
        }
    }
    assert_eq!(deltas, vec!["Hel", "lo, ", "wörld 🙂"]);
    // Settings, not constants, decide where and for which model the request goes.
    let seen = String::from_utf8_lossy(&request.lock().expect("request").clone()).to_string();
    assert!(
        seen.contains("\"model\":\"test-model\""),
        "the configured model is sent: {seen}"
    );
    assert!(
        seen.contains("authorization: Bearer secret"),
        "the keychain key is the bearer token: {seen}"
    );
}

#[test]
fn fetch_image_downloads_http_and_refuses_everything_else() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind image");
    let address = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        let Ok((mut sock, _)) = listener.accept() else {
            return;
        };
        let mut buffer = [0u8; 2048];
        let _ = sock.read(&mut buffer);
        let body = b"\x89PNG\r\n\x1a\n";
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = sock.write_all(head.as_bytes());
        let _ = sock.write_all(body);
    });
    let url = format!("http://{address}/pixel.png");
    let dir = temp_dir();
    assert!(
        engine(&dir).fetch_image(&url).is_err(),
        "mail must not make Kite fetch from the reader's own machine"
    );
    let dir = temp_dir();
    let engine = Engine::open_with(
        &dir,
        Secrets::Memory(Mutex::new(HashMap::new())),
        Options {
            local_images: true,
            ..Default::default()
        },
    )
    .expect("engine");
    let bytes = engine.fetch_image(&url).expect("image");
    assert_eq!(bytes, b"\x89PNG\r\n\x1a\n".to_vec());
    assert!(
        engine.fetch_image("file:///C:/Windows/win.ini").is_err(),
        "only http(s) is fetched for a message"
    );
}

#[test]
fn a_backend_error_is_reported_and_the_loop_recovers() {
    let rt = runtime();
    let world = Arc::new(Mutex::new(common::world()));
    world
        .lock()
        .expect("world")
        .folder("INBOX")
        .push(FIRST, &["\\Seen"]);
    let server = rt.block_on(serve(world.clone()));

    let dir = temp_dir();
    let engine = engine(&dir);
    let events = Events::new(engine.events());
    let account = engine.add_proton(login(server.ports())).expect("add");
    events.wait(|e| matches!(e, Event::Changed(ids) if ids.contains(&account.id)));

    server.stop();
    engine.sync_now();
    events.wait(|e| matches!(e, Event::Error(_)));

    server.start();
    world
        .lock()
        .expect("world")
        .folder("INBOX")
        .push(SECOND, &[]);
    engine.sync_now();
    // Each synced batch announces itself, so Changed events from the first sync may still be queued.
    while !engine
        .threads(&inbox(account.id), 0, 50)
        .expect("threads")
        .iter()
        .any(|t| t.subject == "Lunch?")
    {
        events.wait(|e| matches!(e, Event::Changed(ids) if ids.contains(&account.id)));
    }
    assert_eq!(engine.accounts().expect("accounts").len(), 1);
}

// -------------------------------------------------------------------- drafts

fn drafts(account_id: Id) -> View {
    View {
        account_id: Some(account_id),
        mailbox: Mailbox::Drafts,
        split: None,
        query: None,
        folder: None,
    }
}

fn sent(account_id: Id) -> View {
    View {
        account_id: Some(account_id),
        mailbox: Mailbox::Sent,
        split: None,
        query: None,
        folder: None,
    }
}

/// Polls the fake server's world: ops reach it asynchronously, with no event of their own.
fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

fn draft_folder(world: &common::World) -> Vec<u8> {
    world
        .folders
        .iter()
        .find(|f| f.name == "Drafts")
        .and_then(|f| f.msgs.first())
        .map(|m| m.raw.clone())
        .unwrap_or_default()
}

fn now_ms() -> Ms {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as Ms
}

#[test]
fn a_saved_draft_shows_at_once_and_is_appended_with_reply_headers() {
    let rt = runtime();
    let world = Arc::new(Mutex::new(common::world()));
    world
        .lock()
        .expect("world")
        .folder("INBOX")
        .push(FIRST, &["\\Seen"]);
    let server = rt.block_on(serve(world.clone()));

    let dir = temp_dir();
    let engine = engine(&dir);
    let events = Events::new(engine.events());
    let account = engine.add_proton(login(server.ports())).expect("add");
    events.wait(|e| matches!(e, Event::Changed(ids) if ids.contains(&account.id)));

    let inbox_thread = engine
        .threads(&inbox(account.id), 0, 10)
        .expect("threads")
        .remove(0);
    let message = engine.thread(inbox_thread.id).expect("thread").remove(0);

    let id = engine
        .save_draft(Draft {
            id: None,
            account_id: account.id,
            to: vec![addr("Ada", "ada@example.org")],
            subject: "Re: Quarterly numbers".into(),
            body: "looks good".into(),
            reply_to: Some(message.id),
            ..Default::default()
        })
        .expect("save draft");

    // Local first: the Drafts mailbox shows it before any network round trip.
    let rows = engine.threads(&drafts(account.id), 0, 10).expect("threads");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].subject, "Re: Quarterly numbers");
    assert_eq!(rows[0].participants, "Ada");
    let saved = engine.draft(rows[0].id).expect("draft").expect("saved");
    assert_eq!(saved.id, Some(id));
    assert_eq!(saved.body, "looks good");
    assert_eq!(saved.reply_to, Some(message.id));

    wait_for("the draft to reach the server", || {
        let world = world.lock().expect("world");
        !draft_folder(&world).is_empty()
    });
    let raw = String::from_utf8_lossy(&draft_folder(&world.lock().expect("world"))).to_string();
    assert!(raw.contains("In-Reply-To: <abc@example.org>"), "{raw}");
    assert!(
        raw.contains("References: <root@example.org> <parent@example.org> <abc@example.org>"),
        "{raw}"
    );
    assert!(raw.contains("Subject: Re: Quarterly numbers"), "{raw}");
    assert!(raw.contains("looks good"), "{raw}");

    // Saving again replaces the provider copy: one draft on the server, one row here.
    engine
        .save_draft(Draft {
            id: Some(id),
            body: "looks great".into(),
            ..saved
        })
        .expect("resave");
    wait_for("the replaced copy to be expunged", || {
        let world = world.lock().expect("world");
        let drafts = world
            .folders
            .iter()
            .find(|f| f.name == "Drafts")
            .expect("drafts");
        drafts.msgs.len() == 1
            && String::from_utf8_lossy(&drafts.msgs[0].raw).contains("looks great")
    });
    assert_eq!(
        engine
            .threads(&drafts(account.id), 0, 10)
            .expect("threads")
            .len(),
        1,
        "saving again never adds a row"
    );

    // The copy the server hands back must not become a second draft thread.
    engine.sync_now();
    events.wait(|e| matches!(e, Event::Changed(ids) if ids.contains(&account.id)));
    let rows = engine.threads(&drafts(account.id), 0, 10).expect("threads");
    assert_eq!(rows.len(), 1, "the synced copy dedupes: {rows:?}");
    assert_eq!(
        engine.draft(rows[0].id).expect("draft").expect("saved").id,
        Some(id)
    );
}

#[test]
fn discarding_a_draft_removes_it_locally_and_on_the_server() {
    let rt = runtime();
    let world = Arc::new(Mutex::new(common::world()));
    let server = rt.block_on(serve(world.clone()));

    let dir = temp_dir();
    let engine = engine(&dir);
    let account = engine.add_proton(login(server.ports())).expect("add");
    let id = engine
        .save_draft(Draft {
            account_id: account.id,
            to: vec![addr("Bob", "bob@example.org")],
            subject: "Scratch".into(),
            body: "never mind".into(),
            ..Default::default()
        })
        .expect("save");
    wait_for("the draft to reach the server", || {
        let world = world.lock().expect("world");
        !draft_folder(&world).is_empty()
    });
    let thread = engine.threads(&drafts(account.id), 0, 10).expect("threads")[0].id;

    engine.discard_draft(id).expect("discard");
    assert!(
        engine
            .threads(&drafts(account.id), 0, 10)
            .expect("threads")
            .is_empty(),
        "gone from the Drafts mailbox at once"
    );
    assert!(engine.draft(thread).expect("draft").is_none());
    wait_for("the provider copy to be deleted", || {
        let world = world.lock().expect("world");
        world
            .folders
            .iter()
            .find(|f| f.name == "Drafts")
            .is_some_and(|f| f.msgs.is_empty())
    });
}

#[test]
fn sending_a_saved_draft_removes_it_and_the_copy_lands_in_sent() {
    let rt = runtime();
    let world = Arc::new(Mutex::new(common::world()));
    world.lock().expect("world").files_sent_into_sent = true;
    let server = rt.block_on(serve(world.clone()));

    let dir = temp_dir();
    let engine = engine(&dir);
    engine
        .set_settings(Settings {
            undo_send_secs: 0,
            ..Default::default()
        })
        .expect("settings");
    let account = engine.add_proton(login(server.ports())).expect("add");

    engine
        .save_draft(Draft {
            account_id: account.id,
            to: vec![addr("Bob", "bob@example.org")],
            subject: "Sent from a draft".into(),
            body: "at last".into(),
            ..Default::default()
        })
        .expect("save");
    wait_for("the draft to reach the server", || {
        let world = world.lock().expect("world");
        !draft_folder(&world).is_empty()
    });
    let saved = engine
        .draft(engine.threads(&drafts(account.id), 0, 10).expect("threads")[0].id)
        .expect("draft")
        .expect("saved");
    engine.send(saved).expect("send");

    assert!(
        engine
            .threads(&drafts(account.id), 0, 10)
            .expect("threads")
            .is_empty(),
        "sending removes the draft at once"
    );
    wait_for("the message to leave SMTP", || {
        world.lock().expect("world").sent.len() == 1
    });
    wait_for("the provider's draft copy to be deleted", || {
        let world = world.lock().expect("world");
        world
            .folders
            .iter()
            .find(|f| f.name == "Drafts")
            .is_some_and(|f| f.msgs.is_empty())
    });

    // Bridge files the sent copy itself, so it appears in Sent after the next sync.
    wait_for("the sent copy to sync into Sent", || {
        !engine
            .threads(&sent(account.id), 0, 10)
            .expect("threads")
            .is_empty()
    });
    let rows = engine.threads(&sent(account.id), 0, 10).expect("threads");
    assert_eq!(
        rows.len(),
        1,
        "one conversation, no local duplicate: {rows:?}"
    );
    assert_eq!(rows[0].subject, "Sent from a draft");
}

#[test]
fn reply_all_carries_every_recipient_without_leaking_the_bcc_header() {
    let rt = runtime();
    let world = Arc::new(Mutex::new(common::world()));
    world
        .lock()
        .expect("world")
        .folder("INBOX")
        .push(FIRST, &["\\Seen"]);
    let server = rt.block_on(serve(world.clone()));

    let dir = temp_dir();
    let engine = engine(&dir);
    engine
        .set_settings(Settings {
            undo_send_secs: 0,
            ..Default::default()
        })
        .expect("settings");
    let events = Events::new(engine.events());
    let account = engine.add_proton(login(server.ports())).expect("add");
    events.wait(|e| matches!(e, Event::Changed(ids) if ids.contains(&account.id)));

    let thread = engine
        .threads(&inbox(account.id), 0, 10)
        .expect("threads")
        .remove(0);
    let message = engine.thread(thread.id).expect("thread").remove(0);
    engine
        .send(Draft {
            account_id: account.id,
            to: vec![addr("Ada", "ada@example.org")],
            cc: vec![
                addr("Bob", "bob@example.org"),
                addr("Carol", "carol@example.org"),
            ],
            bcc: vec![addr("Hidden", "hidden@example.org")],
            body: "reply all body".into(),
            reply_to: Some(message.id),
            ..Default::default()
        })
        .expect("send");

    wait_for("the reply to reach SMTP", || {
        world.lock().expect("world").sent.len() == 1
    });
    let world = world.lock().expect("world");
    let sent = &world.sent[0];
    let mut rcpts = sent.rcpts.clone();
    rcpts.sort();
    assert_eq!(
        rcpts,
        vec![
            "ada@example.org",
            "bob@example.org",
            "carol@example.org",
            "hidden@example.org"
        ],
        "the envelope carries To, Cc and Bcc"
    );
    let raw = String::from_utf8_lossy(&sent.raw).to_string();
    let header = |name: &str| -> String {
        raw.lines()
            .find(|line| line.starts_with(name))
            .unwrap_or_default()
            .to_string()
    };
    assert!(header("To: ").contains("ada@example.org"), "{raw}");
    let cc = header("Cc: ");
    assert!(
        cc.contains("bob@example.org") && cc.contains("carol@example.org"),
        "{raw}"
    );
    assert!(
        !raw.to_ascii_lowercase().contains("bcc:"),
        "a Bcc header would expose the hidden recipient: {raw}"
    );
    assert!(raw.contains("Subject: Re: Quarterly numbers"), "{raw}");
    assert!(raw.contains("In-Reply-To: <abc@example.org>"), "{raw}");
}

#[test]
fn a_scheduled_send_waits_for_its_time_and_cancels_cleanly() {
    let rt = runtime();
    let world = Arc::new(Mutex::new(common::world()));
    let server = rt.block_on(serve(world.clone()));

    let dir = temp_dir();
    let engine = engine(&dir);
    engine
        .set_settings(Settings {
            undo_send_secs: 0,
            ..Default::default()
        })
        .expect("settings");
    let account = engine.add_proton(login(server.ports())).expect("add");
    let hour = 3_600_000;

    let later = engine
        .send(Draft {
            account_id: account.id,
            to: vec![addr("Bob", "bob@example.org")],
            subject: "Later".into(),
            body: "later".into(),
            send_at: Some(now_ms() + hour),
            ..Default::default()
        })
        .expect("schedule");
    // A due send proves the outbox ran while the scheduled one stayed put.
    engine
        .send(Draft {
            account_id: account.id,
            to: vec![addr("Bob", "bob@example.org")],
            subject: "Now".into(),
            body: "now".into(),
            send_at: Some(now_ms() - hour),
            ..Default::default()
        })
        .expect("send");
    wait_for("the due message to leave", || {
        world.lock().expect("world").sent.len() == 1
    });
    assert_eq!(
        world.lock().expect("world").sent.len(),
        1,
        "the scheduled message is still waiting"
    );

    let returned = engine.cancel_send(later).expect("cancel").expect("draft");
    assert_eq!(returned.subject, "Later");
    assert!(returned.send_at.is_some_and(|at| at > now_ms()));
}

#[test]
fn a_failed_send_is_reported_and_the_mail_is_kept() {
    let rt = runtime();
    let world = Arc::new(Mutex::new(common::world()));
    let server = rt.block_on(serve(world.clone()));

    let dir = temp_dir();
    let engine = engine(&dir);
    engine
        .set_settings(Settings {
            undo_send_secs: 0,
            ..Default::default()
        })
        .expect("settings");
    let events = Events::new(engine.events());
    let account = engine.add_proton(login(server.ports())).expect("add");

    server.stop();
    let outbox = engine
        .send(Draft {
            account_id: account.id,
            to: vec![addr("Bob", "bob@example.org")],
            subject: "Must not vanish".into(),
            body: "important".into(),
            ..Default::default()
        })
        .expect("send");
    events.wait(|e| matches!(e, Event::Error(text) if text.contains("send failed")));

    let kept = engine
        .cancel_send(outbox)
        .expect("cancel")
        .expect("the failed draft is still here");
    assert_eq!(kept.subject, "Must not vanish");

    // Retrying once the server is back delivers it.
    server.start();
    engine
        .send(Draft {
            subject: "Must arrive".into(),
            ..kept
        })
        .expect("resend");
    wait_for("the retried message to leave", || {
        world.lock().expect("world").sent.len() == 1
    });
    let raw = String::from_utf8_lossy(&world.lock().expect("world").sent[0].raw).to_string();
    assert!(raw.contains("Subject: Must arrive"), "{raw}");
}

#[test]
fn a_send_without_recipients_is_rejected() {
    let rt = runtime();
    let world = Arc::new(Mutex::new(common::world()));
    let server = rt.block_on(serve(world.clone()));
    let dir = temp_dir();
    let engine = engine(&dir);
    let account = engine.add_proton(login(server.ports())).expect("add");

    assert!(
        engine
            .send(Draft {
                account_id: account.id,
                subject: "No one".into(),
                ..Default::default()
            })
            .is_err(),
        "an empty envelope must not reach the outbox"
    );
}

#[test]
fn a_forward_starts_a_new_conversation() {
    let rt = runtime();
    let world = Arc::new(Mutex::new(common::world()));
    world
        .lock()
        .expect("world")
        .folder("INBOX")
        .push(FIRST, &["\\Seen"]);
    let server = rt.block_on(serve(world.clone()));

    let dir = temp_dir();
    let engine = engine(&dir);
    engine
        .set_settings(Settings {
            undo_send_secs: 0,
            ..Default::default()
        })
        .expect("settings");
    let account = engine.add_proton(login(server.ports())).expect("add");

    // A forward carries the quoted original in the body but no threading headers, so it is not a
    // reply to the original.
    engine
        .send(Draft {
            account_id: account.id,
            to: vec![addr("Dana", "dana@example.org")],
            subject: "Fwd: Quarterly numbers".into(),
            body: "fyi\n\n> plain body".into(),
            ..Default::default()
        })
        .expect("forward");

    wait_for("the forward to reach SMTP", || {
        world.lock().expect("world").sent.len() == 1
    });
    let raw = String::from_utf8_lossy(&world.lock().expect("world").sent[0].raw).to_string();
    assert!(raw.contains("Subject: Fwd: Quarterly numbers"), "{raw}");
    assert!(!raw.contains("In-Reply-To:"), "{raw}");
    assert!(!raw.contains("References:"), "{raw}");
}

#[test]
fn a_provider_draft_can_be_trashed_and_undone_over_imap() {
    let rt = runtime();
    let world = Arc::new(Mutex::new(common::world()));
    world
        .lock()
        .expect("world")
        .folder("Drafts")
        .push(FIRST, &["\\Seen", "\\Draft"]);
    let server = rt.block_on(serve(world.clone()));
    let dir = temp_dir();
    let engine = engine(&dir);
    let account = engine.add_proton(login(server.ports())).expect("account");
    let row = {
        let mut rows = Vec::new();
        wait_for("the provider draft", || {
            engine.sync_now();
            rows = engine.threads(&drafts(account.id), 0, 10).expect("drafts");
            !rows.is_empty()
        });
        rows.remove(0)
    };
    let draft = engine.draft(row.id).expect("draft").expect("editable");
    let token = engine.act(&[row.id], &Action::Trash).expect("trash");
    assert!(
        engine
            .threads(&drafts(account.id), 0, 10)
            .expect("drafts")
            .is_empty()
    );
    wait_for("the provider trash", || {
        world
            .lock()
            .expect("world")
            .folders
            .iter()
            .any(|f| f.name == "Trash" && !f.msgs.is_empty())
    });
    engine.undo(token).expect("undo");
    wait_for("the provider draft to return", || {
        world
            .lock()
            .expect("world")
            .folders
            .iter()
            .any(|f| f.name == "Drafts" && !f.msgs.is_empty())
    });
    let rows = engine
        .threads(&drafts(account.id), 0, 10)
        .expect("restored");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        engine
            .draft(rows[0].id)
            .expect("draft")
            .expect("editable")
            .body,
        draft.body
    );
    engine
        .discard_draft(draft.id.expect("id"))
        .expect("discard after undo uses the new IMAP id");
    wait_for("the provider draft to be discarded", || {
        world
            .lock()
            .expect("world")
            .folders
            .iter()
            .all(|f| f.name != "Drafts" || f.msgs.is_empty())
    });
    assert!(
        engine
            .threads(&drafts(account.id), 0, 10)
            .expect("drafts")
            .is_empty()
    );
}
