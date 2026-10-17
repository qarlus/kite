# Kite: Architecture

```
┌──────────── crates/app (GPUI, UI thread) ─────────────┐        ┌────────────── crates/core (kite-core) ───────────────┐
│ main.rs  window + Kite root view                      │        │ api.rs     trait Mail (the UI's only door)           │
│ app.rs   root: split bar, list, thread, overlays      │ Mail   │ engine.rs  impl Mail; owns tokio runtime thread:     │
│ keys.rs  actions + keymap (GPUI actions/key contexts) │ ─────► │            sync loop, op drain, outbox, snooze wake  │
│ list.rs  uniform_list virtualised thread list         │        │ store.rs   SQLite + FTS5: source of truth            │
│ thread.rs conversation view (html.rs via Blitz)       │ Event  │ sync/imap.rs  Proton Bridge IMAP/SMTP                │
│ palette.rs, search, snooze, toasts, compose/, …       │ ◄───── │ sync/graph.rs Microsoft Graph + calendar             │
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
- **Threading**: Graph uses `conversationId`. IMAP uses `References`/`In-Reply-To`/`Message-ID` union, falling back to normalized subject + participants within 30 days.
- **HTML mail**: Blitz parses, styles (Stylo), lays out (Taffy) and paints (Vello CPU) each body into a BGRA `RenderImage`. It has no JS engine and no net provider, so remote images are blocked unless a sender is allowed. Renders are cached per (message, width, theme, scale) and re-rendered on resize after the drag settles. Clicks go through `Rendered::link_at` and open in the system browser. Known gap: no text selection inside HTML bodies yet. `c` (copy) copies the text part.
- **Secrets** (Bridge password, MS refresh token, AI key) live only in the OS keychain (`keyring`). They never go into SQLite or logs.

## Contract files (orchestrator-owned)
`crates/core/src/model.rs` (types), `crates/core/src/api.rs` (`Mail`), `crates/core/src/sync/mod.rs` (backend boundary: `RemoteMsg`, `Delta`, `Op`).

## Ownership
Agents edit only their files. Shared manifests (`Cargo.toml`s) may only gain the lines your task needs. Pin new versions in the root `[workspace.dependencies]`.

| Area | Files |
|---|---|
| Contract + docs (orchestrator) | `model.rs`, `api.rs`, `sync/mod.rs`, `docs/*`, `AGENTS.md`, `.github/*` |
| Store | `crates/core/src/store.rs`, `crates/core/tests/store.rs` |
| IMAP | `crates/core/src/sync/imap.rs`, `crates/core/tests/imap.rs` |
| Graph + auth | `crates/core/src/sync/graph.rs`, `crates/core/src/auth.rs`, `crates/core/tests/graph.rs` |
| Engine | `crates/core/src/engine.rs`, `crates/core/src/ai.rs`, `crates/core/tests/engine.rs` |
| UI shell | `crates/app/src/**` except `compose/`, `crates/app/tests/**` except `compose.rs` |
| Compose | `crates/app/src/compose/**`, `crates/app/tests/compose.rs` |

## Performance notes
- The list is GPUI `uniform_list` (fixed 44 px rows, only visible rows are laid out). The view keeps a window of `ThreadSummary` rows and fetches the next 200 rows when the scroll gets within 50 rows of the end.
- `threads()` is a single indexed SQL query over a denormalized `threads` table, with no N+1.
- On thread open, the UI prefetches `thread()` for the neighbouring threads and renders their HTML on a background thread, so `j`/`k` in thread view are instant.
- Blitz work (parse, style, layout, raster) runs on `cx.background_executor()`. Only the finished `RenderImage` crosses back to the UI thread.

## Sidebar and custom folder contract

`sidebar.rs` owns account/mailbox navigation in a cached GPUI view, invalidated when scope, folder catalog or theme changes, so list navigation does not rebuild the sidebar; `folders.rs` owns folder creation and moving captured conversation IDs. `View.account_id == None` means combined. `View.folder` is an opaque provider folder ID, paired with a specific account, and supersedes the standard mailbox filter. Folder IDs may collide across accounts. `Folder` exposes only ID, account and display name.

`Mail::folders()` must read locally cached folders. `Mail::create_folder(account, name)` may contact a provider and is called on the background executor. A successful return must persist the folder in the local catalog before returning. The default implementation reports unsupported creation and discovers no folders; it never reports fake success. The engine must validate ownership for `Action::Move`, persist the move locally, sync it, and support undo. Creation itself is not undoable through mail undo.

This UI branch still has the placeholder core engine and production `main.rs` uses Empty Mail. The dev-only preview/test fixture implements the new contract with two accounts and retained mailbox/folder state. Connecting the merged engine and implementing provider folder discovery/creation plus `View.folder` queries remain separate integration work. Live account syncing and provider folder operations are not delivered by this branch.

## Appearance and preferences

`preferences.rs` owns the settings page, native theme import, account setup UI and the temporary shell's local settings-file helpers. `theme.rs` resolves owned palette values; imported palettes never require leaked static allocations. `Settings` gains serde-defaulted typography, custom palette and shortcut-footer fields. `CustomTheme` holds a display name, light/dark base and validated colour-token map. No account credentials are added to Settings.

The bottom switcher remains in the cached sidebar. The cached view is invalidated on account, folder, scope, theme or typography changes. Typography scales text and row height; the default 44px list density is unchanged. Every appearance change increments the HTML cache generation, so old background renders cannot replace newly styled mail. Provider account work is globally gated while a request is pending, even if Settings is closed and reopened.

The placeholder `Empty` Mail implementation persists settings as JSON under `%APPDATA%/Kite/settings.json` (XDG config/HOME fallback on other platforms), replacing via a temporary sibling file. The real engine continues to own `Mail::settings`/`set_settings` when connected. serde_json is now a direct app dependency for the theme format and shell persistence; it already existed in the workspace dependency graph. No new dependency package is introduced.
