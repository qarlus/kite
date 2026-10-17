//! Integration tests for the Proton Bridge backend against an in-process fake
//! IMAP4rev1 (UIDPLUS + MOVE + CONDSTORE) server and an SMTP sink.
use std::sync::{Arc, Mutex};

use kite_core::model::Mailbox;
use kite_core::sync::imap::{Imap, ImapConfig};
use kite_core::sync::{Delta, Op};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

// ---- world model ----

struct Msg {
    uid: u32,
    flags: Vec<String>,
    modseq: u64,
    raw: Vec<u8>,
}

struct Folder {
    name: String,
    attrs: Vec<&'static str>,
    uidvalidity: u32,
    uidnext: u32,
    modseq: u64,
    msgs: Vec<Msg>,
}

impl Folder {
    fn new(name: &str, attrs: &[&'static str], uidvalidity: u32) -> Self {
        Self {
            name: name.to_string(),
            attrs: attrs.to_vec(),
            uidvalidity,
            uidnext: 1,
            modseq: 0,
            msgs: Vec::new(),
        }
    }

    fn push(&mut self, raw: &[u8], flags: &[&str]) -> u32 {
        let uid = self.uidnext;
        self.uidnext += 1;
        self.modseq += 1;
        self.msgs.push(Msg {
            uid,
            flags: flags.iter().map(|f| f.to_string()).collect(),
            modseq: self.modseq,
            raw: raw.to_vec(),
        });
        uid
    }

    fn set_flag(&mut self, uid: u32, flag: &str, on: bool) {
        self.modseq += 1;
        let modseq = self.modseq;
        if let Some(msg) = self.msgs.iter_mut().find(|m| m.uid == uid) {
            msg.modseq = modseq;
            let flag = flag.to_string();
            if on {
                if !msg.flags.contains(&flag) {
                    msg.flags.push(flag);
                }
            } else {
                msg.flags.retain(|f| *f != flag);
            }
        }
    }

    fn expunge(&mut self, uid: u32) {
        self.modseq += 1;
        self.msgs.retain(|m| m.uid != uid);
    }

    fn uids(&self) -> Vec<u32> {
        self.msgs.iter().map(|m| m.uid).collect()
    }
}

struct Sent {
    rcpts: Vec<String>,
    raw: Vec<u8>,
}

struct World {
    user: String,
    password: String,
    folders: Vec<Folder>,
    sent: Vec<Sent>,
}

impl World {
    fn folder(&mut self, name: &str) -> &mut Folder {
        self.folders.iter_mut().find(|f| f.name == name).expect("folder")
    }
}

fn world() -> World {
    World {
        user: "karl@example.com".into(),
        password: "secret".into(),
        folders: vec![
            Folder::new("INBOX", &[], 1),
            Folder::new("Sent", &["\\Sent"], 2),
            Folder::new("Drafts", &["\\Drafts"], 3),
            Folder::new("Archive", &["\\Archive"], 4),
            Folder::new("Trash", &["\\Trash"], 5),
            Folder::new("Spam", &["\\Junk"], 6),
            Folder::new("All Mail", &["\\All"], 7),
            Folder::new("Starred", &["\\Flagged"], 8),
            Folder::new("Folders/Travel", &[], 9),
        ],
        sent: Vec::new(),
    }
}

// ---- fake server ----

async fn serve(world: Arc<Mutex<World>>) -> (u16, u16) {
    let imap = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind imap");
    let smtp = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind smtp");
    let (imap_port, smtp_port) = (
        imap.local_addr().expect("addr").port(),
        smtp.local_addr().expect("addr").port(),
    );
    let state = world.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = imap.accept().await {
            tokio::spawn(imap_conn(stream, state.clone()));
        }
    });
    tokio::spawn(async move {
        while let Ok((stream, _)) = smtp.accept().await {
            tokio::spawn(smtp_conn(stream, world.clone()));
        }
    });
    (imap_port, smtp_port)
}

async fn imap_conn(stream: TcpStream, world: Arc<Mutex<World>>) {
    let mut io = BufReader::new(stream);
    if io.get_mut().write_all(b"* OK fake imap ready\r\n").await.is_err() {
        return;
    }
    let mut selected = String::new();
    loop {
        let mut line = String::new();
        match io.read_line(&mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let line = line.trim_end_matches(['\r', '\n']);
        let Some((tag, rest)) = line.split_once(' ') else { return };
        let reply = handle(&mut world.lock().expect("world"), tag, rest, &mut selected);
        if io.get_mut().write_all(&reply).await.is_err() {
            return;
        }
    }
}

fn reply(tag: &str, text: &str) -> Vec<u8> {
    format!("{tag} OK {text}\r\n").into_bytes()
}

fn handle(world: &mut World, tag: &str, rest: &str, selected: &mut String) -> Vec<u8> {
    let (verb, args) = rest.split_once(' ').unwrap_or((rest, ""));
    match verb.to_ascii_uppercase().as_str() {
        "CAPABILITY" => {
            let mut out = b"* CAPABILITY IMAP4rev1 UIDPLUS MOVE CONDSTORE SPECIAL-USE\r\n".to_vec();
            out.extend(reply(tag, "CAPABILITY"));
            out
        }
        "LOGIN" => {
            let creds = quoted(args);
            if creds == [world.user.clone(), world.password.clone()] {
                reply(tag, "LOGIN completed")
            } else {
                format!("{tag} NO [AUTHENTICATIONFAILED] invalid credentials\r\n").into_bytes()
            }
        }
        "LIST" => {
            let mut out = Vec::new();
            for folder in &world.folders {
                let attrs =
                    if folder.attrs.is_empty() { String::new() } else { folder.attrs.join(" ") };
                let line = format!("* LIST ({attrs}) \"/\" \"{}\"\r\n", folder.name);
                out.extend(line.into_bytes());
            }
            out.extend(reply(tag, "LIST completed"));
            out
        }
        "SELECT" | "EXAMINE" => {
            *selected = unquote(args).0;
            let Some(folder) = world.folders.iter().find(|f| f.name == *selected) else {
                return format!("{tag} NO no such mailbox\r\n").into_bytes();
            };
            let mut out = String::new();
            out.push_str("* FLAGS (\\Seen \\Flagged \\Deleted \\Draft)\r\n");
            out.push_str(&format!("* {} EXISTS\r\n", folder.msgs.len()));
            out.push_str(&format!("* OK [UIDVALIDITY {}]\r\n", folder.uidvalidity));
            out.push_str(&format!("* OK [UIDNEXT {}]\r\n", folder.uidnext));
            out.push_str(&format!("* OK [HIGHESTMODSEQ {}]\r\n", folder.modseq));
            out.push_str(&format!("{tag} OK [READ-WRITE] SELECT completed\r\n"));
            out.into_bytes()
        }
        "UID" => {
            let (sub, args) = args.split_once(' ').unwrap_or((args, ""));
            match sub.to_ascii_uppercase().as_str() {
                "SEARCH" => uid_search(world, tag, args, selected),
                "FETCH" => uid_fetch(world, tag, args, selected),
                "STORE" => uid_store(world, tag, args, selected),
                "MOVE" => uid_move(world, tag, args, selected),
                _ => reply(tag, "UID"),
            }
        }
        "LOGOUT" => {
            let mut out = b"* BYE bye\r\n".to_vec();
            out.extend(reply(tag, "LOGOUT completed"));
            out
        }
        _ => reply(tag, "ok"),
    }
}

fn uid_search(world: &mut World, tag: &str, args: &str, selected: &str) -> Vec<u8> {
    let Some(folder) = world.folders.iter().find(|f| f.name == selected) else {
        return reply(tag, "done");
    };
    let all = folder.uids();
    let uids = match args
        .split_once(' ')
        .filter(|(key, _)| key.eq_ignore_ascii_case("UID"))
        .map(|(_, set)| set)
    {
        Some(set) => parse_set(set.trim(), &all),
        // "ALL" and anything we do not model
        None => all,
    };
    let list: Vec<String> = uids.iter().map(u32::to_string).collect();
    let mut out = format!("* SEARCH {}\r\n", list.join(" ")).into_bytes();
    out.extend(reply(tag, "SEARCH completed"));
    out
}

fn uid_fetch(world: &mut World, tag: &str, args: &str, selected: &str) -> Vec<u8> {
    let (set, items) = args.split_once(' ').unwrap_or((args, ""));
    let upper = items.to_ascii_uppercase();
    let Some(folder) = world.folders.iter().find(|f| f.name == selected) else {
        return reply(tag, "done");
    };
    let all = folder.uids();
    let want = parse_set(set, &all);
    let want_body = upper.contains("BODY");
    let since: Option<u64> = upper
        .split_once("CHANGEDSINCE ")
        .and_then(|(_, n)| n.trim_end_matches(')').trim().parse().ok());
    let mut out = Vec::new();
    for (index, msg) in folder.msgs.iter().enumerate() {
        if !want.contains(&msg.uid) {
            continue;
        }
        if since.is_some_and(|since| msg.modseq <= since) {
            continue;
        }
        let mut parts = vec![format!("UID {}", msg.uid)];
        if upper.contains("FLAGS") {
            parts.push(format!("FLAGS ({})", msg.flags.join(" ")));
        }
        if upper.contains("RFC822.SIZE") {
            parts.push(format!("RFC822.SIZE {}", msg.raw.len()));
        }
        let mut line = format!(
            "* {} FETCH ({}{}",
            index + 1,
            parts.join(" "),
            if want_body { format!(" BODY[] {{{}}}\r\n", msg.raw.len()) } else { String::new() }
        )
        .into_bytes();
        if want_body {
            line.extend_from_slice(&msg.raw);
        }
        line.extend_from_slice(b")\r\n");
        out.extend(line);
    }
    out.extend(reply(tag, "FETCH completed"));
    out
}

fn uid_store(world: &mut World, tag: &str, args: &str, selected: &str) -> Vec<u8> {
    let (set, rest) = args.split_once(' ').unwrap_or((args, ""));
    let (verb, flags) = rest.split_once(' ').unwrap_or((rest, ""));
    let flags: Vec<&str> = flags.trim_matches(['(', ')']).split_whitespace().collect();
    let add = !verb.starts_with('-');
    let Some(folder) = world.folders.iter_mut().find(|f| f.name == selected) else {
        return reply(tag, "done");
    };
    let all = folder.uids();
    for uid in parse_set(set, &all) {
        for flag in &flags {
            folder.set_flag(uid, flag, add);
        }
    }
    reply(tag, "STORE completed")
}

fn uid_move(world: &mut World, tag: &str, args: &str, selected: &str) -> Vec<u8> {
    let (set, dest) = args.split_once(' ').unwrap_or((args, ""));
    let dest = unquote(dest).0;
    let Some(source) = world.folders.iter().position(|f| f.name == selected) else {
        return reply(tag, "done");
    };
    let Some(target) = world.folders.iter().position(|f| f.name == dest) else {
        return format!("{tag} NO no such mailbox\r\n").into_bytes();
    };
    let all = world.folders[source].uids();
    let uids = parse_set(set, &all);
    let validity = world.folders[target].uidvalidity;
    let mut moved = Vec::new();
    for uid in uids {
        let index =
            world.folders[source].msgs.iter().position(|m| m.uid == uid).expect("msg");
        let msg = world.folders[source].msgs.remove(index);
        let flags: Vec<&str> = msg.flags.iter().map(String::as_str).collect();
        let new_uid = world.folders[target].push(&msg.raw, &flags);
        moved.push((uid, new_uid));
    }
    if moved.is_empty() {
        return reply(tag, "MOVE completed");
    }
    let from: Vec<String> = moved.iter().map(|(uid, _)| uid.to_string()).collect();
    let to: Vec<String> = moved.iter().map(|(_, uid)| uid.to_string()).collect();
    let mut out = format!("* OK [COPYUID {validity} {} {}] moved\r\n", from.join(","), to.join(","))
        .into_bytes();
    out.extend(reply(tag, "MOVE completed"));
    out
}

async fn smtp_conn(stream: TcpStream, world: Arc<Mutex<World>>) {
    let mut io = BufReader::new(stream);
    if io.get_mut().write_all(b"220 fake smtp\r\n").await.is_err() {
        return;
    }
    let mut rcpts = Vec::new();
    let mut data: Option<Vec<u8>> = None;
    loop {
        let mut line = String::new();
        match io.read_line(&mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if data.is_some() {
            if line == ".\r\n" {
                let raw = data.take().expect("data");
                let rcpts = std::mem::take(&mut rcpts);
                world.lock().expect("world").sent.push(Sent { rcpts, raw });
                if io.get_mut().write_all(b"250 queued\r\n").await.is_err() {
                    return;
                }
            } else {
                data.as_mut().expect("data").extend_from_slice(line.as_bytes());
            }
            continue;
        }
        let line = line.trim_end();
        let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
        let response = match verb.to_ascii_uppercase().as_str() {
            "EHLO" | "HELO" => "250-fake\r\n250 AUTH PLAIN\r\n".to_string(),
            "AUTH" => "235 authenticated\r\n".to_string(),
            "MAIL" => "250 sender ok\r\n".to_string(),
            "RCPT" => {
                rcpts.push(rest.trim().trim_start_matches("TO:").trim_matches(['<', '>']).to_string());
                "250 recipient ok\r\n".to_string()
            }
            "DATA" => {
                data = Some(Vec::new());
                "354 go ahead\r\n".to_string()
            }
            "QUIT" => "221 bye\r\n".to_string(),
            _ => "250 ok\r\n".to_string(),
        };
        if io.get_mut().write_all(response.as_bytes()).await.is_err() {
            return;
        }
    }
}

// ---- helpers ----

/// Splits the first (possibly quoted) argument off the front of `args`.
fn unquote(args: &str) -> (String, String) {
    let args = args.trim();
    if let Some(rest) = args.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = rest.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => return (out, chars.as_str().to_string()),
                '\\' => out.extend(chars.next()),
                c => out.push(c),
            }
        }
        return (out, String::new());
    }
    match args.split_once(' ') {
        Some((head, rest)) => (head.to_string(), rest.to_string()),
        None => (args.to_string(), String::new()),
    }
}

fn quoted(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = args.to_string();
    while rest.contains('"') {
        let (value, tail) = unquote(&rest[rest.find('"').expect("quote")..]);
        out.push(value);
        rest = tail;
    }
    out
}

fn parse_set(set: &str, all: &[u32]) -> Vec<u32> {
    let max = all.last().copied().unwrap_or(0);
    let mut out = Vec::new();
    for part in set.split(',') {
        match part.split_once(':') {
            Some((start, end)) => {
                let start: u32 = start.parse().unwrap_or(0);
                let end = if end.trim() == "*" { max } else { end.parse().unwrap_or(0) };
                for uid in all {
                    if *uid >= start && *uid <= end {
                        out.push(*uid);
                    }
                }
            }
            None => {
                if let Ok(uid) = part.parse()
                    && all.contains(&uid)
                {
                    out.push(uid);
                }
            }
        }
    }
    out
}

fn config(imap_port: u16, smtp_port: u16) -> ImapConfig {
    ImapConfig {
        host: "127.0.0.1".into(),
        imap_port,
        smtp_port,
        user: "karl@example.com".into(),
        password: "secret".into(),
        tls: false,
    }
}

const FIRST: &[u8] = b"From: Ada Lovelace <ada@example.org>\r\n\
To: Karl <karl@example.com>, bob@example.com\r\n\
Cc: carol@example.com\r\n\
Subject: Quarterly numbers\r\n\
Date: Tue, 06 May 2025 10:00:00 +0000\r\n\
Message-ID: <abc@example.org>\r\n\
In-Reply-To: <parent@example.org>\r\n\
References: <root@example.org> <parent@example.org>\r\n\
Content-Type: multipart/mixed; boundary=\"b1\"\r\n\
\r\n\
--b1\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
plain body\r\n\
--b1\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<p>html body</p>\r\n\
--b1\r\n\
Content-Type: application/pdf; name=\"report.pdf\"\r\n\
Content-Disposition: attachment; filename=\"report.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0xLjQ=\r\n\
--b1--\r\n";

const SECOND: &[u8] = b"From: Bob <bob@example.org>\r\n\
To: karl@example.com\r\n\
Subject: Lunch?\r\n\
Date: Wed, 07 May 2025 12:30:00 +0000\r\n\
Message-ID: <lunch@example.org>\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
lunch today?\r\n";

fn upserts(deltas: &[Delta]) -> Vec<&kite_core::sync::RemoteMsg> {
    deltas
        .iter()
        .filter_map(|d| match d {
            Delta::Upsert(msg) => Some(msg.as_ref()),
            _ => None,
        })
        .collect()
}

// ---- tests ----

#[tokio::test]
async fn first_sync_parses_messages() {
    let world = Arc::new(Mutex::new(world()));
    world.lock().expect("world").folder("INBOX").push(FIRST, &["\\Flagged"]);
    world.lock().expect("world").folder("INBOX").push(SECOND, &["\\Seen"]);
    let (imap_port, smtp_port) = serve(world.clone()).await;

    let mut imap = Imap::new(config(imap_port, smtp_port));
    let (deltas, cursor) = imap.sync(None).await.expect("sync");

    let messages = upserts(&deltas);
    assert_eq!(messages.len(), 2, "two inbox messages");
    let first = messages.iter().find(|m| m.subject == "Quarterly numbers").expect("first");
    assert_eq!(first.remote_id, "INBOX/1/1");
    assert_eq!(first.folder, "INBOX");
    assert_eq!(first.mailbox, Mailbox::Inbox);
    assert_eq!(first.from.name, "Ada Lovelace");
    assert_eq!(first.from.email, "ada@example.org");
    assert_eq!(first.to.len(), 2, "two To addresses");
    assert_eq!(first.to[1].email, "bob@example.com");
    assert_eq!(first.cc[0].email, "carol@example.com");
    assert_eq!(first.date, 1_746_525_600_000);
    assert_eq!(first.message_id.as_deref(), Some("abc@example.org"));
    assert_eq!(first.in_reply_to.as_deref(), Some("parent@example.org"));
    assert_eq!(first.references, vec!["root@example.org", "parent@example.org"]);
    assert!(first.text.contains("plain body"));
    assert!(first.html.as_deref().expect("html").contains("<p>html body</p>"));
    assert!(first.unread);
    assert!(first.starred);
    assert_eq!(first.attachments.len(), 1);
    assert_eq!(first.attachments[0].name, "report.pdf");
    assert_eq!(first.attachments[0].mime, "application/pdf");
    assert_eq!(first.attachments[0].size, 8);

    let second = messages.iter().find(|m| m.subject == "Lunch?").expect("second");
    assert!(!second.unread);
    assert!(!second.starred);
    assert!(second.html.is_none());
    assert!(cursor.contains("INBOX"), "cursor remembers the folder");
}

#[tokio::test]
async fn incremental_sync_reports_new_flags_and_deletes() {
    let world = Arc::new(Mutex::new(world()));
    world.lock().expect("world").folder("INBOX").push(FIRST, &[]);
    world.lock().expect("world").folder("INBOX").push(SECOND, &[]);
    world.lock().expect("world").folder("INBOX").push(FIRST, &[]);
    let (imap_port, smtp_port) = serve(world.clone()).await;

    let mut imap = Imap::new(config(imap_port, smtp_port));
    let (_, cursor) = imap.sync(None).await.expect("first sync");

    {
        let mut world = world.lock().expect("world");
        let inbox = world.folder("INBOX");
        inbox.set_flag(1, "\\Seen", true);
        inbox.expunge(3);
        inbox.push(SECOND, &[]);
    }

    let (deltas, _) = imap.sync(Some(cursor)).await.expect("second sync");
    assert_eq!(deltas.len(), 3, "one of each delta: {deltas:#?}");
    assert!(matches!(&deltas[0], Delta::Delete { remote_id } if remote_id == "INBOX/1/3"));
    assert!(
        matches!(&deltas[1], Delta::Upsert(msg) if msg.remote_id == "INBOX/1/4" && msg.subject == "Lunch?")
    );
    assert!(
        matches!(&deltas[2], Delta::Flags { remote_id, unread, starred } if remote_id == "INBOX/1/1" && !unread && !starred)
    );
}

#[tokio::test]
async fn move_returns_the_new_remote_id() {
    let world = Arc::new(Mutex::new(world()));
    world.lock().expect("world").folder("INBOX").push(FIRST, &[]);
    world.lock().expect("world").folder("INBOX").push(SECOND, &[]);
    let (imap_port, smtp_port) = serve(world.clone()).await;

    let mut imap = Imap::new(config(imap_port, smtp_port));
    let (deltas, _) = imap.sync(None).await.expect("sync");
    let message = upserts(&deltas)
        .into_iter()
        .find(|m| m.subject == "Lunch?")
        .expect("message")
        .remote_id
        .clone();

    let new_id = imap
        .apply(&Op::Move { remote_id: message.clone(), to: Mailbox::Archive })
        .await
        .expect("move")
        .expect("copyuid");
    assert_eq!(new_id, "Archive/4/1");

    let world = world.lock().expect("world");
    let archive = world.folders.iter().find(|f| f.name == "Archive").expect("archive");
    assert_eq!(archive.uids(), vec![1]);
    assert_eq!(archive.msgs[0].raw, SECOND);
    let inbox = world.folders.iter().find(|f| f.name == "INBOX").expect("inbox");
    assert_eq!(inbox.uids(), vec![1]);
}

#[tokio::test]
async fn send_delivers_to_every_recipient_without_the_bcc_header() {
    let world = Arc::new(Mutex::new(world()));
    let (imap_port, smtp_port) = serve(world.clone()).await;

    let mime = b"From: Karl <karl@example.com>\r\n\
To: Alice <alice@example.org>\r\n\
Cc: carol@example.org\r\n\
Bcc: hidden@example.org\r\n\
Subject: Hi\r\n\
Message-ID: <new@example.com>\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
first line\r\n\
.hidden line\r\n\
last line\r\n";

    let mut imap = Imap::new(config(imap_port, smtp_port));
    imap.apply(&Op::Send { mime: mime.to_vec() }).await.expect("send");

    let world = world.lock().expect("world");
    assert_eq!(world.sent.len(), 1);
    let sent = &world.sent[0];
    let mut rcpts = sent.rcpts.clone();
    rcpts.sort();
    assert_eq!(rcpts, vec!["alice@example.org", "carol@example.org", "hidden@example.org"]);

    let raw = String::from_utf8_lossy(&sent.raw).to_string();
    assert!(!raw.to_ascii_lowercase().contains("bcc:"), "bcc stripped: {raw}");
    assert!(raw.contains("\r\n..hidden line\r\n"), "dot stuffed: {raw:?}");
    assert!(raw.replace("\r\n..", "\r\n.").contains("\r\n.hidden line\r\n"));
}

#[tokio::test]
async fn wrong_password_fails_verify() {
    let world = Arc::new(Mutex::new(world()));
    let (imap_port, smtp_port) = serve(world.clone()).await;

    let mut cfg = config(imap_port, smtp_port);
    cfg.password = "wrong".into();
    let mut imap = Imap::new(cfg);
    assert!(imap.verify().await.is_err());
}

#[tokio::test]
async fn attachment_downloads_the_mime_part() {
    let world = Arc::new(Mutex::new(world()));
    world.lock().expect("world").folder("INBOX").push(FIRST, &[]);
    let (imap_port, smtp_port) = serve(world.clone()).await;

    let mut imap = Imap::new(config(imap_port, smtp_port));
    let (deltas, _) = imap.sync(None).await.expect("sync");
    let message = upserts(&deltas).into_iter().next().expect("message");
    let attachment = message.attachments[0].clone();

    let bytes = imap
        .attachment(&message.remote_id, &attachment.remote_id)
        .await
        .expect("attachment");
    assert_eq!(bytes, b"%PDF-1.4");
}
