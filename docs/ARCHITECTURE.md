# Kite: Architecture

```
┌──────────── crates/app (GPUI, UI thread) ─────────────┐        ┌────────────── crates/core (kite-core) ───────────────┐
│ main.rs  window + Kite root view                      │        │ api.rs     trait Mail (the UI's only door)           │
│ app.rs   root: split bar, list, thread, overlays      │ Mail   │ engine.rs  impl Mail; owns tokio runtime thread:     │
│ keys.rs  actions + keymap (GPUI actions/key contexts) │ ─────► │            sync loop, op drain, outbox, snooze wake  │
│ list.rs  uniform_list virtualised thread list         │        │ store.rs   SQLite + FTS5: source of truth            │
│ thread.rs conversation view (html.rs via Blitz)       │ Event  │ sync/imap.rs  Proton Bridge IMAP/SMTP                │
│ palette.rs, search, snooze, toasts, compose/, …       │ ◄───── │ sync/graph.rs Microsoft Graph + calendar             │
                                                                 │ sync/proton/  native Proton API (SRP, PGP, events)   │
│ theme.rs tokens   html.rs Blitz → RenderImage         │        │ auth.rs    OAuth PKCE loopback + OS keychain         │
└───────────────────────────────────────────────────────┘        │ ai.rs      OpenAI-compatible streaming chat          │
                                                                 └──────────────────────────────────────────────────────┘
```

## Rules of the road
- **One door.** The UI holds an `Arc<dyn Mail>`. Production uses `engine::Engine`, and UI tests use a fixture in `crates/app/tests/common/`. The UI never touches SQLite, tokio, or a backend directly.
- **Local-first.** `Mail` reads come from SQLite on a dedicated **reader connection**, which WAL keeps from blocking the writer and takes a few ms. Writes (`act`, `send`) commit locally in one short transaction, record a `pending_ops`/`outbox` row, wake the engine, and return. The engine drains ops to the remote with retry and backoff, then emits `Event::Changed`.
- **Optimistic UI.** On `e`, the view removes the row and moves the selection *in the same frame*, then calls `mail.act(...)`. It never waits on anything for paint.
- **Threading model.** GPUI owns the main thread. The engine spawns one background thread with a tokio runtime. Engine → UI traffic goes over `async_channel` (`Mail::events`), consumed by a `cx.spawn` loop in the root view. Blocking `Mail` calls that may be slow (`add_proton`, `add_microsoft`) run on `cx.background_executor()`.
- **Sync writer batching.** The engine applies deltas in chunks of ≤ 200 messages per transaction, so a UI write never waits behind a long first sync.
- **Undo** = the store keeps the inverse of each local action under an undo token. `undo(token)` reapplies the inverse locally and either cancels the not-yet-sent pending ops or enqueues inverse ops.
- **Undo send / send later**: `send` writes an `outbox` row with `due = now + undo_send_secs` (or `send_at`). The engine sends rows that are due. `cancel_send` deletes the row and returns the draft.
- **Snooze**: local only. The engine wakes threads at `snoozed_until`, moving them back to inbox and marking them unread. `remind_at` on a sent draft resurfaces the thread only if no reply arrived.
- **Threading**: The store unites account-scoped `Message-ID`/`In-Reply-To`/`References` links for every provider, regardless of arrival order. Matching subjects or participants never join conversations; Proton `ConversationID` is ignored. Graph parses `internetMessageHeaders` and `internetMessageId`; only when full headers are unavailable may `conversationId` serve as a fallback. Kite-composed replies and forwards retain the original message link; fresh compose has none. Drafts stay individual editable rows. Schema v7 rebuilds existing conversations from stored headers, preserving message IDs, local tags, reminders, drafts and undo; legacy Graph rows without stored reply headers retain the fallback.
- **HTML mail**: Blitz parses, styles (Stylo), lays out (Taffy) and paints (Vello CPU) each body into a BGRA `RenderImage`. It has no JS engine and no net provider, so remote images are blocked unless the reader allows them; allowed images are fetched by the engine (`Mail::fetch_image`) and inlined as `data:` URLs before Blitz sees the body. Renders are cached per (message, width, theme, scale) and re-rendered on resize after the drag settles. Clicks go through `Rendered::link_at` and open in the system browser. Known gap: no text selection inside HTML bodies yet. `c` (copy) copies the text part.
- **Secrets** (Bridge password, MS refresh token, Proton session + key passphrase, AI key) live only in the OS keychain (`keyring`). They never go into SQLite or logs.

## Contract files (orchestrator-owned)
`crates/core/src/model.rs` (types), `crates/core/src/api.rs` (`Mail`), `crates/core/src/sync/mod.rs` (backend boundary: `RemoteMsg`, `Delta`, `Op`).

## Ownership
Agents edit only their files. Shared manifests (`Cargo.toml`s) may only gain the lines your task needs. Pin new versions in the root `[workspace.dependencies]`.

| Area | Files |
|---|---|
| Contract + docs (orchestrator) | `model.rs`, `api.rs`, `sync/mod.rs`, `docs/*`, `AGENTS.md`, `.github/*` |
| Store | `crates/core/src/store.rs`, `crates/core/tests/store.rs` |
| IMAP | `crates/core/src/sync/imap.rs`, `crates/core/tests/imap.rs` |
| Bridge discovery | `crates/core/src/bridge.rs`, `crates/core/tests/bridge.rs` |
| Graph + auth | `crates/core/src/sync/graph.rs`, `crates/core/src/auth.rs`, `crates/core/tests/graph.rs` |
| Native Proton | `crates/core/src/sync/proton/**`, `crates/core/tests/proton.rs`, `crates/core/tests/common/proton_fake.rs` |
| Engine | `crates/core/src/engine.rs`, `crates/core/src/ai.rs`, `crates/core/tests/engine.rs` |
| UI shell | `crates/app/src/**` except `compose/`, `crates/app/tests/**` except `compose.rs` |
| Compose | `crates/app/src/compose/**`, `crates/app/tests/compose.rs` |

## Performance notes
- The list is GPUI `uniform_list` (fixed 88 px three-line rows, only visible rows are laid out; the day gutter is painted per row, never as separate header rows). The view keeps a window of `ThreadSummary` rows and fetches the next 200 rows when the scroll gets within 50 rows of the end.
- `threads()` is a single indexed SQL query over a denormalized `threads` table, with no N+1.
- On thread open, the UI prefetches `thread()` for the neighbouring threads and renders their HTML on a background thread, so `j`/`k` in thread view are instant.
- Blitz work (parse, style, layout, raster) runs on `cx.background_executor()`. Only the finished `RenderImage` crosses back to the UI thread.

## Native Proton backend

`sync/proton/` talks to Proton's HTTP API directly, so free accounts work without Bridge. It follows the same `sync(cursor)` / `apply(&Op)` / `attachment()` shape as `imap.rs` and `graph.rs` and is a `Backend` variant in the engine. It was ported from Proton's MIT-licensed `go-proton-api` and `go-srp`; Bridge (GPLv3) was read for behaviour only.
- `srp.rs`: Proton's SRP-6a variant, with bcrypt password hashing (salt + `"proton"`), the 256-byte `expandHash`, and little-endian group elements. Its proofs are checked byte for byte against go-srp's published `TestSRPauth` vector.
- `api.rs`: the HTTP client. It decides success or failure from the `Code` field, since the HTTP status is not enough. It handles 2FA, the mailbox password and human verification (code 9001, retried with the `x-pm-human-verification-*` headers), and token refresh.
- `crypto.rs`: unlocks keys with a passphrase derived from the mailbox password and the key salt, decrypts bodies and attachments, and builds send packages. Proton recipients get the session key sealed to their key; external recipients get cleartext.
- `mod.rs`: maps Proton labels to Kite mailboxes. A message carries a set of label ids rather than living in a folder. The first sync walks every system label's metadata, and after that the `/core/v4/events` id is the cursor. Bodies stay lazy, as with IMAP.
- **Sign-in:** `Mail::proton_sign_in` and `proton_sign_in_step` keep the half-finished login in the engine between steps, and return `ProtonAuth::{Done, TwoFactor, MailboxPassword, HumanVerification}`.
- **Dependencies:** `pgp` (rpgp), `bcrypt`, `num-bigint-dig` and `rand`, all with default features off. rpgp accounts for most of the ~2 MB binary growth, which is why the size budget is 24 MB.

## Sidebar and custom folder contract

`sidebar.rs` owns account/mailbox navigation in a cached GPUI view, invalidated when scope, folder catalog or theme changes, so list navigation does not rebuild the sidebar; `folders.rs` owns folder creation and moving captured conversation IDs. `View.account_id == None` means combined. `View.folder` is an opaque provider folder ID, paired with a specific account, and supersedes the standard mailbox filter. Folder IDs may collide across accounts. `Folder` exposes only ID, account and display name.

`Mail::folders()` must read locally cached folders. `Mail::create_folder(account, name)` may contact a provider and is called on the background executor. A successful return must persist the folder in the local catalog before returning. The default implementation reports unsupported creation and discovers no folders; it never reports fake success. The engine must validate ownership for `Action::Move`, persist the move locally, sync it, and support undo. Creation itself is not undoable through mail undo.

The engine is connected: production `main.rs` opens `kite_core::engine::Engine` against `engine::data_dir()` and hands the `Arc<Engine>` to `Kite::new`, so no placeholder `Mail` remains in the app. `Store` keeps the catalog in an account-scoped `folders` table (`PRIMARY KEY (account_id, id)`), `View.folder` requires `account_id`, and `Action::Move` is rejected unless the message's account owns the target folder. Provider folder creation is implemented (`Imap::create_folder` issues `CREATE` then `SUBSCRIBE` under `Folders/`; `Graph::create_folder` posts to `/me/mailFolders`) and the folder is persisted only after the provider accepted it. Provider folder *discovery* during sync is still outstanding, so `Mail::folders()` currently returns only folders this client created. The dev-only preview/test fixture implements the same contract with two accounts and retained mailbox/folder state.

## Appearance and preferences

`preferences/` owns the settings page: `mod.rs` is the shell (navigation rail, grouped-card helpers, save/apply, the `OPEN_*` rail indices the shortcuts and sidebar share) and each page owns one file and its own state — `appearance.rs` (theme gallery and import), `typography.rs` (size, family, colour, live preview), `accounts.rs` (connected mailboxes and removal), `connections.rs` (Proton Bridge discovery, Outlook, manual Bridge), `general.rs` (undo send), `privacy.rs` (remote images) and `ai.rs` (optional assistants). A page can change without touching the shell, and page state is reached only through `preferences::<page>::State`. `theme.rs` resolves owned palette values; imported palettes never require leaked static allocations. `Settings` gains serde-defaulted typography, custom palette and shortcut-footer fields. `CustomTheme` holds a display name, light/dark base and validated colour-token map. No account credentials are added to Settings.

Proton Bridge accounts are discovered rather than typed: `crates/core/src/bridge.rs` reads Bridge's own read-only vault (AES-256-GCM over msgpack, keyed by a master key in the OS keychain) to recover each account's primary address, generated Bridge password and the exact IMAP/SMTP ports Bridge chose, then returns `ProtonLogin` values through `Mail::discover_proton`. The vault key is used in memory for one call and never persisted. Discovery runs on the background executor and creates every discovered account that is not already connected, one after another, so opening Connections is the whole Proton setup; it never re-creates an account it already holds, and a failure stops the queue and falls back to the manual form. When the vault is readable but the port is dead, `Mail::start_proton_bridge` starts the installed `proton-bridge.exe` detached and the page waits up to 10 s for it to answer. Bridge keeps the Proton sign-in (SRP, 2FA, security keys) in its own window; Kite cannot and must not unlock it, so a Bridge that stays locked surfaces “finish signing in, then look again” rather than an error. The vault uses only a small msgpack subset, so `bridge.rs` carries its own focused read-only reader instead of a general codec — a full codec pushed the release binary past its then-20 MB budget. `rmp-serde` is therefore a dev-dependency, used only to build real fixture vaults for the core tests, which keep the focused reader honest. Keychain enumeration is Windows-only (the shipped target); other platforms report no accounts and the manual form remains.

The bottom switcher remains in the cached sidebar. The cached view is invalidated on account, folder, scope, theme or typography changes. Typography scales text and row height; the default 44px list density is unchanged. Every appearance change increments the HTML cache generation, so old background renders cannot replace newly styled mail. Provider account work is globally gated while a request is pending, even if Settings is closed and reopened.

Settings are owned by the engine: `Mail::settings`/`set_settings` persist them in the local SQLite store. The shell's old JSON `settings.json` helpers now live only in the app test fixture (`crates/app/tests/common/mod.rs`), so no production code exists solely for tests. serde_json remains a direct app dependency for the theme format and the fixture; it already existed in the workspace dependency graph. No new dependency package is introduced.

## Tags contract

A tag is a named, coloured marker on a conversation. Tags are local-only for now: `Action::Label` / `Action::Unlabel` change the store and undo log but queue no remote op. They are shaped so provider sync (Graph categories, Proton `Labels/` folders) can be added later as an op type without changing the UI contract. `Store` keeps `tags` and `thread_tags` as the truth and `threads.labels` as the denormalized cache that `ThreadSummary.labels` reads, so painting a row never joins. A tab shows a tag by being an ordinary split with the query `label:<name>`; the search grammar's `label:` operator is that join, and comma-OR works as it does for `from:`. A fresh store seeds the starter tags Newsletter, To Reply, Marketing, Calendar, Notification, Cold Email, Team and Urgent, none of them as tabs. Nothing assigns them automatically yet; Reply Zero and rules will.
