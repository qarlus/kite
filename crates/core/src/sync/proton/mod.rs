//! Native Proton Mail backend: Proton's HTTP API directly, no Bridge, no IMAP.
//!
//! Ported from Proton's own MIT-licensed `go-proton-api` (routes, payloads, label
//! semantics) and `go-srp` (login). Proton has no per-folder message copies: a message
//! carries a set of label ids, and the mailbox Kite shows is derived from them, so the
//! first sync walks the metadata of every system label and the live sync follows
//! `/core/v4/events` with the event id as the cursor.
//!
//! Bodies are fetched (and decrypted) one message at a time, like the IMAP backend
//! fetches `BODY.PEEK[]`: `RemoteMsg` carries the text, so a delta always arrives with
//! its content.
mod api;
mod crypto;
pub mod srp;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use mail_parser::{Message as ParsedMessage, MessageParser};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{Delta, Op, RemoteAttachment, RemoteMsg};
use crate::model::{Addr, Mailbox};
use api::Api;
use crypto::Keyring;

/// Proton's system labels (`go-proton-api/label_types.go`).
const INBOX: &str = "0";
const ALL_DRAFTS: &str = "1";
const ALL_SENT: &str = "2";
const TRASH: &str = "3";
const SPAM: &str = "4";
const ALL_MAIL: &str = "5";
const ARCHIVE: &str = "6";
const SENT: &str = "7";
const DRAFTS: &str = "8";
const STARRED: &str = "10";
/// Mailboxes the first sync walks. Sent, archive and custom labels all live under
/// "all mail"; drafts, spam and trash do not.
const BACKFILL: [&str; 4] = [ALL_MAIL, ALL_DRAFTS, TRASH, SPAM];
/// Messages per sync step: each one needs its own body request.
const PAGE: u32 = 50;
/// Parallel body/decrypt requests per page.
const FETCH_CONCURRENCY: usize = 6;
/// Message-package encryption schemes (`go-proton-api/message_send_types.go`).
const SCHEME_INTERNAL: i64 = 1;
const SCHEME_CLEAR: i64 = 4;
const SIGNATURE_DETACHED: i64 = 1;
const SIGNATURE_NONE: i64 = 0;

/// The session and key material behind a signed-in Proton account.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Credentials {
    /// Distinguishes a native account from the Bridge credentials under the same
    /// `proton:{id}` keychain entry.
    pub kind: String,
    pub uid: String,
    pub access: String,
    pub refresh: String,
    /// Key id -> the salted passphrase that unlocks it. Never the account password.
    pub passphrases: BTreeMap<String, String>,
}

impl Credentials {
    pub const KIND: &'static str = "api";

    pub fn parse(raw: &str) -> Result<Option<Self>> {
        let value: Value = serde_json::from_str(raw).context("Proton credentials")?;
        if value["kind"].as_str() != Some(Self::KIND) {
            return Ok(None);
        }
        Ok(Some(
            serde_json::from_value(value).context("Proton credentials")?,
        ))
    }
}

/// Where a sign-in stopped. The engine turns these into `ProtonAuth`.
pub enum Step {
    Done,
    TwoFactor,
    MailboxPassword,
    HumanVerification { url: String },
}

/// A sign-in in progress: SRP proofs, then 2FA, then (in two-password mode) the
/// mailbox password that unlocks the keys.
pub struct SignIn {
    api: Api,
    email: String,
    password: String,
    /// The answer to `/auth/v4`, kept while the 2FA step is outstanding.
    auth: Option<api::Auth>,
    challenge: Option<api::Challenge>,
    pending: Pending,
    done: Option<SignedIn>,
}

#[derive(Clone, Copy, PartialEq)]
enum Pending {
    Srp,
    TwoFactor,
    MailboxPassword,
}

/// A finished sign-in: what the engine stores.
pub struct SignedIn {
    pub email: String,
    pub name: String,
    pub credentials: Credentials,
}

impl SignIn {
    pub fn new(base_url: &str, app_version: &str, email: &str, password: &str) -> Self {
        Self {
            api: Api::new(base_url, app_version),
            email: email.trim().to_string(),
            password: password.to_string(),
            auth: None,
            challenge: None,
            pending: Pending::Srp,
            done: None,
        }
    }

    pub fn take(&mut self) -> Option<SignedIn> {
        self.done.take()
    }

    /// `/auth/v4/info`, the SRP proof and `/auth/v4`.
    pub async fn start(&mut self) -> Result<Step> {
        self.authenticate().await
    }

    /// Answers the step the previous call returned.
    pub async fn step(&mut self, answer: &str) -> Result<Step> {
        let answer = answer.trim().to_string();
        match self.pending {
            Pending::TwoFactor => {
                anyhow::ensure!(
                    !answer.is_empty(),
                    "Enter the code from your authenticator app."
                );
                self.api.auth_2fa(&answer).await?;
                let auth = self.auth.clone().context("no Proton sign-in in progress")?;
                self.after_two_factor(auth).await
            }
            Pending::MailboxPassword => {
                anyhow::ensure!(!answer.is_empty(), "Enter your Proton mailbox password.");
                let auth = self.auth.clone().context("no Proton sign-in in progress")?;
                self.finish_with(auth, &answer).await
            }
            Pending::Srp => {
                // Human verification, as Bridge does it: solving the puzzle in the browser marks
                // the 9001 challenge token as verified server-side, so the retry sends that same
                // token back. The verify page never shows the user a token to paste.
                let challenge = self
                    .challenge
                    .as_mut()
                    .context("no Proton verification in progress")?;
                if !answer.is_empty() {
                    challenge.token = answer;
                }
                self.authenticate().await
            }
        }
    }

    async fn authenticate(&mut self) -> Result<Step> {
        let info = self.api.auth_info(&self.email).await?;
        anyhow::ensure!(
            info.version >= 3,
            "This Proton account still uses SRP version {} (not supported).",
            info.version
        );
        let modulus = crypto::clear_signed_text(&info.modulus)?;
        let modulus = crypto::unb64(&modulus)?;
        let salt = crypto::unb64(&info.salt)?;
        let hashed = srp::hashed_password(self.password.as_bytes(), &salt, &modulus)?;
        let server_ephemeral = crypto::unb64(&info.server_ephemeral)?;
        let proofs = srp::proofs(&modulus, &server_ephemeral, &hashed)?;

        let body = json!({
            "Username": self.email,
            "ClientEphemeral": crypto::b64(&proofs.client_ephemeral),
            "ClientProof": crypto::b64(&proofs.client_proof),
            "SRPSession": info.srp_session,
        });
        let auth = match self.api.auth(&body, self.challenge.as_ref()).await {
            Ok(auth) => auth,
            Err(error) => {
                if let Some(challenge) = error
                    .downcast_ref::<api::ApiError>()
                    .and_then(|e| e.challenge())
                {
                    let url = if challenge.web_url.is_empty() {
                        let methods = challenge.methods.join(",");
                        format!(
                            "https://verify.proton.me/?methods={methods}&token={}",
                            challenge.token
                        )
                    } else {
                        challenge.web_url.clone()
                    };
                    self.challenge = Some(challenge);
                    self.pending = Pending::Srp;
                    return Ok(Step::HumanVerification { url });
                }
                return Err(error);
            }
        };
        if !proofs.expected_server_proof.is_empty() && !auth.server_proof.is_empty() {
            let server = crypto::unb64(&auth.server_proof).unwrap_or_default();
            if server != proofs.expected_server_proof {
                bail!("Proton sent a proof that does not match this session");
            }
        }
        self.finish(auth).await
    }

    /// After `/auth/v4`: the 2FA step, then the keys.
    async fn finish(&mut self, auth: api::Auth) -> Result<Step> {
        if auth.has_totp() {
            // The session exists already; the code is proven against it.
            self.api
                .session(&auth.uid, &auth.access_token, &auth.refresh_token);
            self.pending = Pending::TwoFactor;
            self.auth = Some(auth);
            return Ok(Step::TwoFactor);
        }
        anyhow::ensure!(
            auth.two_fa.enabled == 0,
            "This Proton account protects sign-in with a security key, which Kite cannot use yet. Add an authenticator app in Proton's settings, or use the Bridge option."
        );
        self.after_two_factor(auth).await
    }

    async fn after_two_factor(&mut self, auth: api::Auth) -> Result<Step> {
        if auth.two_password() {
            self.api
                .session(&auth.uid, &auth.access_token, &auth.refresh_token);
            self.pending = Pending::MailboxPassword;
            self.auth = Some(auth);
            return Ok(Step::MailboxPassword);
        }
        let password = self.password.clone();
        self.finish_with(auth, &password).await
    }

    /// Fetches the user, addresses and key salts, then derives each key's passphrase.
    async fn finish_with(&mut self, auth: api::Auth, mailbox_password: &str) -> Result<Step> {
        self.api
            .session(&auth.uid, &auth.access_token, &auth.refresh_token);
        let user = self.api.user().await?;
        let addresses = self.api.addresses().await?;
        let salts = self.api.salts().await?;
        // As Bridge does: one passphrase, salted with the primary user key's salt, unlocks
        // the user keys; address keys either carry a token sealed to the user key (current
        // accounts) or share that passphrase (older ones).
        let primary = user
            .keys
            .iter()
            .find(|key| key.primary)
            .or_else(|| user.keys.first())
            .context("this Proton account has no user key")?;
        let salt = salts
            .iter()
            .find(|salt| salt.id == primary.id)
            .and_then(|salt| salt.key_salt.as_deref())
            .with_context(|| format!("Proton has no salt for user key {}", primary.id))?;
        let salted = srp::key_passphrase(mailbox_password, &crypto::unb64(salt)?)?;
        let user_key = crypto::SecretKey::parse(&primary.id, true, &primary.private_key, &salted)?;

        let mut passphrases = BTreeMap::new();
        for key in &user.keys {
            passphrases.insert(key.id.clone(), salted.clone());
        }
        for key in addresses.iter().flat_map(|a| a.keys.iter()) {
            let passphrase = match (key.token.as_deref(), key.signature.as_deref()) {
                (Some(token), Some(signature)) if !token.is_empty() && !signature.is_empty() => {
                    crypto::token_passphrase(&user_key, token, signature)
                        .with_context(|| format!("unlock Proton address key {}", key.id))?
                }
                _ => salted.clone(),
            };
            passphrases.insert(key.id.clone(), passphrase);
        }
        let email = if user.email.is_empty() {
            self.email.clone()
        } else {
            user.email.clone()
        };
        let name = match user.display_name.trim() {
            "" => email.split('@').next().unwrap_or(&email).to_string(),
            display => display.to_string(),
        };
        self.done = Some(SignedIn {
            email,
            name,
            credentials: Credentials {
                kind: Credentials::KIND.to_string(),
                uid: auth.uid,
                access: auth.access_token,
                refresh: auth.refresh_token,
                passphrases,
            },
        });
        Ok(Step::Done)
    }
}

/// The backend for one Proton account.
pub struct Proton {
    api: Api,
    email: String,
    /// Key id -> the salted passphrase that unlocks it.
    passphrases: BTreeMap<String, String>,
    keys: Option<Keyring>,
    sent: Option<RemoteMsg>,
    /// Custom folders, cached until a label event or mail refresh.
    folders: Option<Vec<(String, String)>>,
    known: BTreeSet<String>,
    reported: HashSet<String>,
    warnings: Vec<String>,
    fetch_concurrency: usize,
    cleanup: Vec<String>,
}

impl Proton {
    pub fn new(base_url: &str, app_version: &str, email: &str, credentials: &Credentials) -> Self {
        let mut api = Api::new(base_url, app_version);
        api.session(&credentials.uid, &credentials.access, &credentials.refresh);
        Self {
            api,
            email: email.to_string(),
            passphrases: credentials.passphrases.clone(),
            keys: None,
            sent: None,
            folders: None,
            known: BTreeSet::new(),
            reported: HashSet::new(),
            warnings: Vec::new(),
            fetch_concurrency: FETCH_CONCURRENCY,
            cleanup: Vec::new(),
        }
    }

    /// The keychain entry the engine persists after a token refresh.
    pub fn credentials(&self) -> Credentials {
        Credentials {
            kind: Credentials::KIND.to_string(),
            uid: self.api.uid.clone(),
            access: self.api.access.clone(),
            refresh: self.api.refresh.clone(),
            passphrases: self.passphrases.clone(),
        }
    }

    /// Ends the session, so removing an account does not leave a token alive.
    pub async fn logout(&mut self) {
        let _ = self.api.logout().await;
    }

    pub(crate) fn has_more(cursor: &str) -> bool {
        serde_json::from_str::<Cursor>(cursor).is_ok_and(|c| c.label < BACKFILL.len())
    }

    pub(crate) fn set_known(&mut self, known: BTreeSet<String>) {
        self.known = known;
    }

    pub(crate) fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    fn warn(&mut self, id: &str, message: String) {
        if self.reported.insert(id.to_string()) {
            self.warnings
                .push(format!("Proton message {id}: {message}"));
        }
    }

    async fn cleanup_drafts(&mut self) -> Result<()> {
        if !self.cleanup.is_empty() {
            self.api.delete_messages(&self.cleanup).await?;
            self.cleanup.clear();
        }
        Ok(())
    }

    pub async fn sync(&mut self, cursor: Option<String>) -> Result<(Vec<Delta>, String)> {
        self.cleanup_drafts().await?;
        self.load_keys().await?;
        let mut cursor = match cursor {
            Some(raw) => serde_json::from_str::<Cursor>(&raw).context("proton cursor")?,
            None => Cursor {
                event: self.api.latest_event().await?,
                label: 0,
                page: 0,
                remaining: None,
            },
        };

        // First sync: newest mail first, one page of bodies per call.
        if cursor.label < BACKFILL.len() {
            let label = BACKFILL[cursor.label];
            let metas = self.api.messages(label, cursor.page, PAGE).await?;
            let mut deltas = Vec::new();
            let mut fetch = Vec::new();
            for meta in &metas {
                if let Some(remaining) = &mut cursor.remaining {
                    remaining.remove(&meta.id);
                }
                if self.known.contains(&meta.id) && !meta.label_ids.iter().any(|l| l == ALL_DRAFTS)
                {
                    let (mailbox, folder) = mailbox_for(&meta.label_ids);
                    deltas.push(Delta::Metadata {
                        remote_id: meta.id.clone(),
                        folder,
                        mailbox,
                        unread: meta.unread,
                        starred: meta.label_ids.iter().any(|l| l == STARRED),
                    });
                } else {
                    fetch.push(meta.clone());
                }
            }
            deltas.extend(self.upserts(&fetch).await?);
            if metas.len() < PAGE as usize {
                cursor.label += 1;
                cursor.page = 0;
            } else {
                cursor.page += 1;
            }
            if cursor.label == BACKFILL.len()
                && let Some(remaining) = cursor.remaining.take()
            {
                deltas.extend(
                    remaining
                        .into_iter()
                        .map(|remote_id| Delta::Delete { remote_id }),
                );
            }
            return Ok((deltas, serde_json::to_string(&cursor)?));
        }

        // Live: the event stream carries creations, flag changes and deletions together.
        let events = match self.api.events(&cursor.event).await {
            Ok(events) => events,
            Err(error) => {
                if !is_stale_event(&error) {
                    return Err(error);
                }
                // The event window expired; walking the labels again is the only way to
                // be sure nothing was missed.
                let event = self.api.latest_event().await?;
                let fresh = Cursor {
                    event,
                    label: 0,
                    page: 0,
                    remaining: Some(self.known.clone()),
                };
                return Ok((Vec::new(), serde_json::to_string(&fresh)?));
            }
        };
        let mut deltas = Vec::new();
        for event in events {
            if !event.event_id.is_empty() {
                cursor.event = event.event_id;
            }
            if !event.labels.is_empty() || event.refresh & 1 != 0 {
                self.folders = None;
            }
            if event.refresh & 1 != 0 {
                cursor.label = 0;
                cursor.page = 0;
                cursor.remaining = Some(self.known.clone());
            }
            let mut changed = Vec::new();
            for message in &event.messages {
                if message.action == api::EVENT_DELETE {
                    deltas.push(Delta::Delete {
                        remote_id: message.id.clone(),
                    });
                } else if !message.id.is_empty() {
                    // Event metadata can omit even its nested ID; the envelope owns it.
                    changed.push(api::Meta {
                        id: message.id.clone(),
                        ..Default::default()
                    });
                }
            }
            deltas.extend(self.upserts(&changed).await?);
        }
        Ok((deltas, serde_json::to_string(&cursor)?))
    }

    pub async fn apply(&mut self, op: &Op) -> Result<Option<String>> {
        self.cleanup_drafts().await?;
        self.load_keys().await?;
        match op {
            Op::Move { remote_id, to } => {
                let Some(label) = mailbox_label(*to) else {
                    return Ok(None);
                };
                self.relabel(remote_id, label).await
            }
            Op::MoveToFolder { remote_id, folder } => self.relabel(remote_id, folder).await,
            Op::Flags {
                remote_id,
                unread,
                starred,
            } => {
                let ids = [remote_id.clone()];
                if let Some(unread) = unread {
                    self.api.mark_read(&ids, !*unread).await?;
                }
                if let Some(starred) = starred {
                    if *starred {
                        self.api.label_messages(&ids, STARRED).await?;
                    } else {
                        self.api.unlabel_messages(&ids, STARRED).await?;
                    }
                }
                Ok(None)
            }
            Op::Send { mime } => self.send(mime).await,
            Op::SaveDraft { mime, replaces } => {
                let draft = self.build_draft(mime).await?;
                let body = draft.body();
                let saved = match replaces {
                    Some(id) if !id.is_empty() => match self.api.update_draft(id, &body).await {
                        Ok(message) => message,
                        // The old copy is gone (sent or discarded elsewhere): save a new one.
                        Err(_) => self.api.create_draft(&body).await?,
                    },
                    _ => self.api.create_draft(&body).await?,
                };
                Ok(Some(saved.meta.id))
            }
            Op::DeleteDraft { remote_id } => {
                if let Err(error) = self
                    .api
                    .delete_messages(std::slice::from_ref(remote_id))
                    .await
                {
                    // Already gone (a sent draft becomes the sent message): not a failure.
                    let missing = error
                        .downcast_ref::<api::ApiError>()
                        .is_some_and(|e| e.status == 404 || e.code == api::CODE_INVALID_VALUE);
                    if !missing {
                        return Err(error);
                    }
                }
                Ok(None)
            }
        }
    }

    pub async fn attachment(
        &mut self,
        _remote_msg_id: &str,
        remote_att_id: &str,
    ) -> Result<Vec<u8>> {
        self.load_keys().await?;
        let data = self.api.attachment(remote_att_id).await?;
        let keys = self.keys.as_ref().context("Proton keys are not loaded")?;
        let mut packets = crypto::unb64(&data.key_packets)?;
        packets.extend(crypto::unb64(&data.data)?);
        keys.decrypt_packets(&packets)
    }

    /// Creates a Proton folder for `Mail::create_folder` and returns its label id.
    pub async fn create_folder(&mut self, name: &str) -> Result<String> {
        let label = self.api.create_label(name.trim()).await?;
        // The catalog is cached; a new folder has to be visible on the next read.
        self.folders = None;
        Ok(label.id)
    }

    /// The account's folders, as `(label id, name)`. Proton labels are folders; plain labels
    /// (type 1) only tag mail that already lives in a mailbox.
    pub async fn folders(&mut self) -> Result<Vec<(String, String)>> {
        if let Some(folders) = &self.folders {
            return Ok(folders.clone());
        }
        let folders: Vec<(String, String)> = self
            .api
            .labels()
            .await?
            .into_iter()
            .filter(|label| label.kind == 3 && !label.id.is_empty() && !label.name.is_empty())
            .map(|label| (label.id, label.name))
            .collect();
        self.folders = Some(folders.clone());
        Ok(folders)
    }

    // ------------------------------------------------------------------ internals

    async fn load_keys(&mut self) -> Result<()> {
        if self.keys.is_some() {
            return Ok(());
        }
        let user = self.api.user().await?;
        let addresses = self.api.addresses().await?;
        let mut keyring = Keyring {
            user: Vec::new(),
            addresses: Vec::new(),
        };
        for key in &user.keys {
            if !key.active {
                continue;
            }
            let Some(passphrase) = self.passphrases.get(&key.id) else {
                continue;
            };
            keyring.user.push(crypto::SecretKey::parse(
                &key.id,
                key.primary,
                &key.private_key,
                passphrase,
            )?);
        }
        for address in &addresses {
            let mut keys = Vec::new();
            for key in &address.keys {
                if !key.active {
                    continue;
                }
                let Some(passphrase) = self.passphrases.get(&key.id) else {
                    continue;
                };
                keys.push(crypto::SecretKey::parse(
                    &key.id,
                    key.primary,
                    &key.private_key,
                    passphrase,
                )?);
            }
            keyring.addresses.push(crypto::AddressKeys {
                email: address.email.clone(),
                keys,
            });
        }
        if keyring.addresses.iter().all(|a| a.keys.is_empty()) {
            bail!("no Proton key could be unlocked: sign in to Proton again");
        }
        self.keys = Some(keyring);
        Ok(())
    }

    /// Bodies for a page of metadata, fetched a few at a time so a large mailbox does
    /// not serialise into minutes of round trips.
    async fn upserts(&mut self, metas: &[api::Meta]) -> Result<Vec<Delta>> {
        let mut out = Vec::with_capacity(metas.len());
        let mut offset = 0;
        while offset < metas.len() {
            let end = (offset + self.fetch_concurrency).min(metas.len());
            let keys = self.keys.as_ref().context("Proton keys are not loaded")?;
            let results = futures_util::future::join_all(
                metas[offset..end]
                    .iter()
                    .map(|meta| fetch_delta(self.api.clone(), keys, meta)),
            )
            .await;
            let mut systemic = None;
            let mut throttled = false;
            for (meta, result) in metas[offset..end].iter().zip(results) {
                match result {
                    Ok((delta, warning)) => {
                        out.push(delta);
                        if let Some(warning) = warning {
                            self.warn(&meta.id, warning);
                        }
                    }
                    Err(error) => {
                        // Only a message-specific missing/invalid response can be skipped.
                        let isolated = error.downcast_ref::<serde_json::Error>().is_some()
                            || error.downcast_ref::<api::ApiError>().is_some_and(|e| {
                                e.status == 404
                                    || (e.status == 400 && e.code == api::CODE_INVALID_VALUE)
                            });
                        if isolated {
                            self.warn(&meta.id, format!("body unavailable: {error:#}"));
                        } else {
                            if error
                                .downcast_ref::<api::ApiError>()
                                .is_some_and(|e| e.status == 429)
                            {
                                throttled = true;
                            }
                            if systemic
                                .as_ref()
                                .is_none_or(|e: &anyhow::Error| retry_at(&error) > retry_at(e))
                            {
                                systemic = Some(error);
                            }
                        }
                    }
                }
            }
            if throttled {
                self.fetch_concurrency = (self.fetch_concurrency / 2).max(1);
            }
            if let Some(error) = systemic {
                return Err(error);
            }
            offset = end;
        }
        Ok(out)
    }

    /// Adds `label` and drops the other mailboxes: Proton archives and trashes by
    /// relabelling, and a message may only sit in one of them.
    async fn relabel(&mut self, remote_id: &str, label: &str) -> Result<Option<String>> {
        let ids = [remote_id.to_string()];
        self.api.label_messages(&ids, label).await?;
        for other in [INBOX, ARCHIVE, TRASH, SPAM] {
            if other != label {
                // Best effort: the message was already moved in the store, and the next
                // event pass corrects anything this misses.
                let _ = self.api.unlabel_messages(&ids, other).await;
            }
        }
        Ok(None)
    }

    /// Proton's send flow: save the draft, then hand it the encrypted packages.
    async fn send(&mut self, mime: &[u8]) -> Result<Option<String>> {
        let draft = self.build_draft(mime).await?;
        let saved = self
            .api
            .create_draft(&draft.body())
            .await
            .context("create the Proton draft")?;
        let result = async {
            let keys = self.keys.as_ref().context("Proton keys are not loaded")?;
            let (message, _) = remote_msg(keys, &saved)?;
            let packages = self.packages(&draft).await?;
            let sent = self
                .api
                .send_draft(&saved.meta.id, &json!({ "Packages": packages }))
                .await
                .context("send the Proton draft")?;
            Ok::<_, anyhow::Error>((message, sent))
        }
        .await;
        let (mut message, sent) = match result {
            Ok(sent) => sent,
            Err(error) => {
                self.cleanup.push(saved.meta.id);
                // A throttle defers cleanup until its deadline, just like other requests.
                let _ = self.cleanup_drafts().await;
                return Err(error);
            }
        };
        // Use the accepted send response and the body we already hold; Sent must not wait
        // for another event poll or body download.
        message.remote_id = sent.meta.id;
        message.message_id = message.message_id.or(draft.message_id.clone());
        message.in_reply_to = draft.in_reply_to.clone();
        message.references = draft.references.clone();
        message.date = sent.meta.time.max(0) * 1000;
        message.mailbox = Mailbox::Sent;
        message.folder = SENT.into();
        message.unread = false;
        message.from = draft.from;
        message.to = draft.to;
        message.cc = draft.cc;
        message.subject = draft.subject;
        message.text = draft.text;
        self.sent = Some(message);
        Ok(None)
    }

    pub(crate) fn take_sent(&mut self) -> Option<RemoteMsg> {
        self.sent.take()
    }

    async fn build_draft(&mut self, mime: &[u8]) -> Result<Outgoing> {
        let mut outgoing = outgoing(mime, &self.email)?;
        if let Some(parent) = &outgoing.in_reply_to {
            // Proton synthesizes reply headers from ParentID, not from the subject.
            let value = self
                .api
                .call_with(
                    reqwest::Method::POST,
                    "/mail/v4/messages",
                    Some(&json!({"ExternalID": parent, "Page": 0, "PageSize": 1})),
                    &[("X-HTTP-Method-Override", "GET")],
                )
                .await?;
            outgoing.parent_id = value["Messages"]
                .as_array()
                .and_then(|messages| messages.first())
                .and_then(|message| message["ID"].as_str())
                .map(str::to_owned);
            if outgoing.parent_id.is_none() {
                bail!("the original Proton message is no longer available for this reply");
            }
        }
        let keys = self.keys.as_ref().context("Proton keys are not loaded")?;
        let address = keys
            .for_sender(&outgoing.from.email)
            .with_context(|| format!("no Proton address for {}", outgoing.from.email))?;
        // Proton checks Sender.Address against the address exactly as it stores it.
        outgoing.from.email = address.email.clone();
        if outgoing.from.name.is_empty() {
            outgoing.from.name = outgoing.from.email.clone();
        }
        let signer = Keyring::signing_key(address)?;
        outgoing.armored_body =
            crypto::encrypt_draft(signer, &outgoing.text, self.api.server_time)?;
        Ok(outgoing)
    }

    async fn packages(&mut self, outgoing: &Outgoing) -> Result<Value> {
        let keys = self.keys.as_ref().context("Proton keys are not loaded")?;
        let address = keys
            .for_sender(&outgoing.from.email)
            .with_context(|| format!("no Proton address for {}", outgoing.from.email))?;
        let signer = Keyring::signing_key(address)?;
        let package = crypto::encrypt_body(signer, &outgoing.text, self.api.server_time)?;
        // As Bridge does: an external copy declares the signature the account's "Sign external
        // messages" setting asks for (the body always carries it), or Proton refuses the send.
        let sign_external = self.api.signs_external_mail().await?;
        let mut addresses = Map::new();
        let mut schemes = 0i64;
        let mut clear = false;
        for recipient in outgoing.recipients() {
            let (public, kind) = self.api.public_keys(&recipient.email).await?;
            // Proton's key flags: bit 0 means not compromised, bit 1 means still in use.
            let usable = public
                .iter()
                .find(|key| key.flags & 1 != 0 && !key.public_key.is_empty())
                .or_else(|| public.iter().find(|key| !key.public_key.is_empty()));
            let internal = kind == 1 && usable.is_some();
            if let (true, Some(public)) = (internal, usable) {
                let key = crypto::pubkey(&public.public_key)?;
                let packet = crypto::session_key_packet(&key, &package.session_key)?;
                addresses.insert(
                    recipient.email.clone(),
                    json!({
                        "Type": SCHEME_INTERNAL,
                        "Signature": SIGNATURE_DETACHED,
                        "BodyKeyPacket": packet,
                    }),
                );
                schemes |= SCHEME_INTERNAL;
            } else {
                // External recipients get a clear package: Proton decrypts it with the
                // session key below and delivers plain text.
                addresses.insert(
                    recipient.email.clone(),
                    json!({
                        "Type": SCHEME_CLEAR,
                        "Signature": if sign_external { SIGNATURE_DETACHED } else { SIGNATURE_NONE },
                    }),
                );
                schemes |= SCHEME_CLEAR;
                clear = true;
            }
        }
        let mut package_json = json!({
            "Addresses": addresses,
            "MIMEType": "text/plain",
            "Type": schemes,
            "Body": package.body,
        });
        if clear {
            package_json["BodyKey"] = json!({
                "Key": package.session_key,
                "Algorithm": crypto::SESSION_ALGORITHM_NAME,
            });
        }
        Ok(Value::Array(vec![package_json]))
    }
}

/// What one event's metadata turns into once the body is decrypted. Needs only the
/// shared pieces of the backend so a page of them can run at once.
async fn fetch_delta(
    mut client: Api,
    keys: &Keyring,
    meta: &api::Meta,
) -> Result<(Delta, Option<String>)> {
    let message = client.message(&meta.id).await?;
    let (message, warning) = remote_msg(keys, &message)?;
    Ok((Delta::Upsert(Box::new(message)), warning))
}

fn remote_msg(keys: &Keyring, message: &api::Message) -> Result<(RemoteMsg, Option<String>)> {
    let meta = &message.meta;
    let mut warning = None;
    // Proton keeps the RFC 822 header block in clear; only the body is encrypted.
    let header = message.header.trim_end().to_string();
    let body = if message
        .body
        .trim_start()
        .starts_with("-----BEGIN PGP MESSAGE-----")
    {
        match keys.decrypt(&message.body) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(_) => {
                warning = Some("could not decrypt; showing the original body".to_string());
                message.body.clone()
            }
        }
    } else {
        message.body.clone()
    };
    let mime_type = if message.mime_type.is_empty() {
        "text/plain"
    } else {
        message.mime_type.as_str()
    };
    // Header block plus the Content-* header the API reports as the body's own type: that
    // is a complete message, so mail-parser can take it apart the same way it does IMAP.
    let raw = if header.is_empty() {
        format!("Content-Type: {mime_type}\r\n\r\n{body}")
    } else {
        format!("{header}\r\nContent-Type: {mime_type}\r\n\r\n{body}")
    };
    let parsed = parser().parse(raw.as_bytes());
    let (mailbox, folder) = mailbox_for(&meta.label_ids);
    let blank = Addr {
        name: String::new(),
        email: String::new(),
    };
    let (from, to, cc) = match &parsed {
        Some(parsed) => (
            parsed
                .from()
                .and_then(|list| addrs(list).into_iter().next())
                .or_else(|| meta.sender.as_ref().map(name_addr))
                .unwrap_or(blank),
            non_empty(
                addrs_of(parsed.to()),
                meta.to_list.iter().map(name_addr).collect(),
            ),
            non_empty(
                addrs_of(parsed.cc()),
                meta.cc_list.iter().map(name_addr).collect(),
            ),
        ),
        None => (
            meta.sender.as_ref().map(name_addr).unwrap_or(blank),
            meta.to_list.iter().map(name_addr).collect(),
            meta.cc_list.iter().map(name_addr).collect(),
        ),
    };
    let subject = match &parsed {
        Some(parsed) if !parsed.subject().unwrap_or_default().is_empty() => {
            parsed.subject().unwrap_or_default().to_string()
        }
        _ => meta.subject.clone(),
    };
    let text = match &parsed {
        Some(parsed) => body_text(parsed),
        None => String::new(),
    };
    let html = parsed.as_ref().and_then(body_html);
    Ok((
        RemoteMsg {
            remote_id: meta.id.clone(),
            folder,
            mailbox,
            // Proton also groups unrelated sends by subject; only RFC header links count.
            thread_hint: None,
            message_id: parsed.as_ref().and_then(|p| p.message_id()).map(bare_id),
            in_reply_to: parsed.as_ref().and_then(|p| match p.in_reply_to() {
                mail_parser::HeaderValue::Text(text) if !text.trim().is_empty() => {
                    Some(bare_id(text))
                }
                _ => None,
            }),
            references: match parsed.as_ref().map(|p| p.references()) {
                Some(mail_parser::HeaderValue::TextList(list)) => {
                    list.iter().map(|id| bare_id(id)).collect()
                }
                Some(mail_parser::HeaderValue::Text(text)) => {
                    text.split_whitespace().map(bare_id).collect()
                }
                _ => Vec::new(),
            },
            from,
            to,
            cc,
            date: meta.time.max(0) * 1000,
            subject,
            text,
            html,
            unread: meta.unread,
            starred: meta.label_ids.iter().any(|id| id == STARRED),
            important: None,
            list_unsubscribe: parsed
                .as_ref()
                .is_some_and(|p| !p.list_unsubscribe().is_empty() || !p.list_id().is_empty()),
            attachments: message
                .attachments
                .iter()
                .map(|attachment| RemoteAttachment {
                    remote_id: attachment.id.clone(),
                    name: if attachment.name.is_empty() {
                        "attachment".to_string()
                    } else {
                        attachment.name.clone()
                    },
                    mime: if attachment.mime_type.is_empty() {
                        "application/octet-stream".to_string()
                    } else {
                        attachment.mime_type.clone()
                    },
                    size: attachment.size.max(0) as u64,
                })
                .collect(),
        },
        warning,
    ))
}

/// The mailbox a set of labels puts a message in (Trash wins, then Spam, then the
/// folder role, and everything else is "all mail"), plus the folder id the store keeps.
/// A message carrying a custom folder label is filed under that folder: Proton has one
/// `folder` column per message, so the first custom label wins when there are several.
fn mailbox_for(labels: &[String]) -> (Mailbox, String) {
    let has = |label: &str| labels.iter().any(|id| id == label);
    for (label, mailbox) in [
        (TRASH, Mailbox::Trash),
        (SPAM, Mailbox::Spam),
        (DRAFTS, Mailbox::Drafts),
        (ALL_DRAFTS, Mailbox::Drafts),
        (SENT, Mailbox::Sent),
        (ALL_SENT, Mailbox::Sent),
        (INBOX, Mailbox::Inbox),
        (ARCHIVE, Mailbox::Archive),
    ] {
        if has(label) {
            let folder = match mailbox {
                Mailbox::Drafts => DRAFTS,
                Mailbox::Sent => SENT,
                other => system_label(other),
            };
            return (mailbox, folder.to_string());
        }
    }
    if let Some(folder) = labels.iter().find(|id| !is_system_label(id)) {
        return (Mailbox::All, folder.clone());
    }
    (Mailbox::All, ALL_MAIL.to_string())
}

/// Proton's system label ids are fixed single/double digit numbers; everything else is a
/// label the account owner made.
fn is_system_label(id: &str) -> bool {
    !id.is_empty() && id.len() <= 2 && id.bytes().all(|b| b.is_ascii_digit())
}

fn system_label(mailbox: Mailbox) -> &'static str {
    match mailbox {
        Mailbox::Inbox => INBOX,
        Mailbox::Sent => SENT,
        Mailbox::Drafts => DRAFTS,
        Mailbox::Archive => ARCHIVE,
        Mailbox::Trash => TRASH,
        Mailbox::Spam => SPAM,
        Mailbox::Starred | Mailbox::Snoozed | Mailbox::All => ALL_MAIL,
    }
}

/// The label a move targets; `None` for mailboxes Proton has no label for.
fn mailbox_label(mailbox: Mailbox) -> Option<&'static str> {
    match mailbox {
        Mailbox::Inbox => Some(INBOX),
        Mailbox::Archive => Some(ARCHIVE),
        Mailbox::Trash => Some(TRASH),
        Mailbox::Spam => Some(SPAM),
        Mailbox::Drafts => Some(DRAFTS),
        Mailbox::Sent | Mailbox::Starred | Mailbox::Snoozed | Mailbox::All => None,
    }
}

/// A message built by the engine, ready to become a Proton draft.
struct Outgoing {
    message_id: Option<String>,
    in_reply_to: Option<String>,
    references: Vec<String>,
    parent_id: Option<String>,
    from: Addr,
    to: Vec<Addr>,
    cc: Vec<Addr>,
    bcc: Vec<Addr>,
    subject: String,
    text: String,
    /// The body encrypted to our own key, which is what the API stores on a draft.
    armored_body: String,
}

impl Outgoing {
    fn recipients(&self) -> Vec<&Addr> {
        let mut seen = Vec::new();
        for addr in self.to.iter().chain(&self.cc).chain(&self.bcc) {
            let email = addr.email.trim().to_ascii_lowercase();
            if email.is_empty() || seen.contains(&email) {
                continue;
            }
            seen.push(email);
        }
        self.to
            .iter()
            .chain(&self.cc)
            .chain(&self.bcc)
            .filter(|a| {
                let email = a.email.trim().to_ascii_lowercase();
                !email.is_empty() && seen.contains(&email)
            })
            .collect()
    }

    /// The create/update draft payload (`CreateDraftReq` in go-proton-api).
    fn body(&self) -> Value {
        let list = |addrs: &[Addr]| -> Vec<Value> {
            addrs
                .iter()
                .map(|a| json!({ "Name": a.name, "Address": a.email }))
                .collect()
        };
        let mut body = json!({
            "Message": {
                "ExternalID": self.message_id,
                "Subject": self.subject,
                "Sender": { "Name": self.from.name, "Address": self.from.email },
                "ToList": list(&self.to),
                "CCList": list(&self.cc),
                "BCCList": list(&self.bcc),
                "Body": self.armored_body,
                "MIMEType": "text/plain",
                "Unread": 1,
            }
        });
        if let Some(parent) = &self.parent_id {
            body["ParentID"] = json!(parent);
            body["Action"] = json!(if self.subject.to_ascii_lowercase().starts_with("fwd:") {
                2
            } else {
                0
            });
        }
        body
    }
}

/// Splits an engine-built MIME message into the parts Proton needs.
fn outgoing(mime: &[u8], account_email: &str) -> Result<Outgoing> {
    let parsed = parser()
        .parse(mime)
        .context("unparseable outbound message")?;
    let from = parsed
        .from()
        .and_then(|list| addrs(list).into_iter().next())
        .filter(|addr| !addr.email.is_empty())
        .unwrap_or_else(|| Addr {
            name: String::new(),
            email: account_email.to_string(),
        });
    let text = body_text(&parsed);
    let ids = |value: &mail_parser::HeaderValue<'_>| match value {
        mail_parser::HeaderValue::Text(text) => vec![bare_id(text)],
        mail_parser::HeaderValue::TextList(list) => list.iter().map(|id| bare_id(id)).collect(),
        _ => Vec::new(),
    };
    Ok(Outgoing {
        message_id: parsed.message_id().map(bare_id),
        in_reply_to: ids(parsed.in_reply_to()).into_iter().next(),
        references: ids(parsed.references()),
        parent_id: None,
        from,
        to: addrs_of(parsed.to()),
        cc: addrs_of(parsed.cc()),
        bcc: addrs_of(parsed.bcc()),
        subject: parsed.subject().unwrap_or_default().to_string(),
        text,
        armored_body: String::new(),
    })
}

fn addrs_of(list: Option<&mail_parser::Address<'_>>) -> Vec<Addr> {
    list.map(addrs).unwrap_or_default()
}

fn addrs(address: &mail_parser::Address<'_>) -> Vec<Addr> {
    address
        .iter()
        .filter_map(|a| {
            Some(Addr {
                name: a.name().unwrap_or_default().to_string(),
                email: a.address()?.to_string(),
            })
        })
        .collect()
}

fn name_addr(addr: &api::NameAddr) -> Addr {
    Addr {
        name: addr.name.clone(),
        email: addr.address.clone(),
    }
}

fn non_empty(primary: Vec<Addr>, fallback: Vec<Addr>) -> Vec<Addr> {
    if primary.is_empty() {
        fallback
    } else {
        primary
    }
}

/// A body, preferring the text part and falling back to a stripped HTML one, so a row
/// always has a snippet and FTS always has something to index.
fn body_text(message: &ParsedMessage<'_>) -> String {
    if let Some(text) = message.body_text(0) {
        return text.into_owned();
    }
    match body_html(message) {
        Some(html) => html_to_text(&html),
        None => String::new(),
    }
}

fn body_html(message: &ParsedMessage<'_>) -> Option<String> {
    message
        .parts
        .iter()
        .find(|part| part.is_text() && part.is_text_html())
        .and_then(|part| part.text_contents().map(str::to_string))
}

/// Tags, scripts and entities out of an HTML body, for snippets and FTS.
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut tag = String::new();
    let (mut in_tag, mut raw) = (false, false);
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let name = tag
                    .trim_start_matches('/')
                    .split(|c: char| c.is_whitespace() || c == '/')
                    .next()
                    .unwrap_or("");
                if matches!(name, "script" | "style") {
                    raw = !tag.starts_with('/');
                } else if !raw && matches!(name, "br" | "p" | "div" | "tr" | "li" | "td") {
                    out.push('\n');
                }
            }
            _ if in_tag => tag.push(c.to_ascii_lowercase()),
            _ if !raw => out.push(c),
            _ => {}
        }
    }
    let text = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Headers carry `<id>`; the store threads on the bare id.
fn bare_id(raw: &str) -> String {
    raw.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
        .to_string()
}

fn parser() -> &'static MessageParser {
    static PARSER: OnceLock<MessageParser> = OnceLock::new();
    PARSER.get_or_init(MessageParser::default)
}

/// A Proton event id that the API no longer knows about: the account has to be walked
/// again. Proton answers with a plain invalid-value code here.
fn is_stale_event(error: &anyhow::Error) -> bool {
    error.downcast_ref::<api::ApiError>().is_some_and(|e| {
        let text = e.message.to_ascii_lowercase();
        e.status == 400
            && e.code == api::CODE_INVALID_VALUE
            && (text.contains("event id") || text.contains("eventid"))
            && ["invalid", "unknown", "expired", "not found"]
                .iter()
                .any(|word| text.contains(word))
    })
}

/// Opaque per-account sync state: the live-sync cursor plus how far the first walk got.
#[derive(Serialize, Deserialize)]
struct Cursor {
    event: String,
    /// Index into [`BACKFILL`]; equal to its length once the first walk is done.
    #[serde(default)]
    label: usize,
    #[serde(default)]
    page: u32,
    /// Persist progress so an interrupted re-walk cannot delete unseen mail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    remaining: Option<BTreeSet<String>>,
}

pub(crate) fn retry_at(error: &anyhow::Error) -> Option<i64> {
    error
        .downcast_ref::<api::ApiError>()
        .and_then(|e| e.retry_at)
}

pub(crate) fn permanent_send(error: &anyhow::Error) -> bool {
    error.downcast_ref::<api::ApiError>().is_some_and(|e| {
        e.retry_at.is_none()
            && (e.code == api::CODE_INVALID_VALUE
                || ((400..500).contains(&e.status) && !matches!(e.status, 408 | 429)))
    }) || error
        .downcast_ref::<reqwest::Error>()
        .and_then(|e| e.status())
        .is_some_and(|s| s.is_client_error() && !matches!(s.as_u16(), 408 | 429))
}

/// Random hex, for the OAuth-style `State` the refresh route asks for.
pub fn random_token(bytes: usize) -> String {
    let mut raw = vec![0u8; bytes];
    let _ = getrandom::fill(&mut raw);
    raw.iter().map(|byte| format!("{byte:02x}")).collect()
}
