//! OpenAI-compatible streaming chat over SSE. Each chunk is delivered as `Event::Ai`.
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;

use crate::engine::Inner;
use crate::model::{AiModel, AiRequest, ChatMsg, Event, Settings};
use crate::store::Store;

#[derive(Clone, Copy, PartialEq)]
enum Provider {
    OpenRouter,
    OpenCode,
    Custom,
}

fn provider(base: &str, host_override: Option<&str>) -> Result<Provider> {
    let url = reqwest::Url::parse(base).context("Invalid AI base URL")?;
    Ok(match host_override.or_else(|| url.host_str()) {
        Some("openrouter.ai") => Provider::OpenRouter,
        Some("opencode.ai") => Provider::OpenCode,
        _ => Provider::Custom,
    })
}

#[derive(serde::Deserialize)]
struct Catalogue {
    data: Vec<serde_json::Value>,
}

fn free_pricing(value: &serde_json::Value) -> bool {
    ["prompt", "completion"].iter().all(|field| {
        value["pricing"][field]
            .as_str()
            .and_then(|s| s.parse::<f64>().ok())
            == Some(0.0)
    })
}

async fn catalogue(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    key: Option<&str>,
    optional: bool,
) -> Result<Option<Catalogue>> {
    let mut request = client.get(format!("{}/{path}", base.trim_end_matches('/')));
    if let Some(key) = key.filter(|key| !key.is_empty()) {
        request = request.bearer_auth(key);
    }
    let response = request.send().await.context("Fetch AI model catalogue")?;
    // Only an absent API permits a curated fallback; auth/server failures must stay visible.
    if optional && matches!(response.status().as_u16(), 404 | 405 | 501) {
        return Ok(None);
    }
    Ok(Some(
        response
            .error_for_status()
            .context("AI model catalogue request failed")?
            .json()
            .await
            .context("Invalid AI model catalogue response")?,
    ))
}

pub(crate) async fn models(
    settings: &Settings,
    key: Option<&str>,
    host_override: Option<&str>,
) -> Result<Vec<AiModel>> {
    let provider = provider(&settings.ai_base_url, host_override)?;
    if provider == Provider::Custom {
        return Ok(Vec::new());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("kite-mail/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let listed = catalogue(
        &client,
        &settings.ai_base_url,
        "models",
        key,
        provider == Provider::OpenCode,
    )
    .await?;
    let mut models = Vec::new();
    if provider == Provider::OpenRouter {
        // https://openrouter.ai/docs/guides/features/zdr: this live list tracks policy changes.
        let zdr = catalogue(&client, &settings.ai_base_url, "endpoints/zdr", key, false)
            .await?
            .context("Missing OpenRouter ZDR catalogue")?;
        for model in listed.context("Missing OpenRouter model catalogue")?.data {
            let Some(id) = model["id"].as_str() else {
                continue;
            };
            if (id.ends_with(":free") || free_pricing(&model))
                && zdr.data.iter().any(|endpoint| {
                    endpoint["model_id"].as_str() == Some(id) && free_pricing(endpoint)
                })
            {
                models.push(AiModel {
                    id: id.into(),
                    name: model["name"].as_str().unwrap_or(id).into(),
                    free: true,
                    zdr: true,
                });
            }
        }
    } else {
        // https://opencode.ai/docs/{go,zen}/ privacy + pricing, checked 2026-10-20.
        // DeepSeek's ZDR agreement is renewed monthly; recheck this table on policy updates.
        let table = [
            ("minimax-m3.1-flash-free", "MiniMax M3.1 Flash Free", false),
            (
                "longcat-2.5-preview-free",
                "LongCat 2.5 Preview Free",
                false,
            ),
            ("glm-5.3-flash-free", "GLM-5.3 Flash Free", true),
            ("deepseek-v4.1-flash-free", "DeepSeek V4.1 Flash Free", true),
        ];
        let url = reqwest::Url::parse(&settings.ai_base_url)?;
        let go = url.path().split('/').any(|segment| segment == "go");
        for (id, name, zen_only) in table {
            if (!go || !zen_only)
                && listed.as_ref().is_none_or(|list| {
                    list.data
                        .iter()
                        .any(|model| model["id"].as_str() == Some(id))
                })
            {
                models.push(AiModel {
                    id: id.into(),
                    name: name.into(),
                    free: true,
                    zdr: true,
                });
            }
        }
    }
    models.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(models)
}

pub(crate) fn prepare(store: &Store, request: &AiRequest) -> Result<Vec<ChatMsg>> {
    let (system, content) = match request {
        AiRequest::Summary { thread } => {
            store.check_ai_thread(*thread)?;
            let messages = store.thread(*thread)?;
            let mut text = String::new();
            for message in messages {
                text.push_str(&format!(
                    "Subject: {}\nFrom: {}\n{}\n\n",
                    message.subject, message.from.email, message.text
                ));
            }
            (
                "Summarize an email thread in at most four short lines: what the sender wants and what is still open.",
                text,
            )
        }
        AiRequest::Compose { draft } => {
            store.check_ai_draft(draft)?;
            let text = if draft.body.trim().is_empty() {
                format!("Write a reply to an email about \"{}\".", draft.subject)
            } else {
                format!(
                    "Write a reply to the email below.\n\nSubject: {}\n\n{}",
                    draft.subject, draft.body
                )
            };
            (
                "You draft one email reply. Answer with the reply body only: no subject line, no commentary.",
                text,
            )
        }
    };
    Ok(vec![
        ChatMsg {
            role: "system".into(),
            content: system.into(),
        },
        ChatMsg {
            role: "user".into(),
            content,
        },
    ])
}

/// OpenCode Go rejects requests without a stable per-conversation `x-opencode-session` (it
/// routes and caches prompts by it). One id per thread or draft, behind a per-run nonce so ids
/// from different installs never collide.
pub(crate) fn session(request: &AiRequest) -> String {
    static NONCE: OnceLock<String> = OnceLock::new();
    let nonce = NONCE.get_or_init(|| {
        let mut bytes = [0u8; 8];
        let _ = getrandom::fill(&mut bytes);
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    });
    match request {
        AiRequest::Summary { thread } => format!("kite-{nonce}-thread-{thread}"),
        AiRequest::Compose { draft } => match (draft.id, draft.reply_to) {
            (Some(id), _) => format!("kite-{nonce}-draft-{id}"),
            (None, Some(reply)) => format!("kite-{nonce}-reply-{reply}"),
            (None, None) => format!("kite-{nonce}-compose"),
        },
    }
}

pub(crate) fn stream(
    settings: &Settings,
    host_override: Option<&str>,
    key: &str,
    req: u32,
    session: &str,
    messages: Vec<ChatMsg>,
    out: &Inner,
) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(request(
        settings,
        host_override,
        key,
        req,
        session,
        messages,
        out,
    ))
}

async fn request(
    settings: &Settings,
    host_override: Option<&str>,
    key: &str,
    req: u32,
    session: &str,
    messages: Vec<ChatMsg>,
    out: &Inner,
) -> Result<()> {
    let url = format!(
        "{}/chat/completions",
        settings.ai_base_url.trim_end_matches('/')
    );
    let mut body = serde_json::json!({
        "model": settings.ai_model,
        "stream": true,
        "messages": messages
            .iter()
            .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
            .collect::<Vec<_>>(),
    });
    if provider(&settings.ai_base_url, host_override)? == Provider::OpenRouter {
        body["provider"] = serde_json::json!({
            "zdr": true,
            "data_collection": "deny",
            "max_price": { "prompt": 0, "completion": 0, "request": 0 },
        });
        body["session_id"] = session.into();
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .user_agent(concat!("kite-mail/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let res = client
        .post(&url)
        .bearer_auth(key)
        .header("x-opencode-session", session)
        .json(&body)
        .send()
        .await
        .context("AI request")?;
    let status = res.status();
    if !status.is_success() {
        let text = res.text().await.unwrap_or_default();
        // OpenAI-compatible errors carry a readable `error.message`; fall back to the raw body.
        let reason = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
            .unwrap_or(text);
        bail!(
            "AI request failed ({status}): {}",
            reason.chars().take(200).collect::<String>()
        );
    }

    let mut stream = res.bytes_stream();
    // Buffer bytes, not text: a multi-byte character can straddle two network chunks.
    let mut buffer: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        buffer.extend_from_slice(&chunk?);
        while let Some(newline) = buffer.iter().position(|b| *b == b'\n') {
            let raw: Vec<u8> = buffer.drain(..=newline).collect();
            let line = String::from_utf8_lossy(&raw);
            let line = line.trim_end_matches(['\r', '\n']);
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
