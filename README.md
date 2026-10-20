<img src="assets/icon.svg" width="64" height="64" alt="">

# Kite

**A fast, private, keyboard-first email client for Proton Mail and Outlook.**

Kite is a native desktop app, not a web page in a wrapper. Your mail lives in a local database, so opening, searching and triaging mail happens instantly, even offline. Every action paints within a single frame.

- **Fast.** The inbox draws in under 2 ms with 10,000 conversations, moving through mail takes about 3 ms per keystroke, and the whole app is a ~22 MB download using ~75 MB of memory.
- **Keyboard-first.** Superhuman-style single-key shortcuts (`j`/`k`, `e` to archive, `r` to reply, `/` to search, `Ctrl+K` for everything). Prefer the mouse? Switch to Classic mode in Settings and every action becomes a button.
- **Private by design.** No tracking pixels, no read receipts, no analytics. Remote images are blocked until you allow them. Kite never sends your mail to its own servers; there are none.
- **Proton and Outlook together.** Native Proton Mail (free and paid accounts, no Bridge needed) and Microsoft 365 / Outlook.com in one combined inbox.

## Download

Get the latest version for Windows from **[Releases](https://github.com/qarlus/kite/releases/latest)** and run `kite-windows-x86_64.exe`. There's no installer; put the file wherever you like.

Windows may show a SmartScreen warning the first time, because the app isn't signed with a paid code-signing certificate yet. Choose **More info › Run anyway**.

**Updates:** Kite checks for new versions once a day and tells you when one is ready. Install it from **Settings › General › Update and restart**. Every update is cryptographically signed, and Kite refuses any download that doesn't match the signature.

macOS and Linux builds aren't published yet.

## Add your accounts

Open **Settings › Connections** (`Ctrl+,`).

- **Proton Mail:** enter your Proton address and password, then any two-factor code or mailbox password Proton asks for. Free and paid accounts both work. Kite talks to Proton's API directly and decrypts mail on your computer; your password is never stored.
- **Outlook / Microsoft 365:** click **Connect Outlook** and sign in in your browser. Work, school and personal Microsoft accounts are supported. Kite never sees your password.

Add as many accounts as you like. The rail on the left switches between them, or shows them all in one inbox.

## What it does

- **Split inbox:** tabs for Social, Newsletters, GitHub, or any search you save.
- **Instant search:** `from:`, `to:`, `subject:`, `has:attachment`, `is:unread`, `before:`, `after:` and more, updating as you type.
- **Snooze and reminders:** bring a conversation back later, or get reminded if nobody replies.
- **Undo send and send later:** a few seconds to take a message back, or schedule it.
- **Tags:** organise conversations with your own tags. The built-in **Private** tag keeps a conversation away from all AI features.
- **Per-account profiles:** display name, signature and avatar for each account (Outlook photos are picked up automatically).
- **Calendar:** today's agenda beside your mail (Outlook accounts).
- **Themes:** five built-in themes, any installed font, and custom themes you can import ([format](docs/THEMES.md)).

## AI (optional, off by default)

Kite can summarise a conversation (`Shift+S`) and help you write (`Ctrl+J`). Turn it on in **Settings › AI**.

Kite only offers **free models whose provider keeps no data and doesn't train on it**. The default is OpenCode Go's free zero-retention model; you can also use OpenRouter, where Kite asks for zero-retention providers only on every request. You need a free API key from the provider you choose. Conversations tagged **Private** are never sent to any AI.

## Keyboard shortcuts

| Key | Action |
|---|---|
| `j` / `k` | Next / previous conversation |
| `Enter` / `Esc` | Open / back to list |
| `e` | Done (archive) |
| `#` | Trash |
| `h` | Snooze |
| `s` | Star |
| `c` / `r` / `a` / `f` | Compose / reply / reply all / forward |
| `/` | Search |
| `Ctrl+K` | Command palette (every action, searchable) |
| `Ctrl+R` | Refresh mail |
| `z` | Undo |
| `?` | All shortcuts |

On macOS, `Ctrl` is `Cmd`. The full list is in [docs/SPEC.md](docs/SPEC.md#keyboard-map).

## Your data

Everything Kite stores stays on your computer, in `%APPDATA%\Kite` on Windows: the mail database (`kite.db`), settings and avatars. Passwords and sign-in tokens are kept in the Windows Credential Manager, never in files. If something goes wrong, `%APPDATA%\Kite\errors.log` records what happened, and is useful to attach to an [issue](https://github.com/qarlus/kite/issues).

---

## Building from source

You don't need this to use Kite; download a release instead. To build it yourself you need Rust 1.91 or newer.

```sh
cargo run -p kite                # first build compiles GPUI and Stylo (~3-5 min)
cargo test --workspace
cargo build --release -p kite    # target/release/kite(.exe)
```

To try the interface with sample conversations and no real accounts:

```sh
cargo run -p kite --example preview
cargo run -p kite --example preview -- --light --narrow --settings
```

Other flags: `--thread`, `--palette`, `--create-folder`, `--move`, `--switcher`, `--empty`.

### Using your own Microsoft app registration

Kite ships with its own Microsoft registration, so this is only for developers who want their own:

1. In https://entra.microsoft.com, create an app registration supporting *any Entra ID directory and personal Microsoft accounts*.
2. Add a *Public client/native* redirect URI `http://localhost`, and turn on **Allow public client flows**.
3. Add Microsoft Graph delegated permissions: `offline_access`, `Mail.ReadWrite`, `Mail.Send`, `Calendars.Read`, `User.Read`.
4. Set the **Application (client) ID** as the `KITE_MS_CLIENT_ID` environment variable.

### Releasing

Releases are built, signed and published by CI when a version tag is pushed. Bump `version` in the root `Cargo.toml`, commit, then:

```sh
git tag v0.2.0
git push origin v0.2.0
```

### Project docs

- [Product spec](docs/SPEC.md) · [Design](docs/DESIGN.md) · [Architecture](docs/ARCHITECTURE.md)
- [Contributor and agent guide](AGENTS.md)

## License

Kite is open source under the [MIT License](LICENSE).
