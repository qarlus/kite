//! Microsoft Graph v1.0 backend: per-folder delta sync, mail ops, attachments and the day's calendar.
use crate::auth::{self, Tokens};
use crate::model::{Addr, CalEvent, Mailbox, Ms};
use crate::sync::{Delta, Op, RemoteAttachment, RemoteMsg};
use anyhow::{Context, Result, bail};
use base64::Engine as _;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt::Write as _;

const SELECT: &str = "id,conversationId,internetMessageId,subject,from,sender,toRecipients,ccRecipients,receivedDateTime,sentDateTime,createdDateTime,lastModifiedDateTime,isRead,flag,importance,hasAttachments,body,bodyPreview,parentFolderId,internetMessageHeaders";
const PREFER: &str = "odata.maxpagesize=100, outlook.body-content-type=\"html\"";
/// Well-known folders synced per account, and the mailbox each maps to.
const FOLDERS: [(&str, Mailbox); 6] = [
    ("inbox", Mailbox::Inbox),
    ("sentitems", Mailbox::Sent),
    ("drafts", Mailbox::Drafts),
    ("archive", Mailbox::Archive),
    ("deleteditems", Mailbox::Trash),
    ("junkemail", Mailbox::Spam),
];
/// A first sync stops paging a folder after this many messages; the nextLink is kept in the cursor.
const PAGE_BUDGET: usize = 1000;
/// Graph `$batch` accepts at most 20 sub-requests.
const BATCH: usize = 20;

pub struct Graph {
    base_url: String,
    authority: String,
    client_id: String,
    tokens: Tokens,
    http: reqwest::Client,
}

impl Graph {
    pub fn new(base_url: &str, authority: &str, client_id: &str, tokens: Tokens) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            authority: authority.trim_end_matches('/').to_string(),
            client_id: client_id.to_string(),
            tokens,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    /// The engine persists these after a refresh.
    pub fn tokens(&self) -> &Tokens {
        &self.tokens
    }

    pub async fn me(&mut self) -> Result<(String, String)> {
        let url = format!(
            "{}/me?$select=displayName,mail,userPrincipalName",
            self.base_url
        );
        let v = self.get_json(&url, "").await?;
        // Personal accounts report no `mail` and often no `displayName`: the MSA address
        // arrives as `userPrincipalName`.
        let email = v["mail"]
            .as_str()
            .filter(|s| s.contains('@'))
            .or_else(|| v["userPrincipalName"].as_str().filter(|s| s.contains('@')))
            .unwrap_or_default()
            .to_string();
        if email.is_empty() {
            bail!("Graph /me returned no address");
        }
        let name = v["displayName"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                email
                    .split('@')
                    .next()
                    .unwrap_or(email.as_str())
                    .to_string()
            });
        Ok((email, name))
    }

    /// Deltas for all well-known folders plus the opaque cursor for the next call.
    pub async fn sync(&mut self, cursor: Option<String>) -> Result<(Vec<Delta>, String)> {
        let mut links: BTreeMap<String, String> = match &cursor {
            Some(c) => serde_json::from_str(c).context("graph cursor")?,
            None => BTreeMap::new(),
        };
        // Delta tokens retain the old $select. Rewalk once to repair incomplete rows and
        // replace the old Focused Inbox importance mapping without writing remote flags.
        if links.get("__kite_schema").map(String::as_str) != Some("2") {
            links.clear();
            links.insert("__kite_schema".into(), "2".into());
        }
        let mut deltas = Vec::new();
        for (folder, mailbox) in FOLDERS {
            let mut link = links.get(folder).cloned().unwrap_or_else(|| {
                format!(
                    "{}/me/mailFolders/{folder}/messages/delta?$select={SELECT}",
                    self.base_url
                )
            });
            let mut seen = 0usize;
            loop {
                let page = self.get_json(&link, PREFER).await?;
                let mut items = page["value"].as_array().cloned().unwrap_or_default();
                // Delta updates can contain only changed properties. Never upsert such
                // a record as a full message: absent flags/body would erase local data.
                for item in &mut items {
                    if item.get("@removed").is_some() {
                        continue;
                    }
                    let incomplete = [
                        "subject",
                        "from",
                        "toRecipients",
                        "receivedDateTime",
                        "isRead",
                        "flag",
                        "importance",
                        "body",
                    ]
                    .iter()
                    .any(|field| item.get(field).is_none());
                    let empty_body = item["body"]["content"].as_str().is_none_or(str::is_empty);
                    if incomplete
                        || empty_body
                        || item["flag"]["flagStatus"].as_str().is_none()
                        || item["isRead"].as_bool().is_none()
                        || item["importance"].as_str().is_none()
                    {
                        let id = item["id"].as_str().context("Graph delta missing id")?;
                        let url = format!(
                            "{}/me/messages/{id}?$select={SELECT},uniqueBody",
                            self.base_url
                        );
                        let res = self
                            .auth_send(|http, token| {
                                http.get(&url).bearer_auth(token).header("Prefer", PREFER)
                            })
                            .await?;
                        if res.status() == reqwest::StatusCode::NOT_FOUND {
                            // A move/delete raced the delta; its removal arrives separately.
                            *item = Value::Null;
                            continue;
                        }
                        *item = json_res(res, "GET full message").await?;
                        if item["flag"]["flagStatus"].as_str().is_none()
                            || item["isRead"].as_bool().is_none()
                            || item["importance"].as_str().is_none()
                        {
                            bail!("Graph full message omitted flags or importance");
                        }
                    }
                }
                let attachments = self.page_attachments(&items).await?;
                for item in &items {
                    if let Some(delta) = delta_for(item, folder, mailbox, &attachments) {
                        deltas.push(delta);
                    }
                }
                seen += items.len();
                match page["@odata.nextLink"].as_str() {
                    Some(next) => link = next.to_string(),
                    None => {
                        link = page["@odata.deltaLink"]
                            .as_str()
                            .unwrap_or(&link)
                            .to_string();
                        break;
                    }
                }
                if seen >= PAGE_BUDGET {
                    break;
                }
            }
            links.insert(folder.to_string(), link);
        }
        Ok((deltas, serde_json::to_string(&links)?))
    }

    pub async fn apply(&mut self, op: &Op) -> Result<Option<String>> {
        match op {
            Op::Move { remote_id, to } => match well_known(*to) {
                Some(dest) => self.move_msg(remote_id, dest).await,
                None => Ok(None),
            },
            Op::MoveToFolder { remote_id, folder } => self.move_msg(remote_id, folder).await,
            Op::Flags {
                remote_id,
                unread,
                starred,
            } => {
                let mut body = serde_json::Map::new();
                if let Some(unread) = unread {
                    body.insert("isRead".to_string(), Value::Bool(!unread));
                }
                if let Some(starred) = starred {
                    let status = if *starred { "flagged" } else { "notFlagged" };
                    body.insert("flag".to_string(), json!({ "flagStatus": status }));
                }
                if body.is_empty() {
                    return Ok(None);
                }
                let url = format!("{}/me/messages/{remote_id}", self.base_url);
                let res = self
                    .auth_send(|http, token| http.patch(&url).bearer_auth(token).json(&body))
                    .await?;
                check(res, "PATCH message").await?;
                Ok(None)
            }
            Op::Send { mime } => {
                let (id, _) = self.prepare_send(mime).await?;
                self.send_prepared(&id).await?;
                Ok(None)
            }
            // Posting MIME to /me/messages creates the message as a draft in Drafts.
            Op::SaveDraft { mime, replaces } => {
                let url = format!("{}/me/messages", self.base_url);
                let body = base64::engine::general_purpose::STANDARD.encode(mime);
                let res = self
                    .auth_send(|http, token| {
                        http.post(&url)
                            .bearer_auth(token)
                            .header("Content-Type", "text/plain")
                            .body(body.clone())
                    })
                    .await?;
                let created = json_res(res, "create draft").await?;
                let id = created["id"].as_str().map(str::to_owned);
                if let Some(old) = replaces
                    && old != id.as_deref().unwrap_or_default()
                {
                    self.delete_message(old).await?;
                }
                Ok(id)
            }
            Op::DeleteDraft { remote_id } => {
                self.delete_message(remote_id).await?;
                Ok(None)
            }
        }
    }

    /// Create an Exchange-authored draft so its real Message-ID is available for local
    /// tag correlation before sending. JSON cannot set standard reply headers.
    pub async fn prepare_send(&mut self, mime: &[u8]) -> Result<(String, String)> {
        let parsed = mail_parser::MessageParser::default()
            .parse(mime)
            .context("parse outgoing Graph message")?;
        let recipients = |addresses: Option<&mail_parser::Address<'_>>| -> Vec<Value> {
            addresses.into_iter().flat_map(|a| a.iter()).filter_map(|a| {
                Some(json!({"emailAddress": {
                    "address": a.address.as_deref()?, "name": a.name.as_deref().unwrap_or_default()
                }}))
            }).collect()
        };
        let text = parsed.body_text(0).unwrap_or_default();
        let message = json!({
            "subject": parsed.subject().unwrap_or_default(),
            "body": {"contentType":"HTML", "content": text_html(&text)},
            "toRecipients": recipients(parsed.to()),
            "ccRecipients": recipients(parsed.cc()),
            "bccRecipients": recipients(parsed.bcc()),
        });
        let reply = parsed.in_reply_to().as_text().or_else(|| {
            parsed
                .in_reply_to()
                .as_text_list()
                .and_then(|ids| ids.first())
                .map(|id| id.as_ref())
        });
        let mut reply_id = None;
        if let Some(reply) = reply {
            let header = format!("<{}>", reply.trim_matches(['<', '>']));
            let mut url = reqwest::Url::parse(&format!("{}/me/messages", self.base_url))?;
            url.query_pairs_mut()
                .append_pair(
                    "$filter",
                    &format!("internetMessageId eq '{}'", header.replace('\'', "''")),
                )
                .append_pair("$select", "id");
            let found = self.get_json(url.as_str(), "").await?;
            reply_id = found["value"]
                .as_array()
                .and_then(|items| items.first())
                .and_then(|item| item["id"].as_str())
                .map(str::to_owned);
        }
        let url = match &reply_id {
            Some(id) => format!("{}/me/messages/{id}/createReply", self.base_url),
            None => format!("{}/me/messages", self.base_url),
        };
        let created = if reply.is_some() && reply_id.is_none() {
            // A reply from another account cannot use createReply. MIME retains its
            // In-Reply-To/References; omit our Message-ID so Exchange generates it.
            let raw = String::from_utf8_lossy(mime);
            let (headers, body) = raw
                .split_once("\r\n\r\n")
                .context("outgoing MIME headers")?;
            let mut omit = false;
            let headers = headers
                .split("\r\n")
                .filter(|line| {
                    if !line.starts_with([' ', '\t']) {
                        omit = line
                            .split_once(':')
                            .is_some_and(|(name, _)| name.eq_ignore_ascii_case("Message-ID"));
                    }
                    !omit
                })
                .collect::<Vec<_>>()
                .join("\r\n");
            let body = base64::engine::general_purpose::STANDARD
                .encode(format!("{headers}\r\n\r\n{body}"));
            let res = self
                .auth_send(|http, token| {
                    http.post(&url)
                        .bearer_auth(token)
                        .header("Prefer", "IdType=\"ImmutableId\"")
                        .header("Content-Type", "text/plain")
                        .body(body.clone())
                })
                .await?;
            json_res(res, "create MIME send draft").await?
        } else {
            let payload = if reply_id.is_some() {
                json!({})
            } else {
                message.clone()
            };
            let res = self
                .auth_send(|http, token| {
                    http.post(&url)
                        .bearer_auth(token)
                        .header("Prefer", "IdType=\"ImmutableId\"")
                        .json(&payload)
                })
                .await?;
            json_res(res, "create send draft").await?
        };
        let id = created["id"]
            .as_str()
            .context("Graph send draft missing id")?
            .to_string();
        let result = async {
            let created = if reply_id.is_some() {
                let url = format!("{}/me/messages/{id}", self.base_url);
                let res = self
                    .auth_send(|http, token| {
                        http.patch(&url)
                            .bearer_auth(token)
                            .header("Prefer", "IdType=\"ImmutableId\"")
                            .json(&message)
                    })
                    .await?;
                json_res(res, "update reply draft").await?
            } else {
                created
            };
            let message_id = created["internetMessageId"]
                .as_str()
                .filter(|id| !id.is_empty())
                .context("Graph send draft missing internetMessageId")?
                .to_string();
            Ok((id.clone(), message_id))
        }
        .await;
        if result.is_err() {
            let _ = self.delete_message(&id).await;
        }
        result
    }

    /// Sending a draft always saves its Sent Items copy (the sendMail flag is inapplicable).
    pub async fn send_prepared(&mut self, id: &str) -> Result<()> {
        let url = format!("{}/me/messages/{id}/send", self.base_url);
        let result = async {
            let res = self
                .auth_send(|http, token| {
                    http.post(&url)
                        .bearer_auth(token)
                        .header("Content-Length", "0")
                })
                .await?;
            check(res, "send draft").await
        }
        .await;
        if result.is_err() {
            let _ = self.delete_message(id).await;
        }
        result
    }

    /// Retain the provider item after an ambiguous transport error. Retrying must inspect
    /// isDraft first: Exchange may already have accepted it into the deferred Outbox.
    pub async fn send_deferred(&mut self, id: &str) -> Result<()> {
        let url = format!("{}/me/messages/{id}/send", self.base_url);
        let response = self
            .auth_send(|http, token| {
                http.post(&url)
                    .bearer_auth(token)
                    .header("Content-Length", "0")
            })
            .await?;
        check(response, "send deferred draft").await
    }

    pub async fn deferred_submitted(&mut self, id: &str) -> Result<bool> {
        let url = format!("{}/me/messages/{id}?$select=isDraft", self.base_url);
        let message = self.get_json(&url, "IdType=\"ImmutableId\"").await?;
        Ok(!message["isDraft"]
            .as_bool()
            .context("Graph scheduled message missing isDraft")?)
    }

    /// MAPI PidTagDeferredSendTime: UTC ISO 8601, not a Unix timestamp.
    pub async fn defer_send(&mut self, id: &str, at: Ms) -> Result<()> {
        let time = chrono::DateTime::from_timestamp_millis(at)
            .context("scheduled time out of range")?
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let url = format!("{}/me/messages/{id}", self.base_url);
        let body =
            json!({"singleValueExtendedProperties":[{"id":"SystemTime 0x3FEF","value":time}]});
        let res = self
            .auth_send(|http, token| http.patch(&url).bearer_auth(token).json(&body))
            .await?;
        check(res, "defer send").await
    }

    async fn delete_message(&mut self, remote_id: &str) -> Result<()> {
        let url = format!("{}/me/messages/{remote_id}", self.base_url);
        let res = self
            .auth_send(|http, token| http.delete(&url).bearer_auth(token))
            .await?;
        // A save already in flight when From changed can have deleted this copy itself.
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        check(res, "delete draft").await
    }

    /// The user's profile photo bytes, or `None` when the account has none (404).
    /// `User.Read`, already in the sign-in scopes, covers it.
    pub async fn photo(&mut self) -> Result<Option<Vec<u8>>> {
        let url = format!("{}/me/photo/$value", self.base_url);
        let res = self
            .auth_send(|http, token| http.get(&url).bearer_auth(token))
            .await?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let status = res.status();
        if !status.is_success() {
            let text = res.text().await.unwrap_or_default();
            bail!("profile photo failed ({status}): {}", clip(&text));
        }
        Ok(Some(res.bytes().await?.to_vec()))
    }

    pub async fn attachment(
        &mut self,
        remote_msg_id: &str,
        remote_att_id: &str,
    ) -> Result<Vec<u8>> {
        let url = format!(
            "{}/me/messages/{remote_msg_id}/attachments/{remote_att_id}/$value",
            self.base_url
        );
        let res = self
            .auth_send(|http, token| http.get(&url).bearer_auth(token))
            .await?;
        let status = res.status();
        let bytes = crate::attachments::download(res).await?;
        if !status.is_success() {
            bail!(
                "attachment download failed ({status}): {}",
                clip(&String::from_utf8_lossy(&bytes))
            );
        }
        Ok(bytes)
    }

    /// Events on the local day, expressed as a UTC instant range for `tz_offset_min`.
    pub async fn calendar_today(&mut self, tz_offset_min: i32) -> Result<Vec<CalEvent>> {
        let shift = i64::from(tz_offset_min) * 60;
        let local = auth::now_ms() / 1000 + shift;
        let start = local - local.rem_euclid(86_400) - shift;
        let url = format!(
            "{}/me/calendarView?startDateTime={}&endDateTime={}&$select=id,subject,start,end,location,isAllDay&$orderby=start/dateTime",
            self.base_url,
            iso_utc(start),
            iso_utc(start + 86_400)
        );
        let v = self.get_json(&url, "outlook.timezone=\"UTC\"").await?;
        Ok(v["value"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|e| CalEvent {
                id: e["id"].as_str().unwrap_or_default().to_string(),
                title: e["subject"].as_str().unwrap_or_default().to_string(),
                start: e["start"]["dateTime"]
                    .as_str()
                    .and_then(parse_iso_ms)
                    .unwrap_or_default(),
                end: e["end"]["dateTime"]
                    .as_str()
                    .and_then(parse_iso_ms)
                    .unwrap_or_default(),
                location: e["location"]["displayName"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
                all_day: e["isAllDay"].as_bool().unwrap_or(false),
            })
            .collect())
    }

    /// Creates a mail folder and returns the provider id it was given.
    pub async fn create_folder(&mut self, name: &str) -> Result<String> {
        let url = format!("{}/me/mailFolders", self.base_url);
        let body = json!({ "displayName": name });
        let res = self
            .auth_send(|http, token| http.post(&url).bearer_auth(token).json(&body))
            .await?;
        let v = json_res(res, "create folder").await?;
        v["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .context("Graph /me/mailFolders returned no id")
    }

    async fn move_msg(&mut self, remote_id: &str, destination: &str) -> Result<Option<String>> {
        let url = format!("{}/me/messages/{remote_id}/move", self.base_url);
        let body = json!({ "destinationId": destination });
        let res = self
            .auth_send(|http, token| http.post(&url).bearer_auth(token).json(&body))
            .await?;
        let v = json_res(res, "move message").await?;
        Ok(v["id"].as_str().map(str::to_owned))
    }

    /// Attachment metadata for the page's messages, in `$batch`es so sync stays few round trips.
    async fn page_attachments(
        &mut self,
        items: &[Value],
    ) -> Result<BTreeMap<String, Vec<RemoteAttachment>>> {
        let ids: Vec<&str> = items
            .iter()
            .filter(|i| i["hasAttachments"] == Value::Bool(true) && i.get("@removed").is_none())
            .filter_map(|i| i["id"].as_str())
            .collect();
        let mut out = BTreeMap::new();
        for chunk in ids.chunks(BATCH) {
            let sub: Vec<Value> = chunk
                .iter()
                .enumerate()
                .map(|(n, id)| {
                    json!({
                        "id": n.to_string(),
                        "method": "GET",
                        "url": format!("/me/messages/{id}/attachments?$select=id,name,contentType,size"),
                    })
                })
                .collect();
            let url = format!("{}/$batch", self.base_url);
            let body = json!({ "requests": sub });
            let res = self
                .auth_send(|http, token| http.post(&url).bearer_auth(token).json(&body))
                .await?;
            let v = json_res(res, "attachment batch").await?;
            for r in v["responses"].as_array().into_iter().flatten() {
                let Some(msg_id) = r["id"]
                    .as_str()
                    .and_then(|n| n.parse::<usize>().ok())
                    .and_then(|n| chunk.get(n))
                else {
                    continue;
                };
                if r["status"].as_u64() != Some(200) {
                    continue;
                }
                let atts = r["body"]["value"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(remote_attachment)
                    .collect();
                out.insert((*msg_id).to_string(), atts);
            }
        }
        Ok(out)
    }

    async fn get_json(&mut self, url: &str, prefer: &str) -> Result<Value> {
        let res = self
            .auth_send(|http, token| {
                let req = http.get(url).bearer_auth(token);
                if prefer.is_empty() {
                    req
                } else {
                    req.header("Prefer", prefer)
                }
            })
            .await?;
        json_res(res, &format!("GET {url}")).await
    }

    /// Bearer-authenticated request; refreshes once when the token is stale or the call says 401.
    async fn auth_send(
        &mut self,
        req: impl Fn(&reqwest::Client, &str) -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response> {
        if auth::now_ms() >= self.tokens.expires_at {
            self.refresh().await?;
        }
        let res = req(&self.http, &self.tokens.access).send().await?;
        if res.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Ok(res);
        }
        self.refresh().await?;
        Ok(req(&self.http, &self.tokens.access).send().await?)
    }

    async fn refresh(&mut self) -> Result<()> {
        self.tokens =
            auth::ms_refresh_at(&self.authority, &self.client_id, &self.tokens.refresh).await?;
        Ok(())
    }
}

/// Plain text to the escaped HTML body Graph sends; URLs and addresses become links.
pub fn text_html(text: &str) -> String {
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    format!(
        "<html><body><div>{}</div></body></html>",
        linkify(&escaped)
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\n', "<br>")
    )
}

/// Wraps URLs and mail addresses in anchors after escaping, so signatures (and any
/// other plain text) keep working links in the HTML Graph sends. Tokens are split on
/// whitespace; escaped entities contain no spaces, so they stay inside one token.
fn linkify(escaped: &str) -> String {
    let mut out = String::with_capacity(escaped.len() + 32);
    let mut token = String::new();
    let flush = |out: &mut String, token: &mut String| {
        // Trailing punctuation typed next to a link is not part of it.
        let bare = token.trim_end_matches(['.', ',', ';', ':', '!', '?', ')']);
        let rest = &token[bare.len()..];
        if bare.starts_with("http://") || bare.starts_with("https://") {
            let _ = write!(out, "<a href=\"{bare}\">{bare}</a>{rest}");
        } else if is_email(bare) {
            let _ = write!(out, "<a href=\"mailto:{bare}\">{bare}</a>{rest}");
        } else {
            out.push_str(token);
        }
        token.clear();
    };
    for c in escaped.chars() {
        if c.is_whitespace() {
            flush(&mut out, &mut token);
            out.push(c);
        } else {
            token.push(c);
        }
    }
    flush(&mut out, &mut token);
    out
}

/// `local@domain.tld` in already-escaped text: one '@', no separators, a dot after it.
fn is_email(token: &str) -> bool {
    let Some((local, domain)) = token.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !local.contains(['/', '<', '>', '&', '"', '\''])
        && domain.contains('.')
        && domain
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_alphanumeric() || c == '-'))
}

fn delta_for(
    item: &Value,
    folder: &str,
    mailbox: Mailbox,
    attachments: &BTreeMap<String, Vec<RemoteAttachment>>,
) -> Option<Delta> {
    let id = item["id"].as_str()?;
    if item.get("@removed").is_some() {
        return Some(Delta::Delete {
            remote_id: id.to_string(),
        });
    }
    let body = [&item["body"], &item["uniqueBody"]]
        .into_iter()
        .find(|body| body["content"].as_str().is_some_and(|s| !s.is_empty()));
    let html = body
        .filter(|body| {
            body["contentType"]
                .as_str()
                .is_some_and(|t| t.eq_ignore_ascii_case("html"))
        })
        .and_then(|body| body["content"].as_str())
        .map(str::to_owned);
    let text = match html.as_deref() {
        Some(html) => html_to_text(html),
        None => body
            .and_then(|body| body["content"].as_str())
            .or_else(|| item["bodyPreview"].as_str())
            .unwrap_or_default()
            .to_string(),
    };
    let headers = &item["internetMessageHeaders"];
    let header = |name: &str| {
        headers
            .as_array()
            .into_iter()
            .flatten()
            .filter(|h| {
                h["name"]
                    .as_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
            })
            .filter_map(|h| h["value"].as_str())
            .collect::<Vec<_>>()
            .join("\r\n")
    };
    let raw = format!(
        "In-Reply-To: {}\r\nReferences: {}\r\nMessage-ID: {}\r\n\r\n",
        header("In-Reply-To"),
        header("References"),
        header("Message-ID")
    );
    let parsed = mail_parser::MessageParser::default().parse(raw.as_bytes());
    let ids = |value: &mail_parser::HeaderValue<'_>| match value {
        mail_parser::HeaderValue::Text(value) => vec![value.to_string()],
        mail_parser::HeaderValue::TextList(values) => {
            values.iter().map(ToString::to_string).collect()
        }
        _ => Vec::new(),
    };
    Some(Delta::Upsert(Box::new(RemoteMsg {
        remote_id: id.to_string(),
        folder: item["parentFolderId"]
            .as_str()
            .unwrap_or(folder)
            .to_string(),
        mailbox,
        // Missing full headers means we cannot distinguish a root from a reply yet.
        thread_hint: (!headers.is_array())
            .then(|| item["conversationId"].as_str().map(str::to_owned))
            .flatten(),
        message_id: item["internetMessageId"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| parsed.as_ref()?.message_id().map(str::to_owned)),
        in_reply_to: parsed
            .as_ref()
            .and_then(|p| ids(p.in_reply_to()).into_iter().next()),
        references: parsed
            .as_ref()
            .map(|p| ids(p.references()))
            .unwrap_or_default(),
        from: addr(
            if item["from"]["emailAddress"]["address"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty())
            {
                &item["from"]["emailAddress"]
            } else {
                &item["sender"]["emailAddress"]
            },
        ),
        to: addrs(&item["toRecipients"]),
        cc: addrs(&item["ccRecipients"]),
        date: [
            "receivedDateTime",
            "sentDateTime",
            "createdDateTime",
            "lastModifiedDateTime",
        ]
        .into_iter()
        .find_map(|field| {
            item[field]
                .as_str()
                .and_then(parse_iso_ms)
                .filter(|date| *date > 0)
        })
        .unwrap_or_else(auth::now_ms),
        subject: item["subject"].as_str().unwrap_or_default().to_string(),
        text,
        html,
        unread: !item["isRead"].as_bool().unwrap_or(false),
        starred: item["flag"]["flagStatus"].as_str() == Some("flagged"),
        important: match item["importance"].as_str() {
            Some("high") => Some(true),
            Some("normal" | "low") => Some(false),
            _ => None,
        },
        list_unsubscribe: has_bulk_header(&item["internetMessageHeaders"]),
        unsubscribe_header: header_value(&item["internetMessageHeaders"], "List-Unsubscribe"),
        unsubscribe_post: header_value(&item["internetMessageHeaders"], "List-Unsubscribe-Post"),
        attachments: attachments.get(id).cloned().unwrap_or_default(),
    })))
}

/// `internetMessageHeaders` is an array of `{name, value}`. Bulk mail announces itself with
/// `List-Unsubscribe` or `List-Id`; both are selected above (Graph only returns them on `$select`).
fn header_value(headers: &Value, name: &str) -> String {
    headers
        .as_array()
        .into_iter()
        .flatten()
        .filter(|h| {
            h["name"]
                .as_str()
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
        .filter_map(|h| h["value"].as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn has_bulk_header(headers: &Value) -> bool {
    headers.as_array().into_iter().flatten().any(|h| {
        matches!(h["name"].as_str(), Some(n)
            if n.eq_ignore_ascii_case("List-Unsubscribe") || n.eq_ignore_ascii_case("List-Id"))
            && h["value"].as_str().is_some_and(|v| !v.trim().is_empty())
    })
}

fn addr(v: &Value) -> Addr {
    Addr {
        name: v["name"].as_str().unwrap_or_default().to_string(),
        email: v["address"].as_str().unwrap_or_default().to_string(),
    }
}

fn addrs(v: &Value) -> Vec<Addr> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|r| addr(&r["emailAddress"]))
        .collect()
}

fn remote_attachment(v: &Value) -> Option<RemoteAttachment> {
    Some(RemoteAttachment {
        remote_id: v["id"].as_str()?.to_string(),
        name: crate::attachments::file_name(v["name"].as_str().unwrap_or("attachment")),
        mime: v["contentType"]
            .as_str()
            .unwrap_or("application/octet-stream")
            .to_string(),
        size: v["size"].as_u64().unwrap_or(0),
    })
}

fn well_known(mailbox: Mailbox) -> Option<&'static str> {
    match mailbox {
        Mailbox::Inbox => Some("inbox"),
        Mailbox::Sent => Some("sentitems"),
        Mailbox::Drafts => Some("drafts"),
        Mailbox::Archive => Some("archive"),
        Mailbox::Trash => Some("deleteditems"),
        Mailbox::Spam => Some("junkemail"),
        Mailbox::Waiting
        | Mailbox::Scheduled
        | Mailbox::Starred
        | Mailbox::Snoozed
        | Mailbox::All => None,
    }
}

async fn json_res(res: reqwest::Response, what: &str) -> Result<Value> {
    let status = res.status();
    let error = res.error_for_status_ref().err();
    let text = res.text().await?;
    if let Some(error) = error {
        return Err(error).with_context(|| format!("{what} failed ({status}): {}", clip(&text)));
    }
    Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
}

async fn check(res: reqwest::Response, what: &str) -> Result<()> {
    if res.status().is_success() {
        return Ok(());
    }
    let status = res.status();
    let error = res.error_for_status_ref().err();
    let text = res.text().await.unwrap_or_default();
    if let Some(error) = error {
        return Err(error).with_context(|| format!("{what} failed ({status}): {}", clip(&text)));
    }
    bail!("{what} failed ({status}): {}", clip(&text))
}

fn clip(s: &str) -> String {
    s.chars().take(200).collect()
}

/// Tags, scripts and entities out of an HTML body, for snippets and FTS.
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut tag = String::new();
    let (mut in_tag, mut raw) = (false, false);
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let name = tag
                    .trim_start_matches('/')
                    .split(|c: char| c.is_whitespace() || c == '/')
                    .next()
                    .unwrap_or("");
                if matches!(name, "script" | "style") {
                    raw = !tag.starts_with('/');
                } else if !raw && matches!(name, "br" | "p" | "div" | "tr" | "li" | "td") {
                    out.push('\n');
                }
            }
            _ if in_tag => tag.push(c.to_ascii_lowercase()),
            _ if !raw => out.push(c),
            _ => {}
        }
    }
    let text = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// "2024-05-01T10:00:00Z" or Graph's "2024-05-01T10:00:00.0000000" (UTC, fractional part dropped).
fn parse_iso_ms(s: &str) -> Option<Ms> {
    let n = |at: usize, len: usize| s.get(at..at + len)?.parse::<i64>().ok();
    let (y, m, d) = (n(0, 4)?, n(5, 2)?, n(8, 2)?);
    let (h, min, sec) = (
        n(11, 2).unwrap_or(0),
        n(14, 2).unwrap_or(0),
        n(17, 2).unwrap_or(0),
    );
    Some((days_from_civil(y, m, d) * 86_400 + h * 3600 + min * 60 + sec) * 1000)
}

fn iso_utc(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let t = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        t / 3600,
        t % 3600 / 60,
        t % 60
    )
}

// Howard Hinnant's civil calendar algorithms, valid for any proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}
