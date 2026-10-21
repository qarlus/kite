//! The UI's only door into the core (replaces the old IPC layer). Owned by the orchestrator.
//! Reads hit local SQLite and must return in a few ms. Writes apply locally, queue remote work, and return.
//! Slow work (account setup, OAuth) is called by the UI from a background executor.
use crate::model::*;
use anyhow::Result;
use std::path::PathBuf;

/// The request cannot be recalled; only `undo` restores archived conversations.
pub struct Unsubscribed {
    pub sender: String,
    pub archived: usize,
    pub undo: i64,
    pub url: Option<String>,
}

pub trait Mail: Send + Sync + 'static {
    fn accounts(&self) -> Result<Vec<Account>>;
    fn add_microsoft(&self) -> Result<Account>;
    fn remove_account(&self, id: Id) -> Result<()>;

    /// The avatar image file for an account: the user's choice first, then the
    /// provider's photo. `None` shows initials. Local files only; never blocks.
    fn avatar(&self, _account: Id) -> Option<PathBuf> {
        None
    }
    /// Replaces the account's chosen avatar with an already-encoded PNG.
    fn save_avatar(&self, _account: Id, _png: &[u8]) -> Result<()> {
        anyhow::bail!("Avatars are not available")
    }
    /// Removes the chosen avatar; a provider photo, when one exists, takes over.
    fn remove_avatar(&self, _account: Id) -> Result<()> {
        anyhow::bail!("Avatars are not available")
    }
    /// Changes the display name used as the From name.
    fn set_account_name(&self, _id: Id, _name: &str) -> Result<()> {
        anyhow::bail!("Renaming is not available")
    }

    /// Starts a native Proton sign-in with the account password. Blocking; run off the UI thread.
    /// The password is used for SRP and key unlock and is never stored.
    fn proton_sign_in(&self, _email: &str, _password: &str) -> Result<ProtonAuth> {
        anyhow::bail!("Proton sign-in is not available")
    }
    /// Answers the step the previous call returned (2FA code, mailbox password or verification
    /// token). Blocking; run off the UI thread.
    fn proton_sign_in_step(&self, _answer: &str) -> Result<ProtonAuth> {
        anyhow::bail!("No Proton sign-in in progress")
    }

    /// Locally known custom folders, with provider identities scoped to their accounts.
    fn folders(&self) -> Result<Vec<Folder>> {
        Ok(Vec::new())
    }
    /// May contact the provider; callers must run this off the UI thread.
    fn create_folder(&self, _account: Id, _name: &str) -> Result<Folder> {
        anyhow::bail!("Folder creation is not available for this account")
    }

    fn threads(&self, view: &View, offset: u32, limit: u32) -> Result<Vec<ThreadSummary>>;
    fn thread(&self, id: Id) -> Result<Vec<Message>>;
    /// Applies locally at once and queues the remote op. Returns an undo token.
    fn act(&self, ids: &[Id], action: &Action) -> Result<i64>;
    fn undo(&self, token: i64) -> Result<()>;
    /// Thread ids in `view` whose latest activity predates `before`, skipping snoozed and —
    /// unless `include_starred` — starred conversations. The bulk-archive dialog counts and
    /// confirms through this; archiving still goes through `act`, so providers sync normally.
    fn archivable(&self, _view: &View, _before: Ms, _include_starred: bool) -> Result<Vec<Id>> {
        Ok(Vec::new())
    }
    /// May contact an unsubscribe service. Call off the UI thread.
    fn unsubscribe(&self, _thread: Id) -> Result<Unsubscribed> {
        anyhow::bail!("Unsubscribe is not available")
    }

    /// Queues to the outbox (due after the undo window or at `send_at`). Returns the outbox id.
    fn send(&self, draft: Draft) -> Result<Id>;
    fn cancel_send(&self, id: Id) -> Result<Option<Draft>>;
    /// Saves a draft locally at once (it shows in the Drafts mailbox) and queues the provider copy.
    /// `draft.id: None` creates; `Some` replaces that draft. Returns the draft id.
    fn save_draft(&self, _draft: Draft) -> Result<Id> {
        anyhow::bail!("Drafts are not available")
    }
    /// The saved draft behind a Drafts-mailbox thread, so compose can reopen it.
    fn draft(&self, _thread: Id) -> Result<Option<Draft>> {
        Ok(None)
    }
    /// Deletes a saved draft locally and on the provider.
    fn discard_draft(&self, _id: Id) -> Result<()> {
        anyhow::bail!("Drafts are not available")
    }
    fn contacts(&self, query: &str) -> Result<Vec<Addr>>;
    /// The contact sidebar's card about one correspondent. Local reads only, a few ms;
    /// `exclude` keeps the open conversation out of the recent list. `None` when the
    /// address is unknown.
    fn contact(&self, _email: &str, _exclude: Option<Id>) -> Result<Option<ContactCard>> {
        Ok(None)
    }
    fn open_attachment(&self, id: Id) -> Result<()>;
    /// One remote image for HTML mail, fetched only when the reader allowed remote images for
    /// that message. Blocking; call it on the background executor.
    fn fetch_image(&self, _url: &str) -> Result<Vec<u8>> {
        anyhow::bail!("Remote images are not available")
    }

    fn splits(&self) -> Result<Vec<Split>>;
    fn set_splits(&self, splits: Vec<Split>) -> Result<()>;
    /// Ordered by `position`.
    fn tags(&self) -> Result<Vec<Tag>> {
        Ok(Vec::new())
    }
    /// Replaces the tag list. Ids <= 0 create; omitted tags are deleted (and leave every thread);
    /// a rename updates the denormalized `ThreadSummary.labels`.
    fn set_tags(&self, _tags: Vec<Tag>) -> Result<()> {
        anyhow::bail!("Tags are not available")
    }
    /// Blocking network work; call from the UI background executor.
    fn check_update(&self, current: &str) -> Result<Option<crate::update::Release>> {
        crate::update::blocking(crate::update::check(current))
    }
    /// Downloads, verifies and swaps the executable. Caller quits cleanly on success.
    fn install_update(&self, release: &crate::update::Release) -> Result<()> {
        crate::update::install_release(release)
    }

    fn settings(&self) -> Result<Settings>;
    fn set_settings(&self, settings: Settings) -> Result<()>;
    fn set_ai_key(&self, key: &str) -> Result<()>;

    fn calendar_today(&self, account: Id) -> Result<Vec<CalEvent>>;
    /// Streams `Event::Ai { req, .. }` chunks. The engine checks source privacy and builds the
    /// prompt; callers cannot submit an unscoped prompt containing protected mail.
    /// Ask emits `Event::AiSources` in citation order before its first completion chunk.
    fn ai(&self, req: u32, request: AiRequest) -> Result<()>;
    /// The free, zero-data-retention models the configured AI endpoint offers. Blocks on the
    /// network: call it from a background task.
    fn ai_models(&self) -> Result<Vec<AiModel>> {
        Ok(Vec::new())
    }
    fn sync_now(&self);
    /// Engine -> UI notifications. Each call returns a new receiver.
    fn events(&self) -> async_channel::Receiver<Event>;
}
