//! Shared domain types used by the store, the engine and the UI. Owned by the orchestrator.
use serde::{Deserialize, Serialize};

pub type Id = i64;
/// Reserved local tag: excludes a conversation from Kite's AI helpers.
pub const PRIVATE_TAG: &str = "Private";

/// The engine builds AI prompts from identified sources, enforcing privacy before reading them.
#[derive(Clone, Debug)]
pub enum AiRequest {
    Summary {
        thread: Id,
    },
    Compose {
        draft: Draft,
    },
    Ask {
        question: String,
        account: Option<Id>,
    },
    Replies {
        thread: Id,
    },
}
pub const AI_TRIAGE_LABELS: [&str; 5] = [
    "Needs reply",
    "FYI",
    "Receipts",
    "Newsletters",
    "Notifications",
];
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
    Waiting,
    Scheduled,
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
    /// Provider folder identity. Always paired with a specific account.
    #[serde(default)]
    pub folder: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Folder {
    pub id: String,
    pub account_id: Id,
    pub name: String,
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
    DoneWaiting,
    Archive,
    Inbox,
    Trash,
    Star,
    Unstar,
    Read,
    Unread,
    Snooze {
        until: Ms,
    },
    Move {
        folder: String,
    },
    /// Local-only for now: no remote op is queued. Unknown tag names are an error.
    Label {
        tag: String,
    },
    Unlabel {
        tag: String,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Draft {
    /// The saved draft this edits (from [`crate::api::Mail::save_draft`]). Sending or discarding it
    /// removes the saved copy locally and on the provider.
    #[serde(default)]
    pub id: Option<Id>,
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

/// A user tag. `ThreadSummary.labels` carries tag names, ordered by `position`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Tag {
    pub id: Id,
    pub name: String,
    /// `#rrggbb`.
    pub color: String,
    pub position: i64,
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

/// Per-account extras that are not already on `Account`. Avatar image bytes never
/// live in settings or SQLite: they are files under `data_dir()/avatars/`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct AccountProfile {
    /// Plain-text signature for new drafts; empty adds nothing.
    pub signature: String,
}

/// A reusable text block inserted in compose (`;shortcut` or the palette).
/// `{first_name}`-style variables expand on insert; a first line `Subject: …` sets
/// an empty subject.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Snippet {
    pub name: String,
    pub body: String,
    pub shortcut: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CustomTheme {
    pub name: String,
    pub base: String,
    pub colors: std::collections::BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    pub theme: String,
    pub custom_theme: Option<CustomTheme>,
    pub font_family: String,
    pub font_size: u8,
    pub font_color: Option<String>,
    pub undo_send_secs: u32,
    pub waiting_after_days: u32,
    pub auto_update_check: bool,
    pub load_remote_images: bool,
    pub ai_enabled: bool,
    /// Optional background classification of new inbox conversations.
    pub ai_triage: bool,
    pub ai_base_url: String,
    pub ai_model: String,
    /// `"keyboard"`: single-key shortcuts (Superhuman style). `"classic"`: click-first, where only
    /// modifier shortcuts (Ctrl/Cmd+…) stay bound and actions are shown as buttons.
    pub input_mode: String,
    /// User shortcut overrides: command slug -> keystrokes (GPUI syntax). Absent = default keys;
    /// an empty list unbinds the command.
    pub shortcuts: std::collections::BTreeMap<String, Vec<String>>,
    /// `true`: list and reading pane side by side. `false`: an opened conversation takes the
    /// full width (Superhuman style).
    pub reading_pane: bool,
    /// Per-account profile (signature), keyed by account id. Display name is `Account.name`.
    pub profiles: std::collections::BTreeMap<Id, AccountProfile>,
    /// Text snippets inserted in compose. Absent in settings written before they existed.
    #[serde(default)]
    pub snippets: Vec<Snippet>,
    /// "Show shortcut tips": clicking an action in keyboard mode names its key in a toast.
    pub shortcut_tips: bool,
    /// Per-command shortcut-tip memory, keyed by command slug: the day the tip last showed and
    /// how often the key ran the command. Absent in settings written before it existed.
    #[serde(default)]
    pub shortcut_hints: std::collections::BTreeMap<String, HintStat>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            custom_theme: None,
            font_family: String::new(),
            font_size: 14,
            font_color: None,
            undo_send_secs: 5,
            waiting_after_days: 2,
            auto_update_check: true,
            load_remote_images: false,
            ai_enabled: false,
            ai_triage: false,
            ai_base_url: "https://opencode.ai/zen/go/v1".into(),
            ai_model: "minimax-m3.1-flash-free".into(),
            input_mode: "keyboard".into(),
            shortcuts: Default::default(),
            reading_pane: true,
            profiles: Default::default(),
            snippets: Default::default(),
            shortcut_tips: true,
            shortcut_hints: Default::default(),
        }
    }
}

/// What the shortcut tips remember about one command.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct HintStat {
    /// `NaiveDate::num_days_from_ce` for the day the tip last showed; one tip per day.
    #[serde(default)]
    pub shown_day: i64,
    /// How often the command's key ran it; the tip retires once the key is learned.
    #[serde(default)]
    pub uses: u32,
}

/// The next step of a native Proton sign-in (Proton's API directly, no Bridge; works on free plans).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ProtonAuth {
    Done(Account),
    /// A six-digit code from the user's authenticator app.
    TwoFactor,
    /// Two-password mode: the separate mailbox password unlocks the keys.
    MailboxPassword,
    /// Proton wants a human-verification challenge solved at `url` (opened in the browser);
    /// the answer is the verification token.
    HumanVerification {
        url: String,
    },
}

/// A model the configured AI endpoint offers. Kite only offers free models whose provider keeps
/// no data (zero data retention), so mail never trains or lingers on a third-party server.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AiModel {
    /// Sent as the request's `model`.
    pub id: String,
    pub name: String,
    pub free: bool,
    pub zdr: bool,
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
    /// Ask citation order, emitted before the first answer chunk.
    AiSources { req: u32, threads: Vec<Id> },
    /// A streamed AI completion chunk for request `req`.
    Ai { req: u32, delta: String, done: bool },
    /// A user-visible background failure (auth expired, send failed, ...).
    Error(String),
}
