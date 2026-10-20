//! The AI request as an OpenAI-compatible endpoint receives it, against a local fake.
#[path = "common/temp_dir.rs"]
mod temp_dir;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
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
