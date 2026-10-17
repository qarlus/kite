# Kite: Architecture

```
┌──────────── UI (SolidJS, src/) ────────────┐        ┌──────────────── Core (Rust, src-tauri/src/) ────────────────┐
│ App ─ keys.ts (global keymap)              │ invoke │ commands.rs  thin IPC glue                                  │
│  ├ List (virtualized)  ├ Thread (iframe)   │ ─────► │ store.rs     SQLite + FTS5: source of truth for the UI      │
│  ├ Palette  ├ Search   ├ Compose           │        │ engine.rs    sync loop, pending-op drain, outbox, snooze    │
│  └ Toasts   └ Calendar └ Settings          │ ◄───── │ sync/imap.rs Proton Bridge IMAP/SMTP                        │
│ ipc.ts (only backend access) types.ts      │ events │ sync/graph.rs Microsoft Graph + calendar                    │
└────────────────────────────────────────────┘        │ auth.rs      OAuth PKCE loopback + OS keychain              │
                                                      │ ai.rs        OpenAI-compatible streaming chat               │
                                                      └─────────────────────────────────────────────────────────────┘
```

## Rules of the road
- **Local-first.** Every read the UI does comes from SQLite. Every write (`thread_action`, `send`) commits locally in one transaction, records a `pending_ops`/`outbox` row, emits `kite://changed`, and returns. The engine drains ops to the remote in the background with retry and backoff.
- **Undo** = the store keeps the inverse of each local action under an undo token. `undo(token)` reapplies the inverse locally and either cancels the not-yet-sent pending ops or enqueues inverse ops.
- **Undo send / send later**: `send` writes an `outbox` row with `due = now + undo_send_secs` (or `send_at`). The engine sends rows whose due time has passed. `outbox_cancel` deletes the row and returns the draft.
- **Snooze**: local only (the thread moves to the virtual `snoozed` mailbox). The engine wakes it at `snoozed_until`, moving it back to inbox and marking it unread. `remind_at` on a sent draft creates a conditional snooze that only fires if no newer inbound message exists in the thread.
- **Threading**: Graph uses `conversationId`. IMAP uses `References`/`In-Reply-To` with `Message-ID` union, falling back to normalized subject + participants within 30 days.
- **HTML mail** is rendered in `<iframe sandbox srcdoc>` (no `allow-scripts`, no `allow-same-origin`) with an injected `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src data: cid:">`, plus `https:` when remote images are allowed. No sanitizer dependency is needed.
- **Secrets** (Bridge password, MS refresh token, AI key) live only in the OS keychain (`keyring` crate). They never go into SQLite or logs.
- **Events** (Rust → UI): `kite://changed {account_ids}` and `kite://ai {req, delta, done}`.

## IPC contract
`src-tauri/src/model.rs` ⇄ `src/types.ts`, with the commands listed in `src/ipc.ts` and `src-tauri/src/commands.rs`. Argument names are single lowercase words, so Tauri's camelCase↔snake_case mapping never matters.

## Ownership
Agents edit only their files. Shared files (`Cargo.toml`, `package.json`) may only gain the lines your task needs.

| Area | Files |
|---|---|
| Contract (orchestrator) | `model.rs`, `types.ts`, `ipc.ts`, `sync/mod.rs`, `docs/*`, `AGENTS.md` |
| Store | `src-tauri/src/store.rs`, `src-tauri/tests/store.rs` |
| IMAP | `src-tauri/src/sync/imap.rs`, `src-tauri/tests/imap.rs` |
| Graph + auth | `src-tauri/src/sync/graph.rs`, `src-tauri/src/auth.rs`, `src-tauri/tests/graph.rs` |
| UI shell | `src/**` except files owned below, `tests/e2e/*.spec.ts` (except the files below) |
| Compose | `src/compose/**`, `tests/e2e/compose.spec.ts` |
| Engine + wiring | `src-tauri/src/engine.rs`, `src-tauri/src/commands.rs`, `src-tauri/src/lib.rs` |
| AI + calendar + settings | `src-tauri/src/ai.rs`, `src/ai/**`, `src/calendar/**`, `src/settings/**`, matching e2e specs |

## Performance notes
- The list is virtualized (fixed 44px rows, only visible rows plus 10 overscan in the DOM). The UI fetches in pages of 200 and prefetches on scroll.
- `threads_list` is a single indexed SQL query, with no N+1 queries. The `threads` table is denormalized (subject, snippet, participants, flags, last date) and updated on write.
- SQLite runs in WAL mode with `synchronous=NORMAL`, `mmap_size`, and prepared statements cached via `prepare_cached`.
- On thread open, the UI prefetches the bodies of the next and previous threads (`thread_get`) so `j`/`k` in thread view are instant.
