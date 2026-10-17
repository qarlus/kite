# Kite: Product Spec (v1)

Kite is a keyboard-first email client modelled on Superhuman. Every action has a shortcut, every view renders from local state, and the inbox is a to-do list you clear.

## Accounts
| Provider | Transport | Auth |
|---|---|---|
| **Proton Mail** | IMAP + SMTP via Proton Bridge (`127.0.0.1`, IMAP 1143 / SMTP 1025, STARTTLS, Bridge self-signed cert) | Bridge username + password, stored in the OS keychain |
| **Microsoft 365 / Outlook** | Microsoft Graph v1.0 (`/me/mailFolders`, `/me/messages/delta`, `/me/sendMail`, `/me/calendarView`) | OAuth 2.0 Auth Code + PKCE, public client, loopback redirect `http://localhost:{port}`. Client ID from `KITE_MS_CLIENT_ID` env var or `config.json`. Refresh token kept in the OS keychain. Scopes: `offline_access Mail.ReadWrite Mail.Send Calendars.Read User.Read` |

Multiple accounts are supported. Account switching uses `Ctrl/Cmd+1..9`.

## Core (Superhuman parity)
- **Split inbox**: tabs across the top (`Important`, `Other`, plus user-defined splits by query, e.g. `from:github.com`). Tab and Shift+Tab cycle through splits. Important = (Graph) `inferenceClassification == focused`, or (IMAP) the sender has been replied to before.
- **Done = archive.** `e` marks done (archives); Inbox Zero shows a calm full-bleed empty state.
- **Thread view**: conversation with collapsed older messages, the latest one expanded. HTML is rendered natively by Blitz, with no scripts and no network. Remote images are blocked by default, with per-sender allow. `Shift+V` opens the original in the system browser as an escape hatch.
- **Command palette** (`Cmd/Ctrl+K`): fuzzy search over every command, showing its shortcut.
- **Instant search** (`/`): local FTS5 with operators `from: to: subject: has:attachment is:unread before: after:`. Results update on each keystroke.
- **Compose** (`c`), reply (`r`), reply all (`a`), forward (`f`). Plain text with minimal formatting (bold, italic, link, list). `Cmd/Ctrl+Enter` sends.
- **Undo send**: a 5 s delay window (configurable 0–30 s). `z` or the toast button cancels.
- **Undo** (`z`) for the last archive/trash/snooze/label action.
- **Snooze / remind** (`h`): presets (later today, tomorrow, weekend, next week, custom with natural language like "fri 9am"). There is also an optional "remind me if no reply in N days" on send.
- **Send later** (`Cmd/Ctrl+Shift+Enter` in compose). This is a local scheduled outbox, so the app must be running. Overdue items send on next launch.
- **Read status**: not implemented (no tracking pixels, by design).
- **Calendar sidebar** (Graph accounts only): today's agenda in the right pane, toggled with `Cmd/Ctrl+Shift+C`.
- **AI (optional, off by default)**: "Write with AI" in compose (`Cmd/Ctrl+J`) and "Summarize thread" (`Shift+S`). This works with any OpenAI-compatible Chat Completions endpoint. The default preset is **OpenCode Go** with `deepseek-v4.1-flash`. Base URL, model and API key are configured in Settings, and the key is stored in the keychain. Responses stream.

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
| `l` | label / move to folder |
| `x` | select (multi-select, then actions apply to the selection) |
| `c` | compose |
| `r` / `a` / `f` | reply / reply all / forward |
| `/` | search |
| `Cmd/Ctrl+K` | command palette |
| `Tab` / `Shift+Tab` | next / previous split |
| `g i` `g s` `g d` `g t` `g h` `g a` | go to inbox / starred / drafts / sent / snoozed / all mail |
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
Read receipts, tracking pixels, rich HTML compose, Gmail/IMAP providers other than Proton Bridge, mobile, plugins.
