//! Local SQLite + FTS5 store: the source of truth the UI reads and the engine writes.
//!
//! The same file is opened twice (a reader for the UI, a writer for sync); WAL makes that work.
//! Every write is one short transaction so a UI action never waits behind a long sync batch.
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Transaction, params, params_from_iter};

use crate::model::*;
use crate::sync::{Delta, Op, RemoteMsg};

const AI_TRIAGE_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS ai_triaged (
    thread_id INTEGER PRIMARY KEY REFERENCES threads(id) ON DELETE CASCADE
);";

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

"#;

/// The split tabs a fresh store starts with, and what a still-unmodified legacy store migrates to.
/// `Inbox` has an empty query on purpose: it is a real row (so the UI tab strip renders it) that
/// filters nothing. The category lenses are not exclusive, so a conversation may appear in several.
const DEFAULT_SPLITS: &str = r#"
INSERT INTO splits(id, name, query, position) VALUES (1, 'Inbox', '', 0);
INSERT INTO splits(id, name, query, position) VALUES (2, 'Social', 'from:linkedin.com,x.com,twitter.com,facebook.com,instagram.com,threads.net,bsky.app,mastodon.social', 1);
INSERT INTO splits(id, name, query, position) VALUES (3, 'Newsletters', 'is:newsletter', 2);
INSERT INTO splits(id, name, query, position) VALUES (4, 'GitHub', 'from:github.com', 3);
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

/// Schema v3 adds the partial index behind the `is:newsletter` flag. The column itself is added in
/// `init` with an existence check because `ALTER TABLE ADD COLUMN` is not idempotent.
const SCHEMA_V3: &str = r#"
CREATE INDEX IF NOT EXISTS messages_bulk ON messages(thread_id) WHERE list_unsubscribe=1;
"#;

/// Schema v4 adds tags. `threads.labels` stays a denormalized JSON cache of tag names ordered by
/// `tags.position`, so listing a view never joins `thread_tags`. `undo_log.tag_id` is the tag
/// counterpart of `message_id`: positive means the action labelled the thread, negative unlabelled.
const SCHEMA_V4: &str = r#"
CREATE TABLE IF NOT EXISTS tags (
  id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL UNIQUE COLLATE NOCASE,
  color TEXT NOT NULL, position INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS thread_tags (
  thread_id INTEGER NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
  tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
  PRIMARY KEY(thread_id, tag_id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS thread_tags_tag ON thread_tags(tag_id, thread_id);
"#;

/// Schema v5 adds persistent drafts. A saved draft is a real message row in the Drafts mailbox (so
/// the list shows it with no UI change) linked to the `drafts` row holding its editable content and
/// its provider copy. `pending_ops.draft_id` lets a completed `SaveDraft` teach the draft its new
/// remote id; `outbox.attempts` backs off a send that keeps failing.
const SCHEMA_V5: &str = r#"
CREATE TABLE IF NOT EXISTS drafts (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  thread_id INTEGER REFERENCES threads(id) ON DELETE SET NULL,
  remote_id TEXT, msg_id TEXT NOT NULL, draft TEXT NOT NULL, updated INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS drafts_thread ON drafts(thread_id);
CREATE INDEX IF NOT EXISTS drafts_msg ON drafts(account_id, msg_id);
"#;

/// Seeded on fresh and migrated stores. None of these become splits; nothing assigns them yet.
const DEFAULT_TAGS: &str = r#"
INSERT OR IGNORE INTO tags(id, name, color, position) VALUES
  (1, 'Newsletter', '#3b82f6', 0),
  (2, 'To Reply', '#f59e0b', 1),
  (3, 'Marketing', '#ec4899', 2),
  (4, 'Calendar', '#8b5cf6', 3),
  (5, 'Notification', '#14b8a6', 4),
  (6, 'Cold Email', '#94a3b8', 5),
  (7, 'Team', '#22c55e', 6),
  (8, 'Urgent', '#ef4444', 7);
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
            tx.execute_batch(DEFAULT_SPLITS)?;
            tx.execute_batch("PRAGMA user_version=1")?;
            tx.commit()?;
        }
        if version < 2 {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V2)?;
            tx.execute_batch("PRAGMA user_version=2")?;
            tx.commit()?;
        }
        if version < 3 {
            let tx = conn.unchecked_transaction()?;
            // A database opened once and then rolled back by hand can already carry the column.
            let has_flag: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='list_unsubscribe')",
                [],
                |r| r.get(0),
            )?;
            if !has_flag {
                tx.execute_batch(
                    "ALTER TABLE messages ADD COLUMN list_unsubscribe INTEGER NOT NULL DEFAULT 0",
                )?;
            }
            tx.execute_batch(SCHEMA_V3)?;
            tx.execute_batch("PRAGMA user_version=3")?;
            tx.commit()?;
        }
        if version < 4 {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V4)?;
            let has_tag: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('undo_log') WHERE name='tag_id')",
                [],
                |r| r.get(0),
            )?;
            if !has_tag {
                tx.execute_batch("ALTER TABLE undo_log ADD COLUMN tag_id INTEGER")?;
            }
            tx.execute_batch(DEFAULT_TAGS)?;
            tx.execute_batch("PRAGMA user_version=4")?;
            tx.commit()?;
        }
        if version < 5 {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V5)?;
            for (table, column, ddl) in [
                (
                    "outbox",
                    "attempts",
                    "ALTER TABLE outbox ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0",
                ),
                (
                    "pending_ops",
                    "draft_id",
                    "ALTER TABLE pending_ops ADD COLUMN draft_id INTEGER",
                ),
            ] {
                let has: bool = tx.query_row(
                    &format!(
                        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('{table}') WHERE name='{column}')"
                    ),
                    [],
                    |r| r.get(0),
                )?;
                if !has {
                    tx.execute_batch(ddl)?;
                }
            }
            tx.execute_batch("PRAGMA user_version=5")?;
            tx.commit()?;
        }
        if version < 6 {
            let tx = conn.unchecked_transaction()?;
            // Earlier versions grouped independent provider drafts with each other and even
            // with sent mail. Give each draft its own row before enabling conversation Trash.
            let drafts: Vec<(Id, Id, Id, String)> = {
                let mut statement = tx.prepare("SELECT m.id, m.thread_id, m.account_id, m.remote_id FROM messages m
                    WHERE m.mailbox='drafts' AND (SELECT COUNT(*) FROM messages other WHERE other.thread_id=m.thread_id)>1")?;
                statement
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                    .collect::<rusqlite::Result<_>>()?
            };
            for (message, old, account, remote) in drafts {
                let thread = new_thread(&tx, account)?;
                tx.execute(
                    "UPDATE messages SET thread_id=?1 WHERE id=?2",
                    params![thread, message],
                )?;
                tx.execute(
                    "UPDATE drafts SET thread_id=?1 WHERE account_id=?2 AND thread_id=?3
                    AND (remote_id=?4 OR 'kite-draft:'||id=?4)",
                    params![thread, account, old, remote],
                )?;
                tx.execute(
                    "UPDATE undo_log SET thread_id=?1 WHERE message_id=?2",
                    params![thread, message],
                )?;
                tx.execute("INSERT INTO thread_tags(thread_id, tag_id) SELECT ?1, tag_id FROM thread_tags WHERE thread_id=?2", params![thread, old])?;
                refresh_thread(&tx, thread)?;
                refresh_thread(&tx, old)?;
            }
            tx.execute_batch("PRAGMA user_version=6")?;
            tx.commit()?;
        }
        if version < 7 {
            let tx = conn.unchecked_transaction()?;
            // Rethreading can merge conversations; its marker propagation needs the table.
            tx.execute_batch(AI_TRIAGE_SCHEMA)?;
            rethread_messages(&tx, None)?;
            tx.execute_batch("DROP INDEX IF EXISTS messages_subject; PRAGMA user_version=7")?;
            tx.commit()?;
        }
        if version < 8 {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS draft_tags (
                draft_id INTEGER NOT NULL REFERENCES drafts(id) ON DELETE CASCADE,
                tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
                PRIMARY KEY(draft_id, tag_id)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS sent_tags (
                account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                identity TEXT NOT NULL,
                tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
                PRIMARY KEY(account_id, identity, tag_id)
            ) WITHOUT ROWID; PRAGMA user_version=8",
            )?;
            tx.commit()?;
        }
        if version < 9 {
            let tx = conn.unchecked_transaction()?;
            for (table, column, definition) in [
                ("messages", "unsubscribe_header", "TEXT NOT NULL DEFAULT ''"),
                ("messages", "unsubscribe_post", "TEXT NOT NULL DEFAULT ''"),
                (
                    "messages",
                    "unsubscribe_checked",
                    "INTEGER NOT NULL DEFAULT 0",
                ),
                ("outbox", "remote_id", "TEXT"),
                ("outbox", "handed_over", "INTEGER NOT NULL DEFAULT 0"),
            ] {
                let exists:bool=tx.query_row(&format!("SELECT EXISTS(SELECT 1 FROM pragma_table_info('{table}') WHERE name='{column}')"),[],|r| r.get(0))?;
                if !exists {
                    tx.execute_batch(&format!(
                        "ALTER TABLE {table} ADD COLUMN {column} {definition}"
                    ))?;
                }
            }
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS unsubscribed_senders (
                account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                sender TEXT NOT NULL, PRIMARY KEY(account_id, sender)
            ) WITHOUT ROWID;
            UPDATE accounts SET cursor=NULL;
            PRAGMA user_version=9;",
            )?;
            tx.commit()?;
        }
        if version < 10 {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(AI_TRIAGE_SCHEMA)?;
            tx.execute_batch("PRAGMA user_version=10;")?;
            tx.commit()?;
        }
        conn.execute(
            "INSERT OR IGNORE INTO tags(name, color, position)
             VALUES (?1, '#64748b', (SELECT COALESCE(MAX(position), -1) + 1 FROM tags))",
            [PRIVATE_TAG],
        )?;
        migrate_split_defaults(&conn)?;
        Ok(Store { conn })
    }
}

/// Rebuild v6's loose conversations atomically without rewriting message IDs, bodies,
/// attachments, pending operations or outbox reply targets. Split conversations inherit
/// their local tags and reminders; message-specific undo follows the individual message.
fn rethread_messages(tx: &Transaction, only_thread: Option<Id>) -> Result<()> {
    tx.execute(
        "CREATE TEMP TABLE rethread_map AS SELECT id AS message_id, thread_id AS old_thread
        FROM messages WHERE ?1 IS NULL OR thread_id=?1",
        [only_thread],
    )?;
    tx.execute_batch(
        "DELETE FROM thread_keys WHERE thread_id IN (SELECT old_thread FROM rethread_map)",
    )?;
    let rows = {
        let mut statement = tx.prepare("SELECT id, account_id, thread_id, mailbox, message_id, in_reply_to, refs,
            CASE WHEN (SELECT provider FROM accounts WHERE id=account_id)='microsoft' THEN thread_hint END
            FROM messages WHERE id IN (SELECT message_id FROM rethread_map) ORDER BY id")?;
        statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, Id>(0)?,
                    r.get::<_, Id>(1)?,
                    r.get::<_, Id>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, Option<String>>(7)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (message, account, old, _, _, _, _, _) in &rows {
        let thread = new_thread(tx, *account)?;
        tx.execute(
            "UPDATE messages SET thread_id=?1 WHERE id=?2",
            params![thread, message],
        )?;
        tx.execute("INSERT INTO thread_tags(thread_id,tag_id) SELECT ?1,tag_id FROM thread_tags WHERE thread_id=?2", params![thread, old])?;
        tx.execute(
            "INSERT OR IGNORE INTO ai_triaged(thread_id)
            SELECT ?1 FROM ai_triaged WHERE thread_id=?2",
            params![thread, old],
        )?;
        tx.execute(
            "UPDATE threads SET
            snoozed_until=(SELECT snoozed_until FROM threads WHERE id=?2),
            remind_after=(SELECT remind_after FROM threads WHERE id=?2) WHERE id=?1",
            params![thread, old],
        )?;
    }
    for (message, account, _, mailbox, message_id, in_reply_to, refs, hint) in rows {
        let references: Vec<String> = serde_json::from_str(&refs)?;
        let keys = msg_keys(message_id.as_deref(), in_reply_to.as_deref(), &references);
        resolve_thread(
            tx,
            account,
            mb_parse(&mailbox),
            &keys,
            hint.as_deref(),
            Some(thread_id_of(tx, message)?),
        )?;
    }
    tx.execute_batch("CREATE TEMP TABLE rethread_undo AS
        SELECT u.token,u.account_id,m.thread_id,u.message_id,u.mailbox,u.unread,u.starred,u.folder,
            u.snoozed_until,u.remind_after,u.op_id,u.inverse,u.tag_id
        FROM undo_log u JOIN rethread_map r ON r.message_id=u.message_id JOIN messages m ON m.id=r.message_id
        WHERE u.message_id IS NOT NULL
        UNION ALL
        SELECT DISTINCT u.token,u.account_id,m.thread_id,u.message_id,u.mailbox,u.unread,u.starred,u.folder,
            u.snoozed_until,u.remind_after,u.op_id,u.inverse,u.tag_id
        FROM undo_log u JOIN rethread_map r ON r.old_thread=u.thread_id JOIN messages m ON m.id=r.message_id
        WHERE u.message_id IS NULL
        UNION ALL SELECT u.* FROM undo_log u
        WHERE NOT EXISTS (SELECT 1 FROM rethread_map r WHERE r.old_thread=u.thread_id);
        DELETE FROM undo_log;
        INSERT INTO undo_log SELECT * FROM rethread_undo;
        UPDATE drafts SET thread_id=COALESCE((SELECT m.thread_id FROM messages m
            WHERE m.account_id=drafts.account_id AND (m.remote_id='kite-draft:'||drafts.id OR m.remote_id=drafts.remote_id)), thread_id);
        DELETE FROM threads WHERE id NOT IN (SELECT thread_id FROM messages);
        DROP TABLE rethread_undo;")?;
    let threads = {
        let mut statement = tx.prepare(
            "SELECT DISTINCT m.thread_id FROM messages m JOIN rethread_map r ON r.message_id=m.id",
        )?;
        statement
            .query_map([], |r| r.get::<_, Id>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for thread in threads {
        refresh_thread(tx, thread)?;
    }
    tx.execute_batch("DROP TABLE rethread_map")?;
    Ok(())
}

/// Databases created before the split redesign hold the two legacy seeds (`Important`/`Other`).
/// Rewrite them to the current defaults only while they are still exactly those two unmodified
/// rows; any rename, added split or changed query means the user owns their splits, so leave them.
fn migrate_split_defaults(conn: &Connection) -> Result<()> {
    let (total, legacy): (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COUNT(*) FILTER (WHERE
             (id=1 AND name='Important' AND query='is:important')
          OR (id=2 AND name='Other' AND query='-is:important'))
         FROM splits",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if total != 2 || legacy != 2 {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM splits", [])?;
    tx.execute_batch(DEFAULT_SPLITS)?;
    tx.commit()?;
    Ok(())
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

    /// The display name used as the From name.
    pub fn set_account_name(&self, id: Id, name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts SET name=?1 WHERE id=?2",
            params![name.trim(), id],
        )?;
        Ok(())
    }

    /// When the account's provider photo was last fetched, for the once-a-day cap.
    pub fn photo_checked(&self, id: Id) -> Result<Option<Ms>> {
        let v: Option<String> = self
            .conn
            .query_row(
                "SELECT v FROM kv WHERE k=?1",
                [format!("photo_checked:{id}")],
                |r| r.get(0),
            )
            .optional()?;
        Ok(v.and_then(|v| v.parse().ok()))
    }

    pub fn set_photo_checked(&self, id: Id, at: Ms) -> Result<()> {
        self.conn.execute(
            "INSERT INTO kv(k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
            params![format!("photo_checked:{id}"), at.to_string()],
        )?;
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

    /// Adds a folder the provider reports if the catalog does not hold it yet. A name
    /// clash with a different id is ignored: one sidebar entry cannot show two folders.
    pub(crate) fn ensure_folder(&self, account: Id, id: &str, name: &str) -> Result<()> {
        if self.folder_exists(account, id)? {
            return Ok(());
        }
        let _ = self.add_folder(account, id, name);
        Ok(())
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
    pub(crate) fn remote_ids(&self, account: Id) -> Result<BTreeSet<String>> {
        let mut statement = self.conn.prepare("SELECT remote_id FROM messages WHERE account_id=?1 AND remote_id NOT LIKE 'kite-draft:%' AND (list_unsubscribe=0 OR unsubscribe_checked=1)")?;
        Ok(statement
            .query_map([account], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Applies one sync batch in a single transaction. Batches are Ã¢â€°Â¤ 200 messages (see ARCHITECTURE).
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
                    // The provider's copy of a draft this client already shows is the same draft, not
                    // a new conversation: keep the local row (and its editable content) instead.
                    if m.mailbox == Mailbox::Drafts && is_our_draft(&tx, account_id, m)? {
                        continue;
                    }
                    let thread = upsert_message(&tx, account_id, &account_email, m)?;
                    tx.execute("UPDATE messages SET unsubscribe_header=?1, unsubscribe_post=?2, unsubscribe_checked=1 WHERE account_id=?3 AND remote_id=?4",
                        params![m.unsubscribe_header, m.unsubscribe_post, account_id, m.remote_id])?;
                    auto_archive(&tx, account_id, thread)?;
                    for identity in [
                        m.message_id
                            .as_deref()
                            .map(|id| format!("m:{}", bare_msg_id(id).to_ascii_lowercase())),
                        Some(format!("r:{}", m.remote_id)),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        tx.execute(
                            "INSERT OR IGNORE INTO thread_tags(thread_id, tag_id)
                            SELECT ?1, tag_id FROM sent_tags WHERE account_id=?2 AND identity=?3",
                            params![thread, account_id, identity],
                        )?;
                        tx.execute(
                            "DELETE FROM sent_tags WHERE account_id=?1 AND identity=?2",
                            params![account_id, identity],
                        )?;
                    }
                    refresh_labels(&tx, thread)?;
                    touched.insert(thread);
                    continue;
                }
                Delta::Metadata { remote_id, .. }
                | Delta::Flags { remote_id, .. }
                | Delta::Delete { remote_id } => find_message(&tx, account_id, remote_id)?,
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
                Delta::Metadata {
                    folder,
                    mailbox,
                    unread,
                    starred,
                    ..
                } => tx.execute(
                    "UPDATE messages SET folder=?1, mailbox=?2, unread=?3, starred=?4 WHERE id=?5
                     AND NOT EXISTS (SELECT 1 FROM pending_ops WHERE message_id=?5)",
                    params![folder, mb_str(*mailbox), unread, starred, mid],
                )?,
                Delta::Delete { .. } => tx.execute("DELETE FROM messages WHERE id=?1", [mid])?,
                Delta::Upsert(_) => unreachable!(),
            };
            if changed > 0 {
                auto_archive(&tx, account_id, tid)?;
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

// Exact normalized addresses, scoped to the receiving account. Undo leaves this preference intact.
fn auto_archive(tx: &Transaction<'_>, account: Id, thread: Id) -> Result<()> {
    let messages = {
        let mut q = tx.prepare(
            "SELECT m.id, m.remote_id FROM messages m JOIN unsubscribed_senders u
            ON u.account_id=m.account_id AND u.sender=m.from_email
            WHERE m.account_id=?1 AND m.thread_id=?2 AND m.mailbox='inbox'",
        )?;
        q.query_map(params![account, thread], |r| {
            Ok((r.get::<_, Id>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, remote) in messages {
        tx.execute("UPDATE messages SET mailbox='archive' WHERE id=?1", [id])?;
        tx.execute(
            "INSERT INTO pending_ops(account_id,message_id,op,at) VALUES (?1,?2,?3,?4)",
            params![
                account,
                id,
                json(&Op::Move {
                    remote_id: remote,
                    to: Mailbox::Archive
                })?,
                now_ms()
            ],
        )?;
    }
    Ok(())
}

fn find_message(tx: &Transaction, account_id: Id, remote_id: &str) -> Result<Option<(Id, Id)>> {
    Ok(tx
        .query_row(
            "SELECT id, thread_id FROM messages WHERE account_id=?1 AND remote_id=?2
             UNION ALL SELECT m.id, m.thread_id FROM drafts d JOIN messages m
               ON m.account_id=d.account_id AND m.remote_id='kite-draft:'||d.id
             WHERE d.account_id=?1 AND d.remote_id=?2 LIMIT 1",
            params![account_id, remote_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

/// Whether a remote message is the provider copy of a draft saved here. Both keys are needed: the
/// former survives a replaced copy, the latter a Message-ID the provider rewrote.
fn is_our_draft(tx: &Transaction, account_id: Id, m: &RemoteMsg) -> Result<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM drafts WHERE account_id=?1
           AND ((?2 IS NOT NULL AND msg_id=?2) OR (?3 IS NOT NULL AND remote_id=?3)))",
        params![
            account_id,
            m.message_id.as_deref().map(bare_msg_id),
            m.remote_id
        ],
        |r| r.get(0),
    )?)
}

/// A Message-ID without its angle brackets: IMAP tags carry them, Graph's `internetMessageId` and
/// the ids Kite generates do not always, and every Message-ID we store is bare.
fn bare_msg_id(raw: &str) -> String {
    raw.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
        .to_string()
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
    let existing = find_message(tx, account_id, &m.remote_id)?.map(|(id, _)| id);
    let mid = match existing {
        Some(id) => {
            let had_fallback: bool = tx.query_row(
                "SELECT m.thread_hint IS NOT NULL AND a.provider='microsoft'
                FROM messages m JOIN accounts a ON a.id=m.account_id WHERE m.id=?1",
                [id],
                |r| r.get(0),
            )?;
            // While a local change is queued, keep the local placement and flags (?22 = pending).
            tx.execute(
                "UPDATE messages SET
                   folder=CASE WHEN ?22 THEN folder ELSE ?1 END, mailbox=CASE WHEN ?22 THEN mailbox ELSE ?2 END,
                   thread_hint=?3, message_id=?4, in_reply_to=?5,
                   refs=?6, from_name=?7, from_email=?8, to_json=?9, cc_json=?10, date=?11, subject=?12,
                   norm_subject=?13, text=?14, html=?15,
                   unread=CASE WHEN ?22 THEN unread ELSE ?16 END, starred=CASE WHEN ?22 THEN starred ELSE ?17 END,
                   important=?18, important_src=?19, from_self=?20, list_unsubscribe=?23 WHERE id=?21",
                params![m.folder, mailbox, m.thread_hint, m.message_id, m.in_reply_to, refs_json, from_name,
                        from_email, to_json, cc_json, m.date, m.subject, ns, m.text, m.html, m.unread,
                        m.starred, important, src, from_self, id,
                        tx.query_row("SELECT EXISTS(SELECT 1 FROM pending_ops WHERE message_id=?1)", [id], |r| r.get::<_, bool>(0))?,
                        m.list_unsubscribe],
            )?;
            if had_fallback && m.thread_hint.is_none() {
                // Full Graph headers arrived after a fallback: rebuild that conversation so
                // independent roots can separate as well as genuine replies unite.
                rethread_messages(tx, Some(thread_id_of(tx, id)?))?;
                return thread_id_of(tx, id);
            }
            return resolve_thread(
                tx,
                account_id,
                m.mailbox,
                &msg_keys(
                    m.message_id.as_deref(),
                    m.in_reply_to.as_deref(),
                    &m.references,
                ),
                m.thread_hint.as_deref(),
                Some(thread_id_of(tx, id)?),
            );
        }
        None => {
            let thread_id = resolve_thread(
                tx,
                account_id,
                m.mailbox,
                &msg_keys(
                    m.message_id.as_deref(),
                    m.in_reply_to.as_deref(),
                    &m.references,
                ),
                m.thread_hint.as_deref(),
                None,
            )?;
            tx.execute(
                "INSERT INTO messages(account_id, thread_id, remote_id, folder, mailbox, thread_hint,
                   message_id, in_reply_to, refs, from_name, from_email, to_json, cc_json, date, subject,
                   norm_subject, text, html, unread, starred, important, important_src, from_self, list_unsubscribe)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24)",
                params![account_id, thread_id, m.remote_id, m.folder, mailbox, m.thread_hint, m.message_id,
                        m.in_reply_to, refs_json, from_name, from_email, to_json, cc_json, m.date, m.subject,
                        ns, m.text, m.html, m.unread, m.starred, important, src, from_self, m.list_unsubscribe],
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

/// Header identity and References/In-Reply-To form an account-scoped union. Provider
/// hints are a fallback only for Graph messages whose full headers were unavailable.
fn resolve_thread(
    tx: &Transaction,
    account_id: Id,
    mailbox: Mailbox,
    keys: &[String],
    hint: Option<&str>,
    existing: Option<Id>,
) -> Result<Id> {
    // Drafts remain individual editable messages, including replies to sent mail.
    if mailbox == Mailbox::Drafts {
        return match existing {
            Some(id) => Ok(id),
            None => new_thread(tx, account_id),
        };
    }
    let mut cands: Vec<Id> = existing.into_iter().collect();
    for key in keys {
        if let Some(thread) = key_thread(tx, account_id, key)? {
            cands.push(thread);
        }
    }
    let hint_key = if hint.is_some()
        && tx.query_row(
            "SELECT provider='microsoft' FROM accounts WHERE id=?1",
            [account_id],
            |r| r.get::<_, bool>(0),
        )? {
        hint.filter(|h| !h.is_empty()).map(|h| format!("h:{h}"))
    } else {
        None
    };
    if let Some(key) = &hint_key
        && cands.len() == usize::from(existing.is_some())
        && let Some(thread) = key_thread(tx, account_id, key)?
    {
        cands.push(thread);
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
        None => new_thread(tx, account_id)?,
    };
    for key in keys.iter().chain(hint_key.iter()) {
        tx.execute(
            "INSERT OR IGNORE INTO thread_keys(account_id, key, thread_id) VALUES (?1,?2,?3)",
            params![account_id, key, tid],
        )?;
    }
    Ok(tid)
}

fn new_thread(tx: &Transaction, account_id: Id) -> Result<Id> {
    tx.execute(
        "INSERT INTO threads(account_id, subject, norm_subject, snippet, participants, date,
           unread, starred, important, attachments, count, mailbox)
         VALUES (?1,'','','','',0,0,0,0,0,0,'inbox')",
        [account_id],
    )?;
    Ok(tx.last_insert_rowid())
}

fn merge_thread(tx: &Transaction, canon: Id, other: Id) -> Result<()> {
    if canon != other {
        // thread_tags is keyed by thread and cascades on delete: move the memberships before the
        // loser row goes away so the survivor keeps the union (the labels cache is refreshed after).
        tx.execute(
            "INSERT OR IGNORE INTO thread_tags(thread_id, tag_id)
             SELECT ?1, tag_id FROM thread_tags WHERE thread_id=?2",
            params![canon, other],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO ai_triaged(thread_id)
            SELECT ?1 FROM ai_triaged WHERE thread_id=?2",
            params![canon, other],
        )?;
        tx.execute(
            "UPDATE OR REPLACE thread_keys SET thread_id=?1 WHERE thread_id=?2",
            params![canon, other],
        )?;
        tx.execute(
            "UPDATE messages SET thread_id=?1 WHERE thread_id=?2",
            params![canon, other],
        )?;
        tx.execute(
            "UPDATE undo_log SET thread_id=?1 WHERE thread_id=?2",
            params![canon, other],
        )?;
        tx.execute(
            "UPDATE drafts SET thread_id=?1 WHERE thread_id=?2",
            params![canon, other],
        )?;
        tx.execute(
            "UPDATE threads SET
            snoozed_until=(SELECT MAX(snoozed_until) FROM threads WHERE id IN (?1,?2)),
            remind_after=(SELECT MAX(remind_after) FROM threads WHERE id IN (?1,?2))
            WHERE id=?1",
            params![canon, other],
        )?;
        tx.execute("DELETE FROM threads WHERE id=?1", [other])?;
        refresh_labels(tx, canon)?;
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

fn msg_keys(
    message_id: Option<&str>,
    in_reply_to: Option<&str>,
    references: &[String],
) -> Vec<String> {
    let mut raw: Vec<&str> = Vec::new();
    raw.extend(message_id);
    raw.extend(in_reply_to);
    raw.extend(references.iter().map(String::as_str));
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
        format!(
            "{}, {}, {}, {}Ã¢â‚¬Â¦",
            names[0], names[1], names[2], names[3]
        )
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
    refresh_labels(tx, thread_id)
}

/// Rewrites the denormalized `threads.labels` cache from current memberships and tag positions.
fn refresh_labels(tx: &Transaction, thread_id: Id) -> Result<()> {
    let labels: Vec<String> = {
        let mut s = tx.prepare(
            "SELECT g.name FROM thread_tags tt JOIN tags g ON g.id=tt.tag_id
             WHERE tt.thread_id=?1 ORDER BY g.position, g.id",
        )?;
        let it = s.query_map([thread_id], |r| r.get(0))?;
        it.collect::<rusqlite::Result<Vec<_>>>()?
    };
    tx.execute(
        "UPDATE threads SET labels=?1 WHERE id=?2",
        params![json(&labels)?, thread_id],
    )?;
    Ok(())
}

// ------------------------------------------------------------- reads: views

impl Store {
    /// The WHERE clause a view produces: mailbox or folder, then the split and search grammar.
    /// Shared by `threads` and `archivable` so both see exactly the same conversation set.
    fn view_conds(&self, view: &View) -> Result<(Vec<String>, Vec<Value>)> {
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
        Ok((conds, ps))
    }

    /// Thread ids in `view` whose latest activity predates `before`: the bulk-archive dialog's
    /// count and confirm set. Snoozed conversations always keep waiting; starred ones stay out
    /// unless the dialog asked for them.
    pub fn archivable(&self, view: &View, before: Ms, include_starred: bool) -> Result<Vec<Id>> {
        let (mut conds, mut ps) = self.view_conds(view)?;
        conds.push("t.date < ?".into());
        ps.push(Value::Integer(before));
        conds.push("t.snoozed_until IS NULL".into());
        if !include_starred {
            conds.push("t.starred=0".into());
        }
        let sql = format!(
            "SELECT t.id FROM threads t WHERE {} ORDER BY t.date DESC",
            conds.join(" AND ")
        );
        let mut s = self.conn.prepare(&sql)?;
        let it = s.query_map(params_from_iter(ps), |r| r.get(0))?;
        Ok(it.collect::<rusqlite::Result<Vec<Id>>>()?)
    }

    /// One indexed query over the denormalized `threads` table. User input never reaches SQL as
    /// syntax: the search grammar only produces fixed fragments plus bound parameters.
    pub fn threads(&self, view: &View, offset: u32, limit: u32) -> Result<Vec<ThreadSummary>> {
        let (conds, mut ps) = self.view_conds(view)?;
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
    tag_id: Option<Id>,
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
        let token = Self::act_in(&tx, ids, action)?;
        tx.commit()?;
        Ok(token)
    }

    fn act_in(tx: &Transaction<'_>, ids: &[Id], action: &Action) -> Result<i64> {
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
            // Tags are thread-level, local-only and queue no remote op. A no-op (already labelled
            // or already unlabelled) records nothing, so undo cannot touch the pre-existing state.
            if let Action::Label { tag } | Action::Unlabel { tag } = action {
                let Some(tag_id) = tag_id_by_name(tx, tag)? else {
                    bail!("unknown tag {tag:?}");
                };
                let labelled: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM thread_tags WHERE thread_id=?1 AND tag_id=?2)",
                    params![thread_id, tag_id],
                    |r| r.get(0),
                )?;
                let on = matches!(action, Action::Label { .. });
                if on != labelled {
                    if on {
                        tx.execute(
                            "INSERT INTO thread_tags(thread_id, tag_id) VALUES (?1,?2)",
                            params![thread_id, tag_id],
                        )?;
                    } else {
                        tx.execute(
                            "DELETE FROM thread_tags WHERE thread_id=?1 AND tag_id=?2",
                            params![thread_id, tag_id],
                        )?;
                    }
                    tx.execute(
                        "INSERT INTO undo_log(token, account_id, thread_id, tag_id) VALUES (?1,?2,?3,?4)",
                        params![token, account_id, thread_id, if on { tag_id } else { -tag_id }],
                    )?;
                    refresh_labels(tx, thread_id)?;
                }
                continue;
            }
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
            let states = message_states(tx, thread_id)?;
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
            refresh_thread(tx, thread_id)?;
        }
        Ok(token)
    }

    /// Restores the previous local state. Pending ops from the action are dropped while unsent;
    /// ops that already went out get their inverse queued.
    pub fn undo(&self, token: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let rows: Vec<UndoRow> = {
            let mut s = tx.prepare(
                "SELECT account_id, thread_id, message_id, tag_id, mailbox, unread, starred, folder,
                        snoozed_until, remind_after, op_id, inverse FROM undo_log WHERE token=?1",
            )?;
            let it = s.query_map([token], |r| {
                Ok(UndoRow {
                    account_id: r.get(0)?,
                    thread_id: r.get(1)?,
                    message_id: r.get(2)?,
                    tag_id: r.get(3)?,
                    mailbox: r.get(4)?,
                    unread: r.get(5)?,
                    starred: r.get(6)?,
                    folder: r.get(7)?,
                    snoozed_until: r.get(8)?,
                    remind_after: r.get(9)?,
                    op_id: r.get(10)?,
                    inverse: r.get(11)?,
                })
            })?;
            it.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut touched: HashSet<Id> = HashSet::new();
        for r in rows {
            if let Some(tag_id) = r.tag_id {
                if tag_id > 0 {
                    tx.execute(
                        "DELETE FROM thread_tags WHERE thread_id=?1 AND tag_id=?2",
                        params![r.thread_id, tag_id],
                    )?;
                } else {
                    // The tag or thread may have been deleted since; then the restored membership
                    // is moot, and the thread guard keeps the INSERT from failing the FK.
                    tx.execute(
                        "INSERT OR IGNORE INTO thread_tags(thread_id, tag_id)
                         SELECT ?1, id FROM tags WHERE id=?2
                           AND EXISTS(SELECT 1 FROM threads WHERE id=?1)",
                        params![r.thread_id, -tag_id],
                    )?;
                }
                touched.insert(r.thread_id);
                continue;
            }
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
                    // A move that already reached the server changed remote_id (op_done). The
                    // stored inverse still names the old one, so re-address it at where the
                    // message actually lives now, or it would fail and retry forever.
                    let mut op: Op = serde_json::from_str(&inv)?;
                    let current = match r.message_id {
                        Some(mid) => current_remote_id(&tx, mid)?,
                        None => None,
                    };
                    if let Some(rid) = current {
                        op = with_remote_id(op, &rid);
                    }
                    tx.execute(
                        "INSERT INTO pending_ops(account_id, message_id, op, inverse, at) VALUES (?1,?2,?3,NULL,?4)",
                        params![r.account_id, r.message_id, json(&op)?, now_ms()],
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

/// Replaces the remote id an op is addressed to. A move changes it (op_done receives the new
/// one), so an inverse captured when the action was taken can name an id the server no longer has.
fn with_remote_id(op: Op, remote_id: &str) -> Op {
    match op {
        Op::Move { to, .. } => Op::Move {
            remote_id: remote_id.into(),
            to,
        },
        Op::MoveToFolder { folder, .. } => Op::MoveToFolder {
            remote_id: remote_id.into(),
            folder,
        },
        Op::Flags {
            unread, starred, ..
        } => Op::Flags {
            remote_id: remote_id.into(),
            unread,
            starred,
        },
        Op::Send { mime } => Op::Send { mime },
        other => other,
    }
}

fn current_remote_id(tx: &Connection, message_id: Id) -> Result<Option<String>> {
    Ok(tx
        .query_row(
            "SELECT COALESCE(d.remote_id, m.remote_id) FROM messages m
             LEFT JOIN drafts d ON d.account_id=m.account_id AND m.remote_id='kite-draft:'||d.id
             WHERE m.id=?1",
            [message_id],
            |r| r.get(0),
        )
        .optional()?)
}

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
        Action::Trash if m.mailbox != "trash" => (
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

fn tag_id_by_name(tx: &Transaction, name: &str) -> Result<Option<Id>> {
    Ok(tx
        .query_row(
            "SELECT id FROM tags WHERE name=?1 COLLATE NOCASE",
            [name.trim()],
            |r| r.get(0),
        )
        .optional()?)
}

fn message_states(tx: &Transaction, thread_id: Id) -> Result<Vec<MsgState>> {
    let mut s = tx.prepare(
        "SELECT m.id, COALESCE(d.remote_id, m.remote_id), m.mailbox, m.folder, m.unread, m.starred
         FROM messages m LEFT JOIN drafts d
           ON d.account_id=m.account_id AND m.remote_id='kite-draft:'||d.id
         WHERE m.thread_id=?1 ORDER BY m.date, m.id",
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

    /// Resolve each op when it runs: an earlier draft save or move in the same batch can change
    /// the provider id after the batch was read.
    pub(crate) fn address_op(&self, op_id: i64, op: Op) -> Result<Op> {
        let message: Option<Id> = self.conn.query_row(
            "SELECT message_id FROM pending_ops WHERE id=?1",
            [op_id],
            |r| r.get(0),
        )?;
        match message
            .map(|id| current_remote_id(&self.conn, id))
            .transpose()?
            .flatten()
        {
            Some(remote) => Ok(with_remote_id(op, &remote)),
            None => Ok(op),
        }
    }

    /// A move changes a message's remote id; keep the local row pointing at the new one. A draft's
    /// save reports the provider copy's id, which the `drafts` row records instead.
    pub fn op_done(&self, op_id: i64, new_remote_id: Option<String>) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        if let Some(rid) = new_remote_id {
            let stale: Option<Id> = tx.query_row(
                "SELECT p.account_id FROM pending_ops p LEFT JOIN drafts d ON d.id=p.draft_id
                 WHERE p.id=?1 AND p.draft_id IS NOT NULL AND (d.id IS NULL OR d.account_id!=p.account_id)",
                [op_id], |r| r.get(0),
            ).optional()?;
            if let Some(account) = stale {
                // The save may already have reached the provider when From changed (or the
                // draft was sent). Its result belongs to the old mailbox and must be removed.
                tx.execute(
                    "INSERT INTO pending_ops(account_id, op, at) VALUES (?1,?2,?3)",
                    params![
                        account,
                        json(&Op::DeleteDraft { remote_id: rid })?,
                        now_ms()
                    ],
                )?;
                tx.execute("DELETE FROM pending_ops WHERE id=?1", [op_id])?;
                tx.commit()?;
                return Ok(());
            }
            tx.execute(
                "UPDATE drafts SET remote_id=?1 WHERE 'kite-draft:'||id=(
                   SELECT remote_id FROM messages WHERE id=(SELECT message_id FROM pending_ops WHERE id=?2))",
                params![rid, op_id],
            )?;
            tx.execute(
                "UPDATE messages SET remote_id=?1 WHERE id=(SELECT message_id FROM pending_ops WHERE id=?2)
                   AND remote_id NOT LIKE 'kite-draft:%'",
                params![rid, op_id],
            )?;
            tx.execute(
                "UPDATE drafts SET remote_id=?1 WHERE id=(SELECT draft_id FROM pending_ops WHERE id=?2)",
                params![rid, op_id],
            )?;
        }
        tx.execute("DELETE FROM pending_ops WHERE id=?1", [op_id])?;
        tx.commit()?;
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

// ---------------------------------------------------------------------- drafts

/// The `messages` row that shows a saved draft in the Drafts mailbox. Derived from the draft id so
/// saving again replaces the same row instead of adding one.
fn draft_row_id(draft: Id) -> String {
    format!("kite-draft:{draft}")
}

/// A draft that arrived from the provider as an ordinary Drafts row, before it was adopted.
struct Adopted {
    message: Id,
    account: Id,
    remote: String,
    msg_id: Option<String>,
    subject: String,
    text: String,
    to: String,
    cc: String,
}

impl Store {
    /// The Message-ID a saved draft keeps across edits, so the provider's own copy stays the same
    /// conversation (`apply_deltas` skips it instead of inserting a second Drafts row).
    pub fn draft_msg_id(&self, id: Id) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT msg_id FROM drafts WHERE id=?1", [id], |r| r.get(0))
            .optional()?)
    }

    /// Writes a draft locally at once and returns `(draft id, provider copy it replaces)`. One short
    /// transaction: the Drafts row is visible to the UI as soon as this returns.
    pub fn save_draft(&self, draft: &Draft, msg_id: &str) -> Result<(Id, Option<String>)> {
        let tx = self.conn.unchecked_transaction()?;
        let (name, email) = tx
            .query_row(
                "SELECT name, email FROM accounts WHERE id=?1",
                [draft.account_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?
            .context("account was removed")?;
        if let Some(id) = draft.id {
            rehome_draft(&tx, id, draft.account_id)?;
        }
        let existing = match draft.id {
            Some(id) => tx
                .query_row(
                    "SELECT thread_id, remote_id FROM drafts WHERE id=?1",
                    [id],
                    |r| Ok((r.get::<_, Option<Id>>(0)?, r.get::<_, Option<String>>(1)?)),
                )
                .optional()?,
            None => None,
        };
        let (id, replaced, thread) = match draft.id {
            Some(id) => match existing {
                Some((thread, remote)) => (id, remote, thread),
                None => {
                    // The UI remembers an id whose row is gone (e.g. a send that failed): keep it,
                    // so the next save still replaces rather than piling up drafts.
                    tx.execute(
                        "INSERT INTO drafts(id, account_id, msg_id, draft, updated) VALUES (?1,?2,?3,?4,?5)",
                        params![id, draft.account_id, msg_id, json(draft)?, now_ms()],
                    )?;
                    (id, None, None)
                }
            },
            None => {
                tx.execute(
                    "INSERT INTO drafts(account_id, msg_id, draft, updated) VALUES (?1,?2,?3,?4)",
                    params![draft.account_id, msg_id, json(draft)?, now_ms()],
                )?;
                (tx.last_insert_rowid(), None, None)
            }
        };
        let mut content = draft.clone();
        content.id = Some(id);
        let msg_id = bare_msg_id(msg_id);
        tx.execute(
            "UPDATE drafts SET msg_id=?1, draft=?2, updated=?3 WHERE id=?4",
            params![msg_id, json(&content)?, now_ms(), id],
        )?;
        let thread = match thread {
            Some(thread) => {
                update_draft_message(&tx, id, draft, &msg_id)?;
                thread
            }
            None => insert_draft_message(&tx, id, draft, &name, &email, &msg_id)?,
        };
        tx.execute(
            "UPDATE drafts SET thread_id=?1 WHERE id=?2",
            params![thread, id],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO thread_tags(thread_id, tag_id)
             SELECT ?1, tag_id FROM draft_tags WHERE draft_id=?2",
            params![thread, id],
        )?;
        refresh_labels(&tx, thread)?;
        tx.execute("DELETE FROM draft_tags WHERE draft_id=?1", [id])?;
        refresh_thread(&tx, thread)?;
        // A draft row reads better as its recipients than as its sender (which is always us).
        tx.execute(
            "UPDATE threads SET participants=?1 WHERE id=?2",
            params![draft_participants(draft), thread],
        )?;
        tx.commit()?;
        Ok((id, replaced))
    }

    /// The draft behind a Drafts-mailbox thread, with its id filled in. A draft written in another
    /// client has no editable copy yet, so it is adopted on the spot and its provider copy becomes
    /// the one a save replaces. Adoption writes, so callers use the writer connection.
    pub fn draft(&self, thread: Id) -> Result<Option<Draft>> {
        let known: Option<(Id, String)> = self
            .conn
            .query_row(
                "SELECT id, draft FROM drafts WHERE thread_id=?1",
                [thread],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((id, raw)) = known {
            let mut draft: Draft = serde_json::from_str(&raw).context("saved draft")?;
            draft.id = Some(id);
            return Ok(Some(draft));
        }

        let row: Option<Adopted> = self
            .conn
            .query_row(
                "SELECT id, account_id, remote_id, message_id, subject, text, to_json, cc_json
                 FROM messages WHERE thread_id=?1 AND mailbox='drafts'
                 ORDER BY date DESC, id DESC LIMIT 1",
                [thread],
                |r| {
                    Ok(Adopted {
                        message: r.get(0)?,
                        account: r.get(1)?,
                        remote: r.get(2)?,
                        msg_id: r.get(3)?,
                        subject: r.get(4)?,
                        text: r.get(5)?,
                        to: r.get(6)?,
                        cc: r.get(7)?,
                    })
                },
            )
            .optional()?;
        let Some(row) = row else {
            return Ok(None);
        };
        let tx = self.conn.unchecked_transaction()?;
        let mut draft = Draft {
            id: None,
            account_id: row.account,
            to: addrs_from_json(&row.to),
            cc: addrs_from_json(&row.cc),
            subject: row.subject,
            body: row.text,
            ..Draft::default()
        };
        tx.execute(
            "INSERT INTO drafts(account_id, thread_id, remote_id, msg_id, draft, updated)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                row.account,
                thread,
                row.remote,
                row.msg_id.as_deref().map(bare_msg_id).unwrap_or_default(),
                json(&draft)?,
                now_ms()
            ],
        )?;
        let id = tx.last_insert_rowid();
        draft.id = Some(id);
        // The local row becomes this draft's editable copy; the provider id it held is kept on the
        // `drafts` row, which is what a save passes as `replaces`.
        tx.execute(
            "UPDATE messages SET remote_id=?1 WHERE id=?2",
            params![draft_row_id(id), row.message],
        )?;
        tx.execute(
            "UPDATE drafts SET draft=?1 WHERE id=?2",
            params![json(&draft)?, id],
        )?;
        tx.execute(
            "UPDATE threads SET participants=?1 WHERE id=?2",
            params![draft_participants(&draft), thread],
        )?;
        tx.commit()?;
        Ok(Some(draft))
    }

    /// Deletes a draft locally, returning `(account, provider copy)` when the provider still holds
    /// one for the engine to queue a delete op.
    pub fn discard_draft(&self, id: Id) -> Result<Option<(Id, String)>> {
        let tx = self.conn.unchecked_transaction()?;
        let row: Option<(Id, Option<Id>, Option<String>)> = tx
            .query_row(
                "SELECT account_id, thread_id, remote_id FROM drafts WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((account, thread, remote)) = row else {
            return Ok(None);
        };
        tx.execute(
            "DELETE FROM messages WHERE account_id=?1 AND remote_id=?2",
            params![account, draft_row_id(id)],
        )?;
        tx.execute("DELETE FROM drafts WHERE id=?1", [id])?;
        if let Some(thread) = thread {
            refresh_thread(&tx, thread)?;
        }
        tx.commit()?;
        Ok(remote.map(|remote| (account, remote)))
    }

    /// Drops a draft row after a successful send, returning the provider copy to delete.
    pub fn draft_sent(&self, id: Id) -> Result<Option<(Id, String)>> {
        let tx = self.conn.unchecked_transaction()?;
        let row: Option<(Id, Option<Id>, Option<String>)> = tx
            .query_row(
                "SELECT account_id, thread_id, remote_id FROM drafts WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((account, thread, remote)) = row else {
            return Ok(None);
        };
        tx.execute("DELETE FROM drafts WHERE id=?1", [id])?;
        if let Some(thread) = thread {
            refresh_thread(&tx, thread)?;
        }
        tx.commit()?;
        Ok(remote.map(|remote| (account, remote)))
    }

    /// Retain the hidden local draft for cancellation of a provider schedule.
    pub fn draft_provider_sent(&self, id: Id) -> Result<Option<(Id, String)>> {
        let tx = self.conn.unchecked_transaction()?;
        let remote = tx
            .query_row(
                "SELECT account_id,remote_id FROM drafts WHERE id=?1 AND remote_id IS NOT NULL",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        tx.execute("UPDATE drafts SET remote_id=NULL WHERE id=?1", [id])?;
        tx.commit()?;
        Ok(remote)
    }

    /// Persist before transmission so tags survive a successful send followed by delayed sync
    /// or restart. Proton additionally supplies its exact provider identity after sending.
    pub fn remember_sent_tags(&self, draft: Id, account: Id, identity: &str) -> Result<()> {
        self.conn.execute("INSERT OR IGNORE INTO sent_tags(account_id, identity, tag_id)
            SELECT ?1, ?2, dt.tag_id FROM draft_tags dt JOIN drafts d ON d.id=dt.draft_id
            WHERE d.id=?3 AND d.account_id=?1
            UNION SELECT ?1, ?2, tt.tag_id FROM thread_tags tt JOIN drafts d ON d.thread_id=tt.thread_id
            WHERE d.id=?3 AND d.account_id=?1", params![account, identity, draft])?;
        Ok(())
    }

    /// Queues a provider-side draft op now. `draft` links a `SaveDraft` to the row that learns its
    /// new remote id once the backend reports it.
    pub fn draft_op(&self, account: Id, op: &Op, draft: Option<Id>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO pending_ops(account_id, message_id, draft_id, op, at) VALUES (?1, NULL, ?2, ?3, ?4)",
            params![account, draft, json(op)?, now_ms()],
        )?;
        Ok(())
    }
}

/// Account-scoped thread and provider ids cannot follow a draft's From account.
fn rehome_draft(tx: &Transaction, id: Id, account: Id) -> Result<()> {
    let old: Option<(Id, Option<String>)> = tx
        .query_row(
            "SELECT account_id, remote_id FROM drafts WHERE id=?1 AND account_id!=?2",
            params![id, account],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((old_account, remote)) = old else {
        return Ok(());
    };
    // A deleted message's SQLite row id can be reused by the new account's draft.
    tx.execute(
        "UPDATE pending_ops SET message_id=NULL, draft_id=?1 WHERE account_id=?2
         AND message_id IN (SELECT id FROM messages WHERE account_id=?2 AND remote_id=?3)",
        params![id, old_account, draft_row_id(id)],
    )?;
    hide_draft_row(tx, id)?;
    tx.execute(
        "UPDATE drafts SET account_id=?1, thread_id=NULL, remote_id=NULL WHERE id=?2",
        params![account, id],
    )?;
    if let Some(remote_id) = remote {
        tx.execute(
            "INSERT INTO pending_ops(account_id, op, at) VALUES (?1,?2,?3)",
            params![old_account, json(&Op::DeleteDraft { remote_id })?, now_ms()],
        )?;
    }
    Ok(())
}

fn insert_draft_message(
    tx: &Transaction,
    id: Id,
    draft: &Draft,
    name: &str,
    email: &str,
    msg_id: &str,
) -> Result<Id> {
    tx.execute(
        "INSERT INTO threads(account_id, subject, norm_subject, snippet, participants, date,
           unread, starred, important, attachments, count, mailbox)
         VALUES (?1,'','','','',0,0,0,0,0,0,'drafts')",
        [draft.account_id],
    )?;
    let thread = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO messages(account_id, thread_id, remote_id, folder, mailbox, message_id,
           from_name, from_email, to_json, cc_json, date, subject, norm_subject, text,
           unread, starred, important, important_src, from_self)
         VALUES (?1,?2,?3,'Drafts','drafts',?4,?5,?6,?7,?8,?9,?10,?11,?12,0,0,0,2,1)",
        params![
            draft.account_id,
            thread,
            draft_row_id(id),
            msg_id,
            display_name(name, email),
            email,
            addrs_json(&draft.to),
            addrs_json(&draft.cc),
            now_ms(),
            draft.subject,
            norm_subject(&draft.subject),
            draft.body,
        ],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO thread_keys(account_id, key, thread_id) VALUES (?1,?2,?3)",
        params![
            draft.account_id,
            format!("i:{}", msg_id.to_ascii_lowercase()),
            thread
        ],
    )?;
    Ok(thread)
}

fn update_draft_message(tx: &Transaction, id: Id, draft: &Draft, msg_id: &str) -> Result<()> {
    tx.execute(
        "UPDATE messages SET message_id=?1, to_json=?2, cc_json=?3, date=?4, subject=?5,
           norm_subject=?6, text=?7 WHERE account_id=?8 AND remote_id=?9",
        params![
            msg_id,
            addrs_json(&draft.to),
            addrs_json(&draft.cc),
            now_ms(),
            draft.subject,
            norm_subject(&draft.subject),
            draft.body,
            draft.account_id,
            draft_row_id(id),
        ],
    )?;
    Ok(())
}

/// Hides a queued-for-send draft from the Drafts mailbox while keeping its row (and provider id),
/// so a cancelled send can put it back.
fn hide_draft_row(tx: &Transaction, id: Id) -> Result<()> {
    let thread: Option<Option<Id>> = tx
        .query_row("SELECT thread_id FROM drafts WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .optional()?;
    if let Some(Some(thread)) = thread {
        tx.execute(
            "INSERT OR IGNORE INTO draft_tags(draft_id, tag_id)
            SELECT ?1, tag_id FROM thread_tags WHERE thread_id=?2",
            params![id, thread],
        )?;
        tx.execute(
            "DELETE FROM messages WHERE remote_id=?1 AND account_id=(SELECT account_id FROM drafts WHERE id=?2)",
            params![draft_row_id(id), id],
        )?;
        refresh_thread(tx, thread)?;
    }
    Ok(())
}

/// Puts a hidden draft back into the Drafts mailbox (a cancelled send).
fn restore_draft_row(tx: &Transaction, id: Id) -> Result<()> {
    let row: Option<(Option<Id>, String, String)> = tx
        .query_row(
            "SELECT thread_id, msg_id, draft FROM drafts WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((None, msg_id, raw)) = row else {
        return Ok(());
    };
    let draft: Draft = serde_json::from_str(&raw).context("saved draft")?;
    let (name, email) = tx.query_row(
        "SELECT name, email FROM accounts WHERE id=?1",
        [draft.account_id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )?;
    let thread = insert_draft_message(tx, id, &draft, &name, &email, &msg_id)?;
    tx.execute(
        "INSERT OR IGNORE INTO thread_tags(thread_id, tag_id)
        SELECT ?1, tag_id FROM draft_tags WHERE draft_id=?2",
        params![thread, id],
    )?;
    tx.execute("DELETE FROM draft_tags WHERE draft_id=?1", [id])?;
    refresh_labels(tx, thread)?;
    tx.execute(
        "UPDATE drafts SET thread_id=?1 WHERE id=?2",
        params![thread, id],
    )?;
    refresh_thread(tx, thread)?;
    tx.execute(
        "UPDATE threads SET participants=?1 WHERE id=?2",
        params![draft_participants(&draft), thread],
    )?;
    Ok(())
}

fn draft_participants(draft: &Draft) -> String {
    let mut names: Vec<String> = Vec::new();
    for a in draft.to.iter().chain(&draft.cc) {
        let name = display_name(&a.name, &a.email);
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    if names.len() > 4 {
        format!(
            "{}, {}, {}, {}Ã¢â‚¬Â¦",
            names[0], names[1], names[2], names[3]
        )
    } else {
        names.join(", ")
    }
}

// ---------------------------------------------------------------- outbox, ...

impl Store {
    pub fn outbox_add(&self, draft: &Draft, due: Ms) -> Result<Id> {
        let tx = self.conn.unchecked_transaction()?;
        // A saved draft leaves the Drafts mailbox the moment it is queued, so sending looks
        // immediate. The row stays for a cancel to restore and a successful send to delete.
        if let Some(id) = draft.id {
            rehome_draft(&tx, id, draft.account_id)?;
            tx.execute(
                "UPDATE drafts SET draft=?1 WHERE id=?2",
                params![json(draft)?, id],
            )?;
            hide_draft_row(&tx, id)?;
        }
        tx.execute(
            "INSERT INTO outbox(account_id, draft, at) VALUES (?1,?2,?3)",
            params![draft.account_id, json(draft)?, due],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }

    pub fn unsubscribe_source(&self, thread: Id) -> Result<(Id, String, String, String)> {
        Ok(self.conn.query_row("SELECT account_id, from_email, unsubscribe_header, unsubscribe_post
            FROM messages WHERE thread_id=?1 AND unsubscribe_header<>'' ORDER BY date DESC,id DESC LIMIT 1",
            [thread], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?)
    }

    pub fn unsubscribe_archive(&self, account: Id, sender: &str) -> Result<(i64, usize)> {
        let tx = self.conn.unchecked_transaction()?;
        let ids = {
            let mut q = tx.prepare("SELECT DISTINCT thread_id FROM messages WHERE account_id=?1 AND from_email=?2 AND mailbox='inbox'")?;
            q.query_map(params![account, sender], |r| r.get::<_, Id>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let token = Self::act_in(&tx, &ids, &Action::Archive)?;
        tx.execute(
            "INSERT OR IGNORE INTO unsubscribed_senders(account_id,sender) VALUES (?1,?2)",
            params![account, sender],
        )?;
        tx.commit()?;
        Ok((token, ids.len()))
    }

    fn expire_scheduled(&self) -> Result<()> {
        let expired: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM outbox WHERE handed_over=1 AND at<=?1)",
            [now_ms()],
            |r| r.get(0),
        )?;
        if !expired {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM drafts WHERE id IN (SELECT json_extract(draft,'$.id') FROM outbox WHERE handed_over=1 AND at<=?1)",[now_ms()])?;
        tx.execute(
            "DELETE FROM outbox WHERE handed_over=1 AND at<=?1",
            [now_ms()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn outbox_remote(&self, id: Id) -> Result<Option<(Id, String)>> {
        self.expire_scheduled()?;
        Ok(self
            .conn
            .query_row(
                "SELECT account_id,remote_id FROM outbox WHERE id=?1 AND remote_id IS NOT NULL",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    pub fn outbox_prepared(&self, id: Id, remote: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE outbox SET remote_id=?1 WHERE id=?2",
            params![remote, id],
        )?;
        Ok(())
    }

    pub fn outbox_handed_over(&self, id: Id) -> Result<()> {
        self.conn.execute(
            "UPDATE outbox SET handed_over=1, at=json_extract(draft, '$.send_at') WHERE id=?1",
            [id],
        )?;
        Ok(())
    }

    pub fn outbox_cancel(&self, id: Id) -> Result<Option<Draft>> {
        let tx = self.conn.unchecked_transaction()?;
        let raw: Option<String> = tx
            .query_row("SELECT draft FROM outbox WHERE id=?1", [id], |r| r.get(0))
            .optional()?;
        let Some(raw) = raw else { return Ok(None) };
        tx.execute("DELETE FROM outbox WHERE id=?1", [id])?;
        let draft: Draft = serde_json::from_str(&raw)?;
        if let Some(draft_id) = draft.id {
            restore_draft_row(&tx, draft_id)?;
        }
        tx.commit()?;
        Ok(Some(draft))
    }

    pub fn outbox_due(&self, now: Ms) -> Result<Vec<(Id, Draft)>> {
        self.expire_scheduled()?;
        let mut s = self
            .conn
            .prepare("SELECT o.id, o.draft FROM outbox o JOIN accounts a ON a.id=o.account_id
            WHERE handed_over=0 AND (at<=?1 OR (a.provider='microsoft' AND at>?1+120000)) ORDER BY at, o.id")?;
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

    /// Stop after eight attempts; return (stopped, first failure) for draft recovery and one retry toast.
    pub fn outbox_failed(
        &self,
        id: Id,
        now: Ms,
        permanent: bool,
        not_before: Option<Ms>,
    ) -> Result<(bool, bool)> {
        let attempts: i64 =
            self.conn
                .query_row("SELECT attempts FROM outbox WHERE id=?1", [id], |r| {
                    r.get(0)
                })?;
        if permanent || attempts >= 7 {
            return Ok((true, attempts == 0));
        }
        let backoff = (5_000i64 << attempts.min(8)).min(15 * 60_000);
        self.conn.execute(
            "UPDATE outbox SET attempts=attempts+1, at=?1 WHERE id=?2",
            params![(now + backoff).max(not_before.unwrap_or(0)), id],
        )?;
        Ok((false, attempts == 0))
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

    pub fn tags(&self) -> Result<Vec<Tag>> {
        let mut s = self
            .conn
            .prepare("SELECT id, name, color, position FROM tags ORDER BY position, id")?;
        let it = s.query_map([], |r| {
            Ok(Tag {
                id: r.get(0)?,
                name: r.get(1)?,
                color: r.get(2)?,
                position: r.get(3)?,
            })
        })?;
        Ok(it.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Local retrieval excludes Private before ranking or loading any source content.
    pub fn ai_search(&self, question: &str, account: Option<Id>) -> Result<Vec<Id>> {
        let stop = [
            "a", "an", "and", "are", "as", "at", "be", "by", "can", "did", "do", "does", "for",
            "from", "had", "has", "have", "how", "i", "in", "is", "it", "me", "my", "of", "on",
            "or", "please", "s", "tell", "that", "the", "their", "this", "to", "was", "were",
            "what", "when", "where", "which", "who", "will", "with", "you",
        ];
        let mut terms = Vec::new();
        for (index, part) in question.split('"').enumerate() {
            if index % 2 == 1 {
                let phrase = part
                    .split(|c: char| !c.is_alphanumeric())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                if !phrase.is_empty() {
                    terms.push(format!("\"{phrase}\""));
                }
            } else {
                for word in part
                    .split(|c: char| !c.is_alphanumeric())
                    .filter(|s| !s.is_empty())
                {
                    if !stop.contains(&word.to_lowercase().as_str()) {
                        terms.push(format!("\"{word}\""));
                    }
                }
            }
            if terms.len() >= 32 {
                break;
            }
        }
        terms.truncate(32);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let mut statement = self.conn.prepare(
            "SELECT DISTINCT m.thread_id FROM msgs_fts
            JOIN messages m ON m.id=msgs_fts.rowid JOIN threads t ON t.id=m.thread_id
            WHERE msgs_fts MATCH ?1 AND (?2 IS NULL OR t.account_id=?2)
            AND NOT EXISTS(SELECT 1 FROM thread_tags tt JOIN tags g ON g.id=tt.tag_id
                WHERE tt.thread_id=t.id AND g.name=?3 COLLATE NOCASE)
            AND NOT EXISTS(SELECT 1 FROM drafts d WHERE d.thread_id=t.id)
            ORDER BY bm25(msgs_fts), t.date DESC, t.id LIMIT 8",
        )?;
        let rows = statement.query_map(params![terms.join(" OR "), account, PRIVATE_TAG], |r| {
            r.get(0)
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn ai_arrival_watermark(&self) -> Result<Id> {
        Ok(self
            .conn
            .query_row("SELECT COALESCE(MAX(id),0) FROM messages", [], |r| r.get(0))?)
    }

    pub fn ai_triage_candidates(&self, account: Id, after: Id) -> Result<Vec<Id>> {
        let mut statement = self.conn.prepare("SELECT t.id FROM threads t
            WHERE t.account_id=?1 AND t.mailbox='inbox'
            AND EXISTS(SELECT 1 FROM messages m WHERE m.thread_id=t.id AND m.id>?2 AND m.mailbox='inbox' AND m.from_self=0)
            AND NOT EXISTS(SELECT 1 FROM ai_triaged a WHERE a.thread_id=t.id)
            AND NOT EXISTS(SELECT 1 FROM thread_tags tt JOIN tags g ON g.id=tt.tag_id
                WHERE tt.thread_id=t.id AND g.name=?3 COLLATE NOCASE)
            ORDER BY t.date DESC, t.id LIMIT 20")?;
        let rows = statement.query_map(params![account, after, PRIVATE_TAG], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Claim before network work: even failures cannot cause repeated automatic disclosure.
    pub fn claim_ai_triage(&self, thread: Id) -> Result<bool> {
        self.check_ai_thread(thread)?;
        Ok(self.conn.execute(
            "INSERT OR IGNORE INTO ai_triaged(thread_id) VALUES (?1)",
            [thread],
        )? != 0)
    }

    pub fn apply_ai_triage(&self, thread: Id, label: &str) -> Result<()> {
        if !AI_TRIAGE_LABELS.contains(&label) {
            bail!("Invalid AI triage label");
        }
        let tx = self.conn.unchecked_transaction()?;
        self.check_ai_thread(thread)?;
        tx.execute(
            "INSERT OR IGNORE INTO tags(name,color,position)
            VALUES (?1,'#64748b',(SELECT COALESCE(MAX(position),-1)+1 FROM tags))",
            [label],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO thread_tags(thread_id,tag_id)
            SELECT ?1,id FROM tags WHERE name=?2 COLLATE NOCASE",
            params![thread, label],
        )?;
        refresh_labels(&tx, thread)?;
        tx.commit()?;
        Ok(())
    }

    /// Missing sources fail closed, so stale UI state cannot authorize an AI request.
    pub fn check_ai_thread(&self, thread: Id) -> Result<()> {
        self.check_ai_thread_path(thread, &mut HashSet::new())
    }

    fn check_ai_thread_path(&self, thread: Id, path: &mut HashSet<Id>) -> Result<()> {
        if !path.insert(thread) || path.len() > 100 {
            bail!("The AI source has circular or excessive draft references");
        }
        let private: Option<bool> = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM thread_tags tt JOIN tags t ON t.id=tt.tag_id
             WHERE tt.thread_id=threads.id AND t.name=?2 COLLATE NOCASE)
             FROM threads WHERE id=?1",
                params![thread, PRIVATE_TAG],
                |r| r.get(0),
            )
            .optional()?;
        match private {
            Some(false) => {}
            Some(true) => bail!("This conversation is private, so AI can't read it"),
            None => bail!("The AI source conversation is no longer available"),
        }
        let mut statement = self
            .conn
            .prepare("SELECT draft FROM drafts WHERE thread_id=?1")?;
        let drafts = statement
            .query_map([thread], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for raw in drafts {
            let draft: Draft = serde_json::from_str(&raw).context("AI draft source")?;
            self.check_ai_reply(&draft, path)?;
        }
        path.remove(&thread);
        Ok(())
    }

    fn check_ai_reply(&self, draft: &Draft, path: &mut HashSet<Id>) -> Result<()> {
        if let Some(message) = draft.reply_to {
            let thread: Id = self
                .conn
                .query_row(
                    "SELECT thread_id FROM messages WHERE id=?1 AND account_id=?2",
                    params![message, draft.account_id],
                    |r| r.get(0),
                )
                .context("The AI reply source is no longer available")?;
            self.check_ai_thread_path(thread, path)?;
        }
        Ok(())
    }

    pub fn check_ai_draft(&self, draft: &Draft) -> Result<()> {
        self.check_ai_reply(draft, &mut HashSet::new())?;
        if let Some(id) = draft.id {
            let private: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM draft_tags dt
                JOIN tags t ON t.id=dt.tag_id WHERE dt.draft_id=?1 AND t.name=?2 COLLATE NOCASE)",
                params![id, PRIVATE_TAG],
                |r| r.get(0),
            )?;
            if private {
                bail!("This draft is private, so AI can't read it");
            }
            // A draft not yet linked to a conversation has no thread to protect.
            let thread: Option<Id> = self
                .conn
                .query_row(
                    "SELECT thread_id FROM drafts WHERE id=?1 AND account_id=?2",
                    params![id, draft.account_id],
                    |r| r.get(0),
                )
                .context("The AI draft is no longer available")?;
            if let Some(thread) = thread {
                self.check_ai_thread(thread)?;
            }
            let raw: String = self.conn.query_row(
                "SELECT draft FROM drafts WHERE id=?1 AND account_id=?2",
                params![id, draft.account_id],
                |r| r.get(0),
            )?;
            let saved: Draft = serde_json::from_str(&raw).context("AI saved draft")?;
            self.check_ai_reply(&saved, &mut HashSet::new())?;
        }
        Ok(())
    }

    /// Replaces the whole list. List order is the display order; ids <= 0 create, omitted tags are
    /// deleted (cascading out of every thread). Validation happens before any write.
    pub fn set_tags(&self, mut tags: Vec<Tag>) -> Result<()> {
        // A tag-list edit must never silently remove protection from existing conversations.
        if let Some(private) = self
            .tags()?
            .into_iter()
            .find(|t| t.name.eq_ignore_ascii_case(PRIVATE_TAG))
        {
            if let Some(tag) = tags.iter().find(|t| t.id == private.id) {
                if !tag.name.eq_ignore_ascii_case(PRIVATE_TAG) {
                    bail!("The Private tag cannot be renamed");
                }
            } else {
                tags.push(private);
            }
        }
        let mut desired: Vec<(Option<Id>, String, String)> = Vec::with_capacity(tags.len());
        let mut seen: HashSet<String> = HashSet::new();
        for t in tags {
            let name = t.name.trim().to_string();
            if name.is_empty() {
                bail!("tag name is empty");
            }
            if name.contains(',') {
                bail!("tag name {name:?} must not contain a comma");
            }
            if name.contains('"') || name.chars().any(char::is_control) {
                bail!("tag name {name:?} must not contain quotes or control characters");
            }
            if !seen.insert(name.to_ascii_lowercase()) {
                bail!("duplicate tag name {name:?}");
            }
            if !valid_tag_color(&t.color) {
                bail!("tag colour {:?} is not #rrggbb", t.color);
            }
            desired.push(((t.id > 0).then_some(t.id), name, t.color));
        }

        let tx = self.conn.unchecked_transaction()?;
        let existing: HashMap<Id, (String, String, i64)> = {
            let mut s = tx.prepare("SELECT id, name, color, position FROM tags")?;
            let it = s.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))?;
            it.collect::<rusqlite::Result<HashMap<_, _>>>()?
        };
        let mut seen_ids: HashSet<Id> = HashSet::new();
        for (id, _, _) in &desired {
            if let Some(id) = id {
                if !seen_ids.insert(*id) {
                    bail!("duplicate tag id {id}");
                }
                if !existing.contains_key(id) {
                    bail!("unknown tag id {id}");
                }
            }
        }
        let kept: HashSet<Id> = desired.iter().filter_map(|(id, _, _)| *id).collect();
        let mut changed_tags: HashSet<Id> = existing
            .keys()
            .filter(|id| !kept.contains(*id))
            .copied()
            .collect();
        let mut renames: HashMap<String, String> = HashMap::new();
        for (i, (id, name, color)) in desired.iter().enumerate() {
            let Some(id) = *id else { continue };
            let (old_name, old_color, old_pos) = &existing[&id];
            if old_name != name || old_color != color || *old_pos != i as i64 {
                changed_tags.insert(id);
            }
            if old_name != name {
                renames.insert(old_name.to_ascii_lowercase(), name.clone());
            }
        }
        // Threads that could see a label change: members of a deleted, renamed or reordered tag.
        let affected: Vec<Id> = if changed_tags.is_empty() {
            Vec::new()
        } else {
            let holes = vec!["?"; changed_tags.len()].join(",");
            let mut s = tx.prepare(&format!(
                "SELECT DISTINCT thread_id FROM thread_tags WHERE tag_id IN ({holes})"
            ))?;
            let it = s.query_map(params_from_iter(changed_tags.iter()), |r| r.get(0))?;
            it.collect::<rusqlite::Result<Vec<_>>>()?
        };

        if kept.is_empty() {
            tx.execute("DELETE FROM tags", [])?;
        } else {
            let holes = vec!["?"; kept.len()].join(",");
            tx.execute(
                &format!("DELETE FROM tags WHERE id NOT IN ({holes})"),
                params_from_iter(kept.iter()),
            )?;
            // Renaming straight from one live name to another can collide (e.g. a swap), so move
            // the retained rows aside first; a control suffix cannot collide with a validated name.
            tx.execute(
                &format!("UPDATE tags SET name = name || char(1) WHERE id IN ({holes})"),
                params_from_iter(kept.iter()),
            )?;
        }
        for (i, (id, name, color)) in desired.iter().enumerate() {
            match id {
                Some(id) => {
                    tx.execute(
                        "UPDATE tags SET name=?1, color=?2, position=?3 WHERE id=?4",
                        params![name, color, i as i64, id],
                    )?;
                }
                None => {
                    tx.execute(
                        "INSERT INTO tags(name, color, position) VALUES (?1,?2,?3)",
                        params![name, color, i as i64],
                    )?;
                }
            }
        }
        rewrite_split_labels(&tx, &renames)?;
        for thread_id in affected {
            refresh_labels(&tx, thread_id)?;
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
        Some("label" | "tag") => p.label(value, neg),
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
        // A comma-separated value is a set: OR within this one term, while separate tokens still
        // AND. Splitting before `glob_pat` keeps the comma out of the pattern.
        let mut groups: Vec<&str> = Vec::new();
        let mut params: Vec<Value> = Vec::new();
        for part in value.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            groups.push("(p.email GLOB ? OR p.domain GLOB ? OR lower(p.name) GLOB ?)");
            let pat = glob_pat(part);
            for _ in 0..3 {
                params.push(Value::Text(pat.clone()));
            }
        }
        if groups.is_empty() {
            // Nothing but separators: this term can never match, and must not widen the search.
            self.conds.push("1=0".into());
            return;
        }
        let ex = format!(
            "EXISTS (SELECT 1 FROM messages m JOIN message_people p ON p.message_id=m.id
                     WHERE m.thread_id=t.id AND {kind} AND ({}))",
            groups.join(" OR ")
        );
        self.conds.push(if neg { format!("NOT {ex}") } else { ex });
        self.params.extend(params);
    }

    fn label(&mut self, value: &str, neg: bool) {
        let names: Vec<&str> = value
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        if names.is_empty() {
            // Nothing but separators can never match, and must not widen the search.
            self.conds.push("1=0".into());
            return;
        }
        // Comma is OR inside one operator, matching the people fields. The name index is used
        // through thread_tags' primary key from the thread side, so this stays a correlated lookup.
        let groups = vec!["g.name = ? COLLATE NOCASE"; names.len()].join(" OR ");
        let ex = format!(
            "EXISTS (SELECT 1 FROM thread_tags tt JOIN tags g ON g.id=tt.tag_id
                     WHERE tt.thread_id=t.id AND ({groups}))"
        );
        self.conds.push(if neg { format!("NOT {ex}") } else { ex });
        self.params
            .extend(names.into_iter().map(|n| Value::Text(n.to_string())));
    }

    fn flag(&mut self, value: &str, neg: bool) {
        let on = !neg;
        let cond = match value {
            "unread" => format!("t.unread={}", usize::from(on)),
            "read" => format!("t.unread={}", usize::from(!on)),
            "starred" => format!("t.starred={}", usize::from(on)),
            "important" => format!("t.important={}", usize::from(on)),
            "snoozed" => format!("t.snoozed_until IS {}NULL", if on { "NOT " } else { "" }),
            // One correlated, partial-indexed lookup over the thread's messages; no N+1.
            "newsletter" => format!(
                "{}EXISTS (SELECT 1 FROM messages m WHERE m.thread_id=t.id AND m.list_unsubscribe=1)",
                if on { "" } else { "NOT " }
            ),
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

/// Rewrites `label:` tokens whose value names a renamed tag, leaving every other token alone.
/// `renames` is keyed by the lowercased old name so case-insensitive matches become the new name.
fn rewrite_split_labels(tx: &Transaction, renames: &HashMap<String, String>) -> Result<()> {
    if renames.is_empty() {
        return Ok(());
    }
    let splits: Vec<(Id, String)> = {
        let mut s = tx.prepare("SELECT id, query FROM splits")?;
        let it = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        it.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, query) in splits {
        let rewritten = rewrite_label_tokens(&query, renames);
        if rewritten != query {
            tx.execute(
                "UPDATE splits SET query=?1 WHERE id=?2",
                params![rewritten, id],
            )?;
        }
    }
    Ok(())
}

fn rewrite_label_tokens(query: &str, renames: &HashMap<String, String>) -> String {
    let mut out: Vec<String> = Vec::new();
    for token in query_tokens(query) {
        let (neg, rest) = match token.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, token),
        };
        let prefix = if rest
            .get(..4)
            .is_some_and(|p| p.eq_ignore_ascii_case("tag:"))
        {
            "tag:"
        } else {
            "label:"
        };
        let Some(value) = rest
            .get(..prefix.len())
            .filter(|p| p.eq_ignore_ascii_case(prefix))
            .map(|_| &rest[prefix.len()..])
        else {
            out.push(token.to_string());
            continue;
        };
        // parse_query drops every quote, so `"A","B"` and `"A,B"` mean the same; mirror that.
        let inner = value.replace('"', "");
        let mut changed = false;
        let parts: Vec<&str> = inner
            .split(',')
            .map(
                |part| match renames.get(&part.trim().to_ascii_lowercase()) {
                    Some(new) => {
                        changed = true;
                        new.as_str()
                    }
                    None => part,
                },
            )
            .collect();
        if !changed {
            out.push(token.to_string());
            continue;
        }
        let joined = parts.join(",");
        let value = if joined.chars().any(char::is_whitespace) {
            format!("\"{joined}\"")
        } else {
            joined
        };
        out.push(format!("{}{prefix}{value}", if neg { "-" } else { "" }));
    }
    out.join(" ")
}

/// Whitespace-separated tokens, keeping quoted sections (and their quotes) intact.
fn query_tokens(query: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut in_quote = false;
    for (i, c) in query.char_indices() {
        match c {
            '"' => {
                in_quote = !in_quote;
                start.get_or_insert(i);
            }
            c if c.is_whitespace() && !in_quote => {
                if let Some(st) = start.take() {
                    out.push(&query[st..i]);
                }
            }
            _ => {
                start.get_or_insert(i);
            }
        }
    }
    if let Some(st) = start {
        out.push(&query[st..]);
    }
    out
}

fn valid_tag_color(c: &str) -> bool {
    let b = c.as_bytes();
    b.len() == 7 && b[0] == b'#' && b[1..].iter().all(|c| c.is_ascii_hexdigit())
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
        Mailbox::Drafts => {
            "EXISTS (SELECT 1 FROM messages m WHERE m.thread_id=t.id AND m.mailbox='drafts')"
        }
        Mailbox::Sent => {
            "EXISTS (SELECT 1 FROM messages m WHERE m.thread_id=t.id AND m.mailbox='sent')"
        }
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
