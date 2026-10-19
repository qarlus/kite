//! Shared test fakes: an in-process fake Proton Bridge (IMAP4rev1 + UIDPLUS + MOVE + CONDSTORE)
//! with an SMTP sink. Used by both `imap.rs` and `engine.rs`.
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use kite_core::sync::imap::ImapConfig;
use kite_core::sync::{Delta, RemoteMsg};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

// ---- world model ----

pub struct Msg {
    pub uid: u32,
    pub flags: Vec<String>,
    pub modseq: u64,
    pub raw: Vec<u8>,
}

pub struct Folder {
    pub name: String,
    pub attrs: Vec<&'static str>,
    pub uidvalidity: u32,
    pub uidnext: u32,
    pub modseq: u64,
    pub msgs: Vec<Msg>,
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

    pub fn push(&mut self, raw: &[u8], flags: &[&str]) -> u32 {
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

    pub fn set_flag(&mut self, uid: u32, flag: &str, on: bool) {
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

    pub fn expunge(&mut self, uid: u32) {
        self.modseq += 1;
        self.msgs.retain(|m| m.uid != uid);
    }

    pub fn uids(&self) -> Vec<u32> {
        self.msgs.iter().map(|m| m.uid).collect()
    }
}

pub struct Sent {
    pub rcpts: Vec<String>,
    pub raw: Vec<u8>,
}

pub struct World {
    pub user: String,
    pub password: String,
    pub folders: Vec<Folder>,
    pub subscribed: Vec<String>,
    pub sent: Vec<Sent>,
    /// Bridge files SMTP-delivered mail into Sent itself. Off by default so a test opts in.
    pub files_sent_into_sent: bool,
}

impl World {
    pub fn folder(&mut self, name: &str) -> &mut Folder {
        self.folders
            .iter_mut()
            .find(|f| f.name == name)
            .expect("folder")
    }
}

pub fn world() -> World {
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
        subscribed: Vec::new(),
        sent: Vec::new(),
        files_sent_into_sent: false,
    }
}

// ---- fake server ----

/// A running fake server. `stop` makes it drop both new and in-flight connections, so a backend
/// reports an error without the port changing; `start` brings it back.
pub struct Server {
    pub imap_port: u16,
    pub smtp_port: u16,
    alive: Arc<AtomicBool>,
}

impl Server {
    pub fn ports(&self) -> (u16, u16) {
        (self.imap_port, self.smtp_port)
    }

    pub fn stop(&self) {
        self.alive.store(false, Ordering::Relaxed);
    }

    pub fn start(&self) {
        self.alive.store(true, Ordering::Relaxed);
    }
}

pub async fn serve(world: Arc<Mutex<World>>) -> Server {
    let alive = Arc::new(AtomicBool::new(true));
    let imap = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind imap");
    let smtp = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind smtp");
    let (imap_port, smtp_port) = (
        imap.local_addr().expect("addr").port(),
        smtp.local_addr().expect("addr").port(),
    );
    let state = world.clone();
    let flag = alive.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = imap.accept().await {
            if !flag.load(Ordering::Relaxed) {
                drop(stream);
                continue;
            }
            tokio::spawn(imap_conn(stream, state.clone(), flag.clone()));
        }
    });
    let flag = alive.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = smtp.accept().await {
            if !flag.load(Ordering::Relaxed) {
                drop(stream);
                continue;
            }
            tokio::spawn(smtp_conn(stream, world.clone(), flag.clone()));
        }
    });
    Server {
        imap_port,
        smtp_port,
        alive,
    }
}

async fn imap_conn(stream: TcpStream, world: Arc<Mutex<World>>, alive: Arc<AtomicBool>) {
    let mut io = BufReader::new(stream);
    if io
        .get_mut()
        .write_all(b"* OK fake imap ready\r\n")
        .await
        .is_err()
    {
        return;
    }
    let mut selected = String::new();
    loop {
        let mut line = String::new();
        match io.read_line(&mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if !alive.load(Ordering::Relaxed) {
            return;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        let Some((tag, rest)) = line.split_once(' ') else {
            return;
        };
        // APPEND carries a literal: answer the continuation, read exactly that many bytes, then
        // accept the message the way a UIDPLUS server does (Bridge included).
        if let Some((mailbox, flags, size)) = append_head(rest) {
            if io.get_mut().write_all(b"+ send it\r\n").await.is_err() {
                return;
            }
            let mut raw = vec![0u8; size];
            if io.read_exact(&mut raw).await.is_err() {
                return;
            }
            let mut crlf = [0u8; 2];
            if io.read_exact(&mut crlf).await.is_err() {
                return;
            }
            let body = handle_append(
                &mut world.lock().expect("world"),
                tag,
                &mailbox,
                &flags,
                raw,
            );
            if io.get_mut().write_all(&body).await.is_err() {
                return;
            }
            continue;
        }
        let reply = handle(&mut world.lock().expect("world"), tag, rest, &mut selected);
        if io.get_mut().write_all(&reply).await.is_err() {
            return;
        }
    }
}

/// `APPEND "Drafts" (\Draft \Seen) {123}` -> `(mailbox, flags, literal size)`.
fn append_head(rest: &str) -> Option<(String, Vec<String>, usize)> {
    let rest = rest.strip_prefix("APPEND ")?;
    let (mailbox, tail) = unquote(rest);
    let open = tail.find('{')?;
    let size = tail[open + 1..].trim_end_matches('}').trim().parse().ok()?;
    let flags = tail[..open]
        .trim()
        .trim_matches(['(', ')'])
        .split_whitespace()
        .map(str::to_string)
        .collect();
    Some((mailbox, flags, size))
}

fn handle_append(
    world: &mut World,
    tag: &str,
    mailbox: &str,
    flags: &[String],
    raw: Vec<u8>,
) -> Vec<u8> {
    let Some(folder) = world.folders.iter_mut().find(|f| f.name == mailbox) else {
        return format!("{tag} NO no such mailbox\r\n").into_bytes();
    };
    let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
    let uid = folder.push(&raw, &flags);
    let validity = folder.uidvalidity;
    format!("{tag} OK [APPENDUID {validity} {uid}] APPEND completed\r\n").into_bytes()
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
                let attrs = if folder.attrs.is_empty() {
                    String::new()
                } else {
                    folder.attrs.join(" ")
                };
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
                "EXPUNGE" => uid_expunge(world, tag, args, selected),
                _ => reply(tag, "UID"),
            }
        }
        "LOGOUT" => {
            let mut out = b"* BYE bye\r\n".to_vec();
            out.extend(reply(tag, "LOGOUT completed"));
            out
        }
        "CREATE" => {
            let name = unquote(args).0;
            if world.folders.iter().any(|f| f.name == name) {
                return format!("{tag} NO [ALREADYEXISTS] mailbox exists\r\n").into_bytes();
            }
            let uidvalidity = 100 + world.folders.len() as u32;
            world.folders.push(Folder::new(&name, &[], uidvalidity));
            reply(tag, "CREATE completed")
        }
        "SUBSCRIBE" => {
            let name = unquote(args).0;
            if !world.subscribed.contains(&name) {
                world.subscribed.push(name);
            }
            reply(tag, "SUBSCRIBE completed")
        }
        "UNSUBSCRIBE" => reply(tag, "UNSUBSCRIBE completed"),
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
            if want_body {
                format!(" BODY[] {{{}}}\r\n", msg.raw.len())
            } else {
                String::new()
            }
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
        let index = world.folders[source]
            .msgs
            .iter()
            .position(|m| m.uid == uid)
            .expect("msg");
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
    let mut out = format!(
        "* OK [COPYUID {validity} {} {}] moved\r\n",
        from.join(","),
        to.join(",")
    )
    .into_bytes();
    out.extend(reply(tag, "MOVE completed"));
    out
}

fn uid_expunge(world: &mut World, tag: &str, args: &str, selected: &str) -> Vec<u8> {
    let Some(folder) = world.folders.iter_mut().find(|f| f.name == selected) else {
        return reply(tag, "done");
    };
    let all = folder.uids();
    for uid in parse_set(args.trim(), &all) {
        let deleted = folder
            .msgs
            .iter()
            .any(|m| m.uid == uid && m.flags.iter().any(|f| f == "\\Deleted"));
        if deleted {
            folder.expunge(uid);
        }
    }
    reply(tag, "EXPUNGE completed")
}

fn file_sent(world: &Mutex<World>, rcpts: Vec<String>, raw: &[u8]) {
    let mut world = world.lock().expect("world");
    world.sent.push(Sent {
        rcpts,
        raw: raw.to_vec(),
    });
    if !world.files_sent_into_sent {
        return;
    }
    if let Some(folder) = world.folders.iter_mut().find(|f| f.name == "Sent") {
        folder.push(raw, &["\\Seen"]);
    }
}

async fn smtp_conn(stream: TcpStream, world: Arc<Mutex<World>>, alive: Arc<AtomicBool>) {
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
        if !alive.load(Ordering::Relaxed) {
            return;
        }
        if data.is_some() {
            if line == ".\r\n" {
                let raw = data.take().expect("data");
                let rcpts = std::mem::take(&mut rcpts);
                file_sent(&world, rcpts, &raw);
                if io.get_mut().write_all(b"250 queued\r\n").await.is_err() {
                    return;
                }
            } else {
                data.as_mut()
                    .expect("data")
                    .extend_from_slice(line.as_bytes());
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
                rcpts.push(
                    rest.trim()
                        .trim_start_matches("TO:")
                        .trim_matches(['<', '>'])
                        .to_string(),
                );
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
                let end = if end.trim() == "*" {
                    max
                } else {
                    end.parse().unwrap_or(0)
                };
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

pub fn config(imap_port: u16, smtp_port: u16) -> ImapConfig {
    ImapConfig {
        host: "127.0.0.1".into(),
        imap_port,
        smtp_port,
        user: "karl@example.com".into(),
        password: "secret".into(),
        tls: false,
    }
}

pub const FIRST: &[u8] = b"From: Ada Lovelace <ada@example.org>\r\n\
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

pub const SECOND: &[u8] = b"From: Bob <bob@example.org>\r\n\
To: karl@example.com\r\n\
Subject: Lunch?\r\n\
Date: Wed, 07 May 2025 12:30:00 +0000\r\n\
Message-ID: <lunch@example.org>\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
lunch today?\r\n";

pub const BULK_UNSUB: &[u8] = b"From: Newsletter <news@stack.example>\r\n\
To: karl@example.com\r\n\
Subject: Weekly digest\r\n\
Date: Thu, 08 May 2025 10:00:00 +0000\r\n\
Message-ID: <weekly@stack.example>\r\n\
List-Unsubscribe: <mailto:unsub@stack.example>\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
weekly digest\r\n";

pub const BULK_LISTID: &[u8] = b"From: Dev list <list@example.org>\r\n\
To: karl@example.com\r\n\
Subject: Mailing list post\r\n\
Date: Fri, 09 May 2025 10:00:00 +0000\r\n\
Message-ID: <listmsg@example.org>\r\n\
List-Id: <dev.example.org>\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
list message\r\n";

pub fn upserts(deltas: &[Delta]) -> Vec<&RemoteMsg> {
    deltas
        .iter()
        .filter_map(|d| match d {
            Delta::Upsert(msg) => Some(msg.as_ref()),
            _ => None,
        })
        .collect()
}
