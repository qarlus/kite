//! OpenAI-compatible streaming chat over SSE. Each chunk is delivered as `Event::Ai`.
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;

use crate::engine::Inner;
use crate::model::{ChatMsg, Event, Settings};

pub(crate) fn stream(
    settings: &Settings,
    key: &str,
    req: u32,
    messages: Vec<ChatMsg>,
    out: &Inner,
) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(request(settings, key, req, messages, out))
}

async fn request(
    settings: &Settings,
    key: &str,
    req: u32,
    messages: Vec<ChatMsg>,
    out: &Inner,
) -> Result<()> {
    let url = format!(
        "{}/chat/completions",
        settings.ai_base_url.trim_end_matches('/')
    );
    let body = serde_json::json!({
        "model": settings.ai_model,
        "stream": true,
        "messages": messages
            .iter()
            .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
            .collect::<Vec<_>>(),
    });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;
    let res = client
        .post(&url)
        .bearer_auth(key)
        .json(&body)
        .send()
        .await
        .context("AI request")?;
    let status = res.status();
    if !status.is_success() {
        let text = res.text().await.unwrap_or_default();
        bail!(
            "AI request failed ({status}): {}",
            text.chars().take(200).collect::<String>()
        );
    }

    let mut stream = res.bytes_stream();
    let mut buffer = String::new();
    while let Some(chunk) = stream.next().await {
        buffer.push_str(&String::from_utf8_lossy(&chunk?));
        while let Some(newline) = buffer.find('\n') {
            let line = buffer[..newline].trim_end_matches('\r').to_string();
            buffer.drain(..=newline);
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" {
                return done(req, out);
            }
            if data.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            let delta = value["choices"][0]["delta"]["content"]
                .as_str()
                .unwrap_or_default();
            if !delta.is_empty() {
                out.emit(Event::Ai {
                    req,
                    delta: delta.to_string(),
                    done: false,
                });
            }
        }
    }
    done(req, out)
}

fn done(req: u32, out: &Inner) -> Result<()> {
    out.emit(Event::Ai {
        req,
        delta: String::new(),
        done: true,
    });
    Ok(())
}
