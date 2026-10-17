//! Local SQLite + FTS5 store: the source of truth the UI reads and the engine writes.
//!
//! The same file is opened twice (a reader for the UI, a writer for sync); WAL makes that work.
//! Every write is one short transaction so a UI action never waits behind a long sync batch.
use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Transaction, params, params_from_iter};

use crate::model::*;
use crate::sync::{Delta, Op, RemoteMsg};

const SCHEMA_V1: &str = r#"
CREATE TABLE accounts (
  id INTEGER PRIMARY KEY, provider TEXT NOT NULL, email TEXT NOT NULL UNIQUE,
  name TEXT NOT NULL, cursor TEXT
);

CREATE TABLE threads (
  id INTEGER PRIMARY KEY, account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  subject TEXT NOT NULL, norm_subject TEXT NOT NULL, snippet TEXT NOT NULL,
  participants TEXT NOT NULL, date INTEGER NOT NULL, unread INTEGER NOT NULL,
  starred INTEGER NOT NULL, important INTEGER NOT NULL, attachments INTEGER NOT NULL,
  labels TEXT NOT NULL DEFAULT '[]', count INTEGER NOT NULL, mailbox TEXT NOT NULL,
  snoozed_until INTEGER, remind_after INTEGER
);
CREATE INDEX threads_view ON threads(mailbox, date DESC);
CREATE INDEX threads_account ON threads(account_id, date DESC);

CREATE TABLE messages (
  id INTEGER PRIMARY KEY, account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  thread_id INTEGER NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
  remote_id TEXT NOT NULL, folder TEXT NOT NULL, mailbox TEXT NOT NULL, thread_hint TEXT,
  message_id TEXT, in_reply_to TEXT, refs TEXT NOT NULL DEFAULT '[]',
  from_name TEXT NOT NULL, from_email TEXT NOT NULL, to_json TEXT NOT NULL, cc_json TEXT NOT NULL,
  date INTEGER NOT NULL, subject TEXT NOT NULL, norm_subject TEXT NOT NULL,
  text TEXT NOT NULL, html TEXT, unread INTEGER NOT NULL, starred INTEGER NOT NULL,
  important INTEGER NOT NULL, important_src INTEGER NOT NULL DEFAULT 0,
  from_self INTEGER NOT NULL DEFAULT 0, UNIQUE(account_id, remote_id)
);
CREATE INDEX messages_thread ON messages(thread_id, date);
CREATE INDEX messages_subject ON messages(account_id, norm_subject, date);

CREATE TABLE message_people (
  message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  email TEXT NOT NULL, domain TEXT NOT NULL, name TEXT NOT NULL, kind TEXT NOT NULL
);
CREATE INDEX people_email ON message_people(email);
CREATE INDEX people_domain ON message_people(domain);
CREATE INDEX people_msg ON message_people(message_id);

CREATE TABLE attachments (
  id INTEGER PRIMARY KEY, message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  remote_id TEXT NOT NULL, name TEXT NOT NULL, mime TEXT NOT NULL, size INTEGER NOT NULL
);
CREATE INDEX attachments_msg ON attachments(message_id);

CREATE TABLE contacts (
  email TEXT PRIMARY KEY, domain TEXT NOT NULL, name TEXT NOT NULL,
  seen INTEGER NOT NULL DEFAULT 0, sent_to INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE thread_keys (
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  key TEXT NOT NULL, thread_id INTEGER NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
  PRIMARY KEY (account_id, key)
) WITHOUT ROWID;

CREATE TABLE pending_ops (
  id INTEGER PRIMARY KEY, account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  message_id INTEGER, op TEXT NOT NULL, inverse TEXT, at INTEGER NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX pending_due ON pending_ops(account_id, at, id);
CREATE INDEX pending_msg ON pending_ops(message_id);

CREATE TABLE undo_seq (token INTEGER PRIMARY KEY AUTOINCREMENT);
CREATE TABLE undo_log (
  token INTEGER NOT NULL, account_id INTEGER NOT NULL, thread_id INTEGER NOT NULL,
  message_id INTEGER, mailbox TEXT, unread INTEGER, starred INTEGER, folder TEXT,
  snoozed_until INTEGER, remind_after INTEGER, op_id INTEGER, inverse TEXT
);
CREATE INDEX undo_log_token ON undo_log(token);

CREATE TABLE outbox (
  id INTEGER PRIMARY KEY, account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  draft TEXT NOT NULL, at INTEGER NOT NULL
);
CREATE INDEX outbox_due ON outbox(at, id);

CREATE TABLE splits (
  id INTEGER PRIMARY KEY, name TEXT NOT NULL, query TEXT NOT NULL, position INTEGER NOT NULL
);
CREATE TABLE kv (k TEXT PRIMARY KEY, v TEXT NOT NULL) WITHOUT ROWID;

CREATE VIRTUAL TABLE msgs_fts USING fts5(
  subject, sender, recipients, body,
  content='messages', content_rowid='id', tokenize='unicode61 remove_diacritics 2'
);
CREATE TRIGGER messages_ai AFTER INSERT ON messages BEGIN
  INSERT INTO msgs_fts(rowid, subject, sender, recipients, body)
  VALUES (new.id, new.subject, new.from_email, new.to_json || ' ' || new.cc_json, new.text);
END;
CREATE TRIGGER messages_ad AFTER DELETE ON messages BEGIN
  INSERT INTO msgs_fts(msgs_fts, rowid, subject, sender, recipients, body)
  VALUES ('delete', old.id, old.subject, old.from_email, old.to_json || ' ' || old.cc_json, old.text);
END;
CREATE TRIGGER messages_au AFTER UPDATE OF subject, from_email, to_json, cc_json, text ON messages BEGIN
  INSERT INTO msgs_fts(msgs_fts, rowid, subject, sender, recipients, body)
  VALUES ('delete', old.id, old.subject, old.from_email, old.to_json || ' ' || old.cc_json, old.text);
  INSERT INTO msgs_fts(rowid, subject, sender, recipients, body)
  VALUES (new.id, new.subject, new.from_email, new.to_json || ' ' || new.cc_json, new.text);
END;

INSERT INTO splits(id, name, query, position) VALUES (1, 'Important', 'is:important', 0);
INSERT INTO splits(id, name, query, position) VALUES (2, 'Other', '-is:important', 1);
"#;

/// Schema v2 adds the custom folder catalog and the index that makes a `View::folder` lookup one
/// indexed query. Folder identities are account-scoped, so two accounts may own the same provider
/// folder id. `IF NOT EXISTS` keeps this safe for databases created before the catalog existed.
const SCHEMA_V2: &str = r#"
CREATE TABLE IF NOT EXISTS folders (
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  id TEXT NOT NULL, name TEXT NOT NULL, created INTEGER NOT NULL,
  PRIMARY KEY (account_id, id)
);
CREATE UNIQUE INDEX IF NOT EXISTS folders_account_name ON folders(account_id, name COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS messages_folder ON messages(folder);
"#;

/// Fields the engine needs to build reply MIME. `references` already ends with `message_id_header`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyCtx {
    pub account_id: Id,
    pub message_id_header: String,
    pub references: Vec<String>,
    pub subject: String,
    pub from: Addr,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        Store::init(Connection::open(path)?)
    }

    pub fn memory() -> Result<Store> {
        Store::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Store> {
        conn.busy_timeout(Duration::from_millis(5000))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON; PRAGMA temp_store=memory; PRAGMA mmap_size=268435456;")?;
        let version = conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?;
        if version < 1 {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V1)?;
            tx.execute_batch("PRAGMA user_version=1")?;
            tx.commit()?;
        }
        if version < 2 {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V2)?;
            tx.execute_batch("PRAGMA user_version=2")?;
            tx.commit()?;
        }
        Ok(Store { conn })
    }
}

// ----------------------------------------------------------------- accounts

impl Store {
    pub fn add_account(&self, provider: Provider, email: &str, name: &str) -> Result<Account> {
        let email = email.trim().to_ascii_lowercase();
        self.conn.execute(
            "INSERT INTO accounts(provider, email, name) VALUES (?1,?2,?3)
             ON CONFLICT(email) DO UPDATE SET provider=excluded.provider, name=excluded.name",
            params![provider_str(provider), email, name],
        )?;
        let id = self
            .conn
            .query_row("SELECT id FROM accounts WHERE email=?1", [&email], |r| {
                r.get(0)
            })?;
        Ok(Account {
            id,
            provider,
            email,
            name: name.to_string(),
        })
    }

    pub fn accounts(&self) -> Result<Vec<Account>> {
        let mut s = self
            .conn
            .prepare("SELECT id, provider, email, name FROM accounts ORDER BY id")?;
        let it = s.query_map([], |r| {
            Ok(Account {
                id: r.get(0)?,
                provider: provider_parse(&r.get::<_, String>(1)?),
                email: r.get(2)?,
                name: r.get(3)?,
            })
        })?;
        Ok(it.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Cascades to threads, messages, pending ops and outbox rows.
    pub fn remove_account(&self, id: Id) -> Result<()> {
        self.conn
            .execute("DELETE FROM accounts WHERE id=?1", [id])?;
        Ok(())
    }

    pub fn account_provider(&self, id: Id) -> Result<Option<Provider>> {
        let p = self
            .conn
            .query_row("SELECT provider FROM accounts WHERE id=?1", [id], |r| {
                r.get::<_, String>(0)
            })
            .optional()?;
        Ok(p.map(|p| provider_parse(&p)))
    }

    pub fn cursor(&self, id: Id) -> Result<Option<String>> {
        let c = self
            .conn
            .query_row("SELECT cursor FROM accounts WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(c.flatten())
    }

    pub fn set_cursor(&self, id: Id, cursor: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts SET cursor=?1 WHERE id=?2",
            params![cursor, id],
        )?;
        Ok(())
    }
}

// ----------------------------------------------------------------- folders

impl Store {
    /// Locally known custom folders, ordered by account then name.
    pub fn folders(&self) -> Result<Vec<Folder>> {
        let mut s = self.conn.prepare(
            "SELECT id, account_id, name FROM folders ORDER BY account_id, name COLLATE NOCASE",
        )?;
        let it = s.query_map([], |r| {
            Ok(Folder {
                id: r.get(0)?,
                account_id: r.get(1)?,
                name: r.get(2)?,
            })
        })?;
        Ok(it.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// A folder whose provider identity is its display name (an IMAP mailbox).
    pub fn create_folder(&self, account: Id, name: &str) -> Result<Folder> {
        self.add_folder(account, name, name)
    }

    /// Persists a folder under its provider identity. The engine calls this only after the provider
    /// accepted the folder, so a successful return means the local catalog holds it.
    pub(crate) fn add_folder(&self, account: Id, id: &str, name: &str) -> Result<Folder> {
        let name = name.trim();
        if name.is_empty() {
            bail!("folder name is empty");
        }
        if id.is_empty() {
            bail!("folder id is empty");
        }
        let tx = self.conn.unchecked_transaction()?;
        let taken: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM folders WHERE account_id=?1 AND name=?2 COLLATE NOCASE)",
            params![account, name],
            |r| r.get(0),
        )?;
        if taken {
            bail!("folder {name:?} already exists in account {account}");
        }
        tx.execute(
            "INSERT INTO folders(account_id, id, name, created) VALUES (?1,?2,?3,?4)",
            params![account, id, name, now_ms()],
        )?;
        tx.commit()?;
        Ok(Folder {
            id: id.to_string(),
            account_id: account,
            name: name.to_string(),
        })
    }

    pub fn folder_exists(&self, account: Id, folder: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM folders WHERE account_id=?1 AND id=?2)",
            params![account, folder],
            |r| r.get(0),
        )?)
    }
}

// -------------------------------------------------------------------- deltas

impl Store {
    /// Applies one sync batch in a single transaction. Batches are ≤ 200 messages (see ARCHITECTURE).
    pub fn apply_deltas(&self, account_id: Id, deltas: &[Delta]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let account_email: String = tx
            .query_row(
                "SELECT email FROM accounts WHERE id=?1",
                [account_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_default();
        let mut touched: HashSet<Id> = HashSet::new();
        for d in deltas {
            let hit = match d {
                Delta::Upsert(m) => {
                    touched.insert(upsert_message(&tx, account_id, &account_email, m)?);
                    continue;
                }
                Delta::Flags { remote_id, .. } | Delta::Delete { remote_id } => {
                    find_message(&tx, account_id, remote_id)?
                }
            };
            let Some((mid, tid)) = hit else { continue };
            let changed = match d {
                // Unchanged flags are a no-op (IMAP without CONDSTORE re-reports every flag on every
                // sync), and a local change wins until its pending op has reached the server.
                Delta::Flags {
                    unread, starred, ..
                } => tx.execute(
                    "UPDATE messages SET unread=?1, starred=?2 WHERE id=?3
                       AND (unread IS NOT ?1 OR starred IS NOT ?2)
                       AND NOT EXISTS (SELECT 1 FROM pending_ops WHERE message_id=?3)",
                    params![unread, starred, mid],
                )?,
                Delta::Delete { .. } => tx.execute("DELETE FROM messages WHERE id=?1", [mid])?,
                Delta::Upsert(_) => unreachable!(),
            };
            if changed > 0 {
                touched.insert(tid);
            }
        }
        for tid in &touched {
            refresh_thread(&tx, *tid)?;
        }
        tx.commit()?;
        Ok(())
    }
}

fn find_message(tx: &Transaction, account_id: Id, remote_id: &str) -> Result<Option<(Id, Id)>> {
    Ok(tx
        .query_row(
            "SELECT id, thread_id FROM messages WHERE account_id=?1 AND remote_id=?2",
            params![account_id, remote_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

fn upsert_message(
    tx: &Transaction,
    account_id: Id,
    account_email: &str,
    m: &RemoteMsg,
) -> Result<Id> {
    let from_email = m.from.email.trim().to_ascii_lowercase();
    let from_name = display_name(&m.from.name, &from_email);
    let from_self = !account_email.is_empty() && from_email == account_email;
    let (to_json, cc_json) = (addrs_json(&m.to), addrs_json(&m.cc));
    let refs_json = json(&m.references)?;
    let ns = norm_subject(&m.subject);
    let mailbox = mb_str(m.mailbox);
    let src: i64 = match m.important {
        Some(true) => 1,
        Some(false) => 2,
        None => 0,
    };
    // `important` is written here as a first guess; `refresh_thread` recomputes the auto ones.
    let sent_to: i64 = tx
        .query_row(
            "SELECT COALESCE(sent_to, 0) FROM contacts WHERE email=?1",
            [&from_email],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0);
    let important = m.important.unwrap_or(from_self || sent_to > 0);
    let existing: Option<Id> = tx
        .query_row(
            "SELECT id FROM messages WHERE account_id=?1 AND remote_id=?2",
            params![account_id, m.remote_id],
            |r| r.get(0),
        )
        .optional()?;
    let mid = match existing {
        Some(id) => {
            // While a local change is queued, keep the local placement and flags (?22 = pending).
            tx.execute(
                "UPDATE messages SET
                   folder=CASE WHEN ?22 THEN folder ELSE ?1 END, mailbox=CASE WHEN ?22 THEN mailbox ELSE ?2 END,
                   thread_hint=?3, message_id=?4, in_reply_to=?5,
                   refs=?6, from_name=?7, from_email=?8, to_json=?9, cc_json=?10, date=?11, subject=?12,
                   norm_subject=?13, text=?14, html=?15,
                   unread=CASE WHEN ?22 THEN unread ELSE ?16 END, starred=CASE WHEN ?22 THEN starred ELSE ?17 END,
                   important=?18, important_src=?19, from_self=?20 WHERE id=?21",
                params![m.folder, mailbox, m.thread_hint, m.message_id, m.in_reply_to, refs_json, from_name,
                        from_email, to_json, cc_json, m.date, m.subject, ns, m.text, m.html, m.unread,
                        m.starred, important, src, from_self, id,
                        tx.query_row("SELECT EXISTS(SELECT 1 FROM pending_ops WHERE message_id=?1)", [id], |r| r.get::<_, bool>(0))?],
            )?;
            return thread_id_of(tx, id);
        }
        None => {
            let thread_id = resolve_thread(tx, account_id, m, &ns)?;
            tx.execute(
                "INSERT INTO messages(account_id, thread_id, remote_id, folder, mailbox, thread_hint,
                   message_id, in_reply_to, refs, from_name, from_email, to_json, cc_json, date, subject,
                   norm_subject, text, html, unread, starred, important, important_src, from_self)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23)",
                params![account_id, thread_id, m.remote_id, m.folder, mailbox, m.thread_hint, m.message_id,
                        m.in_reply_to, refs_json, from_name, from_email, to_json, cc_json, m.date, m.subject,
                        ns, m.text, m.html, m.unread, m.starred, important, src, from_self],
            )?;
            tx.last_insert_rowid()
        }
    };

    tx.execute("DELETE FROM attachments WHERE message_id=?1", [mid])?;
    for a in &m.attachments {
        tx.execute(
            "INSERT INTO attachments(message_id, remote_id, name, mime, size) VALUES (?1,?2,?3,?4,?5)",
            params![mid, a.remote_id, a.name, a.mime, a.size as i64],
        )?;
    }

    // Contacts and people rows are only touched for genuinely new messages, so the counters stay
    // "how many distinct messages" rather than "how many sync passes".
    let people =
        m.to.iter()
            .map(|a| ("to", a))
            .chain(m.cc.iter().map(|a| ("cc", a)))
            .chain([("from", &m.from)]);
    tx.execute("DELETE FROM message_people WHERE message_id=?1", [mid])?;
    let is_sent = mailbox == "sent";
    for (kind, a) in people {
        let email = a.email.trim().to_ascii_lowercase();
        if email.is_empty() {
            continue;
        }
        let name = display_name(&a.name, &email);
        tx.execute(
            "INSERT INTO message_people(message_id, email, domain, name, kind) VALUES (?1,?2,?3,?4,?5)",
            params![mid, email, email_domain(&email), name, kind],
        )?;
        let written_to = is_sent && kind != "from";
        tx.execute(
            "INSERT INTO contacts(email, domain, name, seen, sent_to) VALUES (?1,?2,?3,1,?4)
             ON CONFLICT(email) DO UPDATE SET seen = seen + 1, sent_to = sent_to + ?4,
               name = CASE WHEN excluded.name <> '' THEN excluded.name ELSE contacts.name END",
            params![email, email_domain(&email), name, written_to as i64],
        )?;
    }
    thread_id_of(tx, mid)
}

fn thread_id_of(tx: &Transaction, message_id: Id) -> Result<Id> {
    Ok(tx.query_row(
        "SELECT thread_id FROM messages WHERE id=?1",
        [message_id],
        |r| r.get(0),
    )?)
}

/// Graph's `conversationId` wins; otherwise the Message-ID/In-Reply-To/References union; otherwise
/// normalized subject + overlapping participants within 30 days.
fn resolve_thread(tx: &Transaction, account_id: Id, m: &RemoteMsg, ns: &str) -> Result<Id> {
    let hint = m.thread_hint.as_deref().filter(|s| !s.is_empty());
    if let Some(h) = hint {
        let known = key_thread(tx, account_id, &format!("h:{h}"))?;
        if let Some(t) = known {
            return Ok(t);
        }
    }
    let keys = msg_keys(m);
    let mut cands: Vec<Id> = Vec::new();
    for k in &keys {
        if let Some(t) = key_thread(tx, account_id, k)? {
            cands.push(t);
        }
    }
    cands.sort_unstable();
    cands.dedup();
    let tid = match cands.first() {
        Some(&canon) => {
            for other in &cands[1..] {
                merge_thread(tx, canon, *other)?;
            }
            canon
        }
        None => match subject_thread(tx, account_id, m, ns)? {
            Some(t) => t,
            None => {
                tx.execute(
                    "INSERT INTO threads(account_id, subject, norm_subject, snippet, participants, date,
                       unread, starred, important, attachments, count, mailbox)
                     VALUES (?1,'','','','',0,0,0,0,0,0,'inbox')",
                    [account_id],
                )?;
                tx.last_insert_rowid()
            }
        },
    };
    let mut keys = keys;
    if let Some(h) = hint {
        keys.push(format!("h:{h}"));
    }
    for k in &keys {
        tx.execute(
            "INSERT OR IGNORE INTO thread_keys(account_id, key, thread_id) VALUES (?1,?2,?3)",
            params![account_id, k, tid],
        )?;
    }
    Ok(tid)
}

fn subject_thread(tx: &Transaction, account_id: Id, m: &RemoteMsg, ns: &str) -> Result<Option<Id>> {
    let mut emails: Vec<String> = Vec::new();
    for a in std::iter::once(&m.from)
        .chain(m.to.iter())
        .chain(m.cc.iter())
    {
        let e = a.email.trim().to_ascii_lowercase();
        if !e.is_empty() && !emails.contains(&e) {
            emails.push(e);
        }
    }
    if ns.is_empty() || emails.is_empty() {
        return Ok(None);
    }
    let holes = vec!["?"; emails.len()].join(",");
    let sql = format!(
        "SELECT m.thread_id FROM messages m
         WHERE m.account_id=?1 AND m.norm_subject=?2 AND m.date BETWEEN ?3 AND ?4
           AND EXISTS (SELECT 1 FROM message_people p WHERE p.message_id=m.id AND p.email IN ({holes}))
         ORDER BY m.date DESC LIMIT 1"
    );
    let mut ps: Vec<Value> = vec![
        Value::Integer(account_id),
        Value::Text(ns.to_string()),
        Value::Integer(m.date - 30 * 86_400_000),
        Value::Integer(m.date + 30 * 86_400_000),
    ];
    ps.extend(emails.into_iter().map(Value::Text));
    Ok(tx
        .query_row(&sql, params_from_iter(ps), |r| r.get(0))
        .optional()?)
}

fn merge_thread(tx: &Transaction, canon: Id, other: Id) -> Result<()> {
    if canon != other {
        tx.execute(
            "UPDATE OR REPLACE thread_keys SET thread_id=?1 WHERE thread_id=?2",
            params![canon, other],
        )?;
        tx.execute(
            "UPDATE messages SET thread_id=?1 WHERE thread_id=?2",
            params![canon, other],
        )?;
        tx.execute("DELETE FROM threads WHERE id=?1", [other])?;
    }
    Ok(())
}

fn key_thread(tx: &Transaction, account_id: Id, key: &str) -> Result<Option<Id>> {
    Ok(tx
        .query_row(
            "SELECT thread_id FROM thread_keys WHERE account_id=?1 AND key=?2",
            params![account_id, key],
            |r| r.get(0),
        )
        .optional()?)
}

fn msg_keys(m: &RemoteMsg) -> Vec<String> {
    let mut raw: Vec<&str> = Vec::new();
    raw.extend(m.message_id.as_deref());
    raw.extend(m.in_reply_to.as_deref());
    raw.extend(m.references.iter().map(String::as_str));
    let mut out: Vec<String> = raw
        .into_iter()
        .filter_map(|x| {
            let n = x
                .trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .trim()
                .to_ascii_lowercase();
            (!n.is_empty()).then(|| format!("i:{n}"))
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

struct Meta {
    id: Id,
    date: Ms,
    subject: String,
    text: String,
    from_name: String,
    mailbox: String,
    unread: bool,
    starred: bool,
    important: bool,
    src: i64,
    n_att: i64,
    sent_to: i64,
    from_self: bool,
}

/// Recomputes every denormalized `threads` column from the thread's messages, and deletes the
/// thread when its last message goes away.
fn refresh_thread(tx: &Transaction, thread_id: Id) -> Result<()> {
    let rows: Vec<Meta> = {
        let mut s = tx.prepare(
            "SELECT m.id, m.date, m.subject, m.text, m.from_name, m.mailbox, m.unread, m.starred,
                    m.important, m.important_src, m.from_self,
                    (SELECT COUNT(*) FROM attachments a WHERE a.message_id=m.id), COALESCE(c.sent_to, 0)
             FROM messages m LEFT JOIN contacts c ON c.email=m.from_email
             WHERE m.thread_id=?1 ORDER BY m.date, m.id",
        )?;
        let it = s.query_map([thread_id], |r| {
            Ok(Meta {
                id: r.get(0)?,
                date: r.get(1)?,
                subject: r.get(2)?,
                text: r.get(3)?,
                from_name: r.get(4)?,
                mailbox: r.get(5)?,
                unread: r.get(6)?,
                starred: r.get(7)?,
                important: r.get(8)?,
                src: r.get(9)?,
                from_self: r.get(10)?,
                n_att: r.get(11)?,
                sent_to: r.get(12)?,
            })
        })?;
        it.collect::<rusqlite::Result<Vec<_>>>()?
    };
    if rows.is_empty() {
        tx.execute("DELETE FROM threads WHERE id=?1", [thread_id])?;
        return Ok(());
    }
    // Auto importance: a thread we replied to, or a sender we have written to before.
    let has_self = rows.iter().any(|r| r.from_self);
    let mut eff: Vec<bool> = Vec::with_capacity(rows.len());
    for r in &rows {
        let v = match r.src {
            2 => false,
            1 => true,
            _ => has_self || r.sent_to > 0,
        };
        if v != r.important {
            tx.execute(
                "UPDATE messages SET important=?1 WHERE id=?2",
                params![v, r.id],
            )?;
        }
        eff.push(v);
    }
    let subject = rows
        .iter()
        .map(|r| r.subject.trim())
        .find(|s| !s.is_empty())
        .unwrap_or("");
    let mut names: Vec<&str> = Vec::new();
    for r in &rows {
        if !r.from_name.is_empty() && !names.contains(&r.from_name.as_str()) {
            names.push(&r.from_name);
        }
    }
    let participants = if names.len() > 4 {
        format!("{}, {}, {}, {}…", names[0], names[1], names[2], names[3])
    } else {
        names.join(", ")
    };
    let text = rows
        .iter()
        .rev()
        .map(|r| r.text.as_str())
        .find(|t| !t.trim().is_empty())
        .unwrap_or("");
    // The thread lives where its latest non-sent message lives, so our own replies do not move it.
    let mailbox = rows
        .iter()
        .rev()
        .find(|r| r.mailbox != "sent")
        .or_else(|| rows.last())
        .map(|r| r.mailbox.clone());
    tx.execute(
        "UPDATE threads SET subject=?1, norm_subject=?2, snippet=?3, participants=?4, date=?5,
           unread=?6, starred=?7, important=?8, attachments=?9, count=?10, mailbox=?11 WHERE id=?12",
        params![subject, norm_subject(subject), snip(text), participants,
                rows.last().map(|r| r.date).unwrap_or(0), rows.iter().any(|r| r.unread),
                rows.iter().any(|r| r.starred), eff.iter().any(|v| *v),
                rows.iter().any(|r| r.n_att > 0), rows.len() as i64,
                mailbox.unwrap_or_else(|| "inbox".into()), thread_id],
    )?;
    Ok(())
}

// ------------------------------------------------------------- reads: views

impl Store {
    /// One indexed query over the denormalized `threads` table. User input never reaches SQL as
    /// syntax: the search grammar only produces fixed fragments plus bound parameters.
    pub fn threads(&self, view: &View, offset: u32, limit: u32) -> Result<Vec<ThreadSummary>> {
        let split_q = match view.split {
            Some(id) => self
                .conn
                .query_row("SELECT query FROM splits WHERE id=?1", [id], |r| {
                    r.get::<_, String>(0)
                })
                .optional()?
                .unwrap_or_default(),
            None => String::new(),
        };
        let p = parse_query(&format!(
            "{} {}",
            split_q,
            view.query.clone().unwrap_or_default()
        ));

        let mut conds: Vec<String> = Vec::new();
        let mut ps: Vec<Value> = Vec::new();
        match &view.folder {
            // A folder belongs to exactly one account and supersedes the standard mailbox filter.
            // `View.folder` is always paired with an account (see docs/ARCHITECTURE.md).
            Some(folder) => {
                let Some(account) = view.account_id else {
                    bail!("a folder view requires an account_id");
                };
                conds.push("t.account_id=?".into());
                ps.push(Value::Integer(account));
                conds.push(FOLDER_IN.into());
                ps.push(Value::Text(folder.clone()));
            }
            None => {
                conds.push(mailbox_cond(view.mailbox).to_string());
                if let Some(a) = view.account_id {
                    conds.push("t.account_id=?".into());
                    ps.push(Value::Integer(a));
                }
            }
        }
        if !p.match_terms.is_empty() {
            conds.push(FTS_IN.into());
            ps.push(Value::Text(p.match_terms.join(" ")));
        }
        for nt in &p.not_match {
            conds.push(FTS_NOT_IN.into());
            ps.push(Value::Text(nt.clone()));
        }
        conds.extend(p.conds);
        ps.extend(p.params);
        let sql = format!(
            "SELECT t.id, t.account_id, t.subject, t.snippet, t.participants, t.date, t.unread, t.starred,
                    t.important, t.attachments, t.labels, t.count, t.snoozed_until
             FROM threads t WHERE {} ORDER BY t.date DESC LIMIT ? OFFSET ?",
            conds.join(" AND ")
        );
        ps.push(Value::Integer(limit as i64));
        ps.push(Value::Integer(offset as i64));

        let mut s = self.conn.prepare(&sql)?;
        let it = s.query_map(params_from_iter(ps), |r| {
            Ok(ThreadSummary {
                id: r.get(0)?,
                account_id: r.get(1)?,
                subject: r.get(2)?,
                snippet: r.get(3)?,
                participants: r.get(4)?,
                date: r.get(5)?,
                unread: r.get(6)?,
                starred: r.get(7)?,
                important: r.get(8)?,
                attachments: r.get(9)?,
                labels: serde_json::from_str(&r.get::<_, String>(10)?).unwrap_or_default(),
                count: r.get::<_, i64>(11)? as u32,
                snoozed_until: r.get(12)?,
            })
        })?;
        Ok(it.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn thread(&self, id: Id) -> Result<Vec<Message>> {
        let heads = {
            let mut s = self.conn.prepare(
                "SELECT id, thread_id, from_name, from_email, to_json, cc_json, date, subject, html, text, unread
                 FROM messages WHERE thread_id=?1 ORDER BY date, id",
            )?;
            let it = s.query_map([id], |r| {
                Ok((
                    r.get::<_, Id>(0)?,
                    r.get::<_, Id>(1)?,
                    Addr {
                        name: r.get(2)?,
                        email: r.get(3)?,
                    },
                    addrs_from_json(&r.get::<_, String>(4)?),
                    addrs_from_json(&r.get::<_, String>(5)?),
                    r.get::<_, Ms>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, Option<String>>(8)?,
                    r.get::<_, String>(9)?,
                    r.get::<_, bool>(10)?,
                ))
            })?;
            it.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut s = self.conn.prepare(
            "SELECT id, name, mime, size FROM attachments WHERE message_id=?1 ORDER BY id",
        )?;
        let mut out = Vec::with_capacity(heads.len());
        for (id, thread_id, from, to, cc, date, subject, html, text, unread) in heads {
            let files = s
                .query_map([id], |r| {
                    Ok(Attachment {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        mime: r.get(2)?,
                        size: r.get::<_, i64>(3)? as u64,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            out.push(Message {
                id,
                thread_id,
                from,
                to,
                cc,
                date,
                subject,
                html,
                text,
                attachments: files,
                unread,
            });
        }
        Ok(out)
    }

    pub fn attachment(&self, id: Id) -> Result<Option<(Id, String, String, String)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT m.account_id, m.remote_id, a.remote_id, a.name
                 FROM attachments a JOIN messages m ON m.id=a.message_id WHERE a.id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?)
    }

    /// The thread a message belongs to, used to arm a post-send "remind me if no reply".
    pub fn message_thread(&self, message_id: Id) -> Result<Option<Id>> {
        Ok(self
            .conn
            .query_row(
                "SELECT thread_id FROM messages WHERE id=?1",
                [message_id],
                |r| r.get(0),
            )
            .optional()?)
    }
}

// ---------------------------------------------------------- actions and undo

struct MsgState {
    id: Id,
    remote_id: String,
    mailbox: String,
    folder: String,
    unread: bool,
    starred: bool,
}

struct UndoRow {
    account_id: Id,
    thread_id: Id,
    message_id: Option<Id>,
    mailbox: Option<String>,
    unread: Option<bool>,
    starred: Option<bool>,
    folder: Option<String>,
    snoozed_until: Option<Ms>,
    remind_after: Option<Ms>,
    op_id: Option<Id>,
    inverse: Option<String>,
}

impl Store {
    /// Applies locally at once, queues one remote `Op` per affected message, and records the inverse
    /// under a fresh undo token. Snooze is local-only and queues nothing.
    pub fn act(&self, ids: &[Id], action: &Action) -> Result<i64> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("INSERT INTO undo_seq(token) VALUES (NULL)", [])?;
        let token = tx.last_insert_rowid();
        let now = now_ms();
        for &thread_id in ids {
            let account_id: Option<Id> = tx
                .query_row(
                    "SELECT account_id FROM threads WHERE id=?1",
                    [thread_id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(account_id) = account_id else {
                continue;
            };
            // Moving into a folder the message's account does not own is a caller bug: reject it
            // before anything local changes. Dropping `tx` rolls back the whole action.
            if let Action::Move { folder } = action {
                let owned: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM folders WHERE account_id=?1 AND id=?2)",
                    params![account_id, folder],
                    |r| r.get(0),
                )?;
                if !owned {
                    bail!("folder {folder:?} does not belong to account {account_id}");
                }
            }
            let states = message_states(&tx, thread_id)?;
            if states.is_empty() {
                continue;
            }
            if let Action::Snooze { until } = action {
                let (su, ra): (Option<Ms>, Option<Ms>) = tx.query_row(
                    "SELECT snoozed_until, remind_after FROM threads WHERE id=?1",
                    [thread_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                tx.execute(
                    "UPDATE threads SET snoozed_until=?1, remind_after=NULL WHERE id=?2",
                    params![until, thread_id],
                )?;
                tx.execute(
                    "INSERT INTO undo_log(token, account_id, thread_id, message_id, snoozed_until, remind_after)
                     VALUES (?1,?2,?3,NULL,?4,?5)",
                    params![token, account_id, thread_id, su, ra],
                )?;
                continue;
            }
            for m in &states {
                let (mailbox, unread, starred, folder, op, inv) = plan(action, m);
                if mailbox.is_none() && unread.is_none() && starred.is_none() && folder.is_none() {
                    continue;
                }
                tx.execute(
                    "UPDATE messages SET mailbox=COALESCE(?1,mailbox), unread=COALESCE(?2,unread),
                       starred=COALESCE(?3,starred), folder=COALESCE(?4,folder) WHERE id=?5",
                    params![mailbox, unread, starred, folder, m.id],
                )?;
                let (inv_json, op_id) = match op {
                    Some(op) => {
                        let inv_json = inv.map(|i| json(&i)).transpose()?;
                        tx.execute(
                            "INSERT INTO pending_ops(account_id, message_id, op, inverse, at) VALUES (?1,?2,?3,?4,?5)",
                            params![account_id, m.id, json(&op)?, inv_json, now],
                        )?;
                        (inv_json, Some(tx.last_insert_rowid()))
                    }
                    None => (None, None),
                };
                tx.execute(
                    "INSERT INTO undo_log(token, account_id, thread_id, message_id, mailbox, unread, starred,
                       folder, op_id, inverse) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![token, account_id, thread_id, m.id, m.mailbox, m.unread, m.starred, m.folder, op_id, inv_json],
                )?;
            }
            if matches!(action, Action::Inbox) {
                tx.execute(
                    "UPDATE threads SET snoozed_until=NULL, remind_after=NULL WHERE id=?1",
                    [thread_id],
                )?;
            }
            refresh_thread(&tx, thread_id)?;
        }
        tx.commit()?;
        Ok(token)
    }

    /// Restores the previous local state. Pending ops from the action are dropped while unsent;
    /// ops that already went out get their inverse queued.
    pub fn undo(&self, token: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let rows: Vec<UndoRow> = {
            let mut s = tx.prepare(
                "SELECT account_id, thread_id, message_id, mailbox, unread, starred, folder,
                        snoozed_until, remind_after, op_id, inverse FROM undo_log WHERE token=?1",
            )?;
            let it = s.query_map([token], |r| {
                Ok(UndoRow {
                    account_id: r.get(0)?,
                    thread_id: r.get(1)?,
                    message_id: r.get(2)?,
                    mailbox: r.get(3)?,
                    unread: r.get(4)?,
                    starred: r.get(5)?,
                    folder: r.get(6)?,
                    snoozed_until: r.get(7)?,
                    remind_after: r.get(8)?,
                    op_id: r.get(9)?,
                    inverse: r.get(10)?,
                })
            })?;
            it.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut touched: HashSet<Id> = HashSet::new();
        for r in rows {
            match r.message_id {
                Some(mid) => {
                    tx.execute(
                        "UPDATE messages SET mailbox=COALESCE(?1,mailbox), unread=COALESCE(?2,unread),
                           starred=COALESCE(?3,starred), folder=COALESCE(?4,folder) WHERE id=?5",
                        params![r.mailbox, r.unread, r.starred, r.folder, mid],
                    )?;
                }
                None => {
                    tx.execute(
                        "UPDATE threads SET snoozed_until=?1, remind_after=?2 WHERE id=?3",
                        params![r.snoozed_until, r.remind_after, r.thread_id],
                    )?;
                }
            }
            if let Some(op_id) = r.op_id {
                let queued: Option<()> = tx
                    .query_row("SELECT 1 FROM pending_ops WHERE id=?1", [op_id], |_| Ok(()))
                    .optional()?;
                if queued.is_some() {
                    tx.execute("DELETE FROM pending_ops WHERE id=?1", [op_id])?;
                } else if let Some(inv) = r.inverse {
                    tx.execute(
                        "INSERT INTO pending_ops(account_id, message_id, op, inverse, at) VALUES (?1,?2,?3,NULL,?4)",
                        params![r.account_id, r.message_id, inv, now_ms()],
                    )?;
                }
            }
            touched.insert(r.thread_id);
        }
        tx.execute("DELETE FROM undo_log WHERE token=?1", [token])?;
        tx.execute("DELETE FROM undo_seq WHERE token=?1", [token])?;
        for t in &touched {
            refresh_thread(&tx, *t)?;
        }
        tx.commit()?;
        Ok(())
    }
}

/// What a local action does to one message: new column values, the remote op it queues, and that
/// op's inverse. `None` means "leave it alone".
type Plan = (
    Option<String>,
    Option<bool>,
    Option<bool>,
    Option<String>,
    Option<Op>,
    Option<Op>,
);

fn move_op(remote_id: &str, to: Mailbox) -> Op {
    Op::Move {
        remote_id: remote_id.into(),
        to,
    }
}

fn flag_op(remote_id: &str, unread: Option<bool>, starred: Option<bool>) -> Op {
    Op::Flags {
        remote_id: remote_id.into(),
        unread,
        starred,
    }
}

fn plan(action: &Action, m: &MsgState) -> Plan {
    let rid = m.remote_id.as_str();
    let nothing = (None, None, None, None, None, None);
    match action {
        Action::Archive if m.mailbox == "inbox" => (
            Some("archive".into()),
            Some(false),
            None,
            None,
            Some(move_op(rid, Mailbox::Archive)),
            Some(move_op(rid, Mailbox::Inbox)),
        ),
        Action::Inbox if m.mailbox == "archive" => (
            Some("inbox".into()),
            None,
            None,
            None,
            Some(move_op(rid, Mailbox::Inbox)),
            Some(move_op(rid, Mailbox::Archive)),
        ),
        // The whole conversation goes to the bin, except drafts, which are not conversations.
        Action::Trash if m.mailbox != "trash" && m.mailbox != "drafts" => (
            Some("trash".into()),
            Some(false),
            None,
            None,
            Some(move_op(rid, Mailbox::Trash)),
            Some(move_op(rid, mb_parse(&m.mailbox))),
        ),
        Action::Star if !m.starred => (
            None,
            None,
            Some(true),
            None,
            Some(flag_op(rid, None, Some(true))),
            Some(flag_op(rid, None, Some(false))),
        ),
        Action::Unstar if m.starred => (
            None,
            None,
            Some(false),
            None,
            Some(flag_op(rid, None, Some(false))),
            Some(flag_op(rid, None, Some(true))),
        ),
        Action::Read if m.unread => (
            None,
            Some(false),
            None,
            None,
            Some(flag_op(rid, Some(false), None)),
            Some(flag_op(rid, Some(true), None)),
        ),
        Action::Unread if !m.unread => (
            None,
            Some(true),
            None,
            None,
            Some(flag_op(rid, Some(true), None)),
            Some(flag_op(rid, Some(false), None)),
        ),
        Action::Move { folder } if m.folder != *folder => (
            Some("archive".into()),
            None,
            None,
            Some(folder.clone()),
            Some(Op::MoveToFolder {
                remote_id: rid.into(),
                folder: folder.clone(),
            }),
            Some(Op::MoveToFolder {
                remote_id: rid.into(),
                folder: m.folder.clone(),
            }),
        ),
        _ => nothing,
    }
}

fn message_states(tx: &Transaction, thread_id: Id) -> Result<Vec<MsgState>> {
    let mut s = tx.prepare(
        "SELECT id, remote_id, mailbox, folder, unread, starred FROM messages WHERE thread_id=?1 ORDER BY date, id",
    )?;
    let it = s.query_map([thread_id], |r| {
        Ok(MsgState {
            id: r.get(0)?,
            remote_id: r.get(1)?,
            mailbox: r.get(2)?,
            folder: r.get(3)?,
            unread: r.get(4)?,
            starred: r.get(5)?,
        })
    })?;
    Ok(it.collect::<rusqlite::Result<Vec<_>>>()?)
}

// -------------------------------------------------------------- pending ops

impl Store {
    pub fn pending_ops(&self, account_id: Id, now: Ms, limit: u32) -> Result<Vec<(i64, Op)>> {
        let mut s = self.conn.prepare(
            "SELECT id, op FROM pending_ops WHERE account_id=?1 AND at<=?2 ORDER BY id LIMIT ?3",
        )?;
        let it = s.query_map(params![account_id, now, limit], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in it {
            let (id, json) = row?;
            out.push((id, serde_json::from_str(&json)?));
        }
        Ok(out)
    }

    /// A move changes a message's remote id; keep the local row pointing at the new one.
    pub fn op_done(&self, op_id: i64, new_remote_id: Option<String>) -> Result<()> {
        if let Some(rid) = new_remote_id {
            self.conn.execute(
                "UPDATE messages SET remote_id=?1 WHERE id=(SELECT message_id FROM pending_ops WHERE id=?2)",
                params![rid, op_id],
            )?;
        }
        self.conn
            .execute("DELETE FROM pending_ops WHERE id=?1", [op_id])?;
        Ok(())
    }

    /// Exponential backoff, capped at an hour.
    pub fn op_failed(&self, op_id: i64, now: Ms) -> Result<()> {
        let attempts: i64 = self
            .conn
            .query_row(
                "SELECT attempts FROM pending_ops WHERE id=?1",
                [op_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let backoff = (1_000i64 << attempts.min(12)).min(3_600_000);
        self.conn.execute(
            "UPDATE pending_ops SET attempts=attempts+1, at=?1 WHERE id=?2",
            params![now + backoff, op_id],
        )?;
        Ok(())
    }
}

// ---------------------------------------------------------------- outbox, ...

impl Store {
    pub fn outbox_add(&self, draft: &Draft, due: Ms) -> Result<Id> {
        self.conn.execute(
            "INSERT INTO outbox(account_id, draft, at) VALUES (?1,?2,?3)",
            params![draft.account_id, json(draft)?, due],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn outbox_cancel(&self, id: Id) -> Result<Option<Draft>> {
        let draft: Option<String> = self
            .conn
            .query_row("SELECT draft FROM outbox WHERE id=?1", [id], |r| r.get(0))
            .optional()?;
        let Some(draft) = draft else { return Ok(None) };
        self.conn.execute("DELETE FROM outbox WHERE id=?1", [id])?;
        Ok(Some(serde_json::from_str(&draft)?))
    }

    pub fn outbox_due(&self, now: Ms) -> Result<Vec<(Id, Draft)>> {
        let mut s = self
            .conn
            .prepare("SELECT id, draft FROM outbox WHERE at<=?1 ORDER BY at, id")?;
        let it = s.query_map([now], |r| Ok((r.get::<_, Id>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in it {
            let (id, draft) = row?;
            out.push((id, serde_json::from_str(&draft)?));
        }
        Ok(out)
    }

    pub fn outbox_sent(&self, id: Id) -> Result<()> {
        self.conn.execute("DELETE FROM outbox WHERE id=?1", [id])?;
        Ok(())
    }

    /// Everything the engine needs to build reply MIME. `references` already includes this
    /// message's own id, so `References` is `references` and `In-Reply-To` is `message_id_header`.
    pub fn reply_context(&self, message_id: Id) -> Result<Option<ReplyCtx>> {
        let row = self
            .conn
            .query_row(
                "SELECT account_id, message_id, refs, subject, from_name, from_email, to_json, cc_json
                 FROM messages WHERE id=?1",
                [message_id],
                |r| {
                    Ok((
                        r.get::<_, Id>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?,
                        r.get::<_, String>(6)?, r.get::<_, String>(7)?,
                    ))
                },
            )
            .optional()?;
        let Some((account_id, header, refs, subject, name, email, to, cc)) = row else {
            return Ok(None);
        };
        let mut references: Vec<String> = serde_json::from_str(&refs).unwrap_or_default();
        if let Some(h) = header.as_ref() {
            let dup = references.iter().any(|x| x == h);
            if !dup {
                references.push(h.clone());
            }
        }
        let subject = if subject.to_ascii_lowercase().starts_with("re:") {
            subject
        } else {
            format!("Re: {subject}")
        };
        Ok(Some(ReplyCtx {
            account_id,
            message_id_header: header.unwrap_or_default(),
            references,
            subject,
            from: Addr { name, email },
            to: addrs_from_json(&to),
            cc: addrs_from_json(&cc),
        }))
    }
}

// ----------------------------------------------------------- snooze / remind

impl Store {
    /// Wakes every thread whose snooze is due. A pending "remind me if no reply" is dropped when a
    /// real reply arrived; otherwise the thread returns to the inbox, unread. Returns changed accounts.
    pub fn wake_due(&self, now: Ms) -> Result<Vec<Id>> {
        let tx = self.conn.unchecked_transaction()?;
        let due: Vec<(Id, Id, Option<Ms>)> = {
            let mut s = tx.prepare(
                "SELECT id, account_id, remind_after FROM threads
                 WHERE snoozed_until IS NOT NULL AND snoozed_until<=?1",
            )?;
            let it = s.query_map([now], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            it.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut changed: HashSet<Id> = HashSet::new();
        for (tid, account_id, remind) in due {
            let replied = match remind {
                Some(ra) => tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM messages WHERE thread_id=?1 AND from_self=0 AND date>?2)",
                    params![tid, ra],
                    |r| r.get::<_, bool>(0),
                )?,
                None => false,
            };
            if replied {
                tx.execute(
                    "UPDATE threads SET snoozed_until=NULL, remind_after=NULL WHERE id=?1",
                    [tid],
                )?;
            } else {
                tx.execute(
                    "UPDATE messages SET mailbox='inbox', unread=1 WHERE thread_id=?1 AND mailbox IN ('inbox','archive')",
                    [tid],
                )?;
                tx.execute(
                    "UPDATE threads SET snoozed_until=NULL, remind_after=NULL, mailbox='inbox', unread=1 WHERE id=?1",
                    [tid],
                )?;
            }
            changed.insert(account_id);
        }
        tx.commit()?;
        Ok(changed.into_iter().collect())
    }

    pub fn remind_if_no_reply(&self, thread_id: Id, at: Ms) -> Result<()> {
        self.conn.execute(
            "UPDATE threads SET snoozed_until=?1, remind_after=?2 WHERE id=?3",
            params![at, now_ms(), thread_id],
        )?;
        Ok(())
    }
}

// ------------------------------------------- splits, settings, contacts, files

impl Store {
    pub fn splits(&self) -> Result<Vec<Split>> {
        let mut s = self
            .conn
            .prepare("SELECT id, name, query FROM splits ORDER BY position, id")?;
        let it = s.query_map([], |r| {
            Ok(Split {
                id: r.get(0)?,
                name: r.get(1)?,
                query: r.get(2)?,
            })
        })?;
        Ok(it.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn set_splits(&self, splits: Vec<Split>) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM splits", [])?;
        for (i, s) in splits.iter().enumerate() {
            match s.id {
                0 => tx.execute(
                    "INSERT INTO splits(name, query, position) VALUES (?1,?2,?3)",
                    params![s.name, s.query, i as i64],
                )?,
                id => tx.execute(
                    "INSERT INTO splits(id, name, query, position) VALUES (?1,?2,?3,?4)",
                    params![id, s.name, s.query, i as i64],
                )?,
            };
        }
        tx.commit()?;
        Ok(())
    }

    pub fn settings(&self) -> Result<Settings> {
        let s: Option<String> = self
            .conn
            .query_row("SELECT v FROM kv WHERE k='settings'", [], |r| r.get(0))
            .optional()?;
        Ok(s.and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default())
    }

    pub fn set_settings(&self, settings: Settings) -> Result<()> {
        self.conn.execute(
            "INSERT INTO kv(k, v) VALUES ('settings', ?1) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
            [json(&settings)?],
        )?;
        Ok(())
    }

    /// Ranked by how often we wrote to them, then by how often we saw them.
    pub fn contacts(&self, query: &str, limit: u32) -> Result<Vec<Addr>> {
        let ranked = "ORDER BY sent_to DESC, seen DESC, name LIMIT";
        let mut s = self.conn.prepare(&format!(
            "SELECT name, email FROM contacts WHERE email GLOB ?1 OR domain GLOB ?1 OR lower(name) GLOB ?1
             {ranked} ?2"
        ))?;
        let it = s.query_map(params![glob_pat(query.trim()), limit as i64], |r| {
            Ok(Addr {
                name: r.get(0)?,
                email: r.get(1)?,
            })
        })?;
        Ok(it.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

// ----------------------------------------------------------------- searching

const FTS_IN: &str = "t.id IN (SELECT m.thread_id FROM msgs_fts JOIN messages m ON m.id=msgs_fts.rowid WHERE msgs_fts MATCH ?)";
const FOLDER_IN: &str = "t.id IN (SELECT m.thread_id FROM messages m WHERE m.folder=?)";
const FTS_NOT_IN: &str = "t.id NOT IN (SELECT m.thread_id FROM msgs_fts JOIN messages m ON m.id=msgs_fts.rowid WHERE msgs_fts MATCH ?)";

#[derive(Default)]
struct Parsed {
    /// FTS5 expressions ANDed into a single MATCH.
    match_terms: Vec<String>,
    /// FTS5 expressions that must not match.
    not_match: Vec<String>,
    conds: Vec<String>,
    params: Vec<Value>,
}

/// Hand-written parser for the grammar in docs/SPEC.md. It only ever emits fixed SQL fragments with
/// bound parameters, so user input can never reach SQL as syntax.
fn parse_query(raw: &str) -> Parsed {
    let mut p = Parsed::default();
    let mut free: Vec<(String, bool)> = Vec::new(); // (quoted term, was a quoted phrase)
    let (mut neg, mut field, mut quoted) = (false, None, false);
    let (mut cur, mut in_quote) = (String::new(), false);
    for ch in raw.chars() {
        match ch {
            '"' => {
                in_quote = !in_quote;
                quoted |= !in_quote;
            }
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() || quoted {
                    apply_token(&mut p, &mut free, neg, field.take(), &cur, quoted);
                }
                cur.clear();
                (neg, quoted) = (false, false);
            }
            '-' if cur.is_empty() && field.is_none() && !quoted => neg = true,
            ':' if field.is_none() && !cur.is_empty() && !quoted => {
                field = Some(cur.to_ascii_lowercase());
                cur.clear();
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        apply_token(&mut p, &mut free, neg, field.take(), &cur, quoted);
    }
    // Instant search: the last free-text token becomes a prefix query.
    if let Some(last) = free.iter_mut().next_back().filter(|l| !l.1) {
        last.0.push('*');
    }
    p.match_terms.extend(free.into_iter().map(|(t, _)| t));
    p
}

fn apply_token(
    p: &mut Parsed,
    free: &mut Vec<(String, bool)>,
    neg: bool,
    field: Option<String>,
    value: &str,
    quoted: bool,
) {
    let term = |v: &str| format!("\"{}\"", v.replace('"', "\"\""));
    match field.as_deref() {
        None if value.is_empty() => {}
        None if neg => p.not_match.push(term(value)),
        None => free.push((term(value), quoted)),
        Some(f @ ("from" | "to")) if !value.is_empty() => p.people(f == "from", value, neg),
        Some("subject") => {
            let t = format!("subject:{}", term(value));
            if neg {
                p.not_match.push(t);
            } else if !value.is_empty() {
                p.match_terms.push(t);
            }
        }
        Some("has") if matches!(value, "attachment" | "attachments") => {
            p.conds.push(format!("t.attachments={}", usize::from(!neg)));
        }
        Some("is") => p.flag(value, neg),
        Some("before" | "after") if valid_date(value) => {
            let op = if (field.as_deref() == Some("before")) != neg {
                "<"
            } else {
                ">="
            };
            p.conds.push(format!("t.date {op} unixepoch(?) * 1000"));
            p.params.push(Value::Text(value.to_string()));
        }
        // Unknown operator: fall back to free text so it still searches something sensible.
        Some(other) => {
            let t = term(&format!("{other}:{value}"));
            if neg {
                p.not_match.push(t);
            } else {
                free.push((t, false));
            }
        }
    }
}

impl Parsed {
    fn people(&mut self, is_from: bool, value: &str, neg: bool) {
        let kind = if is_from {
            "p.kind='from'"
        } else {
            "p.kind IN ('to','cc')"
        };
        let ex = format!(
            "EXISTS (SELECT 1 FROM messages m JOIN message_people p ON p.message_id=m.id
                     WHERE m.thread_id=t.id AND {kind}
                       AND (p.email GLOB ? OR p.domain GLOB ? OR lower(p.name) GLOB ?))"
        );
        self.conds.push(if neg { format!("NOT {ex}") } else { ex });
        let pat = glob_pat(value);
        for _ in 0..3 {
            self.params.push(Value::Text(pat.clone()));
        }
    }

    fn flag(&mut self, value: &str, neg: bool) {
        let on = !neg;
        let cond = match value {
            "unread" => format!("t.unread={}", usize::from(on)),
            "read" => format!("t.unread={}", usize::from(!on)),
            "starred" => format!("t.starred={}", usize::from(on)),
            "important" => format!("t.important={}", usize::from(on)),
            "snoozed" => format!("t.snoozed_until IS {}NULL", if on { "NOT " } else { "" }),
            _ => return,
        };
        self.conds.push(cond);
    }
}

/// Lowercased prefix pattern for `GLOB`, with GLOB metacharacters dropped so input stays literal.
fn glob_pat(s: &str) -> String {
    let mut out: String = s
        .to_ascii_lowercase()
        .chars()
        .filter(|&c| !matches!(c, '*' | '?' | '[' | ']'))
        .collect();
    out.push('*');
    out
}

fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

// --------------------------------------------------------------- tiny helpers

fn mailbox_cond(m: Mailbox) -> &'static str {
    match m {
        Mailbox::Inbox => "t.mailbox='inbox' AND t.snoozed_until IS NULL",
        Mailbox::Starred => "t.starred=1 AND t.mailbox NOT IN ('trash','spam')",
        Mailbox::Snoozed => "t.snoozed_until IS NOT NULL AND t.mailbox NOT IN ('trash','spam')",
        Mailbox::All => "t.mailbox NOT IN ('trash','spam')",
        Mailbox::Archive => "t.mailbox='archive'",
        Mailbox::Drafts => "t.mailbox='drafts'",
        Mailbox::Sent => "t.mailbox='sent'",
        Mailbox::Trash => "t.mailbox='trash'",
        Mailbox::Spam => "t.mailbox='spam'",
    }
}

fn mb_str(m: Mailbox) -> &'static str {
    match m {
        Mailbox::Inbox => "inbox",
        Mailbox::Starred => "starred",
        Mailbox::Snoozed => "snoozed",
        Mailbox::Drafts => "drafts",
        Mailbox::Sent => "sent",
        Mailbox::Archive => "archive",
        Mailbox::Trash => "trash",
        Mailbox::Spam => "spam",
        Mailbox::All => "all",
    }
}

fn mb_parse(s: &str) -> Mailbox {
    match s {
        "starred" => Mailbox::Starred,
        "snoozed" => Mailbox::Snoozed,
        "drafts" => Mailbox::Drafts,
        "sent" => Mailbox::Sent,
        "archive" => Mailbox::Archive,
        "trash" => Mailbox::Trash,
        "spam" => Mailbox::Spam,
        "all" => Mailbox::All,
        _ => Mailbox::Inbox,
    }
}

fn provider_str(p: Provider) -> &'static str {
    match p {
        Provider::Proton => "proton",
        Provider::Microsoft => "microsoft",
    }
}

fn provider_parse(s: &str) -> Provider {
    match s {
        "microsoft" => Provider::Microsoft,
        _ => Provider::Proton,
    }
}

fn json<T: serde::Serialize>(v: &T) -> Result<String> {
    Ok(serde_json::to_string(v)?)
}

fn addrs_json(v: &[Addr]) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".into())
}

fn addrs_from_json(s: &str) -> Vec<Addr> {
    serde_json::from_str(s).unwrap_or_default()
}

fn display_name(name: &str, email: &str) -> String {
    let n = name.trim();
    if n.is_empty() {
        email.split('@').next().unwrap_or(email).to_string()
    } else {
        n.to_string()
    }
}

fn email_domain(email: &str) -> String {
    email
        .split_once('@')
        .map(|(_, d)| d)
        .unwrap_or("")
        .to_string()
}

/// Strips `Re:`/`Fwd:`/`AW:`-style prefixes and whitespace noise so replies group.
fn norm_subject(s: &str) -> String {
    let mut t = s.trim();
    while let Some(rest) = strip_reply_prefix(t) {
        t = rest;
    }
    t.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn strip_reply_prefix(s: &str) -> Option<&str> {
    let lower = s.to_ascii_lowercase();
    let b = lower.as_bytes();
    for p in ["re", "fwd", "fw", "aw", "sv", "wg"] {
        if let Some(rest) = lower.strip_prefix(p) {
            let mut off = p.len() + (rest.len() - rest.trim_start().len());
            if b.get(off) == Some(&b'[') {
                match lower[off..].find(']') {
                    Some(end) => off += end + 1,
                    None => continue,
                }
            }
            if b.get(off) == Some(&b':') {
                return Some(s[off + 1..].trim_start());
            }
        }
    }
    None
}

/// First ~140 characters of body text, whitespace collapsed.
fn snip(s: &str) -> String {
    let mut out = String::with_capacity(160);
    let mut n = 0usize;
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if space && n < 140 {
            out.push(' ');
            n += 1;
        }
        space = false;
        if n >= 140 {
            break;
        }
        out.push(c);
        n += 1;
    }
    out
}

fn now_ms() -> Ms {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
