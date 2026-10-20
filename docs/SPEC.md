# Kite: Product Spec (v1)

Kite is a keyboard-first email client modelled on Superhuman. Every action has a shortcut, every view renders from local state, and the inbox is a to-do list you clear.

## Accounts
| Provider | Transport | Auth |
|---|---|---|
| **Proton Mail (native, primary)** | Proton's HTTP API directly (`https://mail.proton.me/api`): `/auth/v4` SRP login, `/core/v4/events` for incremental sync, OpenPGP bodies and attachments decrypted locally (rpgp). Works with **free and paid** accounts. Ported from Proton's MIT-licensed `go-proton-api`/`go-srp`; uses `Other` as its API app version and identifies Kite in its User-Agent | Email + password (SRP), then 2FA code, mailbox password or human verification as Proton asks. Only the session (UID, refresh token) and the derived key passphrase are kept, in the OS keychain; the password is never stored |
| **Microsoft 365 / Outlook** | Microsoft Graph v1.0 (`/me/mailFolders`, `/me/messages/delta`, `/me/sendMail`, `/me/calendarView`) | OAuth 2.0 Auth Code + PKCE, public client, loopback redirect `http://localhost:{port}`. Kite signs in as its own built-in Microsoft Entra app registration (any org directory + personal accounts; public client flows on; redirect `http://localhost`); the `KITE_MS_CLIENT_ID` env var overrides it in development. Users never enter an app ID. Refresh token kept in the OS keychain. Scopes: `offline_access Mail.ReadWrite Mail.Send Calendars.Read User.Read` |

Multiple accounts are supported. The icon rail on the left shows one avatar per account (initials, unread badge) and an all-inboxes icon for the combined view. Account switching uses `Ctrl/Cmd+1..9`; `Ctrl/Cmd+0` returns to the combined inbox. Split tabs and standard mailbox navigation preserve the selected account.

The mailbox picker in the list header offers Inbox, Drafts, Sent, Starred, Snoozed, Archive, Spam, Trash and All mail, plus the account's custom folders. Custom folders belong to an account; the combined sidebar identifies their owner. New folder opens an account chooser and name field (`Ctrl/Cmd+Shift+N`). Creation runs off the UI thread, reports errors, and adds the folder only after the provider accepted it. Folders created in Kite are recorded locally and on the provider; Proton folders created in another client are discovered on label events. `L` moves selected conversations into Inbox, Archive, Trash or a custom folder from their account. Mixed-account selections expose only the shared standard destinations. Moving is undoable. Creating a folder does not move any mail automatically.

## Core (Superhuman parity)
- **Split inbox**: category tabs sit above the mailbox heading and are **filters**, not a partition: `Inbox` shows the whole mailbox, while `Social`, `Newsletters` and `GitHub` are non-exclusive lenses over it, so a conversation can appear in both `Inbox` and its category. The defaults are `Inbox` (no query), `Social` (`from:` with a comma-separated domain set), `Newsletters` (`is:newsletter`, set from the `List-Unsubscribe`/`List-Id` headers) and `GitHub` (`from:github.com`). Splits are stored queries ANDed with the selected mailbox, so users can edit or add their own (e.g. `from:github.com`); Tab and Shift+Tab cycle through them.
- **Done = archive.** `e` marks done (archives); Inbox Zero shows a calm full-bleed empty state.
- **Thread view**: a single click opens the conversation, with collapsed older messages and the latest expanded. The reading view shows the account, subject, complete From/To/Cc names and addresses, full local date and time with UTC offset, and attachment names and sizes. Clicking an attachment delegates opening to the engine. The selection checkbox and `X` select without opening. HTML is rendered natively by Blitz, with no scripts and no network. Remote images are blocked by default (replaced before rendering, so a blocked body never touches the network). A bar offers to load them for that one message, and Settings › Privacy can load them everywhere. Fetches go through the engine (`Mail::fetch_image`): http(s) only, 10 s, ≤ 4 MB, never to loopback or private-network addresses. `Shift+V` opens the original in the system browser as an escape hatch.
- **Command palette** (`Cmd/Ctrl+K`): fuzzy search over every command, showing its shortcut.
- **Instant search** (`/`): local FTS5 with operators `from: to: subject: has:attachment is:unread is:read is:starred is:important is:snoozed is:newsletter before: after:`, plus `-` to negate any term. A comma inside `from:`/`to:` means OR within that term (`from:a.com,b.com`), while two separate terms are ANDed. Results update on each keystroke.
- **Compose** (`c`), reply (`r`), reply all (`a`), forward (`f`). Plain text with minimal formatting (bold, italic, link, list). `Cmd/Ctrl+Enter` sends. Replying to your own message addresses its original recipients; `Re:`/`Fwd:` prefixes never stack; forwards carry the original header block; sending needs at least one recipient in To, Cc or Bcc.

Compose docks within the reading area and can expand to fill that area. To, Cc and Bcc are editable; field errors block normal and scheduled sends for malformed addresses while incomplete drafts remain saveable. Validation checks supported address syntax, not whether the destination mailbox exists.
- **Drafts**: compose autosaves 1.5 s after typing stops and on close, showing Saving…/Saved. Drafts sync to the provider's Drafts folder (Graph `POST /me/messages`, Proton draft API), reopen from Drafts into compose, and are deleted on the provider once sent. `Cmd/Ctrl+Shift+D` discards.
- **Undo send**: a 5 s delay window (configurable 0–30 s). `z` or the toast button cancels. This, and send later, are the only way to take a message back: **recall is not possible** (Graph v1.0 and Proton expose no recall), so it is deliberately not offered.
- **Undo** (`z`) for the last archive/trash/snooze/label action.
- **Snooze / remind** (`h`): presets (later today, tomorrow, weekend, next week, custom with natural language like "fri 9am"). There is also an optional "remind me if no reply in N days" on send.
- **Send later** (`Cmd/Ctrl+Shift+Enter` in compose). This is a local scheduled outbox, so the app must be running. Overdue items send on next launch.
- **Read status**: not implemented (no tracking pixels, by design).
- **Calendar sidebar** (Graph accounts only): today's agenda in the right pane, toggled with `Cmd/Ctrl+Shift+C`.
- **AI (optional, off by default)**: "Write with AI" in compose (`Cmd/Ctrl+J`) and "Summarize thread" (`Shift+S`). This works with any OpenAI-compatible Chat Completions endpoint. The default preset is **OpenCode Go** with `deepseek-v4.1-flash`. Base URL, model and API key are configured in Settings, and the key is stored in the keychain. Responses stream.
- **Tags**: `Shift+L` (or the palette, or right-click on a row or the open conversation) opens the tag menu: every tag as a checkable item over the conversation or the multi-selection, "New tag…" for a one-off label, and the usual triage actions. Tags paint as a chip on the list row; Private's chip carries a lock.

Conversations tagged **Private** are excluded from Kite's AI helpers: the engine rejects summaries and AI writing from protected saved drafts, replies and forwards before contacting the endpoint ("This conversation is private, so AI can't read it"; subjects count as content). The reserved tag is always present and cannot be renamed or deleted through tag-catalog edits; users can explicitly remove it from a conversation or undo tagging. It is local to Kite and does not control other applications, coding agents with filesystem access, or content manually copied into a new message. Applying it cannot retract a request already sent to an AI service.

Mail sync starts when Kite opens. Native Proton events poll every 25-40 seconds with independent random jitter; Refresh requests sync immediately, at most once every 10 seconds, and never bypasses a provider retry deadline. A send that is permanently rejected or exhausts eight attempts returns to Drafts with an error toast.

## Input modes and shortcuts
Settings › General chooses the input mode:
- **Keyboard** (default): the single-key Superhuman map below.
- **Classic**: click-first. Only modifier shortcuts (`Ctrl/Cmd+…`) stay bound; every action is a visible button. That includes the list header's Compose button, the hover action strip on list rows and the reading-pane toolbar.

Settings › Shortcuts lists every command by group with its current keys. Each can be rebound, which captures the next keystroke and warns about conflicts. Each can also be unbound or reset. Overrides persist in `Settings.shortcuts`, and every hint in the UI (`?` sheet, palette, tooltips, aria labels) reflects them. The table below is the default map.

## Keyboard map
| Key | Action |
|---|---|
| `j` / `k` | next / previous thread (or message, in thread view) |
| `Enter` / `o` | open thread |
| `Esc` / `u` (in thread) | back to list |
| `e` | mark done (archive) |
| `Shift+E` | move back to inbox |
| `#` | trash |
| `h` | snooze / remind |
| `s` | star / flag |
| `Shift+U` | toggle read / unread |
| `l` | move to folder |
| `Shift+L` | tag conversation (tag menu) |
| `x` | select (multi-select, then actions apply to the selection) |
| `c` | compose |
| `r` / `a` / `f` | reply / reply all / forward |
| `/` | search |
| `Cmd/Ctrl+K` | command palette |
| `Cmd/Ctrl+R` | refresh mail (also the list header refresh button) |
| `Tab` / `Shift+Tab` | next / previous split |
| `g i` `g s` `g d` `g t` `g h` `g a` | go to inbox / starred / drafts / sent / snoozed / all mail |
| `g e` / `g p` / `g b` | archive / spam / trash |
| `Cmd/Ctrl+0` / `Cmd/Ctrl+1..9` | combined inbox / account |
| `Cmd/Ctrl+Shift+N` | create folder |
| `z` | undo |
| `?` | shortcut cheat sheet |
| `Cmd/Ctrl+Enter` | send |
| `Cmd/Ctrl+Shift+Enter` | send later |
| `Cmd/Ctrl+J` | AI write (compose) |
| `Shift+S` | AI summarize thread |
| `Cmd/Ctrl+Shift+C` | toggle calendar |
| `Shift+V` | view original in browser |
| `Cmd/Ctrl+,` | settings |

## Non-goals (v1)
Read receipts, tracking pixels, rich HTML compose, Gmail and generic IMAP providers, mobile, plugins.

## Settings and appearance

`Ctrl/Cmd+,` opens Settings; `Ctrl/Cmd+Shift+,` opens Accounts directly. Escape or Back to mail returns to the previous mail view. `Ctrl/Cmd+Shift+A` opens the bottom account switcher; arrows/Enter select, Escape dismisses. Mail navigation shortcuts never consume text in editable fields.

Appearance offers Carbon, Paper, Midnight, Linen, Ember (a warm red/orange gradient behind the rail and list) and Match system, plus the reading-pane switch (`Settings.reading_pane`: list and conversation side by side, or the conversation full width). A native file picker imports a JSON palette, validates its tokens and readability, then saves its contents rather than a reference to the source file. [Theme format](THEMES.md). Typography supports any installed font family, a 12–18px base font size and an optional six-digit hex text colour. Explicitly styled HTML email keeps its authored styles; our native UI and default HTML styling follow the user's preference. Changes invalidate HTML caches, including renders still in flight.

Connections offers native Proton sign-in first (email, password, then the 2FA / mailbox password / verification step Proton asks for), then a single Connect Outlook button that opens Microsoft's browser sign-in. Accounts lists connected mailboxes with confirmed removal from Kite. All provider operations use Mail on a background executor and report failures. Sign-in runs against the production engine, and appearance and general settings persist in the local SQLite store through `Mail::settings`/`set_settings`.

General saves the 0/5/10/20/30-second undo-send window. Sending remains engine-owned. New settings fields use serde defaults to preserve existing preferences.
