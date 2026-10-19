//! Thin client for Proton's HTTP API, mirroring the routes `go-proton-api` uses.
//!
//! Every call returns the response body as JSON; the API answers `{"Code": 1000, ...}`
//! on success and `{"Code": <n>, "Error": "…", "Details": {…}}` with a 4xx/5xx status
//! on failure, so `code` is the only reliable discriminator.
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

/// Human verification (a captcha) must be solved in a browser before this call succeeds.
pub const CODE_HUMAN_VERIFICATION: i64 = 9001;
pub const CODE_WRONG_PASSWORD: i64 = 8002;
/// The refresh token is gone: the account has to sign in again.
pub const CODE_REFRESH_INVALID: i64 = 10013;
pub const CODE_INVALID_VALUE: i64 = 2001;
/// `9001` carries the challenge; `Details` is `{HumanVerificationMethods, HumanVerificationToken, WebUrl}`.
#[derive(Clone, Debug, Deserialize)]
pub struct Challenge {
    #[serde(rename = "HumanVerificationToken", default)]
    pub token: String,
    #[serde(rename = "HumanVerificationMethods", default)]
    pub methods: Vec<String>,
    /// Where the user solves it (`https://verify.proton.me/?…`).
    #[serde(rename = "WebUrl", default)]
    pub web_url: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ApiError {
    #[serde(skip)]
    pub status: u16,
    #[serde(rename = "Code", default)]
    pub code: i64,
    #[serde(rename = "Error", default)]
    pub message: String,
    #[serde(rename = "Details", default)]
    pub details: Value,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hint = match self.code {
            CODE_WRONG_PASSWORD => " (wrong password)",
            CODE_HUMAN_VERIFICATION => " (human verification required)",
            CODE_REFRESH_INVALID => " (session expired)",
            _ => "",
        };
        write!(f, "Proton API {}{hint}", self.message)
            .and_then(|_| write!(f, " (code {}, HTTP {})", self.code, self.status))
    }
}

impl std::error::Error for ApiError {}

impl ApiError {
    /// The challenge behind a 9001, when this is one.
    pub fn challenge(&self) -> Option<Challenge> {
        if self.code != CODE_HUMAN_VERIFICATION {
            return None;
        }
        serde_json::from_value(self.details.clone()).ok()
    }
}

/// `Details` as the API sends it: a JSON *string* holding JSON, or an object.
fn details(value: &Value) -> Value {
    match value {
        Value::String(text) => serde_json::from_str(text).unwrap_or(Value::Null),
        other => other.clone(),
    }
}

#[derive(Clone)]
pub struct Api {
    base: String,
    app_version: String,
    pub http: reqwest::Client,
    pub uid: String,
    pub access: String,
    pub refresh: String,
    pub server_time: Option<u32>,
}

impl Api {
    pub fn new(base: &str, app_version: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            app_version: app_version.to_string(),
            http: reqwest::Client::builder()
                .user_agent(concat!("Kite/", env!("CARGO_PKG_VERSION")))
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            uid: String::new(),
            access: String::new(),
            refresh: String::new(),
            server_time: None,
        }
    }

    pub fn session(&mut self, uid: &str, access: &str, refresh: &str) {
        self.uid = uid.to_string();
        self.access = access.to_string();
        self.refresh = refresh.to_string();
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// Unauthenticated call (login, 2FA, refresh).
    pub async fn post(&mut self, path: &str, body: &Value) -> Result<Value> {
        self.send(reqwest::Method::POST, path, Some(body), &[], None)
            .await
    }

    /// Session-authenticated call. Refreshes the access token once on a 401.
    pub async fn call(
        &mut self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value> {
        self.call_with(method, path, body, &[]).await
    }

    pub async fn call_with(
        &mut self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
        headers: &[(&str, &str)],
    ) -> Result<Value> {
        match self.authed(method.clone(), path, body, headers).await {
            Err(e) if is_auth_expired(&e) => {
                self.refresh_auth().await?;
                self.authed(method, path, body, headers).await
            }
            other => other,
        }
    }

    async fn authed(
        &mut self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
        headers: &[(&str, &str)],
    ) -> Result<Value> {
        if self.access.is_empty() {
            bail!("not signed in to Proton");
        }
        let access = self.access.clone();
        let uid = self.uid.clone();
        let mut all = vec![("x-pm-uid", uid.as_str())];
        all.extend_from_slice(headers);
        self.send(method, path, body, &all, Some(&access)).await
    }

    async fn send(
        &mut self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
        headers: &[(&str, &str)],
        bearer: Option<&str>,
    ) -> Result<Value> {
        let mut request = self
            .http
            .request(method, self.url(path))
            .header("x-pm-appversion", &self.app_version)
            .header("Accept", "application/json");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.context("Proton API request")?;
        if let Some(time) = response
            .headers()
            .get(reqwest::header::DATE)
            .and_then(|date| date.to_str().ok())
            .and_then(|date| chrono::DateTime::parse_from_rfc2822(date).ok())
            .and_then(|date| u32::try_from(date.timestamp()).ok())
        {
            self.server_time = Some(time);
        }
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if (200..300).contains(&status) && !is_error_code(&value) {
            return Ok(value);
        }
        let mut error: ApiError = serde_json::from_value(value).unwrap_or(ApiError {
            status,
            code: 0,
            message: String::new(),
            details: Value::Null,
        });
        error.status = status;
        if error.message.is_empty() {
            error.message = text.chars().take(200).collect();
        }
        error.details = details(&error.details);
        Err(error.into())
    }

    pub async fn refresh_auth(&mut self) -> Result<()> {
        let uid = self.uid.clone();
        let refresh = self.refresh.clone();
        if refresh.is_empty() {
            bail!("no Proton refresh token");
        }
        let state = super::random_token(32);
        let body = json!({
            "UID": uid,
            "RefreshToken": refresh,
            "ResponseType": "token",
            "GrantType": "refresh_token",
            "RedirectURI": "https://protonmail.ch",
            "State": state,
        });
        let value = self.post("/auth/v4/refresh", &body).await?;
        // The API answers with the `Auth` fields at the top level (go-proton-api unmarshals
        // `/auth/v4*` responses into an embedded `Auth` struct).
        let auth: Auth = serde_json::from_value(value).context("Proton refresh response")?;
        self.session(&auth.uid, &auth.access_token, &auth.refresh_token);
        Ok(())
    }

    // ---------------------------------------------------------------- routes

    pub async fn auth_info(&mut self, username: &str) -> Result<AuthInfo> {
        let value = self
            .post("/auth/v4/info", &json!({ "Username": username }))
            .await?;
        serde_json::from_value(value).context("Proton auth info")
    }

    /// SRP handshake. `hv` carries the `x-pm-human-verification-token` pair when a
    /// previous call answered 9001.
    pub async fn auth(&mut self, body: &Value, hv: Option<&Challenge>) -> Result<Auth> {
        let token = hv.map(|c| c.token.clone()).unwrap_or_default();
        let methods = hv.map(|c| c.methods.join(",")).unwrap_or_default();
        let headers = if token.is_empty() {
            Vec::new()
        } else {
            vec![
                ("x-pm-human-verification-token", token.as_str()),
                ("x-pm-human-verification-token-type", methods.as_str()),
            ]
        };
        let value = self
            .send(
                reqwest::Method::POST,
                "/auth/v4",
                Some(body),
                &headers,
                None,
            )
            .await?;
        serde_json::from_value(value).context("Proton auth response")
    }

    /// The `2FA` step, proven against the session the previous `/auth/v4` opened. The
    /// route only confirms the code: the tokens from `/auth/v4` are already the session.
    pub async fn auth_2fa(&mut self, code: &str) -> Result<()> {
        let body = json!({ "TwoFactorCode": code });
        self.call(reqwest::Method::POST, "/auth/v4/2fa", Some(&body))
            .await?;
        Ok(())
    }

    /// Ends the session. Removal of an account must not leave a token alive.
    pub async fn logout(&mut self) -> Result<()> {
        let _ = self.call(reqwest::Method::DELETE, "/auth/v4", None).await;
        Ok(())
    }

    pub async fn salts(&mut self) -> Result<Vec<Salt>> {
        let value = self
            .call(reqwest::Method::GET, "/core/v4/keys/salts", None)
            .await?;
        let list = value["KeySalts"].clone();
        serde_json::from_value(list).context("Proton key salts")
    }

    pub async fn user(&mut self) -> Result<User> {
        let value = self
            .call(reqwest::Method::GET, "/core/v4/users", None)
            .await?;
        serde_json::from_value(value["User"].clone()).context("Proton user")
    }

    pub async fn addresses(&mut self) -> Result<Vec<Address>> {
        let value = self
            .call(reqwest::Method::GET, "/core/v4/addresses", None)
            .await?;
        let addresses = value["Addresses"].clone();
        serde_json::from_value(addresses).context("Proton addresses")
    }

    /// One page of message metadata, newest first (`Sort: ID`, `Desc: 1`).
    pub async fn messages(&mut self, label: &str, page: u32, size: u32) -> Result<Vec<Meta>> {
        let body = json!({
            "LabelID": label,
            "Page": page,
            "PageSize": size,
            "Sort": "ID",
            "Desc": 1,
        });
        // The list route is a POST with a GET override, as go-proton-api does it.
        let value = self
            .call_with(
                reqwest::Method::POST,
                "/mail/v4/messages",
                Some(&body),
                &[("X-HTTP-Method-Override", "GET")],
            )
            .await?;
        Ok(serde_json::from_value(value["Messages"].clone()).unwrap_or_default())
    }

    pub async fn message(&mut self, id: &str) -> Result<Message> {
        let value = self
            .call(
                reqwest::Method::GET,
                &format!("/mail/v4/messages/{id}"),
                None,
            )
            .await?;
        serde_json::from_value(value["Message"].clone()).context("Proton message")
    }

    pub async fn attachment(&mut self, id: &str) -> Result<AttachmentData> {
        let value = self
            .call(
                reqwest::Method::GET,
                &format!("/mail/v4/attachments/{id}"),
                None,
            )
            .await?;
        serde_json::from_value(value["Attachment"].clone()).context("Proton attachment")
    }

    pub async fn latest_event(&mut self) -> Result<String> {
        let value = self
            .call(reqwest::Method::GET, "/core/v4/events/latest", None)
            .await?;
        value["EventID"]
            .as_str()
            .map(str::to_owned)
            .context("Proton event id")
    }

    /// Events after `event_id`, following the API's `More` flag (bounded, like
    /// go-proton-api's 50-event batches).
    pub async fn events(&mut self, event_id: &str) -> Result<Vec<Event>> {
        let mut out = Vec::new();
        let mut cursor = event_id.to_string();
        for _ in 0..50 {
            let value = self
                .call(
                    reqwest::Method::GET,
                    &format!("/core/v4/events/{cursor}"),
                    None,
                )
                .await?;
            let event: Event = serde_json::from_value(value.clone()).context("Proton event")?;
            let more = value["More"].as_i64().unwrap_or(0) != 0;
            cursor = event.event_id.clone();
            out.push(event);
            if !more {
                break;
            }
        }
        Ok(out)
    }

    pub async fn label_messages(&mut self, ids: &[String], label: &str) -> Result<()> {
        self.act_on_messages("/mail/v4/messages/label", ids, label)
            .await
    }

    pub async fn unlabel_messages(&mut self, ids: &[String], label: &str) -> Result<()> {
        self.act_on_messages("/mail/v4/messages/unlabel", ids, label)
            .await
    }

    async fn act_on_messages(&mut self, path: &str, ids: &[String], label: &str) -> Result<()> {
        let body = json!({ "LabelID": label, "IDs": ids });
        self.call(reqwest::Method::PUT, path, Some(&body)).await?;
        Ok(())
    }

    pub async fn mark_read(&mut self, ids: &[String], read: bool) -> Result<()> {
        let path = if read {
            "/mail/v4/messages/read"
        } else {
            "/mail/v4/messages/unread"
        };
        let body = json!({ "IDs": ids });
        self.call(reqwest::Method::PUT, path, Some(&body)).await?;
        Ok(())
    }

    /// The account's "Sign external messages" setting (`MailSettings.Sign`).
    pub async fn signs_external_mail(&mut self) -> Result<bool> {
        let value = self
            .call(reqwest::Method::GET, "/mail/v4/settings", None)
            .await?;
        Ok(value["MailSettings"]["Sign"].as_i64().unwrap_or(0) > 0)
    }

    pub async fn delete_messages(&mut self, ids: &[String]) -> Result<()> {
        let body = json!({ "IDs": ids });
        self.call(
            reqwest::Method::PUT,
            "/mail/v4/messages/delete",
            Some(&body),
        )
        .await?;
        Ok(())
    }

    pub async fn create_draft(&mut self, body: &Value) -> Result<Message> {
        let value = self
            .call(reqwest::Method::POST, "/mail/v4/messages", Some(body))
            .await?;
        serde_json::from_value(value["Message"].clone()).context("Proton draft")
    }

    pub async fn update_draft(&mut self, id: &str, body: &Value) -> Result<Message> {
        let value = self
            .call(
                reqwest::Method::PUT,
                &format!("/mail/v4/messages/{id}"),
                Some(body),
            )
            .await?;
        serde_json::from_value(value["Message"].clone()).context("Proton draft")
    }

    pub async fn send_draft(&mut self, id: &str, body: &Value) -> Result<Message> {
        let value = self
            .call(
                reqwest::Method::POST,
                &format!("/mail/v4/messages/{id}"),
                Some(body),
            )
            .await?;
        serde_json::from_value(value["Sent"].clone()).context("Proton sent message")
    }

    /// The account's labels, folders included (`GET /core/v4/labels`).
    pub async fn labels(&mut self) -> Result<Vec<Label>> {
        let value = self
            .call(reqwest::Method::GET, "/core/v4/labels", None)
            .await?;
        Ok(serde_json::from_value(value["Labels"].clone()).unwrap_or_default())
    }

    /// Creates a label (type 1) or folder (type 3) and returns it.
    pub async fn create_label(&mut self, name: &str) -> Result<Label> {
        let body = json!({ "Name": name, "Color": "#8080ff", "Type": 3 });
        let value = self
            .call(reqwest::Method::POST, "/core/v4/labels", Some(&body))
            .await?;
        serde_json::from_value(value["Label"].clone()).context("Proton label")
    }

    /// Public keys for a recipient; `1` means the address is a Proton account.
    pub async fn public_keys(&mut self, email: &str) -> Result<(Vec<PublicKey>, i64)> {
        let path = format!("/core/v4/keys?Email={}", urlencode(email));
        let value = self.call(reqwest::Method::GET, &path, None).await?;
        let keys = serde_json::from_value(value["Keys"].clone()).unwrap_or_default();
        Ok((keys, value["RecipientType"].as_i64().unwrap_or(2)))
    }
}

fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'@' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn is_error_code(value: &Value) -> bool {
    value["Code"]
        .as_i64()
        .is_some_and(|code| code != 1000 && code != 1001)
}

fn is_auth_expired(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<ApiError>()
        .is_some_and(|e| e.status == 401 || e.code == CODE_REFRESH_INVALID)
}

/// `GET /auth/v4` as a `bool`-ish: Proton sends `0`/`1` for every flag.
fn flag(value: &Value) -> bool {
    value.as_i64().unwrap_or(0) != 0 || value.as_bool().unwrap_or(false)
}

fn flag_field<'de, D>(deserializer: D) -> std::result::Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(flag(&value))
}

// ------------------------------------------------------------------ payloads

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthInfo {
    /// 3 and 4 are the bcrypt-based versions; anything older is not supported.
    pub version: i64,
    /// Clear-signed: `-----BEGIN PGP SIGNED MESSAGE-----` around the base64 modulus.
    pub modulus: String,
    pub server_ephemeral: String,
    pub salt: String,
    /// The API capitalises the acronym, so `PascalCase` would look for `SrpSession`.
    #[serde(rename = "SRPSession")]
    pub srp_session: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Auth {
    /// Proton spells the session id `UID`, not `Uid`.
    #[serde(rename = "UID", default)]
    pub uid: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub server_proof: String,
    #[serde(rename = "2FA", default)]
    pub two_fa: TwoFA,
    /// 1 = one password, 2 = login password plus a separate mailbox password.
    #[serde(default)]
    pub password_mode: i64,
}

/// `TwoFAInfo`: `Enabled` is `HasTOTP = 1`, `HasFIDO2 = 2`, `HasFIDO2AndTOTP = 3`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TwoFA {
    #[serde(default)]
    pub enabled: i64,
}

impl Auth {
    pub fn has_totp(&self) -> bool {
        self.two_fa.enabled & 1 != 0
    }
    pub fn two_password(&self) -> bool {
        self.password_mode == 2
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Salt {
    #[serde(rename = "ID")]
    pub id: String,
    /// `null` for keys that are not unlocked from the password (address keys with a token).
    #[serde(default)]
    pub key_salt: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct User {
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub keys: Vec<Key>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Address {
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub keys: Vec<Key>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Key {
    #[serde(rename = "ID", default)]
    pub id: String,
    /// Armored, passphrase-protected secret key.
    #[serde(default)]
    pub private_key: String,
    #[serde(default, deserialize_with = "flag_field")]
    pub primary: bool,
    #[serde(default, deserialize_with = "flag_field")]
    pub active: bool,
    /// Address keys on current accounts: their passphrase, encrypted to the user key.
    #[serde(default)]
    pub token: Option<String>,
    /// The user key's detached signature over the decrypted token.
    #[serde(default)]
    pub signature: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Label {
    #[serde(rename = "ID", default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// 1 = label, 3 = folder; the system labels (0..10) may also arrive here.
    #[serde(rename = "Type", default)]
    pub kind: i64,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct NameAddr {
    #[serde(rename = "Name", default)]
    pub name: String,
    #[serde(rename = "Address", default)]
    pub address: String,
}

/// Message metadata: what the list route returns and events carry.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Meta {
    #[serde(rename = "ID", default)]
    pub id: String,
    #[serde(rename = "LabelIDs", default)]
    pub label_ids: Vec<String>,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub sender: Option<NameAddr>,
    #[serde(default)]
    pub to_list: Vec<NameAddr>,
    #[serde(rename = "CCList", default)]
    pub cc_list: Vec<NameAddr>,
    /// Unix seconds.
    #[serde(default)]
    pub time: i64,
    #[serde(default, deserialize_with = "flag_field")]
    pub unread: bool,
}

/// A full message: metadata plus the encrypted header block and body.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Message {
    #[serde(flatten)]
    pub meta: Meta,
    #[serde(default)]
    pub header: String,
    #[serde(default)]
    pub body: String,
    #[serde(rename = "MIMEType", default)]
    pub mime_type: String,
    #[serde(default)]
    pub attachments: Vec<AttachmentMeta>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AttachmentMeta {
    #[serde(rename = "ID", default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub size: i64,
    #[serde(rename = "MIMEType", default)]
    pub mime_type: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AttachmentData {
    #[serde(default)]
    pub data: String,
    #[serde(default)]
    pub key_packets: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PublicKey {
    #[serde(default)]
    pub public_key: String,
    /// Bit 0 set means the key is not compromised; only such keys may be used.
    #[serde(default)]
    pub flags: i64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Event {
    #[serde(rename = "EventID", default)]
    pub event_id: String,
    #[serde(default)]
    pub messages: Vec<MessageEvent>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MessageEvent {
    #[serde(rename = "ID", default)]
    pub id: String,
    /// 0 delete, 1 create, 2 update, 3 update flags.
    #[serde(default)]
    pub action: i64,
    /// A delete carries only the id, so the metadata is absent (or null).
    #[serde(default)]
    pub message: Option<Meta>,
}

pub const EVENT_DELETE: i64 = 0;
