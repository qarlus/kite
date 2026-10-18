//! Test fixture: an in-memory `Mail` that serves synthetic threads, applies actions
//! to its own data and records every call the UI makes.
#![allow(dead_code)]
use gpui_kit::{
    App, AppContext, Bounds, Entity, Point, Render, Size, TestAppContext, Window, WindowBounds,
    WindowHandle, WindowOptions, base::Root, px,
};
use kite_core::api::Mail;
use kite_core::model::{
    Account, Action, Addr, Attachment, CalEvent, ChatMsg, Draft, Event, Folder, Id, Mailbox,
    Message, Ms, ProtonLogin, Settings, Split, ThreadSummary, View,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

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

pub struct Fixture {
    pub folders: Mutex<Vec<Folder>>,
    pub locations: Mutex<Locations>,
    pub folder_error: Mutex<Option<String>>,
    pub account_error: Mutex<Option<String>>,
    pub settings_path: Mutex<Option<std::path::PathBuf>>,
    pub attachments_opened: Mutex<Vec<Id>>,
    pub settings: Mutex<Settings>,
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
    senders: Mutex<Vec<async_channel::Sender<Event>>>,
    next_token: AtomicI64,
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
            settings_path: Mutex::new(None),
            attachments_opened: Mutex::new(Vec::new()),
            settings: Mutex::new(Settings {
                theme: "dark".into(),
                ..Settings::default()
            }),
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
            senders: Mutex::new(Vec::new()),
            next_token: AtomicI64::new(1),
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

impl Mail for Fixture {
    fn accounts(&self) -> anyhow::Result<Vec<Account>> {
        Ok(self.accounts.lock().unwrap().clone())
    }

    fn add_proton(&self, login: ProtonLogin) -> anyhow::Result<Account> {
        if let Some(error) = self.account_error.lock().unwrap().clone() {
            anyhow::bail!(error);
        }
        let mut accounts = self.accounts.lock().unwrap();
        let account = Account {
            id: accounts.iter().map(|a| a.id).max().unwrap_or(0) + 1,
            provider: kite_core::model::Provider::Proton,
            email: login.email,
            name: "Karl".into(),
        };
        accounts.push(account.clone());
        Ok(account)
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

    fn threads(&self, view: &View, offset: u32, limit: u32) -> anyhow::Result<Vec<ThreadSummary>> {
        self.queries.lock().unwrap().push((view.clone(), offset));
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
            action => {
                for id in ids {
                    let location = match action {
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

    fn send(&self, _: Draft) -> anyhow::Result<Id> {
        Ok(0)
    }

    fn cancel_send(&self, _: Id) -> anyhow::Result<Option<Draft>> {
        Ok(None)
    }

    fn contacts(&self, _: &str) -> anyhow::Result<Vec<Addr>> {
        Ok(Vec::new())
    }

    fn open_attachment(&self, id: Id) -> anyhow::Result<()> {
        self.attachments_opened.lock().unwrap().push(id);
        Ok(())
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

    fn set_ai_key(&self, _: &str) -> anyhow::Result<()> {
        Ok(())
    }

    fn calendar_today(&self, _: Id) -> anyhow::Result<Vec<CalEvent>> {
        Ok(Vec::new())
    }

    fn ai(&self, _: u32, _: Vec<ChatMsg>) -> anyhow::Result<()> {
        Ok(())
    }

    fn sync_now(&self) {}

    fn events(&self) -> async_channel::Receiver<Event> {
        let (tx, rx) = async_channel::unbounded();
        self.senders.lock().unwrap().push(tx);
        rx
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
