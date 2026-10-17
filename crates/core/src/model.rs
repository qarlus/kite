//! Shared domain types used by the store, the engine and the UI. Owned by the orchestrator.
use serde::{Deserialize, Serialize};

pub type Id = i64;
/// Unix epoch milliseconds.
pub type Ms = i64;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Proton,
    Microsoft,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Account {
    pub id: Id,
    pub provider: Provider,
    pub email: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mailbox {
    Inbox,
    Starred,
    Snoozed,
    Drafts,
    Sent,
    Archive,
    Trash,
    Spam,
    All,
}

/// What the list shows. `account_id: None` = unified across accounts.
/// `query` uses the search grammar from docs/SPEC.md and is ANDed with `mailbox`/`split`.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct View {
    pub account_id: Option<Id>,
    pub mailbox: Mailbox,
    pub split: Option<Id>,
    pub query: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Addr {
    pub name: String,
    pub email: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ThreadSummary {
    pub id: Id,
    pub account_id: Id,
    pub subject: String,
    pub snippet: String,
    /// Display string, e.g. "Alice, Bob (3)".
    pub participants: String,
    pub date: Ms,
    pub unread: bool,
    pub starred: bool,
    pub important: bool,
    pub attachments: bool,
    pub labels: Vec<String>,
    pub count: u32,
    pub snoozed_until: Option<Ms>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Attachment {
    pub id: Id,
    pub name: String,
    pub mime: String,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Message {
    pub id: Id,
    pub thread_id: Id,
    pub from: Addr,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub date: Ms,
    pub subject: String,
    /// Raw HTML. Rendered by Blitz with no script engine and no network unless remote images are allowed.
    pub html: Option<String>,
    pub text: String,
    pub attachments: Vec<Attachment>,
    pub unread: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Action {
    Archive,
    Inbox,
    Trash,
    Star,
    Unstar,
    Read,
    Unread,
    Snooze { until: Ms },
    Move { folder: String },
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Draft {
    pub account_id: Id,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub subject: String,
    pub body: String,
    /// Message being replied to / forwarded (sets In-Reply-To / References / threading).
    pub reply_to: Option<Id>,
    /// None = send after the undo window. Some = scheduled send.
    pub send_at: Option<Ms>,
    /// Snooze-style reminder: bring the thread back to the inbox at this time if nobody replied.
    pub remind_at: Option<Ms>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Split {
    pub id: Id,
    pub name: String,
    pub query: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CalEvent {
    pub id: String,
    pub title: String,
    pub start: Ms,
    pub end: Ms,
    pub location: Option<String>,
    pub all_day: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Settings {
    pub theme: String,
    pub undo_send_secs: u32,
    pub load_remote_images: bool,
    pub ai_enabled: bool,
    pub ai_base_url: String,
    pub ai_model: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            undo_send_secs: 5,
            load_remote_images: false,
            ai_enabled: false,
            ai_base_url: "https://opencode.ai/zen/go/v1".into(),
            ai_model: "deepseek-v4.1-flash".into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ProtonLogin {
    pub email: String,
    pub password: String,
    pub host: Option<String>,
    pub imap_port: Option<u16>,
    pub smtp_port: Option<u16>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChatMsg {
    pub role: String,
    pub content: String,
}

/// Engine -> UI notifications, delivered over a channel and handled on the UI thread.
#[derive(Clone, Debug)]
pub enum Event {
    /// The store changed for these accounts, so visible views should re-query.
    Changed(Vec<Id>),
    /// A streamed AI completion chunk for request `req`.
    Ai { req: u32, delta: String, done: bool },
    /// A user-visible background failure (auth expired, send failed, ...).
    Error(String),
}
