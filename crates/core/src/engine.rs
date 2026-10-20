//! The production `Mail`: local SQLite reads, optimistic local writes, and one background thread
//! running a tokio runtime that drains pending ops, syncs accounts, wakes snoozes and sends the
//! outbox. Engine -> UI traffic goes over the `Event` channel; the UI never sees tokio or SQLite.
use rand::Rng;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt::Write as _;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use async_channel::{Receiver, Sender};
use chrono::Local;

use crate::api::Mail;
use crate::auth::{self, Tokens};
use crate::model::*;
use crate::store::Store;
use crate::sync::graph::Graph;
use crate::sync::proton::{self, Credentials as ProtonSession, Proton, SignIn, Step as ProtonStep};
use crate::sync::{Delta, Op};

const DEFAULT_GRAPH: &str = "https://graph.microsoft.com/v1.0";
const DEFAULT_AUTHORITY: &str = "https://login.microsoftonline.com/common";
/// Proton's API. The native backend talks to it directly, so free accounts work.
const DEFAULT_PROTON_API: &str = "https://mail.proton.me/api";
/// Proton's API only accepts `<platform>-<product>@<version>` for Proton's own products
/// (`kite-mail@…` and `windows-kite@…` both fail with code 2064), so naming a product would
/// mean impersonating Bridge or a Proton app. `Other` is the generic value third-party clients
/// (hydroxide, ferroxide) send; Kite names itself honestly in the User-Agent instead.
const PROTON_APP_VERSION: &str = "Other";
/// Messages per transaction: a UI write never waits behind a long first sync (see ARCHITECTURE).
const APPLY_CHUNK: usize = 200;
/// Sync calls per account per tick, so one huge mailbox cannot monopolise the loop.
const SYNC_STEPS: usize = 64;

/// Checks supported mailbox syntax only; it does not establish mailbox existence.
/// Display names are separate from the address and may contain Unicode.
pub fn valid_recipient_address(addr: &Addr) -> bool {
    let email = &addr.email;
    if addr.name.chars().any(char::is_control)
        || email.chars().any(char::is_control)
        || email.len() > 254
    {
        return false;
    }
    let Some((local, domain)) = email.rsplit_once('@') else {
        return false;
    };
    let local_valid = if local.starts_with('"') && local.ends_with('"') && local.len() >= 2 {
        let mut escaped = false;
        let valid = local[1..local.len() - 1].bytes().all(|b| {
            if escaped {
                escaped = false;
                (32..=126).contains(&b)
            } else if b == b'\\' {
                escaped = true;
                true
            } else {
                (32..=126).contains(&b) && b != b'"'
            }
        });
        valid && !escaped
    } else {
        local.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-/=?^_`{|}~".contains(&b))
        })
    };
    local.len() <= 64
        && local_valid
        && domain.len() <= 253
        && domain.contains('.')
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

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
    pub proton_api_url: Option<String>,
    pub poll: Duration,
    /// Fetch remote images from loopback/private addresses. Tests set true to reach a local fake.
    pub local_images: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            graph_base_url: None,
            graph_authority: None,
            proton_api_url: None,
            poll: Duration::from_secs(30),
            local_images: false,
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
    /// The native Proton sign-in between calls, so the UI only has to answer one step.
    sign_in: Mutex<Option<SignIn>>,
    wake: Sender<bool>,
    last_refresh: Mutex<Option<Instant>>,
    secrets: Secrets,
    opts: Options,
    /// Every `Event::Error`, so a failure that only flashed by as a toast can be read later.
    errors_log: PathBuf,
    /// The data dir the store was opened in; avatars live under `<dir>/avatars/`.
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
        if let Event::Error(message) = &event {
            self.log_error(message);
        }
        let mut senders = lock(&self.senders);
        senders.retain(|s| !s.is_closed());
        for s in senders.iter() {
            let _ = s.try_send(event.clone());
        }
    }

    /// Best effort: logging must never turn into a second failure. Rolls over at 1 MB.
    fn log_error(&self, message: &str) {
        use std::io::Write as _;
        let path = &self.errors_log;
        if std::fs::metadata(path).is_ok_and(|m| m.len() > 1 << 20) {
            let _ = std::fs::rename(path, path.with_extension("log.old"));
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(
                file,
                "{}  {message}",
                Local::now().format("%Y-%m-%d %H:%M:%S")
            );
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

    /// Adds an account with its secret. An account whose secret could not be stored can never
    /// sync, so a new one is removed again rather than left behind.
    fn add_account(
        &self,
        provider: Provider,
        email: &str,
        name: &str,
        secret: &str,
    ) -> Result<Account> {
        let before = self.inner.read(Store::accounts)?;
        let account = self.inner.write(|s| s.add_account(provider, email, name))?;
        let key = match provider {
            Provider::Proton => format!("proton:{}", account.id),
            Provider::Microsoft => format!("ms:{}", account.id),
        };
        if let Err(e) = self.inner.secrets.set(&key, secret) {
            // Signing in again to an existing account must never delete its mail.
            if !before.iter().any(|a| a.id == account.id) {
                self.inner.write(|s| s.remove_account(account.id))?;
            }
            return Err(e);
        }
        self.wake();
        Ok(account)
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
            sign_in: Mutex::new(None),
            last_refresh: Mutex::new(None),
            wake,
            secrets,
            opts,
            errors_log: dir.join("errors.log"),
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
        let _ = inner.wake.try_send(false);
        Ok(Arc::new(Engine { inner }))
    }

    fn wake(&self) {
        let _ = self.inner.wake.try_send(false);
    }

    /// Emits `Changed` for every account after a local mutation that queues no remote op (labels,
    /// tag edits and their undos), so the UI refetches instead of going stale.
    fn emit_changed(&self) -> Result<()> {
        let ids = self
            .inner
            .read(Store::accounts)?
            .into_iter()
            .map(|a| a.id)
            .collect();
        self.inner.emit(Event::Changed(ids));
        Ok(())
    }

    /// Turns a sign-in step into a `ProtonAuth`, keeping the half-finished handshake when
    /// the caller has more to answer.
    fn settle(&self, mut sign_in: SignIn, step: ProtonStep) -> Result<ProtonAuth> {
        match step {
            ProtonStep::TwoFactor => {
                *lock(&self.inner.sign_in) = Some(sign_in);
                Ok(ProtonAuth::TwoFactor)
            }
            ProtonStep::MailboxPassword => {
                *lock(&self.inner.sign_in) = Some(sign_in);
                Ok(ProtonAuth::MailboxPassword)
            }
            ProtonStep::HumanVerification { url } => {
                *lock(&self.inner.sign_in) = Some(sign_in);
                Ok(ProtonAuth::HumanVerification { url })
            }
            ProtonStep::Done => {
                let signed = sign_in
                    .take()
                    .context("Proton sign-in returned no session")?;
                let account = self.add_account(
                    Provider::Proton,
                    &signed.email,
                    &signed.name,
                    &serde_json::to_string(&signed.credentials)?,
                )?;
                Ok(ProtonAuth::Done(account))
            }
        }
    }
}

// ------------------------------------------------------------------- the loop

async fn run(weak: Weak<Inner>, wake: Receiver<bool>) {
    let poll = match weak.upgrade() {
        Some(inner) => inner.opts.poll,
        None => return,
    };
    let mut backends: HashMap<Id, Backend> = HashMap::new();
    let mut backoff: HashMap<Id, (u32, Ms, bool)> = HashMap::new();
    let mut next_sync: HashMap<Id, Ms> = HashMap::new();
    loop {
        let delay = next_sync
            .values()
            .chain(backoff.values().map(|(_, at, _)| at))
            .map(|at| at.saturating_sub(auth::now_ms()).max(1) as u64)
            .min()
            .map(Duration::from_millis)
            .unwrap_or(poll)
            .min(poll);
        let mut forced = false;
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            result = wake.recv() => {
                let Ok(refresh) = result else { return; };
                forced = refresh;
                while let Ok(refresh) = wake.try_recv() { forced |= refresh; }
            }
        }
        let Some(inner) = weak.upgrade() else {
            return;
        };
        tick(&inner, &mut backends, &mut backoff, &mut next_sync, forced).await;
    }
}

async fn tick(
    inner: &Inner,
    backends: &mut HashMap<Id, Backend>,
    backoff: &mut HashMap<Id, (u32, Ms, bool)>,
    next_sync: &mut HashMap<Id, Ms>,
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
    next_sync.retain(|id, _| live.contains(id));

    let mut changed: Vec<Id> = Vec::new();
    for account in &accounts {
        if backoff
            .get(&account.id)
            .is_some_and(|(_, next, _)| now < *next)
        {
            continue;
        }
        let sync = forced
            || backoff.contains_key(&account.id)
            || next_sync.get(&account.id).is_none_or(|at| now >= *at);
        match sync_account(inner, account, backends, sync).await {
            Ok(any) => {
                backoff.remove(&account.id);
                if sync {
                    let native = matches!(backends.get(&account.id), Some(Backend::Proton { .. }));
                    let delay = if native && inner.opts.poll == Duration::from_secs(30) {
                        25_000 + rand::thread_rng().gen_range(0..=15_000)
                    } else {
                        inner.opts.poll.as_millis().min(i64::MAX as u128) as i64
                    };
                    next_sync.insert(account.id, auth::now_ms() + delay);
                }
                if any {
                    changed.push(account.id);
                }
            }
            Err(e) => {
                let entry = backoff.entry(account.id).or_insert((0, 0, false));
                let attempts = entry.0 + 1;
                let wait = (1_000i64 << attempts.min(9)).min(600_000);
                *entry = (
                    attempts,
                    (auth::now_ms() + wait).max(proton::retry_at(&e).unwrap_or(0)),
                    proton::retry_at(&e).is_some(),
                );
                inner.emit(Event::Error(format!("{}: {e:#}", account.email)));
            }
        }
    }

    match inner.write(|s| s.wake_due(now)) {
        Ok(ids) => changed.extend(ids),
        Err(e) => inner.emit(Event::Error(format!("{e:#}"))),
    }
    if let Err(e) = send_due(inner, &accounts, backends, backoff, next_sync, &mut changed).await {
        inner.emit(Event::Error(format!("{e:#}")));
    }

    changed.sort_unstable();
    changed.dedup();
    if !changed.is_empty() {
        inner.emit(Event::Changed(changed));
    }
}

/// Drains queued ops, then syncs until backfill is exhausted. Synced batches are announced as they
/// land; the return value says whether drained ops changed the store (announced by the tick).
async fn sync_account(
    inner: &Inner,
    account: &Account,
    backends: &mut HashMap<Id, Backend>,
    sync: bool,
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
            let op = inner.write(|s| s.address_op(op_id, op))?;
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

    if !sync {
        return Ok(changed);
    }
    let mut cursor = inner.write(|s| s.cursor(account.id))?;
    for _ in 0..SYNC_STEPS {
        let backend = backend_mut(backends, account.id)?;
        if let Backend::Proton { client, .. } = backend {
            client.set_known(inner.read(|s| s.remote_ids(account.id))?);
        }
        let result = backend.sync(cursor.clone()).await;
        if let Backend::Proton { client, .. } = backend {
            for warning in client.take_warnings() {
                inner.emit(Event::Error(warning));
            }
        }
        let (deltas, next) = result?;
        persist_tokens(inner, account.id, backend)?;
        if !deltas.is_empty() {
            for chunk in deltas.chunks(APPLY_CHUNK) {
                inner.write(|s| s.apply_deltas(account.id, chunk))?;
            }
            // Show mail as it lands: a first sync can run many steps before the tick ends.
            inner.emit(Event::Changed(vec![account.id]));
        }
        // Keep going only while the cursor advances (backfill/paging).
        let advanced = cursor.as_deref() != Some(next.as_str());
        inner.write(|s| s.set_cursor(account.id, &next))?;
        let pending_walk = matches!(backend, Backend::Proton { .. }) && Proton::has_more(&next);
        cursor = Some(next);
        if !advanced
            || (matches!(backend, Backend::Proton { .. }) && !pending_walk)
            || (deltas.is_empty() && !pending_walk)
        {
            break;
        }
    }

    // Proton's custom labels are folders and exist before Kite ever created one, so the
    // sidebar catalog is refreshed from the API (cached after the first call).
    let backend = backend_mut(backends, account.id)?;
    for (id, name) in backend.folders().await? {
        let existed = inner.read(|s| s.folder_exists(account.id, &id))?;
        inner.write(|s| s.ensure_folder(account.id, &id, &name))?;
        changed |= !existed;
    }
    if let Backend::Graph { client, .. } = backend {
        changed |= fetch_photo(inner, account.id, client).await;
    }
    Ok(changed)
}

/// `GET /me/photo/$value`, at most once a day per account (tracked in the store, so a
/// restart does not re-ask). A 404 or a failed fetch never fails the sync; errors just
/// retry on the next pass.
async fn fetch_photo(inner: &Inner, account: Id, client: &mut Graph) -> bool {
    const DAY: Ms = 86_400_000;
    let checked = inner.read(|s| s.photo_checked(account)).ok().flatten();
    if checked.is_some_and(|at| auth::now_ms() - at < DAY) {
        return false;
    }
    let bytes = match client.photo().await {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    let _ = inner.write(|s| s.set_photo_checked(account, auth::now_ms()));
    let Some(png) = bytes.and_then(|bytes| avatar_png(&bytes)) else {
        return false;
    };
    write_avatar(&inner.dir.join("avatars"), account, "-provider", &png)
}

/// Decodes an image, centre-crops it to a square and downsizes it to 128×128 PNG.
/// Avatars are always this shape, so the chosen-image picker and the provider
/// photo fetch share the one pipeline.
pub fn avatar_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory(bytes).ok()?;
    let image = image.resize_to_fill(128, 128, image::imageops::FilterType::Lanczos3);
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// Atomically writes `<dir>/<id><suffix>.png` (suffix "" for the user's choice,
/// "-provider" for the fetched photo).
fn write_avatar(dir: &Path, account: Id, suffix: &str, png: &[u8]) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let file = dir.join(format!("{account}{suffix}.png"));
    let tmp = dir.join(format!("{account}{suffix}.tmp"));
    std::fs::write(&tmp, png)
        .and_then(|()| std::fs::rename(&tmp, &file))
        .is_ok()
}

async fn send_due(
    inner: &Inner,
    accounts: &[Account],
    backends: &mut HashMap<Id, Backend>,
    backoff: &mut HashMap<Id, (u32, Ms, bool)>,
    next_sync: &mut HashMap<Id, Ms>,
    changed: &mut Vec<Id>,
) -> Result<()> {
    let due = inner.write(|s| s.outbox_due(auth::now_ms()))?;
    let mut queued = false;
    for (outbox_id, draft) in due {
        if backoff
            .get(&draft.account_id)
            .is_some_and(|(_, at, throttled)| *throttled && auth::now_ms() < *at)
        {
            continue;
        }
        let Some(account) = accounts.iter().find(|a| a.id == draft.account_id) else {
            inner.write(|s| s.outbox_sent(outbox_id))?;
            continue;
        };
        let outgoing_id = message_id(&account.email);
        let mime = match build_mime(inner, account, &draft, &outgoing_id) {
            Ok(mime) => mime,
            Err(e) => {
                failed_send(inner, outbox_id, &draft, &e, true, changed)?;
                continue;
            }
        };
        if let Entry::Vacant(slot) = backends.entry(account.id) {
            slot.insert(backend_for(inner, account)?);
        }
        let backend = backend_mut(backends, account.id)?;
        let prepared = if let Backend::Graph { client, .. } = backend {
            match client.prepare_send(&mime).await {
                Ok(prepared) => Some(prepared),
                Err(e) => {
                    failed_send(
                        inner,
                        outbox_id,
                        &draft,
                        &e,
                        proton::permanent_send(&e),
                        changed,
                    )?;
                    continue;
                }
            }
        } else {
            None
        };
        let sent_message_id = prepared
            .as_ref()
            .map(|(_, id)| id.as_str())
            .unwrap_or(&outgoing_id);
        if let Some(id) = draft.id {
            inner.write(|s| {
                s.remember_sent_tags(
                    id,
                    account.id,
                    &format!(
                        "m:{}",
                        sent_message_id
                            .trim_matches(['<', '>'])
                            .to_ascii_lowercase()
                    ),
                )
            })?;
        }
        let result = match (backend, &prepared) {
            (Backend::Graph { client, .. }, Some((id, _))) => {
                client.send_prepared(id).await.map(|()| None)
            }
            (backend, _) => backend.apply(&Op::Send { mime }).await,
        };
        let backend = backend_mut(backends, account.id)?;
        match result {
            Ok(_) => {
                persist_tokens(inner, account.id, backend)?;
                if let Backend::Proton { client, .. } = backend
                    && let Some(message) = client.take_sent()
                {
                    inner.write(|s| {
                        if let Some(id) = draft.id {
                            s.remember_sent_tags(
                                id,
                                account.id,
                                &format!("r:{}", message.remote_id),
                            )?;
                        }
                        s.apply_deltas(account.id, &[Delta::Upsert(Box::new(message))])
                    })?;
                }
                inner.write(|s| s.outbox_sent(outbox_id))?;
                queued = true;
                if !matches!(backend, Backend::Proton { .. }) {
                    next_sync.remove(&account.id);
                }
                // The provider copy of a saved draft is stale now that it has been sent; delete it
                // on the next pass rather than leaving a duplicate in Drafts.
                if let Some(id) = draft.id
                    && let Some((account_id, remote)) = inner.write(|s| s.draft_sent(id))?
                {
                    inner.write(|s| {
                        s.draft_op(account_id, &Op::DeleteDraft { remote_id: remote }, None)
                    })?;
                }
                changed.push(account.id);
                if let (Some(at), Some(reply)) = (draft.remind_at, draft.reply_to)
                    && let Some(thread) = inner.read(|s| s.message_thread(reply))?
                {
                    inner.write(|s| s.remind_if_no_reply(thread, at))?;
                }
            }
            Err(e) => {
                if let Some(at) = proton::retry_at(&e) {
                    backoff.insert(account.id, (0, at, true));
                }
                failed_send(
                    inner,
                    outbox_id,
                    &draft,
                    &e,
                    proton::permanent_send(&e),
                    changed,
                )?;
            }
        }
    }
    if queued {
        // Sync the sent copy and drain draft deletions without waiting out the poll interval.
        let _ = inner.wake.try_send(false);
    }
    Ok(())
}

fn failed_send(
    inner: &Inner,
    id: Id,
    draft: &Draft,
    error: &anyhow::Error,
    permanent: bool,
    changed: &mut Vec<Id>,
) -> Result<()> {
    let (stopped, first) =
        inner.write(|s| s.outbox_failed(id, auth::now_ms(), permanent, proton::retry_at(error)))?;
    if stopped {
        if draft.id.is_none() {
            let account = inner
                .read(Store::accounts)?
                .into_iter()
                .find(|a| a.id == draft.account_id)
                .context("account was removed")?;
            inner.write(|s| s.save_draft(draft, &message_id(&account.email)))?;
        }
        inner.write(|s| s.outbox_cancel(id))?;
        changed.push(draft.account_id);
        inner.emit(Event::Error(format!(
            "Send stopped; message saved in Drafts: {error:#}"
        )));
    } else if first {
        inner.emit(Event::Error(format!(
            "send failed; queued for retry: {error:#}"
        )));
    }
    Ok(())
}

// ------------------------------------------------------------------- backends

enum Backend {
    /// `saved` is the token JSON last written to the keychain, so a refresh is persisted once.
    Graph { client: Box<Graph>, saved: String },
    /// Native Proton (API, not Bridge). Same reason for `saved`.
    Proton { client: Box<Proton>, saved: String },
}

impl Backend {
    async fn sync(&mut self, cursor: Option<String>) -> Result<(Vec<Delta>, String)> {
        match self {
            Backend::Graph { client, .. } => client.sync(cursor).await,
            Backend::Proton { client, .. } => client.sync(cursor).await,
        }
    }

    async fn apply(&mut self, op: &Op) -> Result<Option<String>> {
        match self {
            Backend::Graph { client, .. } => client.apply(op).await,
            Backend::Proton { client, .. } => client.apply(op).await,
        }
    }

    async fn attachment(&mut self, message: &str, attachment: &str) -> Result<Vec<u8>> {
        match self {
            Backend::Graph { client, .. } => client.attachment(message, attachment).await,
            Backend::Proton { client, .. } => client.attachment(message, attachment).await,
        }
    }

    /// Folders only the provider can enumerate. Graph folders reach the store
    /// through `create_folder`, so only Proton has anything to add.
    async fn folders(&mut self) -> Result<Vec<(String, String)>> {
        match self {
            Backend::Proton { client, .. } => client.folders().await,
            Backend::Graph { .. } => Ok(Vec::new()),
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
            let reconnect =
                "This Proton account used Bridge; sign in again in Settings › Connections";
            let session = ProtonSession::parse(&raw)
                .context(reconnect)?
                .context(reconnect)?;
            let client = Proton::new(
                &proton_api_url(inner),
                PROTON_APP_VERSION,
                &account.email,
                &session,
            );
            Ok(Backend::Proton {
                client: Box::new(client),
                saved: raw,
            })
        }
        Provider::Microsoft => {
            let raw = inner
                .secrets
                .get(&format!("ms:{}", account.id))?
                .with_context(|| format!("no Microsoft tokens for {}", account.email))?;
            let tokens: Tokens = serde_json::from_str(&raw).context("Microsoft tokens")?;
            let client = graph(inner, &ms_client_id(), tokens);
            Ok(Backend::Graph {
                client: Box::new(client),
                saved: raw,
            })
        }
    }
}

fn proton_api_url(inner: &Inner) -> String {
    inner
        .opts
        .proton_api_url
        .clone()
        .unwrap_or_else(|| DEFAULT_PROTON_API.to_string())
}

fn persist_tokens(inner: &Inner, account: Id, backend: &mut Backend) -> Result<()> {
    match backend {
        Backend::Graph { client, saved } => {
            let current = serde_json::to_string(client.tokens())?;
            if current != *saved {
                inner.secrets.set(&format!("ms:{account}"), &current)?;
                *saved = current;
            }
        }
        Backend::Proton { client, saved } => {
            // `Credentials` serialises field by field (its map is ordered), so a token
            // refresh shows up as a different string exactly once.
            let current = serde_json::to_string(&client.credentials())?;
            if current != *saved {
                inner.secrets.set(&format!("proton:{account}"), &current)?;
                *saved = current;
            }
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

/// The Microsoft app registration Kite signs in as. It identifies the app, not the mailbox, so
/// one registration serves every user; `KITE_MS_CLIENT_ID` overrides it in development.
fn ms_client_id() -> String {
    const BUILT_IN: &str = "a82018ad-1eb2-4c7c-b005-2144e4c61611";
    std::env::var("KITE_MS_CLIENT_ID")
        .map(|id| id.trim().to_string())
        .ok()
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| BUILT_IN.to_string())
}

// ----------------------------------------------------------------------- MIME

fn build_mime(inner: &Inner, account: &Account, draft: &Draft, msg_id: &str) -> Result<Vec<u8>> {
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
        .message_id(msg_id)
        .date(auth::now_ms() / 1000)
        .text_body(draft.body.as_str());
    if account.provider == Provider::Microsoft {
        builder = builder.html_body(crate::sync::graph::text_html(&draft.body));
    }
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

    fn avatar(&self, account: Id) -> Option<PathBuf> {
        let dir = self.inner.dir.join("avatars");
        // The user's choice always wins over the provider photo.
        [format!("{account}.png"), format!("{account}-provider.png")]
            .into_iter()
            .map(|name| dir.join(name))
            .find(|path| path.is_file())
    }

    fn save_avatar(&self, account: Id, png: &[u8]) -> Result<()> {
        anyhow::ensure!(
            write_avatar(&self.inner.dir.join("avatars"), account, "", png),
            "could not save the avatar image"
        );
        self.emit_changed()?;
        Ok(())
    }

    fn remove_avatar(&self, account: Id) -> Result<()> {
        let file = self
            .inner
            .dir
            .join("avatars")
            .join(format!("{account}.png"));
        match std::fs::remove_file(&file) {
            Ok(()) => self.emit_changed()?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    fn set_account_name(&self, id: Id, name: &str) -> Result<()> {
        self.inner.write(|s| s.set_account_name(id, name))?;
        self.emit_changed()?;
        Ok(())
    }

    fn add_microsoft(&self) -> Result<Account> {
        let client_id = ms_client_id();
        let tokens = run_async(auth::ms_login(&client_id))?;
        let mut client = graph(&self.inner, &client_id, tokens);
        let (email, name) = run_async(client.me())?;
        self.add_account(
            Provider::Microsoft,
            &email,
            &name,
            &serde_json::to_string(client.tokens())?,
        )
    }

    fn remove_account(&self, id: Id) -> Result<()> {
        let key = match self.inner.read(|s| s.account_provider(id))? {
            Some(Provider::Proton) => Some(format!("proton:{id}")),
            Some(Provider::Microsoft) => Some(format!("ms:{id}")),
            None => None,
        };
        if let Some(key) = key {
            // A native Proton session must be revoked, not just forgotten locally.
            if let Some(raw) = self.inner.secrets.get(&key)?
                && let Some(session) = ProtonSession::parse(&raw)?
                && let Ok(account) = self
                    .inner
                    .read(Store::accounts)
                    .map(|all| all.into_iter().find(|a| a.id == id))
                && let Some(account) = account
            {
                let mut client = Proton::new(
                    &proton_api_url(&self.inner),
                    PROTON_APP_VERSION,
                    &account.email,
                    &session,
                );
                let _ = run_async(async {
                    client.logout().await;
                    Ok(())
                });
            }
            self.inner.secrets.del(&key)?;
        }
        self.inner.write(|s| s.remove_account(id))?;
        self.wake();
        Ok(())
    }

    fn proton_sign_in(&self, email: &str, password: &str) -> Result<ProtonAuth> {
        anyhow::ensure!(!email.trim().is_empty(), "Enter your Proton address.");
        anyhow::ensure!(!password.is_empty(), "Enter your Proton password.");
        let mut sign_in = SignIn::new(
            &proton_api_url(&self.inner),
            PROTON_APP_VERSION,
            email,
            password,
        );
        let step = run_async(sign_in.start())?;
        self.settle(sign_in, step)
    }

    fn proton_sign_in_step(&self, answer: &str) -> Result<ProtonAuth> {
        let mut sign_in = lock(&self.inner.sign_in)
            .take()
            .context("No Proton sign-in in progress")?;
        let step = match run_async(sign_in.step(answer)) {
            Ok(step) => step,
            Err(error) => {
                // The answer was wrong, but the rest of the handshake is still valid: the
                // next answer resumes at the same step instead of starting over.
                *lock(&self.inner.sign_in) = Some(sign_in);
                return Err(error);
            }
        };
        self.settle(sign_in, step)
    }

    fn folders(&self) -> Result<Vec<Folder>> {
        self.inner.read(Store::folders)
    }

    fn create_folder(&self, account: Id, name: &str) -> Result<Folder> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "Enter a folder name.");
        anyhow::ensure!(
            !self
                .inner
                .read(Store::folders)?
                .iter()
                .any(|f| f.account_id == account && f.name.eq_ignore_ascii_case(name)),
            "A folder with this name already exists in that account."
        );
        let account_row = self
            .inner
            .read(Store::accounts)?
            .into_iter()
            .find(|a| a.id == account)
            .with_context(|| format!("no account {account}"))?;
        let mut backend = backend_for(&self.inner, &account_row)?;
        // Contact the provider first and only persist after it accepted the folder, so a failed
        // create never leaves a phantom folder in the local catalog.
        let id = run_async(async {
            match &mut backend {
                Backend::Graph { client, .. } => client.create_folder(name).await,
                Backend::Proton { client, .. } => client.create_folder(name).await,
            }
        })?;
        persist_tokens(&self.inner, account, &mut backend)?;
        let folder = self.inner.write(|s| s.add_folder(account, &id, name))?;
        self.wake();
        Ok(folder)
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
        if matches!(action, Action::Label { .. } | Action::Unlabel { .. }) {
            self.emit_changed()?;
        }
        Ok(token)
    }

    fn undo(&self, token: i64) -> Result<()> {
        self.inner.write(|s| s.undo(token))?;
        self.wake();
        self.emit_changed()?;
        Ok(())
    }

    fn send(&self, draft: Draft) -> Result<Id> {
        anyhow::ensure!(
            draft
                .to
                .iter()
                .chain(&draft.cc)
                .chain(&draft.bcc)
                .any(|a| !a.email.trim().is_empty()),
            "Add a recipient before sending."
        );
        for (field, recipients) in [("To", &draft.to), ("Cc", &draft.cc), ("Bcc", &draft.bcc)] {
            anyhow::ensure!(
                recipients.iter().all(valid_recipient_address),
                "{field}: correct the recipient address (use name@example.com) before sending."
            );
        }
        let seconds = self.inner.read(Store::settings)?.undo_send_secs;
        let due = draft
            .send_at
            .unwrap_or_else(|| auth::now_ms() + i64::from(seconds) * 1000);
        let saved = draft.id;
        let account = draft.account_id;
        let id = self.inner.write(|s| s.outbox_add(&draft, due))?;
        self.wake();
        if saved.is_some() {
            // It left the Drafts mailbox; tell the list right away.
            self.inner.emit(Event::Changed(vec![account]));
        }
        Ok(id)
    }

    fn cancel_send(&self, id: Id) -> Result<Option<Draft>> {
        let draft = self.inner.write(|s| s.outbox_cancel(id))?;
        if let Some(draft) = &draft
            && draft.id.is_some()
        {
            // The cancelled draft is back in the Drafts mailbox.
            self.inner.emit(Event::Changed(vec![draft.account_id]));
        }
        Ok(draft)
    }

    fn save_draft(&self, draft: Draft) -> Result<Id> {
        let account = self
            .inner
            .read(Store::accounts)?
            .into_iter()
            .find(|a| a.id == draft.account_id)
            .context("account was removed")?;
        // A draft keeps its Message-ID across edits so the provider's own copy never becomes a
        // second conversation.
        let msg_id = match draft.id {
            Some(id) => self
                .inner
                .read(|s| s.draft_msg_id(id))?
                .unwrap_or_else(|| message_id(&account.email)),
            None => message_id(&account.email),
        };
        let mime = build_mime(&self.inner, &account, &draft, &msg_id)?;
        let (id, replaces) = self.inner.write(|s| s.save_draft(&draft, &msg_id))?;
        self.inner
            .write(|s| s.draft_op(account.id, &Op::SaveDraft { mime, replaces }, Some(id)))?;
        self.wake();
        self.inner.emit(Event::Changed(vec![account.id]));
        Ok(id)
    }

    fn draft(&self, thread: Id) -> Result<Option<Draft>> {
        // Adopting a draft written in another client writes, so this takes the writer.
        self.inner.write(|s| s.draft(thread))
    }

    fn discard_draft(&self, id: Id) -> Result<()> {
        let remote = self.inner.write(|s| s.discard_draft(id))?;
        if let Some((account, remote_id)) = remote {
            self.inner
                .write(|s| s.draft_op(account, &Op::DeleteDraft { remote_id }, None))?;
            self.wake();
        }
        self.emit_changed()?;
        Ok(())
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

    fn fetch_image(&self, url: &str) -> Result<Vec<u8>> {
        const MAX: usize = 4 << 20;
        let url = reqwest::Url::parse(url).context("image URL")?;
        anyhow::ensure!(
            matches!(url.scheme(), "https" | "http"),
            "Remote images must be http(s)"
        );
        // Mail must not be able to make Kite probe the reader's own machine or LAN.
        let host = url.host_str().unwrap_or_default();
        let local = match host.trim_matches(['[', ']']).parse::<std::net::IpAddr>() {
            Ok(std::net::IpAddr::V4(ip)) => {
                ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified()
            }
            Ok(std::net::IpAddr::V6(ip)) => {
                let first = ip.segments()[0];
                ip.is_loopback()
                    || ip.is_unspecified()
                    || (first & 0xfe00) == 0xfc00
                    || (first & 0xffc0) == 0xfe80
            }
            Err(_) => {
                host.is_empty()
                    || host.eq_ignore_ascii_case("localhost")
                    || host.to_ascii_lowercase().ends_with(".localhost")
            }
        };
        anyhow::ensure!(
            !local || self.inner.opts.local_images,
            "Remote images on local addresses are not loaded"
        );
        run_async(async move {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()?;
            let mut res = client.get(url).send().await.context("fetch image")?;
            anyhow::ensure!(
                res.status().is_success(),
                "image request failed ({})",
                res.status()
            );
            // Stop at the cap rather than buffering an arbitrarily large body first.
            let mut bytes = Vec::new();
            while let Some(chunk) = res.chunk().await? {
                bytes.extend_from_slice(&chunk);
                anyhow::ensure!(bytes.len() <= MAX, "image is too large to display");
            }
            Ok(bytes)
        })
    }

    fn splits(&self) -> Result<Vec<Split>> {
        self.inner.read(Store::splits)
    }

    fn set_splits(&self, splits: Vec<Split>) -> Result<()> {
        self.inner.write(|s| s.set_splits(splits))
    }

    fn tags(&self) -> Result<Vec<Tag>> {
        self.inner.read(Store::tags)
    }

    fn set_tags(&self, tags: Vec<Tag>) -> Result<()> {
        self.inner.write(|s| s.set_tags(tags))?;
        self.emit_changed()?;
        Ok(())
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
                Backend::Proton { .. } => Ok(Vec::new()),
            }
        })?;
        // A calendar call can refresh the access token; keep the keychain current.
        persist_tokens(&self.inner, account.id, &mut backend)?;
        Ok(events)
    }

    fn ai(&self, req: u32, request: AiRequest) -> Result<()> {
        let settings = self.inner.read(Store::settings)?;
        if !settings.ai_enabled {
            bail!("AI is disabled in settings");
        }
        self.inner
            .read(|store| crate::ai::prepare(store, &request).map(|_| ()))?;
        let key = self
            .inner
            .secrets
            .get("ai_key")?
            .context("no AI key configured")?;
        let inner = self.inner.clone();
        std::thread::Builder::new()
            .name("kite-ai".into())
            .spawn(move || {
                // Recheck after queueing: the user may have marked the source Private.
                let result = inner
                    .read(|store| crate::ai::prepare(store, &request))
                    .and_then(|messages| crate::ai::stream(&settings, &key, req, messages, &inner));
                if let Err(e) = result {
                    inner.emit(Event::Ai {
                        req,
                        delta: String::new(),
                        done: true,
                    });
                    inner.emit(Event::Error(format!("AI: {e:#}")));
                }
            })
            .context("start AI stream")?;
        Ok(())
    }

    fn sync_now(&self) {
        let mut last = lock(&self.inner.last_refresh);
        if last.is_some_and(|at| at.elapsed() < Duration::from_secs(10)) {
            return;
        }
        *last = Some(Instant::now());
        let _ = self.inner.wake.try_send(true);
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
