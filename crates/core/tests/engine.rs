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

/// A minimal SSE server: answers one request with the given `data:` payloads.
fn sse_server(chunks: &[&str]) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind sse");
    let address = listener.local_addr().expect("addr").to_string();
    let payloads: Vec<String> = chunks.iter().map(|c| format!("data: {c}\n\n")).collect();
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
        let header =
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
        let _ = sock.write_all(header.as_bytes());
        for payload in &payloads {
            let _ = sock.write_all(payload.as_bytes());
        }
        let _ = sock.flush();
    });
    address
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

    let folder = engine.create_folder(account.id, "Receipts").expect("create");
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
    let address = sse_server(&[
        r#"{"choices":[{"delta":{"content":"Hel"}}]}"#,
        r#"{"choices":[{"delta":{"content":"lo, "}}]}"#,
        r#"{"choices":[{"delta":{"content":"world"}}]}"#,
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
    assert_eq!(deltas, vec!["Hel", "lo, ", "world"]);
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
    events.wait(|e| matches!(e, Event::Changed(ids) if ids.contains(&account.id)));

    let threads = engine.threads(&inbox(account.id), 0, 50).expect("threads");
    assert!(threads.iter().any(|t| t.subject == "Lunch?"));
    assert_eq!(engine.accounts().expect("accounts").len(), 1);
}
