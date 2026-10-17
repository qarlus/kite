//! The UI's only door into the core (replaces the old IPC layer). Owned by the orchestrator.
//! Reads hit local SQLite and must return in a few ms. Writes apply locally, queue remote work, and return.
//! Slow work (account setup, OAuth) is called by the UI from a background executor.
use crate::model::*;
use anyhow::Result;

pub trait Mail: Send + Sync + 'static {
    fn accounts(&self) -> Result<Vec<Account>>;
    fn add_proton(&self, login: ProtonLogin) -> Result<Account>;
    fn add_microsoft(&self) -> Result<Account>;
    fn remove_account(&self, id: Id) -> Result<()>;

    fn threads(&self, view: &View, offset: u32, limit: u32) -> Result<Vec<ThreadSummary>>;
    fn thread(&self, id: Id) -> Result<Vec<Message>>;
    /// Applies locally at once and queues the remote op. Returns an undo token.
    fn act(&self, ids: &[Id], action: &Action) -> Result<i64>;
    fn undo(&self, token: i64) -> Result<()>;

    /// Queues to the outbox (due after the undo window or at `send_at`). Returns the outbox id.
    fn send(&self, draft: Draft) -> Result<Id>;
    fn cancel_send(&self, id: Id) -> Result<Option<Draft>>;
    fn contacts(&self, query: &str) -> Result<Vec<Addr>>;
    fn open_attachment(&self, id: Id) -> Result<()>;

    fn splits(&self) -> Result<Vec<Split>>;
    fn set_splits(&self, splits: Vec<Split>) -> Result<()>;
    fn settings(&self) -> Result<Settings>;
    fn set_settings(&self, settings: Settings) -> Result<()>;
    fn set_ai_key(&self, key: &str) -> Result<()>;

    fn calendar_today(&self, account: Id) -> Result<Vec<CalEvent>>;
    /// Streams `Event::Ai { req, .. }` chunks.
    fn ai(&self, req: u32, messages: Vec<ChatMsg>) -> Result<()>;
    fn sync_now(&self);
    /// Engine -> UI notifications. Each call returns a new receiver.
    fn events(&self) -> async_channel::Receiver<Event>;
}
