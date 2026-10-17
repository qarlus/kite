//! Proton Mail Bridge backend: IMAP4rev1 for reading and mutating mail, hand-rolled SMTP for sending.
//!
//! `sync` is incremental. The opaque cursor is JSON holding, per folder, its `UIDVALIDITY`,
//! `UIDNEXT`, `HIGHESTMODSEQ` (when CONDSTORE is available), the lowest UID held locally
//! (the backfill frontier) and the known UID set range-encoded. Each call fetches new UIDs,
//! flag changes, expunges and one page (500) of older mail, so a huge mailbox never blocks the
//! first paint.
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use async_imap::Session;
use async_imap::imap_proto::rfc4315::UidSetMember;
use async_imap::imap_proto::{Response, ResponseCode, Status};
use async_imap::types::{Fetch, Flag, Name, NameAttribute, Uid};
use futures_util::TryStreamExt;
use mail_parser::{Address, HeaderValue, Message, MessageParser, MimeHeaders};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;

use super::{Delta, Op, RemoteAttachment, RemoteMsg};
use crate::model::{Addr, Mailbox};

const TIMEOUT: Duration = Duration::from_secs(15);
/// Messages per page: the first sync and every backfill step.
const PAGE: usize = 500;

/// Every command is time-boxed, so a stalled Bridge surfaces as an error instead of a hang.
async fn timed<T, E>(command: impl Future<Output = std::result::Result<T, E>>) -> Result<T>
where
    E: Into<anyhow::Error>,
{
    match tokio::time::timeout(TIMEOUT, command).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(error.into()),
        Err(_) => Err(anyhow!("imap command timed out")),
    }
}

#[derive(Clone, Debug)]
pub struct ImapConfig {
    pub host: String,
    pub imap_port: u16,
    pub smtp_port: u16,
    pub user: String,
    pub password: String,
    /// STARTTLS. Plaintext is only accepted for loopback hosts (the test fake server).
    pub tls: bool,
}

pub struct Imap {
    cfg: ImapConfig,
    session: Option<Session<Conn>>,
    caps: Caps,
}

#[derive(Clone, Copy, Default)]
struct Caps {
    mv: bool,
    condstore: bool,
}

impl Imap {
    pub fn new(cfg: ImapConfig) -> Self {
        Self { cfg, session: None, caps: Caps::default() }
    }

    /// Login check for account setup.
    pub async fn verify(&mut self) -> Result<()> {
        self.session().await.map(|_| ())
    }

    pub async fn sync(&mut self, cursor: Option<String>) -> Result<(Vec<Delta>, String)> {
        match self.sync_once(cursor.clone()).await {
            Ok(v) => Ok(v),
            Err(_) => {
                self.session = None;
                self.sync_once(cursor).await
            }
        }
    }

    pub async fn apply(&mut self, op: &Op) -> Result<Option<String>> {
        // Never replay a send: a duplicate delivery is worse than a failed one.
        if matches!(op, Op::Send { .. }) {
            return self.apply_once(op).await;
        }
        match self.apply_once(op).await {
            Ok(v) => Ok(v),
            Err(_) => {
                self.session = None;
                self.apply_once(op).await
            }
        }
    }

    pub async fn attachment(&mut self, remote_msg_id: &str, remote_att_id: &str) -> Result<Vec<u8>> {
        match self.attachment_once(remote_msg_id, remote_att_id).await {
            Ok(bytes) => Ok(bytes),
            Err(first) => {
                self.session = None;
                self.attachment_once(remote_msg_id, remote_att_id).await.map_err(|_| first)
            }
        }
    }

    // ---- connection ----

    async fn session(&mut self) -> Result<&mut Session<Conn>> {
        if self.session.is_none() {
            let (session, caps) = self.open().await?;
            self.session = Some(session);
            self.caps = caps;
        }
        self.session.as_mut().ok_or_else(|| anyhow!("no imap session"))
    }

    async fn open(&self) -> Result<(Session<Conn>, Caps)> {
        if !self.cfg.tls && !is_loopback(&self.cfg.host) {
            bail!("plaintext IMAP is only allowed for loopback hosts");
        }
        let tcp = timed(TcpStream::connect((self.cfg.host.as_str(), self.cfg.imap_port))).await
            .with_context(|| format!("connect to {}:{}", self.cfg.host, self.cfg.imap_port))?;
        let _ = tcp.set_nodelay(true);
        let mut client = async_imap::Client::new(Conn::Plain(tcp));
        timed(client.read_response()).await.context("imap greeting")?;
        if self.cfg.tls {
            timed(client.run_command_and_check_ok("STARTTLS", None)).await.context("imap starttls")?;
            let conn = start_tls(&self.cfg.host, client.into_inner()).await?;
            client = async_imap::Client::new(conn);
        }
        let login = client.login(self.cfg.user.as_str(), self.cfg.password.as_str());
        let login = tokio::time::timeout(TIMEOUT, login).await.map_err(|_| anyhow!("imap login timed out"))?;
        let mut session = login.map_err(|(error, _)| error).context("imap login")?;
        let caps = timed(session.capabilities()).await.context("imap capability")?;
        let caps = Caps { mv: caps.has_str("MOVE"), condstore: caps.has_str("CONDSTORE") };
        Ok((session, caps))
    }

    // ---- sync ----

    async fn sync_once(&mut self, cursor: Option<String>) -> Result<(Vec<Delta>, String)> {
        let mut cursor: Cursor = match cursor.as_deref() {
            Some(raw) => serde_json::from_str(raw).context("sync cursor")?,
            None => Cursor::default(),
        };
        let folders = self.folders().await?;
        let mut deltas = Vec::new();
        for (name, role) in &folders {
            let prev = cursor.folders.get(name).cloned();
            let state = self.sync_folder(name, *role, prev, &mut deltas).await?;
            cursor.folders.insert(name.clone(), state);
        }
        // A folder that vanished (or is now skipped) drops every message we held for it.
        let live: BTreeSet<&str> = folders.iter().map(|(name, _)| name.as_str()).collect();
        let gone: Vec<String> = cursor.folders.keys().filter(|n| !live.contains(n.as_str())).cloned().collect();
        for name in gone {
            let Some(state) = cursor.folders.remove(&name) else { continue };
            for uid in decode_uids(&state.uids) {
                deltas.push(Delta::Delete { remote_id: remote_id(&name, state.uidvalidity, uid) });
            }
        }
        Ok((deltas, serde_json::to_string(&cursor)?))
    }

    async fn sync_folder(
        &mut self,
        folder: &str,
        role: Mailbox,
        prev: Option<FolderSync>,
        out: &mut Vec<Delta>,
    ) -> Result<FolderSync> {
        self.session().await?;
        let condstore = self.caps.condstore;
        let session = self.session().await?;
        let select = async {
            if condstore { session.select_condstore(folder).await } else { session.select(folder).await }
        };
        let selected = timed(select).await.with_context(|| format!("select {folder}"))?;
        let validity = selected.uid_validity.unwrap_or(0);
        let next = selected.uid_next.unwrap_or(0);
        let modseq = selected.highest_modseq;

        let mut known = match &prev {
            Some(p) if p.uidvalidity == validity => decode_uids(&p.uids),
            Some(p) => {
                for uid in decode_uids(&p.uids) {
                    out.push(Delta::Delete { remote_id: remote_id(folder, p.uidvalidity, uid) });
                }
                BTreeSet::new()
            }
            None => BTreeSet::new(),
        };
        let mut low = match &prev {
            Some(p) if p.uidvalidity == validity => p.low,
            _ => 0,
        };

        if known.is_empty() {
            // First sync: the newest page only.
            let all = search(session, "ALL").await?;
            let page: BTreeSet<Uid> = all.iter().rev().take(PAGE).copied().collect();
            known = fetch_msgs(session, &page, folder, role, validity, out).await?;
            low = if known.len() < PAGE { 0 } else { known.first().copied().unwrap_or(0) };
        } else {
            // Expunges: anything we hold that the server no longer lists.
            let all = search(session, "ALL").await?;
            for uid in known.difference(&all) {
                out.push(Delta::Delete { remote_id: remote_id(folder, validity, *uid) });
            }
            known.retain(|uid| all.contains(uid));
            let held = known.clone();

            // New arrivals.
            let from = prev.as_ref().map(|p| p.uidnext).unwrap_or(0).max(1);
            if next > from {
                let page: BTreeSet<Uid> = all.range(from..).copied().collect();
                known.extend(fetch_msgs(session, &page, folder, role, validity, out).await?);
            }

            // Flag changes: CONDSTORE tells us exactly which, otherwise re-read the known set.
            let since = prev.as_ref().and_then(|p| p.modseq);
            let (set, items) = match (condstore, since, modseq) {
                (true, Some(since), Some(now)) if now >= since => {
                    ("1:*".to_string(), format!("(FLAGS) (CHANGEDSINCE {since})"))
                }
                _ => (encode_uids(&known), "(FLAGS)".to_string()),
            };
            let stream = timed(session.uid_fetch(set.as_str(), items.as_str())).await
                .context("uid fetch flags")?;
            let changed: Vec<Fetch> = timed(stream.try_collect()).await.context("uid fetch flags")?;
            for f in &changed {
                if let Some(uid) = f.uid.filter(|u| held.contains(u)) {
                    let (unread, starred) = flags(f);
                    out.push(Delta::Flags { remote_id: remote_id(folder, validity, uid), unread, starred });
                }
            }

            // Backfill one page of older mail.
            if low > 1 {
                let older = search(session, &format!("UID 1:{}", low - 1)).await?;
                let page: BTreeSet<Uid> = older.iter().rev().take(PAGE).copied().collect();
                let got = fetch_msgs(session, &page, folder, role, validity, out).await?;
                known.extend(got.iter().copied());
                low = match got.first().copied() {
                    Some(first) if got.len() >= PAGE => first,
                    _ => 0,
                };
            }
        }

        Ok(FolderSync { uidvalidity: validity, uidnext: next, modseq, low, uids: encode_uids(&known) })
    }

    // ---- mutations ----

    async fn apply_once(&mut self, op: &Op) -> Result<Option<String>> {
        match op {
            Op::Move { remote_id, to } => {
                let folder = self.folder_for(*to).await?;
                self.move_to(remote_id, &folder).await
            }
            Op::MoveToFolder { remote_id, folder } => self.move_to(remote_id, folder).await,
            Op::Flags { remote_id, unread, starred } => {
                let (folder, _, uid) = split_remote_id(remote_id)?;
                let mut add = Vec::new();
                let mut remove = Vec::new();
                for (want, flag) in [(unread, "\\Seen"), (starred, "\\Flagged")] {
                    match want {
                        Some(true) => add.push(flag),
                        Some(false) => remove.push(flag),
                        None => {}
                    }
                }
                if add.is_empty() && remove.is_empty() {
                    return Ok(None);
                }
                let session = self.session().await?;
                timed(session.select(folder)).await.with_context(|| format!("select {folder}"))?;
                for (verb, flags) in [("+FLAGS.SILENT", add), ("-FLAGS.SILENT", remove)] {
                    if !flags.is_empty() {
                        let cmd = format!("UID STORE {uid} {verb} ({})", flags.join(" "));
                        timed(session.run_command_and_check_ok(cmd.as_str())).await
                            .context("uid store")?;
                    }
                }
                Ok(None)
            }
            Op::Send { mime } => {
                self.send(mime).await?;
                Ok(None)
            }
        }
    }

    async fn move_to(&mut self, message: &str, folder: &str) -> Result<Option<String>> {
        let (source, _, uid) = split_remote_id(message)?;
        self.session().await?;
        let mv = self.caps.mv;
        let session = self.session().await?;
        timed(session.select(source)).await.with_context(|| format!("select {source}"))?;
        let copy = if mv {
            uid_move(session, uid, folder).await?
        } else {
            let set = uid.to_string();
            timed(session.uid_copy(set.as_str(), folder)).await.context("uid copy")?;
            timed(session.run_command_and_check_ok(format!(
                "UID STORE {uid} +FLAGS.SILENT (\\Deleted)"
            ))).await
            .context("uid store")?;
            timed(session.run_command_and_check_ok(format!("UID EXPUNGE {uid}"))).await
                .context("uid expunge")?;
            None
        };
        let Some((validity, new_uid)) = copy else { return Ok(None) };
        Ok(Some(remote_id(folder, validity, new_uid)))
    }

    async fn folder_for(&mut self, role: Mailbox) -> Result<String> {
        let folders = self.folders().await?;
        if let Some((name, _)) = folders.iter().find(|(_, r)| *r == role) {
            return Ok(name.clone());
        }
        match role {
            Mailbox::Inbox => Ok("INBOX".into()),
            Mailbox::Sent => Ok("Sent".into()),
            Mailbox::Drafts => Ok("Drafts".into()),
            Mailbox::Archive => Ok("Archive".into()),
            Mailbox::Trash => Ok("Trash".into()),
            Mailbox::Spam => Ok("Spam".into()),
            Mailbox::Starred | Mailbox::Snoozed | Mailbox::All => {
                bail!("{role:?} is not a server folder")
            }
        }
    }

    async fn attachment_once(&mut self, remote_msg_id: &str, remote_att_id: &str) -> Result<Vec<u8>> {
        let (folder, _, uid) = split_remote_id(remote_msg_id)?;
        let index: usize = remote_att_id.parse().context("attachment id")?;
        let session = self.session().await?;
        timed(session.select(folder)).await.with_context(|| format!("select {folder}"))?;
        let stream = timed(session.uid_fetch(uid.to_string(), "(BODY.PEEK[])")).await
            .context("uid fetch")?;
        let fetched: Vec<Fetch> = timed(stream.try_collect()).await.context("uid fetch")?;
        let raw = fetched.first().and_then(|f| f.body()).context("message not found")?;
        let msg = parser().parse(raw).context("unparseable message")?;
        let part = msg
            .parts
            .get(index)
            .with_context(|| format!("no mime part {index}"))?;
        Ok(part.contents().to_vec())
    }

    // ---- smtp ----

    async fn send(&self, mime: &[u8]) -> Result<()> {
        if !self.cfg.tls && !is_loopback(&self.cfg.host) {
            bail!("plaintext SMTP is only allowed for loopback hosts");
        }
        let msg = parser().parse(mime).context("unparseable outbound message")?;
        let mut rcpts = Vec::new();
        for list in [msg.to(), msg.cc(), msg.bcc()].into_iter().flatten() {
            for addr in list.iter() {
                if let Some(email) = addr.address() {
                    rcpts.push(email.to_string());
                }
            }
        }
        if rcpts.is_empty() {
            bail!("refusing to send a message without recipients");
        }
        let body = strip_bcc(mime);

        let tcp = timed(TcpStream::connect((self.cfg.host.as_str(), self.cfg.smtp_port))).await
            .with_context(|| format!("connect to {}:{}", self.cfg.host, self.cfg.smtp_port))?;
        let _ = tcp.set_nodelay(true);
        let mut conn = Conn::Plain(tcp);
        expect(&mut conn, 220, "greeting").await?;
        say(&mut conn, &format!("EHLO {}", self.ehlo_name())).await?;
        expect(&mut conn, 250, "ehlo").await?;
        if self.cfg.tls {
            say(&mut conn, "STARTTLS").await?;
            expect(&mut conn, 220, "starttls").await?;
            conn = start_tls(&self.cfg.host, conn).await?;
            say(&mut conn, &format!("EHLO {}", self.ehlo_name())).await?;
            expect(&mut conn, 250, "ehlo").await?;
        }
        let mut credentials = vec![0];
        credentials.extend_from_slice(self.cfg.user.as_bytes());
        credentials.push(0);
        credentials.extend_from_slice(self.cfg.password.as_bytes());
        say(&mut conn, &format!("AUTH PLAIN {}", base64(&credentials))).await?;
        expect(&mut conn, 235, "auth").await?;
        say(&mut conn, &format!("MAIL FROM:<{}>", self.cfg.user)).await?;
        expect(&mut conn, 250, "mail from").await?;
        for rcpt in &rcpts {
            say(&mut conn, &format!("RCPT TO:<{rcpt}>")).await?;
            expect(&mut conn, 250, "rcpt to").await?;
        }
        say(&mut conn, "DATA").await?;
        expect(&mut conn, 354, "data").await?;
        write_data(&mut conn, &body).await?;
        expect(&mut conn, 250, "message").await?;
        say(&mut conn, "QUIT").await?;
        Ok(())
    }

    fn ehlo_name(&self) -> String {
        self.cfg.user.split('@').nth(1).unwrap_or("localhost").to_string()
    }

    // ---- folder discovery ----

    async fn folders(&mut self) -> Result<Vec<(String, Mailbox)>> {
        let session = self.session().await?;
        let stream = timed(session.list(Some(""), Some("*"))).await.context("list")?;
        let names: Vec<Name> = timed(stream.try_collect()).await.context("list")?;
        let mut out = Vec::new();
        for name in &names {
            if name.attributes().contains(&NameAttribute::NoSelect) {
                continue;
            }
            if let Some(role) = role_of(name.name(), name.attributes()) {
                out.push((name.name().to_string(), role));
            }
        }
        Ok(out)
    }
}

// ---- protocol helpers ----

/// `UID MOVE` with the COPYUID mapping the async-imap wrapper discards.
async fn uid_move(session: &mut Session<Conn>, uid: Uid, dest: &str) -> Result<Option<(u32, Uid)>> {
    let tag = timed(session.run_command(format!("UID MOVE {uid} {}", quote(dest))))
        .await
        .context("uid move")?;
    let mut mapping = None;
    loop {
        let response = timed(session.read_response()).await.context("uid move")?;
        let Some(response) = response else { bail!("imap connection closed during move") };
        match response.parsed() {
            Response::Data { status: Status::Ok, outcome } => {
                if let Some(ResponseCode::CopyUid(validity, _, dest)) = &outcome.code {
                    mapping = Some((*validity, first_uid(dest)));
                }
            }
            Response::Done { tag: done, status, .. } if done == &tag => {
                if *status != Status::Ok {
                    bail!("imap move failed: {status:?}");
                }
                return Ok(mapping);
            }
            _ => {}
        }
    }
}

fn first_uid(set: &[UidSetMember]) -> Uid {
    match set.first() {
        Some(UidSetMember::Uid(uid)) => *uid,
        Some(UidSetMember::UidRange(range)) => *range.start(),
        None => 0,
    }
}

async fn search(session: &mut Session<Conn>, query: &str) -> Result<BTreeSet<Uid>> {
    let found = timed(session.uid_search(query)).await.with_context(|| format!("uid search {query}"))?;
    Ok(found.into_iter().collect())
}

async fn fetch_msgs(
    session: &mut Session<Conn>,
    uids: &BTreeSet<Uid>,
    folder: &str,
    role: Mailbox,
    validity: u32,
    out: &mut Vec<Delta>,
) -> Result<BTreeSet<Uid>> {
    if uids.is_empty() {
        return Ok(BTreeSet::new());
    }
    let set = encode_uids(uids);
    let stream = timed(session.uid_fetch(set.as_str(), "(FLAGS RFC822.SIZE BODY.PEEK[])")).await
        .with_context(|| format!("fetch {folder}"))?;
    let fetched: Vec<Fetch> = timed(stream.try_collect()).await.with_context(|| format!("fetch {folder}"))?;
    let mut got = BTreeSet::new();
    for f in &fetched {
        let Some(uid) = f.uid else { continue };
        got.insert(uid);
        if let Some(msg) = to_msg(f, uid, folder, role, validity) {
            out.push(Delta::Upsert(Box::new(msg)));
        }
    }
    Ok(got)
}

fn to_msg(f: &Fetch, uid: Uid, folder: &str, role: Mailbox, validity: u32) -> Option<RemoteMsg> {
    let msg = parser().parse(f.body()?)?;
    let (unread, starred) = flags(f);
    let (from, to, cc) = (msg.from(), msg.to(), msg.cc());
    Some(RemoteMsg {
        remote_id: remote_id(folder, validity, uid),
        folder: folder.to_string(),
        mailbox: role,
        thread_hint: None,
        message_id: msg.message_id().map(trim_id),
        in_reply_to: match msg.in_reply_to() {
            HeaderValue::Text(text) if !text.trim().is_empty() => Some(trim_id(text)),
            _ => None,
        },
        references: match msg.references() {
            HeaderValue::TextList(list) => list.iter().map(|s| trim_id(s)).collect(),
            HeaderValue::Text(text) => text.split_whitespace().map(trim_id).collect(),
            _ => Vec::new(),
        },
        from: from.and_then(|a| addrs(a).into_iter().next()).unwrap_or(Addr { name: String::new(), email: String::new() }),
        to: to.map(addrs).unwrap_or_default(),
        cc: cc.map(addrs).unwrap_or_default(),
        date: msg.date().map(|d| d.to_timestamp() * 1000).unwrap_or(0),
        subject: msg.subject().unwrap_or_default().to_string(),
        text: match body(&msg, false) {
            Some(text) => text,
            None => msg.body_text(0).map(|t| t.into_owned()).unwrap_or_default(),
        },
        html: body(&msg, true),
        unread,
        starred,
        important: None,
        attachments: msg
            .attachments
            .iter()
            .filter_map(|id| {
                let part = msg.parts.get(*id as usize)?;
                Some(RemoteAttachment {
                    remote_id: id.to_string(),
                    name: part.attachment_name().unwrap_or("attachment").to_string(),
                    mime: part
                        .content_type()
                        .map(|ct| match &ct.c_subtype {
                            Some(sub) => format!("{}/{sub}", ct.c_type),
                            None => ct.c_type.to_string(),
                        })
                        .unwrap_or_else(|| "application/octet-stream".to_string()),
                    size: part.len() as u64,
                })
            })
            .collect(),
    })
}

/// mail-parser lists every body candidate in both lists, so the part itself decides.
fn body(msg: &Message<'_>, html: bool) -> Option<String> {
    let part = msg.parts.iter().find(|p| p.is_text() && p.is_text_html() == html)?;
    part.text_contents().map(str::to_string)
}

fn flags(f: &Fetch) -> (bool, bool) {
    let mut unread = true;
    let mut starred = false;
    for flag in f.flags() {
        match flag {
            Flag::Seen => unread = false,
            Flag::Flagged => starred = true,
            _ => {}
        }
    }
    (unread, starred)
}

fn addrs(address: &Address<'_>) -> Vec<Addr> {
    address
        .iter()
        .filter_map(|a| {
            Some(Addr { name: a.name().unwrap_or_default().to_string(), email: a.address()?.to_string() })
        })
        .collect()
}

/// Headers carry `<id>`; the store threads on the bare id.
fn trim_id(raw: &str) -> String {
    raw.trim().trim_start_matches('<').trim_end_matches('>').trim().to_string()
}

fn parser() -> &'static MessageParser {
    static PARSER: OnceLock<MessageParser> = OnceLock::new();
    PARSER.get_or_init(MessageParser::default)
}

fn remote_id(folder: &str, validity: u32, uid: Uid) -> String {
    format!("{folder}/{validity}/{uid}")
}

fn split_remote_id(id: &str) -> Result<(&str, u32, Uid)> {
    let mut parts = id.rsplitn(3, '/');
    let uid = parts.next().and_then(|s| s.parse().ok());
    let validity = parts.next().and_then(|s| s.parse().ok());
    match (parts.next(), validity, uid) {
        (Some(folder), Some(validity), Some(uid)) => Ok((folder, validity, uid)),
        _ => bail!("malformed remote id: {id}"),
    }
}

fn role_of(name: &str, attributes: &[NameAttribute<'_>]) -> Option<Mailbox> {
    for attribute in attributes {
        match attribute {
            // These duplicate messages we already hold under their real folder.
            NameAttribute::All | NameAttribute::Flagged => return None,
            NameAttribute::Sent => return Some(Mailbox::Sent),
            NameAttribute::Drafts => return Some(Mailbox::Drafts),
            NameAttribute::Trash => return Some(Mailbox::Trash),
            NameAttribute::Junk => return Some(Mailbox::Spam),
            NameAttribute::Archive => return Some(Mailbox::Archive),
            _ => {}
        }
    }
    if name.eq_ignore_ascii_case("INBOX") {
        return Some(Mailbox::Inbox);
    }
    if name.starts_with("Labels/") {
        return None;
    }
    // User folders have no role of their own; they belong to "all mail" and nothing else.
    if name.starts_with("Folders/") {
        return Some(Mailbox::All);
    }
    match name {
        "Sent" => Some(Mailbox::Sent),
        "Drafts" => Some(Mailbox::Drafts),
        "Trash" => Some(Mailbox::Trash),
        "Spam" | "Junk" => Some(Mailbox::Spam),
        "Archive" => Some(Mailbox::Archive),
        _ => None,
    }
}

/// Compact UID set for storage and for `UID FETCH`: `3,5:9,12`.
fn encode_uids(uids: &BTreeSet<Uid>) -> String {
    let mut out = Vec::new();
    let mut iter = uids.iter().copied().peekable();
    while let Some(start) = iter.next() {
        let mut end = start;
        while let Some(next) = iter.peek() {
            if *next != end + 1 {
                break;
            }
            end = *next;
            iter.next();
        }
        out.push(if start == end { start.to_string() } else { format!("{start}:{end}") });
    }
    out.join(",")
}

fn decode_uids(encoded: &str) -> BTreeSet<Uid> {
    let mut out = BTreeSet::new();
    for part in encoded.split(',').filter(|p| !p.is_empty()) {
        match part.split_once(':') {
            Some((start, end)) => {
                if let (Ok(start), Ok(end)) = (start.parse::<Uid>(), end.parse::<Uid>()) {
                    out.extend(start..=end);
                }
            }
            None => {
                if let Ok(uid) = part.parse() {
                    out.insert(uid);
                }
            }
        }
    }
    out
}

fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// AUTH PLAIN payload encoding (RFC 4616).
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "::1" | "[::1]" | "localhost")
}

// ---- bytes on the wire ----

/// One stream type for both plaintext (tests, loopback) and STARTTLS.
#[derive(Debug)]
enum Conn {
    Plain(TcpStream),
    Tls(Box<tokio_native_tls::TlsStream<TcpStream>>),
}

impl AsyncRead for Conn {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Conn::Plain(tcp) => Pin::new(tcp).poll_read(cx, buf),
            Conn::Tls(tls) => Pin::new(tls.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Conn {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Conn::Plain(tcp) => Pin::new(tcp).poll_write(cx, buf),
            Conn::Tls(tls) => Pin::new(tls.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Conn::Plain(tcp) => Pin::new(tcp).poll_flush(cx),
            Conn::Tls(tls) => Pin::new(tls.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Conn::Plain(tcp) => Pin::new(tcp).poll_shutdown(cx),
            Conn::Tls(tls) => Pin::new(tls.as_mut()).poll_shutdown(cx),
        }
    }
}

async fn start_tls(host: &str, conn: Conn) -> Result<Conn> {
    let mut builder = native_tls::TlsConnector::builder();
    if is_loopback(host) {
        // Bridge serves its own self-signed certificate on loopback only.
        builder.danger_accept_invalid_certs(true).danger_accept_invalid_hostnames(true);
    }
    let connector = builder.build().context("tls connector")?;
    let connector = tokio_native_tls::TlsConnector::from(connector);
    let stream = match conn {
        Conn::Plain(tcp) => tcp,
        Conn::Tls(_) => bail!("connection is already encrypted"),
    };
    let tls = tokio::time::timeout(TIMEOUT, connector.connect(host, stream))
        .await
        .map_err(|_| anyhow!("tls handshake timed out"))?
        .context("tls handshake")?;
    Ok(Conn::Tls(Box::new(tls)))
}

async fn say(conn: &mut Conn, line: &str) -> Result<()> {
    timed(async { conn.write_all(format!("{line}\r\n").as_bytes()).await }).await.context("smtp")?;
    timed(conn.flush()).await.context("smtp")
}

async fn read_line(conn: &mut Conn) -> Result<String> {
    let mut line = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        let read = timed(conn.read(&mut byte)).await.context("smtp")?;
        if read == 0 {
            bail!("smtp connection closed");
        }
        if byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
        if line.len() > 8192 {
            bail!("smtp reply too long");
        }
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Ok(String::from_utf8_lossy(&line).into_owned())
}

/// Reads one (possibly multi-line) reply and returns its status code.
async fn read_reply(conn: &mut Conn) -> Result<u16> {
    let first = read_line(conn).await?;
    let code: u16 = first.get(..3).and_then(|c| c.parse().ok()).context("smtp reply code")?;
    if first.as_bytes().get(3) == Some(&b'-') {
        loop {
            let line = read_line(conn).await?;
            if line.starts_with(&format!("{code} ")) || line.len() < 4 {
                break;
            }
        }
    }
    Ok(code)
}

async fn expect(conn: &mut Conn, want: u16, what: &str) -> Result<()> {
    let code = read_reply(conn).await?;
    if code != want {
        bail!("smtp {what} failed with {code}");
    }
    Ok(())
}

/// DATA body: CRLF line endings and dot-stuffing (RFC 5321 4.5.2).
async fn write_data(conn: &mut Conn, mime: &[u8]) -> Result<()> {
    let mut end = mime.len();
    while end > 0 && (mime[end - 1] == b'\r' || mime[end - 1] == b'\n') {
        end -= 1;
    }
    let mut out = Vec::with_capacity(mime.len() + 16);
    for line in mime[..end].split(|b| *b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.first() == Some(&b'.') {
            out.push(b'.');
        }
        out.extend_from_slice(line);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b".\r\n");
    timed(conn.write_all(&out)).await.context("smtp")?;
    timed(conn.flush()).await.context("smtp")
}

/// Bridge files the sent copy itself, so the Bcc recipients must never see the header.
fn strip_bcc(mime: &[u8]) -> Vec<u8> {
    let split = mime
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (i, 4))
        .or_else(|| mime.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2)));
    let Some((head_end, sep)) = split else { return mime.to_vec() };
    let mut out = Vec::with_capacity(mime.len());
    let mut dropping = false;
    for line in mime[..head_end].split(|b| *b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let continuation = matches!(line.first(), Some(b' ' | b'\t'));
        if !continuation {
            dropping = line
                .split(|b| *b == b':')
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case(b"bcc"));
        }
        if !dropping {
            out.extend_from_slice(line);
            out.extend_from_slice(b"\r\n");
        }
    }
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(&mime[head_end + sep..]);
    out
}

// ---- cursor ----

#[derive(Default, Serialize, Deserialize)]
struct Cursor {
    #[serde(default)]
    folders: BTreeMap<String, FolderSync>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct FolderSync {
    uidvalidity: u32,
    uidnext: Uid,
    modseq: Option<u64>,
    /// Lowest UID held locally; 0 once the backfill is complete.
    low: Uid,
    /// Known UIDs, range encoded.
    uids: String,
}
