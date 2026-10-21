//! The AI request as an OpenAI-compatible endpoint receives it, against a local fake.
#[path = "common/temp_dir.rs"]
mod temp_dir;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Mutex;
use std::time::Duration;

use kite_core::api::Mail;
use kite_core::engine::{Engine, Options, Secrets};
use kite_core::model::*;
use kite_core::store::Store;

/// Answers one streamed completion and returns the request's lower-cased header lines.
fn serve_one(listener: &TcpListener) -> Vec<String> {
    let (stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let line = line.trim_end().to_ascii_lowercase();
        if line.is_empty() {
            break;
        }
        headers.push(line);
    }
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\ndata: [DONE]\n\n";
    let mut stream = stream;
    write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    headers
}

fn header<'a>(headers: &'a [String], name: &str) -> &'a str {
    headers
        .iter()
        .find_map(|line| line.strip_prefix(&format!("{name}: ")))
        .unwrap_or_else(|| panic!("no {name} header in {headers:?}"))
}

#[test]
fn summaries_send_a_stable_per_conversation_session_and_kites_user_agent() {
    let dir = temp_dir::TempDir::new("ai-session");
    let store = Store::open(&dir.join("kite.db")).unwrap();
    let account = store
        .add_account(Provider::Proton, "me@example.com", "Me")
        .unwrap();
    for subject in ["First", "Second"] {
        let draft = Draft {
            account_id: account.id,
            subject: subject.into(),
            body: "Body".into(),
            ..Draft::default()
        };
        store
            .save_draft(&draft, &format!("{subject}@example.com"))
            .unwrap();
    }
    let view = View {
        account_id: Some(account.id),
        mailbox: Mailbox::Drafts,
        split: None,
        query: None,
        folder: None,
    };
    let threads: Vec<Id> = store
        .threads(&view, 0, 10)
        .unwrap()
        .iter()
        .map(|t| t.id)
        .collect();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
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

    let mut sessions = Vec::new();
    for (req, thread) in [(1, threads[0]), (2, threads[0]), (3, threads[1])] {
        engine.ai(req, AiRequest::Summary { thread }).unwrap();
        let headers = serve_one(&listener);
        assert!(
            header(&headers, "user-agent").starts_with("kite-mail/"),
            "{headers:?}"
        );
        sessions.push(header(&headers, "x-opencode-session").to_string());
    }
    assert!(!sessions[0].is_empty());
    assert_eq!(
        sessions[0], sessions[1],
        "one conversation keeps its session"
    );
    assert_ne!(
        sessions[0], sessions[2],
        "each conversation has its own session"
    );
    drop(engine);
}

type CapturedRequests = Vec<(Vec<String>, String)>;

fn fake(
    responses: Vec<(&'static str, u16, String)>,
) -> (String, std::thread::JoinHandle<CapturedRequests>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let response_base = base.clone();
    let worker = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for (path, status, response) in responses {
            let response = response.replace("__FAKE_BASE__", &response_base);
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "no request for {path}"
                        );
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("accept: {e}"),
                }
            };
            // Windows accepted sockets can inherit the listener's nonblocking mode.
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut headers = Vec::new();
            loop {
                let mut line = String::new();
                assert_ne!(reader.read_line(&mut line).unwrap(), 0);
                if line.trim().is_empty() {
                    break;
                }
                headers.push(line.trim_end().to_ascii_lowercase());
            }
            assert_eq!(headers[0].split_whitespace().nth(1), Some(path));
            let length = headers
                .iter()
                .find_map(|h| h.strip_prefix("content-length: "))
                .map(|s| s.parse::<usize>().unwrap())
                .unwrap_or(0);
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            requests.push((headers, String::from_utf8(body).unwrap()));
            write!(stream, "HTTP/1.1 {status} Fake\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response}", response.len()).unwrap();
        }
        requests
    });
    (base, worker)
}

fn model_engine(
    dir: &temp_dir::TempDir,
    base: String,
    host: Option<&str>,
) -> std::sync::Arc<Engine> {
    let engine = Engine::open_with(
        dir,
        Secrets::Memory(Mutex::new(HashMap::new())),
        Options {
            ai_provider_host: host.map(str::to_owned),
            ..Options::default()
        },
    )
    .unwrap();
    engine
        .set_settings(Settings {
            ai_enabled: true,
            ai_base_url: base,
            ai_model: "author/free:free".into(),
            ..Settings::default()
        })
        .unwrap();
    engine
}

#[test]
fn openrouter_offers_only_free_zdr_endpoints_and_enforces_private_free_routing() {
    let models = serde_json::json!({"data": [
        {"id":"author/paid", "pricing":{"prompt":"1", "completion":"1"}},
        {"id":"author/retaining:free"},
        {"id":"author/paid-zdr:free"},
        {"id":"author/free:free", "name":"Z free"},
        {"id":"author/zero", "name":"A zero", "pricing":{"prompt":"0.0", "completion":"0"}}
    ]});
    let zdr = serde_json::json!({"data": [
        {"model_id":"author/paid", "pricing":{"prompt":"1", "completion":"1"}},
        {"model_id":"author/paid-zdr:free", "pricing":{"prompt":"0", "completion":"1"}},
        {"model_id":"author/free:free", "pricing":{"prompt":"0", "completion":"0"}},
        {"model_id":"author/zero", "pricing":{"prompt":"0", "completion":"0"}}
    ]});
    let (base, worker) = fake(vec![
        ("/api/v1/models", 200, models.to_string()),
        ("/api/v1/endpoints/zdr", 200, zdr.to_string()),
        (
            "/api/v1/chat/completions",
            200,
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\ndata: [DONE]\n\n".into(),
        ),
    ]);
    let dir = temp_dir::TempDir::new("ai-openrouter");
    let engine = model_engine(&dir, format!("{base}/api/v1"), Some("openrouter.ai"));
    engine.set_ai_key("test-only").unwrap();
    let models = engine.ai_models().unwrap();
    assert_eq!(
        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["author/zero", "author/free:free"]
    );
    assert!(models.iter().all(|m| m.free && m.zdr));
    let events = engine.events();
    engine
        .ai(
            7,
            AiRequest::Compose {
                draft: Draft {
                    subject: "Hello".into(),
                    ..Draft::default()
                },
            },
        )
        .unwrap();
    let requests = worker.join().unwrap();
    for (headers, _) in &requests {
        assert_eq!(header(headers, "authorization"), "bearer test-only");
        assert!(header(headers, "user-agent").starts_with("kite-mail/"));
    }
    let body: serde_json::Value = serde_json::from_str(&requests[2].1).unwrap();
    assert_eq!(body["provider"]["zdr"], true);
    assert_eq!(body["provider"]["data_collection"], "deny");
    assert_eq!(
        body["provider"]["max_price"],
        serde_json::json!({"prompt":0,"completion":0,"request":0})
    );
    assert!(body["session_id"].as_str().unwrap().starts_with("kite-"));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut text = String::new();
    loop {
        assert!(std::time::Instant::now() < deadline, "AI never completed");
        match events.try_recv() {
            Ok(Event::Ai {
                req: 7,
                delta,
                done,
            }) => {
                text.push_str(&delta);
                if done {
                    break;
                }
            }
            Ok(Event::Error(e)) => panic!("AI failed: {e}"),
            _ => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    assert_eq!(text, "Hi");
}

#[test]
fn opencode_go_curated_list_excludes_beam_and_withdrawn_models() {
    for available in [true, false] {
        let mut data = vec![
            serde_json::json!({"id":"minimax-m3.1-flash-free"}),
            serde_json::json!({"id":"beam-free"}),
            serde_json::json!({"id":"glm-5.3-flash-free"}),
        ];
        if available {
            data.push(serde_json::json!({"id":"longcat-2.5-preview-free"}));
        }
        let (base, worker) = fake(vec![(
            "/zen/go/v1/models",
            200,
            serde_json::json!({"data":data}).to_string(),
        )]);
        let dir = temp_dir::TempDir::new("ai-opencode-go");
        let engine = model_engine(&dir, format!("{base}/zen/go/v1/"), Some("opencode.ai"));
        let models = engine.ai_models().unwrap();
        let expected = if available {
            vec!["longcat-2.5-preview-free", "minimax-m3.1-flash-free"]
        } else {
            vec!["minimax-m3.1-flash-free"]
        };
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            expected
        );
        assert!(models.iter().all(|m| m.free && m.zdr));
        let requests = worker.join().unwrap();
        assert!(
            !requests[0]
                .0
                .iter()
                .any(|h| h.starts_with("authorization:"))
        );
    }
}

#[test]
fn opencode_missing_models_api_uses_curated_zen_list_without_beam() {
    let (base, worker) = fake(vec![("/zen/v1/models", 404, "{}".into())]);
    let dir = temp_dir::TempDir::new("ai-opencode-zen");
    let engine = model_engine(&dir, format!("{base}/zen/v1"), Some("opencode.ai"));
    let models = engine.ai_models().unwrap();
    assert_eq!(
        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        [
            "deepseek-v4.1-flash-free",
            "glm-5.3-flash-free",
            "longcat-2.5-preview-free",
            "minimax-m3.1-flash-free"
        ]
    );
    worker.join().unwrap();
}

#[test]
fn model_catalogue_errors_are_reported_instead_of_claiming_zdr() {
    for (status, response) in [(401, "{}"), (500, "{}"), (200, "{\"models\":[]}")] {
        let (base, worker) = fake(vec![("/zen/go/v1/models", status, response.into())]);
        let dir = temp_dir::TempDir::new("ai-models-error");
        let engine = model_engine(&dir, format!("{base}/zen/go/v1"), Some("opencode.ai"));
        assert!(
            engine
                .ai_models()
                .unwrap_err()
                .to_string()
                .contains("catalogue")
        );
        worker.join().unwrap();
    }
}

#[test]
fn remote_custom_endpoint_has_no_verified_models() {
    let dir = temp_dir::TempDir::new("ai-custom");
    let engine = model_engine(
        &dir,
        "https://example.com/openrouter.ai/zen/go/v1".into(),
        None,
    );
    assert!(engine.ai_models().unwrap().is_empty());
}

fn completion(text: &str) -> String {
    format!(
        "data: {}\n\ndata: [DONE]\n\n",
        serde_json::json!({"choices":[{"delta":{"content":text}}]})
    )
}

fn inbox_message(id: &str, subject: &str, text: &str, date: Ms) -> kite_core::sync::RemoteMsg {
    kite_core::sync::RemoteMsg {
        remote_id: id.into(),
        folder: "inbox".into(),
        mailbox: Mailbox::Inbox,
        thread_hint: None,
        message_id: Some(format!("<{id}@example.com>")),
        in_reply_to: None,
        references: vec![],
        from: Addr {
            name: "Alice".into(),
            email: "alice@example.com".into(),
        },
        to: vec![],
        cc: vec![],
        date,
        subject: subject.into(),
        text: text.into(),
        html: None,
        unread: true,
        starred: false,
        important: None,
        list_unsubscribe: false,
        unsubscribe_header: String::new(),
        unsubscribe_post: String::new(),
        attachments: vec![],
    }
}

fn seed(store: &Store, account: Id, id: &str, subject: &str, text: &str, date: Ms) -> Id {
    store
        .apply_deltas(
            account,
            &[kite_core::sync::Delta::Upsert(Box::new(inbox_message(
                id, subject, text, date,
            )))],
        )
        .unwrap();
    store
        .threads(
            &View {
                account_id: Some(account),
                mailbox: Mailbox::Inbox,
                split: None,
                query: Some(format!("subject:\"{subject}\"")),
                folder: None,
            },
            0,
            20,
        )
        .unwrap()[0]
        .id
}

fn answer(events: &async_channel::Receiver<Event>, req: u32) -> (String, Option<Vec<Id>>) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut text = String::new();
    let mut sources = None;
    loop {
        assert!(std::time::Instant::now() < deadline, "AI did not finish");
        match events.try_recv() {
            Ok(Event::AiSources { req: id, threads }) if id == req => {
                assert!(text.is_empty(), "sources must precede the answer");
                sources = Some(threads);
            }
            Ok(Event::Ai {
                req: id,
                delta,
                done,
            }) if id == req => {
                text.push_str(&delta);
                if done {
                    return (text, sources);
                }
            }
            Ok(Event::Error(error)) if error.starts_with("AI:") => panic!("{error}"),
            _ => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

#[test]
fn ask_retrieves_public_sources_in_citation_order_and_streams_without_a_key() {
    let dir = temp_dir::TempDir::new("ai-ask");
    let store = Store::open(&dir.join("kite.db")).unwrap();
    let account = store
        .add_account(Provider::Proton, "me@example.com", "Me")
        .unwrap();
    let other = store
        .add_account(Provider::Proton, "other@example.com", "Other")
        .unwrap();
    let public = seed(
        &store,
        account.id,
        "public",
        "Orion delivery",
        "Orion will arrive on Friday. Public delivery details.",
        100,
    );
    let second = seed(
        &store,
        account.id,
        "second",
        "Orion update",
        "Alice confirmed the shipment.",
        200,
    );
    let private = seed(
        &store,
        account.id,
        "private",
        "Orion Orion Orion delivery",
        "SECRET Orion Orion delivery Friday",
        300,
    );
    seed(
        &store,
        other.id,
        "other",
        "Orion overseas",
        "OTHER ACCOUNT CONTENT",
        400,
    );
    store
        .act(
            &[private],
            &Action::Label {
                tag: PRIVATE_TAG.into(),
            },
        )
        .unwrap();
    let expected = store
        .ai_search("When will Orion delivery arrive?", Some(account.id))
        .unwrap();
    assert_eq!(expected.len(), 2);
    assert!(expected.contains(&public));
    assert!(expected.contains(&second));
    let (base, worker) = fake(vec![
        ("/chat/completions", 200, completion("Friday [1].")),
        ("/chat/completions", 200, completion("Friday [1].")),
    ]);
    let engine = model_engine(&dir, base, None);
    let events = engine.events();
    for req in [11, 12] {
        engine
            .ai(
                req,
                AiRequest::Ask {
                    question: "When will Orion delivery arrive?".into(),
                    account: Some(account.id),
                },
            )
            .unwrap();
        let (text, sources) = answer(&events, req);
        assert_eq!(text, "Friday [1].");
        assert_eq!(sources, Some(expected.clone()));
    }
    let requests = worker.join().unwrap();
    assert_ne!(
        header(&requests[0].0, "x-opencode-session"),
        header(&requests[1].0, "x-opencode-session")
    );
    for (headers, body) in requests {
        assert!(!headers.iter().any(|h| h.starts_with("authorization:")));
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        let prompt = value["messages"][1]["content"].as_str().unwrap();
        assert!(prompt.contains("Public delivery details"));
        assert!(!prompt.contains("SECRET"));
        assert!(!prompt.contains("OTHER ACCOUNT"));
        assert!(prompt.chars().count() <= 12000);
        let first_subject = &store.thread(expected[0]).unwrap()[0].subject;
        let second_subject = &store.thread(expected[1]).unwrap()[0].subject;
        assert!(prompt.find(first_subject).unwrap() < prompt.find(second_subject).unwrap());
    }
}

#[test]
fn local_models_are_free_zdr_and_non_loopback_http_is_refused() {
    let (base, worker) = fake(vec![(
        "/v1/models",
        200,
        r#"{"data":[{"id":"local-paid-name","name":"Local"},{"id":"llama"}]}"#.into(),
    )]);
    let dir = temp_dir::TempDir::new("ai-local");
    let engine = model_engine(&dir, format!("{base}/v1"), None);
    let models = engine.ai_models().unwrap();
    assert_eq!(models.len(), 2);
    assert!(models.iter().all(|m| m.free && m.zdr));
    assert!(
        !worker.join().unwrap()[0]
            .0
            .iter()
            .any(|h| h.starts_with("authorization:"))
    );
    engine
        .set_settings(Settings {
            ai_enabled: true,
            ai_base_url: "http://example.com/v1".into(),
            ..Settings::default()
        })
        .unwrap();
    assert!(
        engine
            .ai_models()
            .unwrap_err()
            .to_string()
            .contains("HTTPS")
    );
    assert!(
        engine
            .ai(
                1,
                AiRequest::Compose {
                    draft: Draft::default()
                }
            )
            .unwrap_err()
            .to_string()
            .contains("HTTPS")
    );
}

#[test]
fn ollama_falls_back_to_root_tags() {
    let (base, worker) = fake(vec![
        ("/v1/models", 404, "{}".into()),
        (
            "/api/tags",
            200,
            r#"{"models":[{"name":"llama:latest"}]}"#.into(),
        ),
    ]);
    let dir = temp_dir::TempDir::new("ai-ollama");
    let engine = model_engine(&dir, format!("{base}/v1"), None);
    let models = engine.ai_models().unwrap();
    assert_eq!(models[0].id, "llama:latest");
    assert!(models[0].free && models[0].zdr);
    worker.join().unwrap();
}

#[test]
fn replies_include_thread_and_refuse_private_before_request() {
    let dir = temp_dir::TempDir::new("ai-replies");
    let store = Store::open(&dir.join("kite.db")).unwrap();
    let account = store
        .add_account(Provider::Proton, "me@example.com", "Me")
        .unwrap();
    let thread = seed(
        &store,
        account.id,
        "reply",
        "Meeting",
        "Can we meet on Friday?",
        100,
    );
    let (base, worker) = fake(vec![(
        "/chat/completions",
        200,
        completion("1. Friday works.\n2. What time?\n3. Could we meet Monday?"),
    )]);
    let engine = model_engine(&dir, base, None);
    let events = engine.events();
    engine.ai(5, AiRequest::Replies { thread }).unwrap();
    let (text, _) = answer(&events, 5);
    assert_eq!(text.lines().count(), 3);
    for (index, line) in text.lines().enumerate() {
        assert!(line.starts_with(&format!("{}. ", index + 1)));
        assert!(line.split_whitespace().count() < 22);
    }
    let requests = worker.join().unwrap();
    assert!(requests[0].1.contains("Can we meet on Friday?"));
    assert!(requests[0].1.contains("language and register"));
    store
        .act(
            &[thread],
            &Action::Label {
                tag: PRIVATE_TAG.into(),
            },
        )
        .unwrap();
    assert!(
        engine
            .ai(6, AiRequest::Replies { thread })
            .unwrap_err()
            .to_string()
            .contains("private")
    );
}

#[test]
fn triage_after_sync_tags_new_inbox_excludes_private_and_marked_and_ignores_invalid_labels() {
    let dir = temp_dir::TempDir::new("ai-triage");
    let store = Store::open(&dir.join("kite.db")).unwrap();
    let account = store
        .add_account(Provider::Microsoft, "me@example.com", "Me")
        .unwrap();
    let private = seed(
        &store,
        account.id,
        "private-triage",
        "Protected",
        "PRIVATE TRIAGE CONTENT",
        100,
    );
    store
        .act(
            &[private],
            &Action::Label {
                tag: PRIVATE_TAG.into(),
            },
        )
        .unwrap();
    let marked = seed(
        &store,
        account.id,
        "marked",
        "Already triaged",
        "ALREADY TRIAGED CONTENT",
        100,
    );
    assert!(store.claim_ai_triage(marked).unwrap());
    let graph_message = |id: &str, subject: &str, body: &str, parent: Option<&str>, date: &str| {
        let mut headers =
            vec![serde_json::json!({"name":"Message-ID", "value":format!("<{id}@example.com>")})];
        if let Some(parent) = parent {
            headers.push(serde_json::json!({"name":"In-Reply-To", "value":format!("<{parent}@example.com>")}));
        }
        serde_json::json!({"id":id,"internetMessageId":format!("<{id}@example.com>"),"subject":subject,
            "from":{"emailAddress":{"name":"Alice","address":"alice@example.com"}},"toRecipients":[],"ccRecipients":[],
            "receivedDateTime":date,"isRead":false,"flag":{"flagStatus":"notFlagged"},"importance":"normal",
            "hasAttachments":false,"body":{"contentType":"text","content":body},"internetMessageHeaders":headers})
    };
    let arrivals = vec![
        graph_message(
            "valid",
            "Fresh actionable mail",
            "Please confirm lunch.",
            None,
            "2026-10-21T12:00:00Z",
        ),
        graph_message(
            "invalid",
            "Fresh invalid mail",
            "Unclassifiable.",
            None,
            "2026-10-21T11:00:00Z",
        ),
        graph_message(
            "private-reply",
            "Re: Protected",
            "PRIVATE TRIAGE NEW CONTENT",
            Some("private-triage"),
            "2026-10-21T10:00:00Z",
        ),
        graph_message(
            "marked-reply",
            "Re: Already triaged",
            "ALREADY TRIAGED NEW CONTENT",
            Some("marked"),
            "2026-10-21T09:00:00Z",
        ),
    ];
    let folders = [
        "inbox",
        "sentitems",
        "drafts",
        "archive",
        "deleteditems",
        "junkemail",
    ];
    let responses = folders.iter().map(|folder| {
        let path: &'static str = match *folder { "inbox" => "/inbox", "sentitems" => "/sentitems", "drafts" => "/drafts", "archive" => "/archive", "deleteditems" => "/deleteditems", _ => "/junkemail" };
        (path, 200, serde_json::json!({"value": if *folder == "inbox" { arrivals.clone() } else { vec![] }, "@odata.deltaLink":format!("__FAKE_BASE__/{folder}")}).to_string())
    }).chain(std::iter::once(("/me/photo/$value", 404, "{}".into()))).collect();
    let (graph, graph_worker) = fake(responses);
    let mut cursor = serde_json::Map::new();
    cursor.insert("__kite_schema".into(), "2".into());
    for folder in folders {
        cursor.insert(folder.into(), format!("{graph}/{folder}").into());
    }
    store
        .set_cursor(account.id, &serde_json::Value::Object(cursor).to_string())
        .unwrap();
    let (base, worker) = fake(vec![
        (
            "/chat/completions",
            200,
            serde_json::json!({"choices":[{"message":{"content":"Needs reply"}}]}).to_string(),
        ),
        (
            "/chat/completions",
            200,
            serde_json::json!({"choices":[{"message":{"content":"Not a valid label"}}]})
                .to_string(),
        ),
    ]);
    store
        .set_settings(Settings {
            ai_enabled: true,
            ai_triage: true,
            ai_base_url: base,
            ai_model: "local".into(),
            ..Settings::default()
        })
        .unwrap();
    let tokens = kite_core::auth::Tokens {
        access: "token".into(),
        refresh: "refresh".into(),
        expires_at: kite_core::auth::now_ms() + 3600000,
    };
    let engine = Engine::open_with(
        &dir,
        Secrets::Memory(Mutex::new(HashMap::from([(
            format!("ms:{}", account.id),
            serde_json::to_string(&tokens).unwrap(),
        )]))),
        Options {
            graph_base_url: Some(graph),
            poll: Duration::from_secs(3600),
            ..Options::default()
        },
    )
    .unwrap();
    let requests = worker.join().unwrap();
    graph_worker.join().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].1.contains("Please confirm lunch"));
    assert!(requests[1].1.contains("Unclassifiable"));
    for (_, body) in &requests {
        assert!(!body.contains("PRIVATE TRIAGE"));
        assert!(!body.contains("ALREADY TRIAGED"));
    }
    let view = View {
        account_id: Some(account.id),
        mailbox: Mailbox::Inbox,
        split: None,
        query: Some("tag:\"Needs reply\"".into()),
        folder: None,
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if store.threads(&view, 0, 20).unwrap().len() == 1
            && std::fs::read_to_string(dir.join("errors.log"))
                .is_ok_and(|s| s.contains("Invalid AI triage label"))
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "triage did not persist label or log invalid response"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        store.threads(&view, 0, 20).unwrap()[0].subject,
        "Fresh actionable mail"
    );
    assert!(
        !store
            .tags()
            .unwrap()
            .iter()
            .any(|t| t.name == "Not a valid label")
    );
    let fresh_invalid = store
        .threads(
            &View {
                query: Some("subject:\"Fresh invalid mail\"".into()),
                ..view.clone()
            },
            0,
            10,
        )
        .unwrap()[0]
        .id;
    assert!(!store.claim_ai_triage(fresh_invalid).unwrap());
    assert_eq!(
        store.ai_triage_candidates(account.id, 0).unwrap(),
        Vec::<Id>::new()
    );
    drop(engine);
    let reopened = Store::open(&dir.join("kite.db")).unwrap();
    assert!(
        reopened
            .ai_triage_candidates(account.id, 0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn malformed_reply_suggestions_emit_no_partial_suggestions() {
    let dir = temp_dir::TempDir::new("ai-invalid-replies");
    let store = Store::open(&dir.join("kite.db")).unwrap();
    let account = store
        .add_account(Provider::Proton, "me@example.com", "Me")
        .unwrap();
    let thread = seed(
        &store,
        account.id,
        "invalid-reply",
        "Meeting",
        "Can we meet?",
        100,
    );
    let (base, worker) = fake(vec![(
        "/chat/completions",
        200,
        completion("1. Only one suggestion."),
    )]);
    let engine = model_engine(&dir, base, None);
    let events = engine.events();
    engine.ai(1, AiRequest::Replies { thread }).unwrap();
    worker.join().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        assert!(std::time::Instant::now() < deadline);
        match events.try_recv() {
            Ok(Event::Ai { delta, .. }) => {
                assert!(delta.is_empty(), "invalid suggestions reached the user")
            }
            Ok(Event::Error(error)) if error.starts_with("AI:") => {
                assert!(error.contains("exactly three"));
                break;
            }
            _ => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

#[test]
fn unverified_remote_and_paid_opencode_models_are_refused_before_request() {
    let dir = temp_dir::TempDir::new("ai-policy");
    let engine = model_engine(&dir, "https://example.com/v1".into(), None);
    assert!(
        engine
            .ai(
                1,
                AiRequest::Compose {
                    draft: Draft::default()
                }
            )
            .unwrap_err()
            .to_string()
            .contains("verified free ZDR")
    );
    engine
        .set_settings(Settings {
            ai_enabled: true,
            ai_base_url: "https://opencode.ai/zen/go/v1".into(),
            ai_model: "paid-model".into(),
            ..Settings::default()
        })
        .unwrap();
    assert!(
        engine
            .ai(
                2,
                AiRequest::Compose {
                    draft: Draft::default()
                }
            )
            .unwrap_err()
            .to_string()
            .contains("not verified free")
    );
}

#[test]
fn v9_upgrade_preserves_mail_and_defaults_triage_off() {
    let dir = temp_dir::TempDir::new("ai-migration");
    let path = dir.join("kite.db");
    let store = Store::open(&path).unwrap();
    let account = store
        .add_account(Provider::Proton, "me@example.com", "Me")
        .unwrap();
    let thread = seed(
        &store,
        account.id,
        "upgrade",
        "Existing mail",
        "Keep this mail",
        100,
    );
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch("DROP TABLE ai_triaged; PRAGMA user_version=9;")
        .unwrap();
    // Old preferences have no ai_triage property.
    connection
        .execute(
            "INSERT OR REPLACE INTO kv(k,v) VALUES ('settings',?1)",
            [r#"{"ai_enabled":true}"#],
        )
        .unwrap();
    drop(connection);
    let store = Store::open(&path).unwrap();
    assert!(!store.settings().unwrap().ai_triage);
    assert_eq!(store.thread(thread).unwrap()[0].text, "Keep this mail");
    assert!(store.claim_ai_triage(thread).unwrap());
    store.apply_ai_triage(thread, "FYI").unwrap();
    assert_eq!(
        store
            .threads(
                &View {
                    account_id: Some(account.id),
                    mailbox: Mailbox::Inbox,
                    split: None,
                    query: Some("tag:FYI".into()),
                    folder: None
                },
                0,
                10
            )
            .unwrap()[0]
            .id,
        thread
    );
    drop(store);
    assert!(!Store::open(&path).unwrap().claim_ai_triage(thread).unwrap());
}

#[test]
fn v6_upgrade_of_linked_mail_preserves_ai_triage_after_rethreading() {
    let dir = temp_dir::TempDir::new("ai-v6-migration");
    let path = dir.join("kite.db");
    let store = Store::open(&path).unwrap();
    let account = store
        .add_account(Provider::Proton, "me@example.com", "Me")
        .unwrap();
    seed(
        &store,
        account.id,
        "legacy-parent",
        "Linked legacy mail",
        "Original",
        100,
    );
    let mut reply = inbox_message("legacy-reply", "Re: Linked legacy mail", "Reply", 200);
    reply.in_reply_to = Some("<legacy-parent@example.com>".into());
    store
        .apply_deltas(
            account.id,
            &[kite_core::sync::Delta::Upsert(Box::new(reply))],
        )
        .unwrap();
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch("DROP TABLE ai_triaged; PRAGMA user_version=6;")
        .unwrap();
    drop(connection);
    let store = Store::open(&path).unwrap();
    let threads = store
        .ai_search("Linked legacy mail", Some(account.id))
        .unwrap();
    assert_eq!(threads.len(), 1);
    assert_eq!(store.thread(threads[0]).unwrap().len(), 2);
    assert!(store.claim_ai_triage(threads[0]).unwrap());
    store.apply_ai_triage(threads[0], "FYI").unwrap();
    // Removing a message rethreads the conversation; its surviving mail stays triaged.
    store
        .apply_deltas(
            account.id,
            &[kite_core::sync::Delta::Delete {
                remote_id: "legacy-reply".into(),
            }],
        )
        .unwrap();
    let threads = store
        .ai_search("Linked legacy mail", Some(account.id))
        .unwrap();
    assert_eq!(threads.len(), 1);
    assert!(!store.claim_ai_triage(threads[0]).unwrap());
}
