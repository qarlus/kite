//! Microsoft Graph v1.0 backend: per-folder delta sync, mail ops, attachments and the day's calendar.
use crate::auth::{self, Tokens};
use crate::model::{Addr, CalEvent, Mailbox, Ms};
use crate::sync::{Delta, Op, RemoteAttachment, RemoteMsg};
use anyhow::{Context, Result, bail};
use base64::Engine as _;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SELECT: &str = "id,conversationId,internetMessageId,subject,from,toRecipients,ccRecipients,receivedDateTime,isRead,flag,inferenceClassification,hasAttachments,body,bodyPreview,parentFolderId";
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
        let name = v["displayName"].as_str().unwrap_or_default().to_string();
        let email = v["mail"]
            .as_str()
            .or_else(|| v["userPrincipalName"].as_str())
            .unwrap_or_default()
            .to_string();
        if email.is_empty() {
            bail!("Graph /me returned no address");
        }
        Ok((email, name))
    }

    /// Deltas for all well-known folders plus the opaque cursor for the next call.
    pub async fn sync(&mut self, cursor: Option<String>) -> Result<(Vec<Delta>, String)> {
        let mut links: BTreeMap<String, String> = match &cursor {
            Some(c) => serde_json::from_str(c).context("graph cursor")?,
            None => BTreeMap::new(),
        };
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
                let items = page["value"].as_array().cloned().unwrap_or_default();
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
                let url = format!("{}/me/sendMail", self.base_url);
                let body = base64::engine::general_purpose::STANDARD.encode(mime);
                let res = self
                    .auth_send(|http, token| {
                        http.post(&url)
                            .bearer_auth(token)
                            .header("Content-Type", "text/plain")
                            .body(body.clone())
                    })
                    .await?;
                check(res, "sendMail").await?;
                Ok(None)
            }
        }
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
        if !status.is_success() {
            let text = res.text().await.unwrap_or_default();
            bail!("attachment download failed ({status}): {}", clip(&text));
        }
        Ok(res.bytes().await?.to_vec())
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
        if auth::now_ms() >= self.tokens.expires_at() {
            self.refresh().await?;
        }
        let res = req(&self.http, self.tokens.access()).send().await?;
        if res.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Ok(res);
        }
        self.refresh().await?;
        Ok(req(&self.http, self.tokens.access()).send().await?)
    }

    async fn refresh(&mut self) -> Result<()> {
        self.tokens =
            auth::ms_refresh_at(&self.authority, &self.client_id, self.tokens.refresh()).await?;
        Ok(())
    }
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
    let html = item["body"]["content"]
        .as_str()
        .filter(|c| !c.is_empty())
        .map(str::to_owned);
    let text = match html.as_deref() {
        Some(html) => html_to_text(html),
        None => item["bodyPreview"].as_str().unwrap_or_default().to_string(),
    };
    Some(Delta::Upsert(Box::new(RemoteMsg {
        remote_id: id.to_string(),
        folder: item["parentFolderId"]
            .as_str()
            .unwrap_or(folder)
            .to_string(),
        mailbox,
        thread_hint: item["conversationId"].as_str().map(str::to_owned),
        message_id: item["internetMessageId"].as_str().map(str::to_owned),
        // conversationId already threads Graph mail; header parsing is not worth a round trip.
        in_reply_to: None,
        references: Vec::new(),
        from: addr(&item["from"]["emailAddress"]),
        to: addrs(&item["toRecipients"]),
        cc: addrs(&item["ccRecipients"]),
        date: item["receivedDateTime"]
            .as_str()
            .and_then(parse_iso_ms)
            .unwrap_or_default(),
        subject: item["subject"].as_str().unwrap_or_default().to_string(),
        text,
        html,
        unread: !item["isRead"].as_bool().unwrap_or(false),
        starred: item["flag"]["flagStatus"].as_str() == Some("flagged"),
        important: match item["inferenceClassification"].as_str() {
            Some("focused") => Some(true),
            _ => None,
        },
        attachments: attachments.get(id).cloned().unwrap_or_default(),
    })))
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
        name: v["name"].as_str().unwrap_or("attachment").to_string(),
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
        Mailbox::Starred | Mailbox::Snoozed | Mailbox::All => None,
    }
}

async fn json_res(res: reqwest::Response, what: &str) -> Result<Value> {
    let status = res.status();
    let text = res.text().await?;
    if !status.is_success() {
        bail!("{what} failed ({status}): {}", clip(&text));
    }
    Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
}

async fn check(res: reqwest::Response, what: &str) -> Result<()> {
    if res.status().is_success() {
        return Ok(());
    }
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
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
