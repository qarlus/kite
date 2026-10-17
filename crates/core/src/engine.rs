//! The production `Mail`: local SQLite reads, optimistic local writes, and one background thread
//! running a tokio runtime that drains pending ops, syncs accounts, wakes snoozes and sends the
//! outbox. Engine -> UI traffic goes over the `Event` channel; the UI never sees tokio or SQLite.
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt::Write as _;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use async_channel::{Receiver, Sender};
use chrono::Local;

use crate::api::Mail;
use crate::auth::{self, Tokens};
use crate::model::*;
use crate::store::Store;
use crate::sync::graph::Graph;
use crate::sync::imap::{Imap, ImapConfig};
use crate::sync::{Delta, Op};

const DEFAULT_GRAPH: &str = "https://graph.microsoft.com/v1.0";
const DEFAULT_AUTHORITY: &str = "https://login.microsoftonline.com/common";
/// Messages per transaction: a UI write never waits behind a long first sync (see ARCHITECTURE).
const APPLY_CHUNK: usize = 200;
/// Sync calls per account per tick, so one huge mailbox cannot monopolise the loop.
const SYNC_STEPS: usize = 64;

/// Where the store and config live for the current user.
pub fn data_dir() -> PathBuf {
    let base = if cfg!(target_os = "windows") {
        std::env::var("APPDATA")
            .ok()
            .map(|dir| PathBuf::from(dir).join("Kite"))
    } else if cfg!(target_os = "macos") {
        std::env::var("HOME")
            .ok()
            .map(|home| PathBuf::from(home).join("Library/Application Support/Kite"))
    } else {
        std::env::var("XDG_DATA_HOME")
            .ok()
            .filter(|dir| !dir.is_empty())
            .map(|dir| PathBuf::from(dir).join("kite"))
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|home| PathBuf::from(home).join(".local/share/kite"))
            })
    };
    base.unwrap_or_else(|| std::env::temp_dir().join("kite"))
}

/// Secret storage. Production uses the OS keychain; tests inject `Memory` so no keychain is touched.
pub enum Secrets {
    Os,
    Memory(Mutex<HashMap<String, String>>),
}

impl Secrets {
    fn get(&self, key: &str) -> Result<Option<String>> {
        match self {
            Secrets::Os => auth::secret_get(key),
            Secrets::Memory(map) => Ok(lock(map).get(key).cloned()),
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<()> {
        match self {
            Secrets::Os => auth::secret_set(key, value),
            Secrets::Memory(map) => {
                lock(map).insert(key.to_string(), value.to_string());
                Ok(())
            }
        }
    }

    fn del(&self, key: &str) -> Result<()> {
        match self {
            Secrets::Os => auth::secret_del(key),
            Secrets::Memory(map) => {
                lock(map).remove(key);
                Ok(())
            }
        }
    }
}

/// Test seams. Production uses [`Options::default`].
#[derive(Clone)]
pub struct Options {
    pub graph_base_url: Option<String>,
    pub graph_authority: Option<String>,
    pub poll: Duration,
    /// Send STARTTLS on Proton connections. Tests set false to reach a local plaintext fake server.
    pub proton_tls: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            graph_base_url: None,
            graph_authority: None,
            poll: Duration::from_secs(30),
            proton_tls: true,
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) struct Inner {
    reader: Mutex<Store>,
    writer: Mutex<Store>,
    senders: Mutex<Vec<Sender<Event>>>,
    wake: Sender<()>,
    secrets: Secrets,
    opts: Options,
    dir: PathBuf,
}

impl Inner {
    fn read<T>(&self, f: impl FnOnce(&Store) -> Result<T>) -> Result<T> {
        f(&lock(&self.reader))
    }

    fn write<T>(&self, f: impl FnOnce(&Store) -> Result<T>) -> Result<T> {
        f(&lock(&self.writer))
    }

    pub(crate) fn emit(&self, event: Event) {
        let mut senders = lock(&self.senders);
        senders.retain(|s| !s.is_closed());
        for s in senders.iter() {
            let _ = s.try_send(event.clone());
        }
    }
}

pub struct Engine {
    inner: Arc<Inner>,
}

impl Engine {
    /// Opens `<dir>/kite.db`, spawns the background loop and returns a shared handle.
    pub fn open(dir: &Path) -> Result<Arc<Engine>> {
        Engine::open_with(dir, Secrets::Os, Options::default())
    }

    pub fn open_with(dir: &Path, secrets: Secrets, opts: Options) -> Result<Arc<Engine>> {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        let path = dir.join("kite.db");
        let writer = Store::open(&path)?;
        let reader = Store::open(&path)?;
        let (wake, wake_rx) = async_channel::unbounded();
        let inner = Arc::new(Inner {
            reader: Mutex::new(reader),
            writer: Mutex::new(writer),
            senders: Mutex::new(Vec::new()),
            wake,
            secrets,
            opts,
            dir: dir.to_path_buf(),
        });
        let _ = std::thread::Builder::new()
            .name("kite-engine".into())
            .spawn({
                let weak = Arc::downgrade(&inner);
                move || {
                    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    else {
                        return;
                    };
                    rt.block_on(run(weak, wake_rx));
                }
            });
        Ok(Arc::new(Engine { inner }))
    }

    fn wake(&self) {
        let _ = self.inner.wake.try_send(());
    }
}

// ------------------------------------------------------------------- the loop

async fn run(weak: Weak<Inner>, wake: Receiver<()>) {
    let poll = match weak.upgrade() {
        Some(inner) => inner.opts.poll,
        None => return,
    };
    let mut backends: HashMap<Id, Backend> = HashMap::new();
    let mut backoff: HashMap<Id, (u32, Ms)> = HashMap::new();
    loop {
        let mut forced = drain(&wake);
        if !forced {
            tokio::select! {
                _ = tokio::time::sleep(poll) => {}
                result = wake.recv() => {
                    if result.is_err() {
                        return;
                    }
                    // A wake always overrides a per-account backoff (sync_now, act, send, undo).
                    forced = true;
                    while wake.try_recv().is_ok() {}
                }
            }
        }
        let Some(inner) = weak.upgrade() else {
            return;
        };
        tick(&inner, &mut backends, &mut backoff, forced).await;
    }
}

fn drain(wake: &Receiver<()>) -> bool {
    let mut any = false;
    while wake.try_recv().is_ok() {
        any = true;
    }
    any
}

async fn tick(
    inner: &Inner,
    backends: &mut HashMap<Id, Backend>,
    backoff: &mut HashMap<Id, (u32, Ms)>,
    forced: bool,
) {
    let now = auth::now_ms();
    let accounts = match inner.read(Store::accounts) {
        Ok(accounts) => accounts,
        Err(e) => return inner.emit(Event::Error(format!("{e:#}"))),
    };
    let live: Vec<Id> = accounts.iter().map(|a| a.id).collect();
    backends.retain(|id, _| live.contains(id));
    backoff.retain(|id, _| live.contains(id));

    let mut changed: Vec<Id> = Vec::new();
    for account in &accounts {
        if !forced
            && backoff
                .get(&account.id)
                .is_some_and(|(_, next)| now < *next)
        {
            continue;
        }
        match sync_account(inner, account, backends).await {
            Ok(any) => {
                backoff.remove(&account.id);
                if any {
                    changed.push(account.id);
                }
            }
            Err(e) => {
                let entry = backoff.entry(account.id).or_insert((0, 0));
                let attempts = entry.0 + 1;
                let wait = (1_000i64 << attempts.min(9)).min(600_000);
                *entry = (attempts, now + wait);
                inner.emit(Event::Error(format!("{}: {e:#}", account.email)));
            }
        }
    }

    match inner.write(|s| s.wake_due(now)) {
        Ok(ids) => changed.extend(ids),
        Err(e) => inner.emit(Event::Error(format!("{e:#}"))),
    }
    if let Err(e) = send_due(inner, &accounts, backends, &mut changed).await {
        inner.emit(Event::Error(format!("{e:#}")));
    }

    changed.sort_unstable();
    changed.dedup();
    if !changed.is_empty() {
        inner.emit(Event::Changed(changed));
    }
}

/// Drains queued ops, then syncs until backfill is exhausted. Returns whether the store changed.
async fn sync_account(
    inner: &Inner,
    account: &Account,
    backends: &mut HashMap<Id, Backend>,
) -> Result<bool> {
    if let Entry::Vacant(slot) = backends.entry(account.id) {
        slot.insert(backend_for(inner, account)?);
    }
    let mut changed = false;

    for _ in 0..32 {
        let ops = inner.write(|s| s.pending_ops(account.id, auth::now_ms(), APPLY_CHUNK as u32))?;
        if ops.is_empty() {
            break;
        }
        let backend = backend_mut(backends, account.id)?;
        for (op_id, op) in ops {
            match backend.apply(&op).await {
                Ok(new_remote_id) => {
                    inner.write(|s| s.op_done(op_id, new_remote_id))?;
                    changed = true;
                }
                Err(e) => {
                    inner.write(|s| s.op_failed(op_id, auth::now_ms()))?;
                    return Err(e);
                }
            }
            persist_tokens(inner, account.id, backend)?;
        }
    }

    let mut cursor = inner.write(|s| s.cursor(account.id))?;
    for _ in 0..SYNC_STEPS {
        let backend = backend_mut(backends, account.id)?;
        let (deltas, next) = backend.sync(cursor.clone()).await?;
        persist_tokens(inner, account.id, backend)?;
        let more = !deltas.is_empty();
        if more {
            for chunk in deltas.chunks(APPLY_CHUNK) {
                inner.write(|s| s.apply_deltas(account.id, chunk))?;
            }
            changed = true;
        }
        inner.write(|s| s.set_cursor(account.id, &next))?;
        cursor = Some(next);
        if !more {
            break;
        }
    }
    Ok(changed)
}

async fn send_due(
    inner: &Inner,
    accounts: &[Account],
    backends: &mut HashMap<Id, Backend>,
    changed: &mut Vec<Id>,
) -> Result<()> {
    let due = inner.write(|s| s.outbox_due(auth::now_ms()))?;
    for (outbox_id, draft) in due {
        let Some(account) = accounts.iter().find(|a| a.id == draft.account_id) else {
            inner.write(|s| s.outbox_sent(outbox_id))?;
            continue;
        };
        let mime = match build_mime(inner, account, &draft) {
            Ok(mime) => mime,
            Err(e) => {
                inner.emit(Event::Error(format!("could not build message: {e:#}")));
                continue;
            }
        };
        if let Entry::Vacant(slot) = backends.entry(account.id) {
            slot.insert(backend_for(inner, account)?);
        }
        let backend = backend_mut(backends, account.id)?;
        match backend.apply(&Op::Send { mime }).await {
            Ok(_) => {
                persist_tokens(inner, account.id, backend)?;
                inner.write(|s| s.outbox_sent(outbox_id))?;
                changed.push(account.id);
                if let (Some(at), Some(reply)) = (draft.remind_at, draft.reply_to)
                    && let Some(thread) = inner.read(|s| s.message_thread(reply))?
                {
                    inner.write(|s| s.remind_if_no_reply(thread, at))?;
                }
            }
            Err(e) => inner.emit(Event::Error(format!("send failed: {e:#}"))),
        }
    }
    Ok(())
}

// ------------------------------------------------------------------- backends

/// Stored Proton credentials. `ImapConfig` is not serializable, so this mirrors its fields.
#[derive(serde::Serialize, serde::Deserialize)]
struct ProtonCreds {
    host: String,
    imap_port: u16,
    smtp_port: u16,
    user: String,
    password: String,
    tls: bool,
}

impl ProtonCreds {
    fn imap(&self) -> ImapConfig {
        ImapConfig {
            host: self.host.clone(),
            imap_port: self.imap_port,
            smtp_port: self.smtp_port,
            user: self.user.clone(),
            password: self.password.clone(),
            tls: self.tls,
        }
    }
}

enum Backend {
    Imap(Box<Imap>),
    /// `saved` is the token JSON last written to the keychain, so a refresh is persisted once.
    Graph {
        client: Box<Graph>,
        saved: String,
    },
}

impl Backend {
    async fn sync(&mut self, cursor: Option<String>) -> Result<(Vec<Delta>, String)> {
        match self {
            Backend::Imap(imap) => imap.sync(cursor).await,
            Backend::Graph { client, .. } => client.sync(cursor).await,
        }
    }

    async fn apply(&mut self, op: &Op) -> Result<Option<String>> {
        match self {
            Backend::Imap(imap) => imap.apply(op).await,
            Backend::Graph { client, .. } => client.apply(op).await,
        }
    }

    async fn attachment(&mut self, message: &str, attachment: &str) -> Result<Vec<u8>> {
        match self {
            Backend::Imap(imap) => imap.attachment(message, attachment).await,
            Backend::Graph { client, .. } => client.attachment(message, attachment).await,
        }
    }
}

fn backend_mut(backends: &mut HashMap<Id, Backend>, account: Id) -> Result<&mut Backend> {
    backends
        .get_mut(&account)
        .ok_or_else(|| anyhow!("no backend for account {account}"))
}

fn backend_for(inner: &Inner, account: &Account) -> Result<Backend> {
    match account.provider {
        Provider::Proton => {
            let key = format!("proton:{}", account.id);
            let raw = inner
                .secrets
                .get(&key)?
                .with_context(|| format!("no Proton credentials for {}", account.email))?;
            let creds: ProtonCreds = serde_json::from_str(&raw).context("Proton credentials")?;
            Ok(Backend::Imap(Box::new(Imap::new(creds.imap()))))
        }
        Provider::Microsoft => {
            let raw = inner
                .secrets
                .get(&format!("ms:{}", account.id))?
                .with_context(|| format!("no Microsoft tokens for {}", account.email))?;
            let tokens: Tokens = serde_json::from_str(&raw).context("Microsoft tokens")?;
            let client = graph(inner, &ms_client_id(inner)?, tokens);
            Ok(Backend::Graph {
                client: Box::new(client),
                saved: raw,
            })
        }
    }
}

fn persist_tokens(inner: &Inner, account: Id, backend: &mut Backend) -> Result<()> {
    if let Backend::Graph { client, saved } = backend {
        let current = serde_json::to_string(client.tokens())?;
        if current != *saved {
            inner.secrets.set(&format!("ms:{account}"), &current)?;
            *saved = current;
        }
    }
    Ok(())
}

fn graph(inner: &Inner, client_id: &str, tokens: Tokens) -> Graph {
    let base = inner
        .opts
        .graph_base_url
        .clone()
        .unwrap_or_else(|| DEFAULT_GRAPH.into());
    let authority = inner
        .opts
        .graph_authority
        .clone()
        .unwrap_or_else(|| DEFAULT_AUTHORITY.into());
    Graph::new(&base, &authority, client_id, tokens)
}

fn ms_client_id(inner: &Inner) -> Result<String> {
    if let Some(id) = std::env::var("KITE_MS_CLIENT_ID")
        .ok()
        .filter(|id| !id.trim().is_empty())
    {
        return Ok(id);
    }
    let path = inner.dir.join("config.json");
    let text = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "set KITE_MS_CLIENT_ID or add ms_client_id to {}",
            path.display()
        )
    })?;
    serde_json::from_str::<serde_json::Value>(&text)?
        .get("ms_client_id")
        .and_then(|v| v.as_str())
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .context("config.json has no ms_client_id")
}

// ----------------------------------------------------------------------- MIME

fn build_mime(inner: &Inner, account: &Account, draft: &Draft) -> Result<Vec<u8>> {
    use mail_builder::MessageBuilder;
    let reply = match draft.reply_to {
        Some(id) => inner.read(|s| s.reply_context(id))?,
        None => None,
    };
    let subject = match (draft.subject.trim(), &reply) {
        ("", Some(ctx)) => ctx.subject.clone(),
        _ => draft.subject.clone(),
    };
    let mut builder = MessageBuilder::new()
        .from((account.name.as_str(), account.email.as_str()))
        .subject(subject.as_str())
        .message_id(message_id(&account.email))
        .date(auth::now_ms() / 1000)
        .text_body(draft.body.as_str());
    if !draft.to.is_empty() {
        builder = builder.to(pairs(&draft.to));
    }
    if !draft.cc.is_empty() {
        builder = builder.cc(pairs(&draft.cc));
    }
    if !draft.bcc.is_empty() {
        builder = builder.bcc(pairs(&draft.bcc));
    }
    if let Some(ctx) = &reply {
        if !ctx.message_id_header.is_empty() {
            builder = builder.in_reply_to(ctx.message_id_header.as_str());
        }
        if !ctx.references.is_empty() {
            builder = builder.references(ctx.references.clone());
        }
    }
    Ok(builder.write_to_vec()?)
}

fn pairs(addrs: &[Addr]) -> Vec<(&str, &str)> {
    addrs
        .iter()
        .map(|a| (a.name.as_str(), a.email.as_str()))
        .collect()
}

/// A bare RFC 5322 id; `mail-builder` adds the angle brackets.
fn message_id(email: &str) -> String {
    let mut bytes = [0u8; 16];
    let _ = getrandom::fill(&mut bytes);
    let mut id = String::with_capacity(40);
    for byte in bytes {
        let _ = write!(id, "{byte:02x}");
    }
    let domain = email.split('@').nth(1).unwrap_or("kite.local");
    format!("{id}@{domain}")
}

// ----------------------------------------------------- blocking async bridge

/// Runs a future to completion from synchronous `Mail` code. The UI calls these from a background
/// executor (never from inside a tokio runtime), and they are rare: setup, calendar and files.
fn run_async<T>(future: impl Future<Output = Result<T>>) -> Result<T> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(future)
}

// --------------------------------------------------------------------- Mail

impl Mail for Engine {
    fn accounts(&self) -> Result<Vec<Account>> {
        self.inner.read(Store::accounts)
    }

    fn add_proton(&self, login: ProtonLogin) -> Result<Account> {
        let creds = ProtonCreds {
            host: login.host.unwrap_or_else(|| "127.0.0.1".into()),
            imap_port: login.imap_port.unwrap_or(1143),
            smtp_port: login.smtp_port.unwrap_or(1025),
            user: login.email.clone(),
            password: login.password,
            tls: self.inner.opts.proton_tls,
        };
        run_async(Imap::new(creds.imap()).verify())?;
        let name = login
            .email
            .split('@')
            .next()
            .unwrap_or(&login.email)
            .to_string();
        let account = self
            .inner
            .write(|s| s.add_account(Provider::Proton, &login.email, &name))?;
        let key = format!("proton:{}", account.id);
        self.inner
            .secrets
            .set(&key, &serde_json::to_string(&creds)?)?;
        self.wake();
        Ok(account)
    }

    fn add_microsoft(&self) -> Result<Account> {
        let client_id = ms_client_id(&self.inner)?;
        let tokens = run_async(auth::ms_login(&client_id))?;
        let mut client = graph(&self.inner, &client_id, tokens);
        let (email, name) = run_async(client.me())?;
        let account = self
            .inner
            .write(|s| s.add_account(Provider::Microsoft, &email, &name))?;
        let key = format!("ms:{}", account.id);
        self.inner
            .secrets
            .set(&key, &serde_json::to_string(client.tokens())?)?;
        self.wake();
        Ok(account)
    }

    fn remove_account(&self, id: Id) -> Result<()> {
        let key = match self.inner.read(|s| s.account_provider(id))? {
            Some(Provider::Proton) => Some(format!("proton:{id}")),
            Some(Provider::Microsoft) => Some(format!("ms:{id}")),
            None => None,
        };
        if let Some(key) = key {
            self.inner.secrets.del(&key)?;
        }
        self.inner.write(|s| s.remove_account(id))?;
        self.wake();
        Ok(())
    }

    fn threads(&self, view: &View, offset: u32, limit: u32) -> Result<Vec<ThreadSummary>> {
        self.inner.read(|s| s.threads(view, offset, limit))
    }

    fn thread(&self, id: Id) -> Result<Vec<Message>> {
        self.inner.read(|s| s.thread(id))
    }

    fn act(&self, ids: &[Id], action: &Action) -> Result<i64> {
        let token = self.inner.write(|s| s.act(ids, action))?;
        self.wake();
        Ok(token)
    }

    fn undo(&self, token: i64) -> Result<()> {
        self.inner.write(|s| s.undo(token))?;
        self.wake();
        Ok(())
    }

    fn send(&self, draft: Draft) -> Result<Id> {
        let seconds = self.inner.read(Store::settings)?.undo_send_secs;
        let due = draft
            .send_at
            .unwrap_or_else(|| auth::now_ms() + i64::from(seconds) * 1000);
        let id = self.inner.write(|s| s.outbox_add(&draft, due))?;
        self.wake();
        Ok(id)
    }

    fn cancel_send(&self, id: Id) -> Result<Option<Draft>> {
        self.inner.write(|s| s.outbox_cancel(id))
    }

    fn contacts(&self, query: &str) -> Result<Vec<Addr>> {
        self.inner.read(|s| s.contacts(query, 20))
    }

    fn open_attachment(&self, id: Id) -> Result<()> {
        let (account_id, message, attachment, name) = self
            .inner
            .read(|s| s.attachment(id))?
            .context("no such attachment")?;
        let account = self
            .inner
            .read(Store::accounts)?
            .into_iter()
            .find(|a| a.id == account_id)
            .context("account was removed")?;
        let mut backend = backend_for(&self.inner, &account)?;
        let bytes = run_async(backend.attachment(&message, &attachment))?;
        persist_tokens(&self.inner, account_id, &mut backend)?;
        let dir = std::env::temp_dir().join("kite");
        std::fs::create_dir_all(&dir)?;
        let file = dir.join(
            Path::new(&name)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("attachment"),
        );
        std::fs::write(&file, &bytes)?;
        open_path(&file)
    }

    fn splits(&self) -> Result<Vec<Split>> {
        self.inner.read(Store::splits)
    }

    fn set_splits(&self, splits: Vec<Split>) -> Result<()> {
        self.inner.write(|s| s.set_splits(splits))
    }

    fn settings(&self) -> Result<Settings> {
        self.inner.read(Store::settings)
    }

    fn set_settings(&self, settings: Settings) -> Result<()> {
        self.inner.write(|s| s.set_settings(settings))
    }

    fn set_ai_key(&self, key: &str) -> Result<()> {
        self.inner.secrets.set("ai_key", key)
    }

    fn calendar_today(&self, account: Id) -> Result<Vec<CalEvent>> {
        if self.inner.read(|s| s.account_provider(account))? != Some(Provider::Microsoft) {
            return Ok(Vec::new());
        }
        let account = self
            .inner
            .read(Store::accounts)?
            .into_iter()
            .find(|a| a.id == account)
            .context("account was removed")?;
        let mut backend = backend_for(&self.inner, &account)?;
        let offset = Local::now().offset().local_minus_utc() / 60;
        let events = run_async(async {
            match &mut backend {
                Backend::Graph { client, .. } => client.calendar_today(offset).await,
                Backend::Imap(_) => Ok(Vec::new()),
            }
        })?;
        // A calendar call can refresh the access token; keep the keychain current.
        persist_tokens(&self.inner, account.id, &mut backend)?;
        Ok(events)
    }

    fn ai(&self, req: u32, messages: Vec<ChatMsg>) -> Result<()> {
        let settings = self.inner.read(Store::settings)?;
        if !settings.ai_enabled {
            bail!("AI is disabled in settings");
        }
        let key = self
            .inner
            .secrets
            .get("ai_key")?
            .context("no AI key configured")?;
        let inner = self.inner.clone();
        std::thread::Builder::new()
            .name("kite-ai".into())
            .spawn(move || {
                if let Err(e) = crate::ai::stream(&settings, &key, req, messages, &inner) {
                    inner.emit(Event::Error(format!("AI: {e:#}")));
                }
            })
            .context("start AI stream")?;
        Ok(())
    }

    fn sync_now(&self) {
        self.wake();
    }

    fn events(&self) -> Receiver<Event> {
        let (tx, rx) = async_channel::unbounded();
        lock(&self.inner.senders).push(tx);
        rx
    }
}

#[cfg(target_os = "windows")]
fn open_path(path: &Path) -> Result<()> {
    let (program, args) = ("rundll32", &["url.dll,FileProtocolHandler"][..]);
    launch(program, args, path)
}

#[cfg(target_os = "macos")]
fn open_path(path: &Path) -> Result<()> {
    launch("open", &[], path)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn open_path(path: &Path) -> Result<()> {
    launch("xdg-open", &[], path)
}

fn launch(program: &str, args: &[&str], path: &Path) -> Result<()> {
    std::process::Command::new(program)
        .args(args)
        .arg(path)
        .spawn()
        .context("open attachment")?;
    Ok(())
}
