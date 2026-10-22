//! Test fixture: an in-memory `Mail` that serves synthetic threads, applies actions
//! to its own data and records every call the UI makes.
#![allow(dead_code)]
// engine_boot.rs declares its own copy at its crate root; each test binary compiles
// `common` as a nested module, so the fixture needs the path declared here too.
#[allow(clippy::duplicate_mod)]
#[path = "temp_dir.rs"]
pub mod temp_dir;
use gpui_kit::{
    App, AppContext, Bounds, Entity, Point, Render, Size, TestAppContext, Window, WindowBounds,
    WindowHandle, WindowOptions, base::Root, px,
};
use kite_core::api::Mail;
use kite_core::model::{
    Account, Action, Addr, AiModel, AiRequest, Attachment, CalEvent, ContactCard, ContactFile,
    ContactThread, Draft, Event, Folder, Id, Mailbox, Message, Ms, ProtonAuth, Settings, Split,
    Tag, ThreadSummary, View,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use temp_dir::TempDir;

pub const SPLITS: [Id; 4] = [1, 2, 3, 4];
/// The seeded split set, in tab order: the plain inbox lens, then the categories.
pub const SPLIT_NAMES: [&str; 4] = ["Inbox", "Social", "Newsletters", "GitHub"];
/// Comma inside `from:` means OR, matching the engine's term syntax. Mirrors the
/// shipped `DEFAULT_SPLITS` seed so the fixture cannot drift from the product.
pub const SOCIAL_QUERY: &str = "from:linkedin.com,x.com,twitter.com,facebook.com,instagram.com,threads.net,bsky.app,mastodon.social";
pub const NEWSLETTER_QUERY: &str = "is:newsletter";
pub const GITHUB_QUERY: &str = "from:github.com";
/// The HTML body served for even-numbered threads.
pub const HTML: &str = "<p>Hello there.</p><p>Second paragraph with enough text to lay out.</p>";

type Locations = HashMap<Id, (Mailbox, Option<String>)>;
type Snapshot = (Vec<ThreadSummary>, Locations);

/// A 1x1 PNG, so the fixture answers a real image without any network.
pub const PNG: &[u8] = &[
    0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, b'I', b'H', b'D', b'R',
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0a, b'I', b'D', b'A', b'T', 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, b'I', b'E', b'N', b'D', 0xae,
    0x42, 0x60, 0x82,
];

pub struct Fixture {
    pub folders: Mutex<Vec<Folder>>,
    pub locations: Mutex<Locations>,
    pub folder_error: Mutex<Option<String>>,
    pub account_error: Mutex<Option<String>>,
    /// What `proton_sign_in` answers instead of an account (2FA, mailbox password, HV).
    pub proton_step: Mutex<Option<ProtonAuth>>,
    /// Address and password the native Proton form submitted.
    pub proton_attempts: Mutex<Vec<(String, String)>>,
    /// Answers passed to `proton_sign_in_step`.
    pub proton_answers: Mutex<Vec<String>>,
    pub settings_path: Mutex<Option<std::path::PathBuf>>,
    pub attachments_opened: Mutex<Vec<Id>>,
    pub attachments_revealed: Mutex<Vec<Id>>,
    /// URLs the UI asked the engine to fetch as remote images, in order.
    pub images_fetched: Mutex<Vec<String>>,
    pub refreshes: AtomicI64,
    /// The bytes `fetch_image` answers with; a 1x1 PNG unless a test replaces it.
    pub image_bytes: Mutex<Vec<u8>>,
    /// Every `ai` request the UI made, with the prompt it sent.
    pub ai_requests: Mutex<Vec<(u32, AiRequest)>>,
    /// The text the next `ai` call streams back as one chunk; empty stays silent.
    pub ai_reply: Mutex<String>,
    /// The `AiSources` an `Ask` request emits before its chunks.
    pub ai_sources: Mutex<Vec<Id>>,
    /// When set, `ai` ends the stream and emits `Event::Error`, like a failed stream.
    pub ai_error: Mutex<Option<String>>,
    /// The model list `ai_models` answers with.
    pub ai_models: Mutex<Vec<AiModel>>,
    /// What `ai_models` answers instead of a list.
    pub ai_models_error: Mutex<Option<String>>,
    /// Every `ai_models` call, so refetches are visible.
    pub ai_model_calls: Mutex<u32>,
    /// The tag list `tags`/`set_tags` keep, mirroring the store's seeded set.
    pub tags: Mutex<Vec<Tag>>,
    pub settings: Mutex<Settings>,
    /// The AI key the UI handed to `set_ai_key`; never part of `Settings`.
    pub ai_key: Mutex<Option<String>>,
    pub accounts: Mutex<Vec<Account>>,
    threads: Mutex<Vec<ThreadSummary>>,
    messages: Mutex<HashMap<Id, Vec<Message>>>,
    /// Every `act` call, in order.
    pub actions: Mutex<Vec<(Vec<Id>, Action)>>,
    pub undos: Mutex<Vec<i64>>,
    /// Every `threads` query, in order.
    pub queries: Mutex<Vec<(View, u32)>>,
    pub opened: Mutex<Vec<Id>>,
    /// Threads the seeded `is:newsletter` split matches.
    pub newsletters: Mutex<Vec<Id>>,
    snapshots: Mutex<HashMap<i64, Snapshot>>,
    /// Every `send` draft, in order, and the outbox it was queued into.
    sent: Mutex<Vec<Draft>>,
    outbox: Mutex<HashMap<Id, Draft>>,
    cancelled: Mutex<Vec<Id>>,
    send_seq: AtomicI64,
    /// Drafts written by `save_draft`, each with its returned id.
    saved_drafts: Mutex<Vec<Draft>>,
    /// draft id -> the Drafts-mailbox thread that shows it.
    draft_threads: Mutex<HashMap<Id, Id>>,
    /// Draft ids passed to `discard_draft`.
    discarded: Mutex<Vec<Id>>,
    draft_seq: AtomicI64,
    senders: Mutex<Vec<async_channel::Sender<Event>>>,
    next_token: AtomicI64,
    /// The fixture's own data dir; avatars are files under `<dir>/avatars/`.
    pub dir: TempDir,
}

impl Fixture {
    pub fn new(count: usize) -> Arc<Self> {
        let now = chrono::Local::now().timestamp_millis();
        let mut threads = Vec::new();
        let mut messages = HashMap::new();
        for i in 0..count {
            let id = i as Id + 1;
            threads.push(ThreadSummary {
                id,
                account_id: 1,
                subject: format!("Thread {id}"),
                snippet: format!("Snippet {id}"),
                participants: format!("Sender {id}"),
                date: now - (id * 3_600_000),
                unread: i % 2 == 0,
                starred: false,
                important: true,
                attachments: false,
                labels: Vec::new(),
                count: 2,
                snoozed_until: None,
            });
            let html = (i % 2 == 1).then(|| {
                "<p>Hello there.</p><p>Second paragraph with enough text to lay out.</p>"
                    .to_string()
            });
            messages.insert(
                id,
                vec![
                    message(
                        id * 10 + 1,
                        id,
                        "Older note",
                        "Earlier plain text",
                        None,
                        now,
                    ),
                    message(
                        id * 10 + 2,
                        id,
                        &format!("Subject {id}"),
                        &format!("Plain body for thread {id}"),
                        html,
                        now,
                    ),
                ],
            );
        }
        Arc::new(Self {
            folders: Mutex::new(Vec::new()),
            locations: Mutex::new(HashMap::new()),
            folder_error: Mutex::new(None),
            account_error: Mutex::new(None),
            proton_step: Mutex::new(None),
            proton_attempts: Mutex::new(Vec::new()),
            proton_answers: Mutex::new(Vec::new()),
            settings_path: Mutex::new(None),
            attachments_opened: Mutex::new(Vec::new()),
            attachments_revealed: Mutex::new(Vec::new()),
            images_fetched: Mutex::new(Vec::new()),
            image_bytes: Mutex::new(PNG.to_vec()),
            refreshes: AtomicI64::new(0),
            ai_requests: Mutex::new(Vec::new()),
            ai_reply: Mutex::new(String::new()),
            ai_sources: Mutex::new(Vec::new()),
            ai_error: Mutex::new(None),
            ai_models: Mutex::new(Vec::new()),
            ai_models_error: Mutex::new(None),
            ai_model_calls: Mutex::new(0),
            tags: Mutex::new(
                ["Newsletter", "To Reply", "Private"]
                    .into_iter()
                    .enumerate()
                    .map(|(i, name)| Tag {
                        id: i as Id + 1,
                        name: name.into(),
                        color: "#8b7cff".into(),
                        position: i as i64,
                    })
                    .collect(),
            ),
            settings: Mutex::new(Settings {
                theme: "dark".into(),
                ..Settings::default()
            }),
            ai_key: Mutex::new(None),
            accounts: Mutex::new(vec![Account {
                id: 1,
                provider: kite_core::model::Provider::Proton,
                email: "me@example.com".into(),
                name: "Me".into(),
            }]),
            threads: Mutex::new(threads),
            messages: Mutex::new(messages),
            actions: Mutex::new(Vec::new()),
            undos: Mutex::new(Vec::new()),
            queries: Mutex::new(Vec::new()),
            opened: Mutex::new(Vec::new()),
            newsletters: Mutex::new(Vec::new()),
            snapshots: Mutex::new(HashMap::new()),
            sent: Mutex::new(Vec::new()),
            outbox: Mutex::new(HashMap::new()),
            cancelled: Mutex::new(Vec::new()),
            send_seq: AtomicI64::new(9001),
            saved_drafts: Mutex::new(Vec::new()),
            draft_threads: Mutex::new(HashMap::new()),
            discarded: Mutex::new(Vec::new()),
            draft_seq: AtomicI64::new(1),
            senders: Mutex::new(Vec::new()),
            next_token: AtomicI64::new(1),
            dir: TempDir::new("fixture"),
        })
    }

    pub fn thread_count(&self) -> usize {
        self.threads
            .lock()
            .unwrap()
            .iter()
            .filter(|t| {
                self.locations
                    .lock()
                    .unwrap()
                    .get(&t.id)
                    .is_none_or(|(mailbox, folder)| *mailbox == Mailbox::Inbox && folder.is_none())
            })
            .count()
    }

    pub fn actions(&self) -> Vec<(Vec<Id>, Action)> {
        self.actions.lock().unwrap().clone()
    }

    pub fn last_action(&self) -> Option<(Vec<Id>, Action)> {
        self.actions.lock().unwrap().last().cloned()
    }

    pub fn drafts(&self) -> Vec<Draft> {
        self.sent.lock().unwrap().clone()
    }

    pub fn cancelled(&self) -> Vec<Id> {
        self.cancelled.lock().unwrap().clone()
    }

    /// Drafts written through `save_draft`, newest last, each carrying its id.
    pub fn saved_drafts(&self) -> Vec<Draft> {
        self.saved_drafts.lock().unwrap().clone()
    }

    /// Draft ids the UI threw away through `discard_draft`.
    pub fn discarded(&self) -> Vec<Id> {
        self.discarded.lock().unwrap().clone()
    }

    /// Replaces the latest message's To and Cc lists, so reply-all has addresses to filter.
    pub fn set_message_recipients(&self, thread: Id, to: Vec<Addr>, cc: Vec<Addr>) {
        if let Some(message) = self
            .messages
            .lock()
            .unwrap()
            .get_mut(&thread)
            .and_then(|messages| messages.last_mut())
        {
            message.to = to;
            message.cc = cc;
        }
    }

    /// Replaces the latest message's sender, so reply has a display name to quote.
    pub fn set_message_from(&self, thread: Id, from: Addr) {
        if let Some(message) = self
            .messages
            .lock()
            .unwrap()
            .get_mut(&thread)
            .and_then(|messages| messages.last_mut())
        {
            message.from = from;
        }
    }

    /// Replaces the latest message's subject, so subject prefixes can be exercised.
    pub fn set_message_subject(&self, thread: Id, subject: &str) {
        if let Some(message) = self
            .messages
            .lock()
            .unwrap()
            .get_mut(&thread)
            .and_then(|messages| messages.last_mut())
        {
            message.subject = subject.into();
        }
    }

    /// Replaces the latest message's HTML body, so image handling can be exercised.
    pub fn set_attachments(&self, thread: Id, attachments: Vec<Attachment>) {
        self.messages
            .lock()
            .unwrap()
            .get_mut(&thread)
            .unwrap()
            .last_mut()
            .unwrap()
            .attachments = attachments;
    }

    pub fn set_message_html(&self, thread: Id, html: &str) {
        if let Some(message) = self
            .messages
            .lock()
            .unwrap()
            .get_mut(&thread)
            .and_then(|messages| messages.last_mut())
        {
            message.html = Some(html.into());
        }
    }

    /// Replaces the latest message's text body (and drops its HTML), so text-body links
    /// can be exercised.
    pub fn set_message_text(&self, thread: Id, text: &str) {
        if let Some(message) = self
            .messages
            .lock()
            .unwrap()
            .get_mut(&thread)
            .and_then(|messages| messages.last_mut())
        {
            message.html = None;
            message.text = text.into();
        }
    }

    pub fn next_id(&self) -> Id {
        self.threads
            .lock()
            .unwrap()
            .iter()
            .map(|t| t.id)
            .max()
            .unwrap_or(0)
            + 1
    }

    /// Realistic local content for the native preview; never included in the app binary.
    pub fn showcase() -> Arc<Self> {
        let samples = [
            (
                "Alice Chen",
                "A few thoughts on the next chapter",
                "Hey, I've been thinking about what we talked about yesterday.",
            ),
            (
                "Daniel, Sofia",
                "Re: A place to get away",
                "That little house by the coast looks perfect. Let's book it.",
            ),
            (
                "Linear",
                "Your team's week in review",
                "A quieter inbox, a faster workflow, and a few things shipped.",
            ),
            (
                "Oliver Grant",
                "Coffee on Thursday?",
                "I'll be near your office around 10. Your usual place?",
            ),
            (
                "Maya Patel",
                "The final selects",
                "Here are my favourites from the shoot. Would love your thoughts.",
            ),
            (
                "GitHub",
                "[kite] Improve keyboard navigation",
                "The changes you requested are ready for another look.",
            ),
            (
                "Nora Williams",
                "One for the reading list",
                "I think you'll enjoy this essay. Especially the last paragraph.",
            ),
            (
                "James, You",
                "Re: Sunday lunch",
                "Perfect. I'll bring something for dessert.",
            ),
            (
                "Figma",
                "A little inspiration for your next project",
                "Fresh ideas from the design community.",
            ),
            (
                "Amelia Brooks",
                "Notes from our conversation",
                "Thanks again for making the time. A few things to follow up on.",
            ),
            (
                "Theo Martin",
                "A good weekend for a long walk",
                "The forecast is looking promising. Shall we head out early?",
            ),
            (
                "Sofia Reyes",
                "Something worth sharing",
                "This reminded me of your idea. Thought you'd want to see it.",
            ),
        ];
        let fixture = Self::new(samples.len());
        fixture.accounts.lock().unwrap()[0].email = "karl@proton.me".into();
        fixture.accounts.lock().unwrap().push(Account {
            id: 2,
            provider: kite_core::model::Provider::Microsoft,
            email: "karl@outlook.com".into(),
            name: "Karl".into(),
        });
        *fixture.folders.lock().unwrap() = vec![
            Folder {
                id: "work".into(),
                account_id: 1,
                name: "Work".into(),
            },
            Folder {
                id: "receipts".into(),
                account_id: 2,
                name: "Receipts".into(),
            },
        ];
        let mut rows = fixture.threads.lock().unwrap();
        let mut messages = fixture.messages.lock().unwrap();
        let mut newsletters = Vec::new();
        for (ix, (sender, subject, snippet)) in samples.into_iter().enumerate() {
            let row = &mut rows[ix];
            row.account_id = if ix % 2 == 0 { 1 } else { 2 };
            row.participants = sender.into();
            row.subject = subject.into();
            row.snippet = snippet.into();
            row.unread = ix < 3;
            row.starred = ix == 4;
            row.date = chrono::Local::now().timestamp_millis() - ix as i64 * 1_800_000;
            // A few senders carry an address in a category the seeded splits look
            // for, so the tabs narrow the list the way the engine's queries would.
            let address = match sender {
                "GitHub" => Some("noreply@github.com"),
                "Maya Patel" => Some("maya@instagram.com"),
                "Sofia Reyes" => Some("sofia@instagram.com"),
                _ => None,
            };
            if matches!(sender, "Figma" | "Nora Williams") {
                newsletters.push(row.id);
            }
            if let Some(thread) = messages.get_mut(&row.id) {
                for message in thread {
                    message.from.name = sender.into();
                    if let Some(address) = address {
                        message.from.email = address.into();
                    }
                    message.to = vec![Addr {
                        name: "Karl".into(),
                        email: if row.account_id == 1 {
                            "karl@proton.me"
                        } else {
                            "karl@outlook.com"
                        }
                        .into(),
                    }];
                    if ix == 0 {
                        message.cc = vec![Addr {
                            name: "Sofia Reyes".into(),
                            email: "sofia@example.com".into(),
                        }];
                        message.attachments = vec![Attachment {
                            id: 501,
                            name: "Next chapter.pdf".into(),
                            mime: "application/pdf".into(),
                            size: 34567,
                        }];
                        row.attachments = true;
                    }
                    message.subject = subject.into();
                    message.html = None;
                    message.text = format!(
                        "Hi Karl,\n\n{snippet}\n\nThere is a lot to be said for doing fewer things, with a little more care. I'd love to hear what you think when you have a moment.\n\nTalk soon,\n{sender}"
                    );
                }
            }
        }
        drop(rows);
        drop(messages);
        *fixture.newsletters.lock().unwrap() = newsletters;
        fixture
    }

    /// The query a seeded split stands for, as the engine would resolve it.
    fn split_query(&self, id: Id) -> Option<String> {
        self.splits()
            .ok()?
            .into_iter()
            .find(|split| split.id == id)
            .map(|split| split.query)
    }

    /// The fixture's slice of the query language: `from:` term lists (comma = OR)
    /// and `is:newsletter`. Enough for the seeded splits and the search tests.
    fn matches(&self, query: &str, t: &ThreadSummary) -> bool {
        query.split_whitespace().all(|term| {
            if let Some(domains) = term.strip_prefix("from:") {
                let from = self.sender_address(t.id).to_lowercase();
                domains
                    .split(',')
                    .any(|domain| !domain.is_empty() && from.contains(&domain.to_lowercase()))
            } else if term == "is:newsletter" {
                self.newsletters.lock().unwrap().contains(&t.id)
            } else {
                let term = term.to_lowercase();
                t.subject.to_lowercase().contains(&term)
                    || t.participants.to_lowercase().contains(&term)
            }
        })
    }

    fn sender_address(&self, thread: Id) -> String {
        self.messages
            .lock()
            .unwrap()
            .get(&thread)
            .and_then(|messages| messages.first())
            .map(|message| message.from.email.clone())
            .unwrap_or_default()
    }

    /// Moves a conversation onto a specific date, so the day gutter's groups can be exercised.
    pub fn set_thread_date(&self, thread: Id, date: Ms) {
        if let Some(row) = self
            .threads
            .lock()
            .unwrap()
            .iter_mut()
            .find(|row| row.id == thread)
        {
            row.date = date;
        }
    }

    /// Stars a conversation without going through `act`, so the actions log stays clean.
    pub fn set_starred(&self, thread: Id) {
        if let Some(row) = self
            .threads
            .lock()
            .unwrap()
            .iter_mut()
            .find(|row| row.id == thread)
        {
            row.starred = true;
        }
    }

    /// Snoozes a conversation until `until`, moving it out of the inbox like the engine does.
    pub fn set_snoozed(&self, thread: Id, until: Ms) {
        if let Some(row) = self
            .threads
            .lock()
            .unwrap()
            .iter_mut()
            .find(|row| row.id == thread)
        {
            row.snoozed_until = Some(until);
        }
        self.locations
            .lock()
            .unwrap()
            .insert(thread, (Mailbox::Snoozed, None));
    }

    /// Adds a thread and tells the UI the store changed.
    pub fn push_thread(&self) -> Id {
        let id = self.next_id();
        let now = chrono::Local::now().timestamp_millis();
        self.threads.lock().unwrap().insert(
            0,
            ThreadSummary {
                id,
                account_id: 1,
                subject: format!("Thread {id}"),
                snippet: "New".into(),
                participants: "New Sender".into(),
                date: now,
                unread: true,
                starred: false,
                important: true,
                attachments: false,
                labels: Vec::new(),
                count: 1,
                snoozed_until: None,
            },
        );
        self.messages.lock().unwrap().insert(
            id,
            vec![message(
                id * 10 + 2,
                id,
                "New",
                "Brand new message",
                None,
                now,
            )],
        );
        id
    }

    pub fn emit(&self, event: Event) {
        for sender in self.senders.lock().unwrap().iter() {
            let _ = sender.try_send(event.clone());
        }
    }
}

fn message(
    id: Id,
    thread_id: Id,
    subject: &str,
    text: &str,
    html: Option<String>,
    date: Ms,
) -> Message {
    Message {
        id,
        thread_id,
        from: Addr {
            name: format!("Sender {thread_id}"),
            email: format!("sender{thread_id}@example.com"),
        },
        to: vec![Addr {
            name: "Me".into(),
            email: "me@example.com".into(),
        }],
        cc: Vec::new(),
        date,
        subject: subject.into(),
        html,
        text: text.into(),
        attachments: Vec::new(),
        unread: false,
    }
}

/// The Drafts-list display name: its recipients, since the sender is always us.
fn draft_participants(draft: &Draft) -> String {
    let names: Vec<String> = draft
        .to
        .iter()
        .chain(&draft.cc)
        .map(|addr| {
            if addr.name.trim().is_empty() {
                addr.email.clone()
            } else {
                addr.name.clone()
            }
        })
        .filter(|name| !name.is_empty())
        .collect();
    if names.is_empty() {
        "(no recipients)".into()
    } else {
        names.join(", ")
    }
}

impl Fixture {
    /// The account a finished native Proton sign-in creates.
    fn add_native_proton(&self, email: &str, _password: &str) -> anyhow::Result<ProtonAuth> {
        if let Some(error) = self.account_error.lock().unwrap().clone() {
            anyhow::bail!(error);
        }
        let mut accounts = self.accounts.lock().unwrap();
        let account = Account {
            id: accounts.iter().map(|a| a.id).max().unwrap_or(0) + 1,
            provider: kite_core::model::Provider::Proton,
            email: email.to_string(),
            name: "Karl".into(),
        };
        accounts.push(account.clone());
        Ok(ProtonAuth::Done(account))
    }
}

impl Mail for Fixture {
    fn accounts(&self) -> anyhow::Result<Vec<Account>> {
        Ok(self.accounts.lock().unwrap().clone())
    }

    fn add_microsoft(&self) -> anyhow::Result<Account> {
        if let Some(error) = self.account_error.lock().unwrap().clone() {
            anyhow::bail!(error);
        }
        let mut accounts = self.accounts.lock().unwrap();
        let account = Account {
            id: accounts.iter().map(|a| a.id).max().unwrap_or(0) + 1,
            provider: kite_core::model::Provider::Microsoft,
            email: "new@outlook.com".into(),
            name: "Karl".into(),
        };
        accounts.push(account.clone());
        Ok(account)
    }

    fn avatar(&self, account: Id) -> Option<std::path::PathBuf> {
        let dir = self.dir.join("avatars");
        [format!("{account}.png"), format!("{account}-provider.png")]
            .into_iter()
            .map(|name| dir.join(name))
            .find(|path| path.is_file())
    }

    fn save_avatar(&self, account: Id, png: &[u8]) -> anyhow::Result<()> {
        let dir = self.dir.join("avatars");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(format!("{account}.png")), png)?;
        Ok(())
    }

    fn remove_avatar(&self, account: Id) -> anyhow::Result<()> {
        let file = self.dir.join("avatars").join(format!("{account}.png"));
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    fn set_account_name(&self, id: Id, name: &str) -> anyhow::Result<()> {
        let mut accounts = self.accounts.lock().unwrap();
        let account = accounts
            .iter_mut()
            .find(|account| account.id == id)
            .ok_or_else(|| anyhow::anyhow!("Unknown account"))?;
        account.name = name.trim().to_string();
        Ok(())
    }

    fn remove_account(&self, id: Id) -> anyhow::Result<()> {
        if let Some(error) = self.account_error.lock().unwrap().clone() {
            anyhow::bail!(error);
        }
        self.accounts
            .lock()
            .unwrap()
            .retain(|account| account.id != id);
        self.threads
            .lock()
            .unwrap()
            .retain(|thread| thread.account_id != id);
        self.folders
            .lock()
            .unwrap()
            .retain(|folder| folder.account_id != id);
        Ok(())
    }

    fn folders(&self) -> anyhow::Result<Vec<Folder>> {
        Ok(self.folders.lock().unwrap().clone())
    }

    fn create_folder(&self, account: Id, name: &str) -> anyhow::Result<Folder> {
        if let Some(error) = self.folder_error.lock().unwrap().clone() {
            anyhow::bail!(error);
        }
        anyhow::ensure!(
            self.accounts
                .lock()
                .unwrap()
                .iter()
                .any(|a| a.id == account),
            "Unknown account"
        );
        let mut folders = self.folders.lock().unwrap();
        anyhow::ensure!(
            !folders
                .iter()
                .any(|f| f.account_id == account && f.name.eq_ignore_ascii_case(name)),
            "Folder already exists"
        );
        let folder = Folder {
            id: format!("folder-{}", folders.len() + 1),
            account_id: account,
            name: name.into(),
        };
        folders.push(folder.clone());
        Ok(folder)
    }

    fn view_count(&self, view: &View) -> anyhow::Result<usize> {
        if view.mailbox == Mailbox::Scheduled {
            return Ok(self
                .outbox
                .lock()
                .unwrap()
                .values()
                .filter(|d| {
                    d.send_at.is_some() && view.account_id.is_none_or(|a| a == d.account_id)
                })
                .count());
        }
        if view.mailbox == Mailbox::Waiting {
            let all = self.threads.lock().unwrap();
            let locations = self.locations.lock().unwrap();
            let days = self.settings.lock().unwrap().waiting_after_days;
            return Ok(all
                .iter()
                .filter(|t| {
                    view.account_id.is_none_or(|a| a == t.account_id)
                        && locations
                            .get(&t.id)
                            .is_some_and(|(m, _)| *m == Mailbox::Waiting)
                        && chrono::Local::now().timestamp_millis() - t.date
                            >= i64::from(days) * 86_400_000
                })
                .count());
        }
        Ok(self.threads(view, 0, u32::MAX)?.len())
    }

    fn threads(&self, view: &View, offset: u32, limit: u32) -> anyhow::Result<Vec<ThreadSummary>> {
        self.queries.lock().unwrap().push((view.clone(), offset));
        if view.mailbox == Mailbox::Scheduled {
            let mut rows: Vec<_> = self
                .outbox
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, d)| {
                    d.send_at.is_some() && view.account_id.is_none_or(|a| a == d.account_id)
                })
                .map(|(id, d)| ThreadSummary {
                    id: *id,
                    account_id: d.account_id,
                    subject: d.subject.clone(),
                    snippet: d.body.clone(),
                    participants: d
                        .to
                        .iter()
                        .map(|a| a.email.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    date: d.send_at.unwrap(),
                    unread: false,
                    starred: false,
                    important: false,
                    attachments: false,
                    labels: vec![],
                    count: 1,
                    snoozed_until: None,
                })
                .collect();
            rows.sort_by_key(|r| r.date);
            return Ok(rows
                .into_iter()
                .skip(offset as usize)
                .take(limit as usize)
                .collect());
        }
        let all = self.threads.lock().unwrap();
        // The engine resolves a split into its query; so does the fixture.
        let query = view
            .query
            .clone()
            .or_else(|| view.split.and_then(|id| self.split_query(id)))
            .unwrap_or_default();
        let rows: Vec<ThreadSummary> = all
            .iter()
            .filter(|t| view.account_id.is_none_or(|id| id == t.account_id))
            .filter(|t| {
                let locations = self.locations.lock().unwrap();
                let (mailbox, folder) = locations
                    .get(&t.id)
                    .cloned()
                    .unwrap_or((Mailbox::Inbox, None));
                if let Some(wanted) = &view.folder {
                    return folder.as_ref() == Some(wanted);
                }
                match view.mailbox {
                    Mailbox::All => !matches!(mailbox, Mailbox::Trash | Mailbox::Spam),
                    Mailbox::Starred => {
                        t.starred && !matches!(mailbox, Mailbox::Trash | Mailbox::Spam)
                    }
                    Mailbox::Waiting => {
                        mailbox == Mailbox::Waiting
                            && (chrono::Local::now().timestamp_millis() - t.date
                                >= i64::from(self.settings.lock().unwrap().waiting_after_days)
                                    * 86_400_000)
                    }
                    Mailbox::Snoozed => t.snoozed_until.is_some(),
                    wanted => mailbox == wanted && folder.is_none(),
                }
            })
            .filter(|t| query.trim().is_empty() || self.matches(&query, t))
            .cloned()
            .collect();
        Ok(rows
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect())
    }

    fn thread(&self, id: Id) -> anyhow::Result<Vec<Message>> {
        self.opened.lock().unwrap().push(id);
        Ok(self
            .messages
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .unwrap_or_default())
    }

    fn act(&self, ids: &[Id], action: &Action) -> anyhow::Result<i64> {
        self.actions
            .lock()
            .unwrap()
            .push((ids.to_vec(), action.clone()));
        let token = self.next_token.fetch_add(1, Ordering::SeqCst);
        let mut all = self.threads.lock().unwrap();
        self.snapshots
            .lock()
            .unwrap()
            .insert(token, (all.clone(), self.locations.lock().unwrap().clone()));
        match action {
            Action::Star => all
                .iter_mut()
                .filter(|t| ids.contains(&t.id))
                .for_each(|t| t.starred = true),
            Action::Unstar => all
                .iter_mut()
                .filter(|t| ids.contains(&t.id))
                .for_each(|t| t.starred = false),
            Action::Read => all
                .iter_mut()
                .filter(|t| ids.contains(&t.id))
                .for_each(|t| t.unread = false),
            Action::Unread => all
                .iter_mut()
                .filter(|t| ids.contains(&t.id))
                .for_each(|t| t.unread = true),
            Action::Snooze { until } => {
                for t in all.iter_mut().filter(|t| ids.contains(&t.id)) {
                    t.snoozed_until = Some(*until);
                    self.locations
                        .lock()
                        .unwrap()
                        .insert(t.id, (Mailbox::Snoozed, None));
                }
            }
            Action::Label { tag } => all
                .iter_mut()
                .filter(|t| ids.contains(&t.id))
                .for_each(|t| {
                    if !t.labels.contains(tag) {
                        t.labels.push(tag.clone());
                    }
                }),
            Action::Unlabel { tag } => all
                .iter_mut()
                .filter(|t| ids.contains(&t.id))
                .for_each(|t| t.labels.retain(|l| l != tag)),
            action => {
                for id in ids {
                    let location = match action {
                        Action::DoneWaiting => (Mailbox::Sent, None),
                        Action::Archive => (Mailbox::Archive, None),
                        Action::Inbox => (Mailbox::Inbox, None),
                        Action::Trash => (Mailbox::Trash, None),
                        Action::Move { folder } => (Mailbox::Archive, Some(folder.clone())),
                        _ => unreachable!(),
                    };
                    self.locations.lock().unwrap().insert(*id, location);
                }
            }
        }
        Ok(token)
    }

    fn undo(&self, token: i64) -> anyhow::Result<()> {
        self.undos.lock().unwrap().push(token);
        if let Some(snapshot) = self.snapshots.lock().unwrap().get(&token).cloned() {
            *self.threads.lock().unwrap() = snapshot.0;
            *self.locations.lock().unwrap() = snapshot.1;
        }
        Ok(())
    }

    /// Same slice the store cuts: the view's threads, older than `before`, never snoozed,
    /// starred only when asked for.
    fn archivable(
        &self,
        view: &View,
        before: Ms,
        include_starred: bool,
    ) -> anyhow::Result<Vec<Id>> {
        Ok(self
            .threads(view, 0, u32::MAX)?
            .into_iter()
            .filter(|t| t.date < before)
            .filter(|t| t.snoozed_until.is_none())
            .filter(|t| include_starred || !t.starred)
            .map(|t| t.id)
            .collect())
    }

    fn send(&self, draft: Draft) -> anyhow::Result<Id> {
        if let Some(id) = draft.id
            && let Some(thread) = self.draft_threads.lock().unwrap().remove(&id)
        {
            self.threads.lock().unwrap().retain(|row| row.id != thread);
            self.messages.lock().unwrap().remove(&thread);
            self.locations.lock().unwrap().remove(&thread);
        }
        let id = self.send_seq.fetch_add(1, Ordering::SeqCst);
        self.sent.lock().unwrap().push(draft.clone());
        self.outbox.lock().unwrap().insert(id, draft);
        Ok(id)
    }

    fn cancel_send(&self, id: Id) -> anyhow::Result<Option<Draft>> {
        self.cancelled.lock().unwrap().push(id);
        let removed = self.outbox.lock().unwrap().remove(&id);
        if let Some(mut draft) = removed {
            draft.send_at = None;
            draft.id = Some(self.save_draft(draft.clone())?);
            Ok(Some(draft))
        } else {
            Ok(None)
        }
    }

    fn save_draft(&self, draft: Draft) -> anyhow::Result<Id> {
        let id = match draft.id {
            Some(id) => id,
            None => self.draft_seq.fetch_add(1, Ordering::SeqCst),
        };
        let content = Draft {
            id: Some(id),
            ..draft
        };
        let now = chrono::Local::now().timestamp_millis();
        if let Some(thread) = self.draft_threads.lock().unwrap().get(&id).copied() {
            if let Some(row) = self
                .threads
                .lock()
                .unwrap()
                .iter_mut()
                .find(|row| row.id == thread)
            {
                row.subject = content.subject.clone();
                row.snippet = content.body.lines().next().unwrap_or_default().to_string();
                row.participants = draft_participants(&content);
                row.date = now;
            }
        } else {
            let thread = self.next_id();
            self.threads.lock().unwrap().insert(
                0,
                ThreadSummary {
                    id: thread,
                    account_id: content.account_id,
                    subject: content.subject.clone(),
                    snippet: content.body.lines().next().unwrap_or_default().to_string(),
                    participants: draft_participants(&content),
                    date: now,
                    unread: false,
                    starred: false,
                    important: false,
                    attachments: false,
                    labels: Vec::new(),
                    count: 1,
                    snoozed_until: None,
                },
            );
            self.messages.lock().unwrap().insert(
                thread,
                vec![Message {
                    id: thread * 10 + 1,
                    thread_id: thread,
                    from: Addr {
                        name: String::new(),
                        email: content
                            .to
                            .first()
                            .map(|addr| addr.email.clone())
                            .unwrap_or_default(),
                    },
                    to: content.to.clone(),
                    cc: content.cc.clone(),
                    date: now,
                    subject: content.subject.clone(),
                    html: None,
                    text: content.body.clone(),
                    attachments: Vec::new(),
                    unread: false,
                }],
            );
            self.locations
                .lock()
                .unwrap()
                .insert(thread, (Mailbox::Drafts, None));
            self.draft_threads.lock().unwrap().insert(id, thread);
        }
        let mut saved = self.saved_drafts.lock().unwrap();
        saved.retain(|draft| draft.id != Some(id));
        saved.push(content.clone());
        drop(saved);
        self.emit(Event::Changed(vec![content.account_id]));
        Ok(id)
    }

    fn draft(&self, thread: Id) -> anyhow::Result<Option<Draft>> {
        let id = self
            .draft_threads
            .lock()
            .unwrap()
            .iter()
            .find(|(_, shown)| **shown == thread)
            .map(|(id, _)| *id);
        let Some(id) = id else {
            return Ok(None);
        };
        Ok(self
            .saved_drafts
            .lock()
            .unwrap()
            .iter()
            .find(|draft| draft.id == Some(id))
            .cloned())
    }

    fn discard_draft(&self, id: Id) -> anyhow::Result<()> {
        self.discarded.lock().unwrap().push(id);
        let account = {
            let mut saved = self.saved_drafts.lock().unwrap();
            let account = saved
                .iter()
                .find(|draft| draft.id == Some(id))
                .map(|draft| draft.account_id);
            saved.retain(|draft| draft.id != Some(id));
            account
        };
        if let Some(thread) = self.draft_threads.lock().unwrap().remove(&id) {
            self.threads.lock().unwrap().retain(|row| row.id != thread);
            self.messages.lock().unwrap().remove(&thread);
            self.locations.lock().unwrap().remove(&thread);
        }
        if let Some(account) = account {
            self.emit(Event::Changed(vec![account]));
        }
        Ok(())
    }

    fn contacts(&self, _: &str) -> anyhow::Result<Vec<Addr>> {
        Ok(Vec::new())
    }

    /// The store's card, cut the same way the engine cuts it: every message the address
    /// touches counts, the open conversation stays out of the recent list.
    fn contact(&self, email: &str, exclude: Option<Id>) -> anyhow::Result<Option<ContactCard>> {
        let threads = self.threads.lock().unwrap();
        let messages = self.messages.lock().unwrap();
        let (mut first, mut latest, mut conversations) = (None, None, 0u32);
        let mut name = String::new();
        let mut involved: Vec<Id> = Vec::new();
        let mut files: Vec<ContactFile> = Vec::new();
        for (thread, msgs) in messages.iter() {
            let mut takes_part = false;
            for m in msgs {
                let them = m.from.email.eq_ignore_ascii_case(email)
                    || m.to
                        .iter()
                        .chain(&m.cc)
                        .any(|a| a.email.eq_ignore_ascii_case(email));
                if !them {
                    continue;
                }
                takes_part = true;
                if m.from.email.eq_ignore_ascii_case(email) && !m.from.name.is_empty() {
                    name = m.from.name.clone();
                }
                first = Some(first.map_or(m.date, |seen: Ms| seen.min(m.date)));
                latest = Some(latest.map_or(m.date, |seen: Ms| seen.max(m.date)));
                files.extend(m.attachments.iter().map(|a| ContactFile {
                    thread: *thread,
                    name: a.name.clone(),
                    size: a.size,
                    date: m.date,
                }));
            }
            if takes_part {
                conversations += 1;
                involved.push(*thread);
            }
        }
        let (Some(first), Some(latest)) = (first, latest) else {
            return Ok(None);
        };
        let mut recent: Vec<ContactThread> = involved
            .into_iter()
            .filter(|id| Some(*id) != exclude)
            .filter_map(|id| {
                threads.iter().find(|t| t.id == id).map(|t| ContactThread {
                    id,
                    subject: t.subject.clone(),
                    date: t.date,
                })
            })
            .collect();
        recent.sort_by_key(|t| std::cmp::Reverse(t.date));
        recent.truncate(5);
        files.sort_by_key(|f| std::cmp::Reverse(f.date));
        files.truncate(5);
        Ok(Some(ContactCard {
            name,
            email: email.to_string(),
            domain: email.rsplit('@').next().unwrap_or_default().to_string(),
            first,
            latest,
            conversations,
            reply_median: None,
            replies_back: None,
            recent,
            files,
        }))
    }

    fn reveal_attachment(&self, id: Id) -> anyhow::Result<()> {
        self.attachments_revealed.lock().unwrap().push(id);
        Ok(())
    }

    fn open_attachment(&self, id: Id) -> anyhow::Result<()> {
        self.attachments_opened.lock().unwrap().push(id);
        Ok(())
    }

    fn fetch_image(&self, url: &str) -> anyhow::Result<Vec<u8>> {
        self.images_fetched.lock().unwrap().push(url.to_string());
        Ok(self.image_bytes.lock().unwrap().clone())
    }

    fn splits(&self) -> anyhow::Result<Vec<Split>> {
        Ok(vec![
            Split {
                id: SPLITS[0],
                name: SPLIT_NAMES[0].into(),
                query: String::new(),
            },
            Split {
                id: SPLITS[1],
                name: SPLIT_NAMES[1].into(),
                query: SOCIAL_QUERY.into(),
            },
            Split {
                id: SPLITS[2],
                name: SPLIT_NAMES[2].into(),
                query: NEWSLETTER_QUERY.into(),
            },
            Split {
                id: SPLITS[3],
                name: SPLIT_NAMES[3].into(),
                query: GITHUB_QUERY.into(),
            },
        ])
    }

    fn set_splits(&self, _: Vec<Split>) -> anyhow::Result<()> {
        Ok(())
    }

    fn settings(&self) -> anyhow::Result<Settings> {
        if let Some(path) = &*self.settings_path.lock().unwrap() {
            return read_settings(path);
        }
        Ok(self.settings.lock().unwrap().clone())
    }

    fn set_settings(&self, settings: Settings) -> anyhow::Result<()> {
        if let Some(path) = &*self.settings_path.lock().unwrap() {
            write_settings(path, &settings)?;
        }
        *self.settings.lock().unwrap() = settings;
        Ok(())
    }

    fn set_ai_key(&self, key: &str) -> anyhow::Result<()> {
        *self.ai_key.lock().unwrap() = Some(key.to_string());
        Ok(())
    }

    fn calendar_today(&self, _: Id) -> anyhow::Result<Vec<CalEvent>> {
        Ok(Vec::new())
    }

    fn tags(&self) -> anyhow::Result<Vec<Tag>> {
        Ok(self.tags.lock().unwrap().clone())
    }

    /// The fixture enforces the store's rule too: Private cannot be left out.
    fn set_tags(&self, tags: Vec<Tag>) -> anyhow::Result<()> {
        let mut kept = self.tags.lock().unwrap();
        anyhow::ensure!(
            tags.iter().any(|t| t.name.eq_ignore_ascii_case("Private")),
            "the Private tag cannot be deleted"
        );
        let mut next = kept.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        *kept = tags
            .into_iter()
            .enumerate()
            .map(|(i, mut tag)| {
                if tag.id <= 0 {
                    tag.id = next;
                    next += 1;
                }
                tag.position = i as i64;
                tag
            })
            .collect();
        Ok(())
    }

    fn ai(&self, req: u32, request: AiRequest) -> anyhow::Result<()> {
        // The store refuses a Private source before it builds a prompt, so the fixture refuses
        // it too: the UI must show that refusal, and no request may reach the engine.
        let source = match &request {
            AiRequest::Summary { thread } | AiRequest::Replies { thread } => Some(*thread),
            _ => None,
        };
        if let Some(thread) = source {
            let private = self
                .threads
                .lock()
                .unwrap()
                .iter()
                .find(|t| t.id == thread)
                .is_some_and(|t| {
                    t.labels
                        .iter()
                        .any(|tag| tag.eq_ignore_ascii_case(kite_core::model::PRIVATE_TAG))
                });
            anyhow::ensure!(
                !private,
                "This conversation is private, so AI can't read it"
            );
        }
        if let AiRequest::Ask { .. } = &request {
            self.emit(Event::AiSources {
                req,
                threads: self.ai_sources.lock().unwrap().clone(),
            });
        }
        if let Some(error) = self.ai_error.lock().unwrap().clone() {
            // A failed stream ends quietly and reports through the error event.
            self.emit(Event::Ai {
                req,
                delta: String::new(),
                done: true,
            });
            self.emit(Event::Error(format!("AI: {error}")));
        }
        let reply = self.ai_reply.lock().unwrap().clone();
        if !reply.is_empty() {
            self.emit(Event::Ai {
                req,
                delta: reply,
                done: true,
            });
        }
        self.ai_requests.lock().unwrap().push((req, request));
        Ok(())
    }

    fn ai_models(&self) -> anyhow::Result<Vec<AiModel>> {
        *self.ai_model_calls.lock().unwrap() += 1;
        if let Some(error) = self.ai_models_error.lock().unwrap().clone() {
            anyhow::bail!(error);
        }
        Ok(self.ai_models.lock().unwrap().clone())
    }

    fn sync_now(&self) {
        self.refreshes.fetch_add(1, Ordering::Relaxed);
    }

    fn events(&self) -> async_channel::Receiver<Event> {
        let (tx, rx) = async_channel::unbounded();
        self.senders.lock().unwrap().push(tx);
        rx
    }

    fn proton_sign_in(&self, email: &str, password: &str) -> anyhow::Result<ProtonAuth> {
        self.proton_attempts
            .lock()
            .unwrap()
            .push((email.to_string(), password.to_string()));
        if let Some(error) = self.account_error.lock().unwrap().clone() {
            anyhow::bail!(error);
        }
        if let Some(step) = self.proton_step.lock().unwrap().clone() {
            return Ok(step);
        }
        self.add_native_proton(email, password)
    }

    fn proton_sign_in_step(&self, answer: &str) -> anyhow::Result<ProtonAuth> {
        self.proton_answers.lock().unwrap().push(answer.to_string());
        if let Some(error) = self.account_error.lock().unwrap().clone() {
            anyhow::bail!(error);
        }
        self.proton_step.lock().unwrap().take();
        let email = self
            .proton_attempts
            .lock()
            .unwrap()
            .last()
            .map(|(email, _)| email.clone())
            .unwrap_or_default();
        self.add_native_proton(&email, answer)
    }
}

/// Settings-file helpers for the fixture only; production persists settings in the engine.
pub fn read_settings(path: &std::path::Path) -> anyhow::Result<Settings> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(error) => Err(error.into()),
    }
}

pub fn write_settings(path: &std::path::Path, settings: &Settings) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(settings)?;
    std::fs::write(&temp, bytes)?;
    std::fs::rename(temp, path)?;
    Ok(())
}

/// The production window entry point with deterministic test bounds.
pub fn open_window<V: Render>(
    cx: &mut TestAppContext,
    size: Option<Size<gpui_kit::Pixels>>,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> (WindowHandle<Root>, Entity<V>) {
    cx.update(|cx| {
        let bounds = size
            .map(|size| Bounds {
                origin: Point::default(),
                size,
            })
            .unwrap_or_else(|| Bounds::maximized(None, cx));
        let (window, content) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            build,
        )
        .expect("open test window");
        (window.downcast::<Root>().expect("Base Root"), content)
    })
}

/// Opens Kite on the given fixture.
pub fn mount(
    cx: &mut TestAppContext,
    mail: Arc<dyn Mail>,
) -> (WindowHandle<Root>, Entity<kite::app::Kite>) {
    cx.update(gpui_kit::init);
    open_window(
        cx,
        Some(gpui_kit::size(px(1200.), px(800.))),
        |window, cx| cx.new(|cx| kite::app::Kite::new(mail, window, cx)),
    )
}

pub fn update_content<V: Render, R>(
    window: WindowHandle<Root>,
    content: &Entity<V>,
    cx: &mut TestAppContext,
    update: impl FnOnce(&mut V, &mut Window, &mut gpui_kit::Context<V>) -> R,
) -> R {
    cx.update_window(window.into(), |_, window, cx| {
        content.update(cx, |view, cx| update(view, window, cx))
    })
    .unwrap()
}
