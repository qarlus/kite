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
    let worker = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for (path, status, response) in responses {
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
fn custom_endpoint_has_no_verified_models() {
    let dir = temp_dir::TempDir::new("ai-custom");
    let engine = model_engine(
        &dir,
        "http://127.0.0.1:1/openrouter.ai/zen/go/v1".into(),
        None,
    );
    assert!(engine.ai_models().unwrap().is_empty());
}
