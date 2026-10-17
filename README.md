<img src="assets/icon.svg" width="64" height="64" alt="">

# Kite

A keyboard-first, fully native email client that aims to be faster than Superhuman. It's pure Rust: GPUI for the UI, Blitz for HTML mail and SQLite for local-first storage. It supports Proton Mail (via Proton Bridge) and Microsoft 365 / Outlook (via Graph).

- Product spec: [docs/SPEC.md](docs/SPEC.md)
- Design: [docs/DESIGN.md](docs/DESIGN.md)
- Architecture: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- Contributor / agent guide: [AGENTS.md](AGENTS.md)

## Develop
```sh
cargo run -p kite          # first build compiles GPUI + Stylo (~3-5 min)
cargo test --workspace
```

To inspect the native UI with local sample conversations:

```sh
cargo run -p kite --example preview
cargo run -p kite --example preview -- --light --narrow
cargo run -p kite --example preview -- --thread
cargo run -p kite --example preview -- --palette
cargo run -p kite --example preview -- --create-folder
cargo run -p kite --example preview -- --move
```

The preview uses two sample accounts, account-owned Work/Receipts folders and local conversations. Folder creation, moving and undo change only the in-memory integration-test fixture; it sends no email. `--empty` shows
Inbox Zero. Sample messages and the preview are not included in the shipped app.
The current UI-shell build still starts without a connected mail engine.

## Microsoft account setup (one-time, ~2 min)
1. Go to https://entra.microsoft.com, then **App registrations → New registration**.
2. Name it `Kite`. For supported accounts, choose *Any Entra ID directory + personal Microsoft accounts*.
3. Under **Redirect URI**, choose platform *Public client/native (mobile & desktop)* and enter `http://localhost`.
4. Copy the **Application (client) ID** and set it as the `KITE_MS_CLIENT_ID` env var, or as `ms_client_id` in Kite's `config.json`.
5. Under **API permissions**, add Microsoft Graph delegated permissions: `offline_access`, `Mail.ReadWrite`, `Mail.Send`, `Calendars.Read`, `User.Read`.

## Proton setup
Install and sign in to [Proton Mail Bridge](https://proton.me/mail/bridge). In Kite, add a Proton account using the Bridge-generated password (not your Proton password).
