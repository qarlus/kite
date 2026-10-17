//! Microsoft 365 sign-in (OAuth 2.0 authorization code + PKCE over a loopback redirect) and the OS
//! keychain. Public client: no secret, the code is bound to the PKCE verifier and the `state`.
use crate::model::Ms;
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const AUTHORITY: &str = "https://login.microsoftonline.com/common";
const SCOPES: &str = "offline_access Mail.ReadWrite Mail.Send Calendars.Read User.Read";
const SERVICE: &str = "app.kite.mail";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

/// Stores a secret in the OS keychain (Windows Credential Manager, macOS Keychain).
pub fn secret_set(key: &str, val: &str) -> Result<()> {
    keyring::Entry::new(SERVICE, key)?.set_password(val)?;
    Ok(())
}

pub fn secret_get(key: &str) -> Result<Option<String>> {
    match keyring::Entry::new(SERVICE, key)?.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn secret_del(key: &str) -> Result<()> {
    match keyring::Entry::new(SERVICE, key)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Tokens for one account. Serialize them into the keychain; the engine persists refreshed ones.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Tokens {
    access: String,
    refresh: String,
    expires_at: Ms,
}

impl Tokens {
    pub fn new(access: impl Into<String>, refresh: impl Into<String>, expires_at: Ms) -> Self {
        Self {
            access: access.into(),
            refresh: refresh.into(),
            expires_at,
        }
    }

    pub fn access(&self) -> &str {
        &self.access
    }

    pub fn refresh(&self) -> &str {
        &self.refresh
    }

    pub fn expires_at(&self) -> Ms {
        self.expires_at
    }
}

pub fn now_ms() -> Ms {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// Interactive sign-in against the public Microsoft authority, opening the system browser.
pub async fn ms_login(client_id: &str) -> Result<Tokens> {
    ms_login_at(AUTHORITY, client_id, open_browser).await
}

/// `open` receives the authorize URL; tests inject a closure that captures it instead of a browser.
pub async fn ms_login_at(
    authority: &str,
    client_id: &str,
    open: impl FnOnce(&str),
) -> Result<Tokens> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .context("bind loopback redirect")?;
    let redirect = format!("http://localhost:{}", listener.local_addr()?.port());
    let verifier = random_token(32)?;
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    let state = random_token(16)?;
    open(&authorize_url(
        authority, client_id, &redirect, &challenge, &state,
    )?);
    let code = tokio::time::timeout(LOGIN_TIMEOUT, accept_code(&listener, &state))
        .await
        .map_err(|_| anyhow!("Microsoft sign-in timed out"))??;
    exchange(authority, client_id, &redirect, &code, &verifier).await
}

pub async fn ms_refresh(client_id: &str, refresh: &str) -> Result<Tokens> {
    ms_refresh_at(AUTHORITY, client_id, refresh).await
}

/// Authority-injectable refresh, used by [`ms_refresh`] and by the Graph backend in tests.
pub async fn ms_refresh_at(authority: &str, client_id: &str, refresh: &str) -> Result<Tokens> {
    let res = http()
        .post(format!("{authority}/oauth2/v2.0/token"))
        .form(&[
            ("client_id", client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
            ("scope", SCOPES),
        ])
        .send()
        .await?;
    read_tokens(res, Some(refresh)).await
}

fn authorize_url(
    authority: &str,
    client_id: &str,
    redirect: &str,
    challenge: &str,
    state: &str,
) -> Result<String> {
    let mut url = reqwest::Url::parse(&format!("{authority}/oauth2/v2.0/authorize"))?;
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect)
        .append_pair("response_mode", "query")
        .append_pair("scope", SCOPES)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state);
    Ok(url.into())
}

async fn exchange(
    authority: &str,
    client_id: &str,
    redirect: &str,
    code: &str,
    verifier: &str,
) -> Result<Tokens> {
    let res = http()
        .post(format!("{authority}/oauth2/v2.0/token"))
        .form(&[
            ("client_id", client_id),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect),
            ("code_verifier", verifier),
            ("scope", SCOPES),
        ])
        .send()
        .await?;
    read_tokens(res, None).await
}

/// Accepts the single browser redirect and returns the authorization code.
async fn accept_code(listener: &tokio::net::TcpListener, state: &str) -> Result<String> {
    loop {
        let (mut sock, _) = listener.accept().await?;
        let mut head = Vec::with_capacity(1024);
        let mut buf = [0u8; 1024];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 8192 {
            match sock.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => head.extend_from_slice(&buf[..n]),
            }
        }
        let head = String::from_utf8_lossy(&head);
        let target = head.split_whitespace().nth(1).unwrap_or("");
        let Ok(url) = reqwest::Url::parse(&format!("http://localhost{target}")) else {
            continue;
        };
        let get = |k: &str| {
            url.query_pairs()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.into_owned())
        };
        if let Some(err) = get("error") {
            let _ = reply(&mut sock, 400).await;
            bail!("Microsoft sign-in failed: {err}");
        }
        let Some(code) = get("code") else {
            let _ = reply(&mut sock, 404).await;
            continue;
        };
        if get("state").as_deref() != Some(state) {
            let _ = reply(&mut sock, 400).await;
            bail!("Microsoft sign-in state mismatch");
        }
        let _ = reply(&mut sock, 200).await;
        return Ok(code);
    }
}

async fn reply(sock: &mut tokio::net::TcpStream, status: u16) -> std::io::Result<()> {
    let body = if status == 200 {
        "<!doctype html><title>Kite</title><p>You can close this tab and return to Kite."
    } else {
        "<!doctype html><title>Kite</title><p>Sign-in failed. You can close this tab."
    };
    let head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    sock.write_all(head.as_bytes()).await?;
    sock.write_all(body.as_bytes()).await
}

#[derive(Deserialize)]
struct TokenBody {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
}

async fn read_tokens(res: reqwest::Response, prior_refresh: Option<&str>) -> Result<Tokens> {
    let status = res.status();
    let text = res.text().await?;
    if !status.is_success() {
        bail!("token request failed ({status}): {text}");
    }
    let body: TokenBody = serde_json::from_str(&text).context("token response")?;
    let refresh = body
        .refresh_token
        .or_else(|| prior_refresh.map(str::to_owned))
        .ok_or_else(|| anyhow!("token response had no refresh token"))?;
    // Expire a minute early so a request never races the server clock.
    let expires_at = now_ms() + body.expires_in.unwrap_or(3600) * 1000 - 60_000;
    Ok(Tokens::new(body.access_token, refresh, expires_at))
}

fn random_token(bytes: usize) -> Result<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf)?;
    Ok(b64url(&buf))
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

#[cfg(target_os = "windows")]
fn open_browser(url: &str) {
    // cmd.exe treats `&` inside the URL as a command separator unless it is caret-escaped.
    let _ = Command::new("cmd")
        .args(["/C", "start", "", &url.replace('&', "^&")])
        .spawn();
}

#[cfg(target_os = "macos")]
fn open_browser(url: &str) {
    let _ = Command::new("open").arg(url).spawn();
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn open_browser(url: &str) {
    let _ = Command::new("xdg-open").arg(url).spawn();
}
