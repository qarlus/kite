//! Boundary between remote backends and the local store.
//! Backends never touch SQLite. They turn remote state into `Delta`s and execute `Op`s.
//! The engine (wave 2) owns the loop: `sync` -> `Store::apply_deltas`, and `Store::pending_ops` -> `apply`.
//!
//! Every backend exposes these inherent methods with these exact signatures (enum dispatch, no trait objects):
//! ```ignore
//! async fn sync(&mut self, cursor: Option<String>) -> anyhow::Result<(Vec<Delta>, String)>;
//! async fn apply(&mut self, op: &Op) -> anyhow::Result<Option<String>>; // Some(new remote_id) when a move changes it
//! async fn attachment(&mut self, remote_msg_id: &str, remote_att_id: &str) -> anyhow::Result<Vec<u8>>;
//! ```
//! `cursor` is opaque backend state (JSON string) persisted by the store between runs.
use crate::model::{Addr, Mailbox, Ms};
use serde::{Deserialize, Serialize};

pub mod graph;
pub mod imap;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RemoteAttachment {
    pub remote_id: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RemoteMsg {
    /// Stable per account. IMAP: "{folder}/{uidvalidity}/{uid}". Graph: message id.
    pub remote_id: String,
    /// Remote folder id/path the message lives in.
    pub folder: String,
    pub mailbox: Mailbox,
    /// Provider thread id if any (Graph conversationId). The store threads by headers otherwise.
    pub thread_hint: Option<String>,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub from: Addr,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub date: Ms,
    pub subject: String,
    pub text: String,
    pub html: Option<String>,
    pub unread: bool,
    pub starred: bool,
    /// Provider importance signal (Graph inferenceClassification). None = let the store decide.
    pub important: Option<bool>,
    pub attachments: Vec<RemoteAttachment>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Delta {
    Upsert(Box<RemoteMsg>),
    Flags {
        remote_id: String,
        unread: bool,
        starred: bool,
    },
    Delete {
        remote_id: String,
    },
}

/// Remote mutation queued by the store and executed by a backend.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Op {
    Move {
        remote_id: String,
        to: Mailbox,
    },
    MoveToFolder {
        remote_id: String,
        folder: String,
    },
    Flags {
        remote_id: String,
        unread: Option<bool>,
        starred: Option<bool>,
    },
    /// Raw RFC 5322 bytes (built with mail-builder). Both backends send MIME as-is.
    Send {
        mime: Vec<u8>,
    },
}
