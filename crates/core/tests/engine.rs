//! Production Mail tests with real SQLite, in-memory secrets and local HTTP servers.
#[path = "common/temp_dir.rs"]
mod temp_dir;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kite_core::api::Mail;
use kite_core::engine::{Engine, Options, Secrets};
use kite_core::model::*;
use kite_core::store::Store;

fn temp_dir() -> temp_dir::TempDir {
    temp_dir::TempDir::new("engine")
}

fn engine(dir: &Path) -> Arc<Engine> {
    Engine::open_with(
        dir,
        Secrets::Memory(Mutex::new(HashMap::new())),
        Options::default(),
    )
    .expect("engine")
}

fn local_account(dir: &Path) -> Account {
    Store::open(&dir.join("kite.db"))
        .expect("store")
        .add_account(Provider::Proton, "karl@example.com", "Karl")
        .expect("account")
}

fn addr(name: &str, email: &str) -> Addr {
    Addr {
        name: name.into(),
        email: email.into(),
    }
}

fn drafts(account: Id) -> View {
    View {
        account_id: Some(account),
        mailbox: Mailbox::Drafts,
        split: None,
        query: None,
        folder: None,
    }
}

fn now_ms() -> Ms {
    kite_core::auth::now_ms()
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

#[test]
fn ai_streams_sse_chunks_in_order() {
    let (address, request) = sse_server(&[
        r#"{"choices":[{"delta":{"content":"Hel"}}]}"#,
        r#"{"choices":[{"delta":{"content":"lo, "}}]}"#,
        r#"{"choices":[{"delta":{"content":"wörld ðŸ™‚"}}]}"#,
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
            AiRequest::Compose {
                draft: Draft {
                    subject: "hi".into(),
                    ..Draft::default()
                },
            },
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
    assert_eq!(deltas, vec!["Hel", "lo, ", "wörld ðŸ™‚"]);
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
fn a_send_without_recipients_is_rejected() {
    let dir = temp_dir();
    let engine = engine(&dir);
    let account = local_account(&dir);

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
fn invalid_recipients_never_leave_drafts_or_enter_scheduled_outbox() {
    let dir = temp_dir();
    let engine = engine(&dir);
    let account = local_account(&dir);
    let invalid = [
        addr("", ""),
        addr("", "missing-at"),
        addr("", "a..b@example.com"),
        addr("", "a@-example.com"),
        addr("", "a@example-.com"),
        addr("", "a@example..com"),
        addr("", "a@example_com.org"),
        addr("", "a@example.com\r\nBcc: injected@example.com"),
        addr("Injected\r\nBcc: other@example.com", "a@example.com"),
    ];
    for field in ["To", "Cc", "Bcc"] {
        for recipient in &invalid {
            for send_at in [None, Some(now_ms() + 3_600_000)] {
                let mut draft = Draft {
                    account_id: account.id,
                    to: vec![addr("Valid", "valid@example.org")],
                    subject: "Invalid envelope".into(),
                    send_at,
                    ..Default::default()
                };
                match field {
                    "To" => draft.to.push(recipient.clone()),
                    "Cc" => draft.cc.push(recipient.clone()),
                    _ => draft.bcc.push(recipient.clone()),
                }
                let error = engine
                    .send(draft)
                    .expect_err("invalid address must be rejected");
                assert!(error.to_string().starts_with(field), "{error}");
            }
        }
    }
    // Incomplete recipients remain editable drafts, including after a blocked send.
    let id = engine
        .save_draft(Draft {
            account_id: account.id,
            to: vec![addr("", "unfinished")],
            subject: "Keep editing".into(),
            ..Default::default()
        })
        .expect("save incomplete draft");
    let row = engine
        .threads(&drafts(account.id), 0, 10)
        .expect("drafts")
        .remove(0);
    let saved = engine.draft(row.id).expect("draft").expect("editable");
    assert_eq!(saved.id, Some(id));
    assert!(engine.send(saved.clone()).is_err());
    assert_eq!(
        engine.draft(row.id).expect("retained").expect("draft").to,
        saved.to
    );
    let store = kite_core::store::Store::open(&dir.join("kite.db")).expect("local store");
    assert!(
        store.outbox_due(i64::MAX).expect("outbox").is_empty(),
        "no immediate or future invalid send was queued"
    );
}

#[test]
fn a_legacy_bridge_account_reports_how_to_sign_in_and_can_be_removed() {
    let dir = temp_dir();
    let account = local_account(&dir);
    let credentials = serde_json::json!({
        "host": "127.0.0.1", "imap_port": 1143, "smtp_port": 1025,
        "user": account.email, "password": "old-bridge-password", "tls": true,
    });
    let engine = Engine::open_with(
        &dir,
        Secrets::Memory(Mutex::new(HashMap::from([(
            format!("proton:{}", account.id),
            credentials.to_string(),
        )]))),
        Options::default(),
    )
    .expect("engine");
    // The synchronous account operation checks the same backend selection as sync.
    let error = engine
        .create_folder(account.id, "Projects")
        .expect_err("Bridge is gone");
    assert!(
        error
            .to_string()
            .contains("This Proton account used Bridge; sign in again in Settings › Connections"),
        "{error:#}"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let errors = std::fs::read_to_string(dir.join("errors.log")).unwrap_or_default();
        if errors
            .contains("This Proton account used Bridge; sign in again in Settings › Connections")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "startup must report the account error: {errors}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(engine.accounts().expect("accounts").len(), 1);
    engine
        .remove_account(account.id)
        .expect("remove old account");
    assert!(engine.accounts().expect("accounts").is_empty());
}

#[test]
fn scheduled_and_undo_send_can_be_cancelled_back_to_editable_drafts() {
    let dir = temp_dir();
    let account = local_account(&dir);
    let engine = engine(&dir);
    engine
        .set_settings(Settings {
            undo_send_secs: 30,
            ..Settings::default()
        })
        .unwrap();
    for send_at in [None, Some(now_ms() + 3_600_000)] {
        let id = engine
            .save_draft(Draft {
                account_id: account.id,
                to: vec![addr("Bob", "bob@example.org")],
                subject: "Keep editing".into(),
                send_at,
                ..Draft::default()
            })
            .unwrap();
        let row = engine
            .threads(&drafts(account.id), 0, 10)
            .unwrap()
            .remove(0);
        let draft = engine.draft(row.id).unwrap().unwrap();
        let queued = engine.send(draft).unwrap();
        assert!(
            engine
                .threads(&drafts(account.id), 0, 10)
                .unwrap()
                .is_empty()
        );
        let returned = engine.cancel_send(queued).unwrap().unwrap();
        assert_eq!(returned.id, Some(id));
        assert_eq!(returned.subject, "Keep editing");
        assert_eq!(returned.send_at, None);
        let row = engine
            .threads(&drafts(account.id), 0, 10)
            .unwrap()
            .remove(0);
        assert_eq!(engine.draft(row.id).unwrap().unwrap().id, Some(id));
        engine.discard_draft(id).unwrap();
    }
}

fn newsletter(id: &str, header: &str) -> kite_core::sync::RemoteMsg {
    serde_json::from_value(serde_json::json!({
        "remote_id":id,"folder":"inbox","mailbox":"inbox","thread_hint":null,
        "message_id":format!("<{id}@newsletter.example>"),"in_reply_to":null,"references":[],
        "from":{"name":"News","email":"news@newsletter.example"},"to":[],"cc":[],
        "date":now_ms(),"subject":id,"text":"News","html":null,"unread":true,"starred":false,
        "important":null,"list_unsubscribe":true,"unsubscribe_header":header,
        "unsubscribe_post":"List-Unsubscribe=One-Click","attachments":[]
    }))
    .unwrap()
}

#[test]
fn mailto_unsubscribe_uses_receiving_account_archives_exact_sender_undo_and_future_arrivals() {
    use kite_core::sync::Delta;
    let dir = temp_dir();
    let account = local_account(&dir);
    let store = Store::open(&dir.join("kite.db")).unwrap();
    let header = "<mailto:leave@newsletter.example?subject=Stop%20mail&body=Remove%20me>";
    store
        .apply_deltas(
            account.id,
            &[
                Delta::Upsert(Box::new(newsletter("first", header))),
                Delta::Upsert(Box::new(newsletter("second", header))),
            ],
        )
        .unwrap();
    let other = store
        .add_account(Provider::Proton, "other@example.com", "Other")
        .unwrap();
    store
        .apply_deltas(
            other.id,
            &[Delta::Upsert(Box::new(newsletter("other-account", header)))],
        )
        .unwrap();
    let engine = engine(&dir);
    let inbox = View {
        account_id: Some(account.id),
        mailbox: Mailbox::Inbox,
        split: None,
        query: None,
        folder: None,
    };
    let thread = engine.threads(&inbox, 0, 10).unwrap()[0].id;
    let result = engine.unsubscribe(thread).unwrap();
    assert_eq!(result.archived, 2);
    assert!(engine.threads(&inbox, 0, 10).unwrap().is_empty());
    assert_eq!(
        engine
            .threads(
                &View {
                    account_id: Some(other.id),
                    ..inbox.clone()
                },
                0,
                10
            )
            .unwrap()
            .len(),
        1
    );
    let pending = store.outbox_due(i64::MAX).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].1.account_id, account.id);
    assert_eq!(pending[0].1.to[0].email, "leave@newsletter.example");
    assert_eq!(pending[0].1.subject, "Stop mail");
    assert_eq!(pending[0].1.body, "Remove me");
    engine.undo(result.undo).unwrap();
    assert_eq!(engine.threads(&inbox, 0, 10).unwrap().len(), 2);
    drop(store);
    let store = Store::open(&dir.join("kite.db")).unwrap();
    store
        .apply_deltas(
            account.id,
            &[Delta::Upsert(Box::new(newsletter("third", header)))],
        )
        .unwrap();
    assert_eq!(
        engine.threads(&inbox, 0, 10).unwrap().len(),
        2,
        "new mail auto-archives while undo restores previous conversations"
    );
    assert_eq!(
        engine
            .threads(
                &View {
                    mailbox: Mailbox::Archive,
                    ..inbox
                },
                0,
                10
            )
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn unsubscribe_refuses_private_addresses_before_archiving() {
    use kite_core::sync::Delta;
    for url in [
        "https://127.0.0.1/unsubscribe",
        "https://10.0.0.1/unsubscribe",
        "https://[::ffff:127.0.0.1]/unsubscribe",
        "http://example.com/unsubscribe",
    ] {
        let dir = temp_dir();
        let account = local_account(&dir);
        let store = Store::open(&dir.join("kite.db")).unwrap();
        store
            .apply_deltas(
                account.id,
                &[Delta::Upsert(Box::new(newsletter(
                    "private",
                    &format!("<{url}>"),
                )))],
            )
            .unwrap();
        let engine = engine(&dir);
        let inbox = View {
            account_id: Some(account.id),
            mailbox: Mailbox::Inbox,
            split: None,
            query: None,
            folder: None,
        };
        let thread = engine.threads(&inbox, 0, 10).unwrap()[0].id;
        assert!(engine.unsubscribe(thread).is_err(), "must refuse {url}");
        assert_eq!(engine.threads(&inbox, 0, 10).unwrap().len(), 1);
    }
}

#[test]
fn one_click_unsubscribe_posts_rfc8058_body_over_https() {
    use kite_core::sync::Delta;
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    let dir = temp_dir();
    let openssl = if cfg!(windows) {
        "C:/Program Files/Git/usr/bin/openssl.exe"
    } else {
        "openssl"
    };
    let cert = dir.join("cert.pem");
    let key = dir.join("key.pem");
    assert!(
        Command::new(openssl)
            .args([
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-days",
                "1",
                "-subj",
                "/CN=localhost",
                "-keyout"
            ])
            .arg(&key)
            .arg("-out")
            .arg(&cert)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut child = Command::new(openssl)
        .args(["s_server", "-quiet", "-naccept", "1", "-accept"])
        .arg(format!("127.0.0.1:{port}"))
        .arg("-cert")
        .arg(&cert)
        .arg("-key")
        .arg(&key)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut header = String::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 {
                break;
            }
            header.push_str(&line);
            if line == "\r\n" {
                break;
            }
        }
        let length = header
            .lines()
            .find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(str::to_owned)
            })
            .unwrap()
            .trim()
            .parse::<usize>()
            .unwrap();
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        stdin
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
        stdin.flush().unwrap();
        tx.send((header, body)).unwrap();
    });
    // Wait for the helper to bind without consuming its single TLS connection.
    std::thread::sleep(Duration::from_millis(200));
    let account = local_account(&dir);
    let store = Store::open(&dir.join("kite.db")).unwrap();
    store
        .apply_deltas(
            account.id,
            &[Delta::Upsert(Box::new(newsletter(
                "https",
                &format!("<https://127.0.0.1:{port}/leave>"),
            )))],
        )
        .unwrap();
    let engine = Engine::open_with(
        &dir,
        Secrets::Memory(Mutex::new(HashMap::new())),
        Options {
            local_images: true,
            ..Default::default()
        },
    )
    .unwrap();
    let inbox = View {
        account_id: Some(account.id),
        mailbox: Mailbox::Inbox,
        split: None,
        query: None,
        folder: None,
    };
    let thread = engine.threads(&inbox, 0, 10).unwrap()[0].id;
    let result = engine.unsubscribe(thread);
    let request = rx.recv_timeout(Duration::from_secs(2));
    let _ = child.kill();
    let _ = child.wait();
    let result = result.unwrap();
    let (header, body) = request.unwrap();
    assert!(header.starts_with("POST /leave HTTP/1.1"));
    assert!(
        header
            .to_ascii_lowercase()
            .contains("content-type: application/x-www-form-urlencoded")
    );
    assert_eq!(body, b"List-Unsubscribe=One-Click");
    assert_eq!(result.archived, 1);
    assert!(
        store.outbox_due(i64::MAX).unwrap().is_empty(),
        "one-click never sends an email"
    );
    assert!(engine.threads(&inbox, 0, 10).unwrap().is_empty());
}

#[test]
fn proton_schedule_reschedule_send_now_and_cancel_keep_a_recoverable_draft() {
    let dir = temp_dir();
    let account = local_account(&dir);
    let engine = engine(&dir);
    engine
        .set_settings(Settings {
            undo_send_secs: 30,
            ..Default::default()
        })
        .unwrap();
    let scheduled = |mailbox| kite_core::model::View {
        account_id: None,
        mailbox,
        split: None,
        query: None,
        folder: None,
    };
    let id = engine
        .send(Draft {
            account_id: account.id,
            to: vec![addr("Bob", "bob@example.org")],
            subject: "Scheduled locally".into(),
            body: "Keep me".into(),
            send_at: Some(now_ms() + 3_600_000),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        engine
            .threads(&scheduled(Mailbox::Scheduled), 0, 20)
            .unwrap()
            .len(),
        1
    );
    let at = now_ms() + 7_200_000;
    let next = engine.reschedule_send(id, Some(at)).unwrap().unwrap();
    let rows = engine
        .threads(&scheduled(Mailbox::Scheduled), 0, 20)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].date, at);
    let immediate = engine.reschedule_send(next, None).unwrap().unwrap();
    assert!(
        engine
            .threads(&scheduled(Mailbox::Scheduled), 0, 20)
            .unwrap()
            .is_empty()
    );
    let draft = engine.cancel_send(immediate).unwrap().unwrap();
    assert!(draft.id.is_some());
    assert_eq!(draft.send_at, None);
    assert_eq!(draft.body, "Keep me");
    assert_eq!(
        engine
            .threads(&scheduled(Mailbox::Drafts), 0, 20)
            .unwrap()
            .len(),
        1
    );
}
