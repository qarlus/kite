Kite implementation report — 20 October 2026

This change improves the local desktop client. It does not connect an Outlook account automatically, merge a pull request, or deploy a service. New behavior becomes available when the rebuilt app is opened.

Delivered behavior:

- Settings occupies the full window without the mail sidebar. Its cog is in the top strip.
- The empty reading pane uses a flat background; the unintended checkerboard texture is removed. Ember keeps its intentional rail/list gradient.
- Shortcut settings show each binding once and use tighter spacing while preserving ordinary button sizes.
- Compose docks within the reading area, expands to fill it, and keeps its actions visible at the minimum window size. Bcc is editable. Malformed To/Cc/Bcc addresses block normal and scheduled sends; incomplete drafts remain editable. This checks syntax, not mailbox existence.
- Right-click menus support local tags, new tags, and triage actions. Shift+L opens the keyboard menu. Actions use the conversations captured by the menu, including multi-selection and the reading-pane preview.
- Private blocks Kite's AI helpers from sending a conversation's subject or body. Checks follow persisted reply/forward draft ancestry. Tags survive queued-draft hiding, undo send, and transfer to the Sent copy.
- Personal Outlook response handling and OAuth errors are improved. Settings explains the Microsoft application registration and validates the client ID's format.

The independent review found saved-draft summary bypasses, tag loss across sending, and context-menu actions targeting the wrong conversation. These were repaired and covered by integration tests using real SQLite, local protocol servers, or real headless GPUI views.

Contract and storage changes:

- `Mail::ai` now accepts `AiRequest` identifying a summary thread or compose draft. The engine builds prompts after checking privacy rather than accepting an arbitrary UI-built prompt.
- `PRIVATE_TAG` reserves the local Private tag. Catalog edits cannot silently delete or rename it; explicit removal from a conversation remains possible.
- SQLite schema v8 adds `draft_tags` and `sent_tags` to retain tags while drafts are hidden and Sent copies have not arrived. Outgoing associations are scoped to an account and exact Message-ID; Proton additionally uses the returned provider identity.
- No dependencies were added. The pre-existing local `.cargo/config.toml` build override is preserved and excluded from the commit.

Verification:

- `cargo test --workspace --no-fail-fast`: 324 passed, zero failed; one pre-existing documentation example ignored.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed, no warnings.
- `cargo build --release -p kite`: passed; binary is 22,682,624 bytes (21.63 MiB), below the 24 MiB limit.
- `cargo test --release -p kite-core --test store`: all 54 passed, including performance budgets.
- `cargo test --release -p kite --test perf --test html --test budget -- --nocapture`: all 8 passed. First frame with 10,000 threads: 1.61 ms (50 ms limit); median selection repaint: 2.80 ms (4 ms limit); median HTML render: 5.52 ms (15 ms limit). The size test passed against the new release binary.

Limits and next action:

- No live Microsoft authentication or live send was exercised. Kite needs one Microsoft application registration configured for personal accounts, a desktop `http://localhost` redirect, and public-client authentication. Supply its Application (client) ID in Settings → Connections and complete browser consent. A client secret is not needed.
- Graph/IMAP Sent-tag preservation is tested with preserved outgoing Message-ID. Behavior when a live server rewrites that ID remains unverified; Proton also has the returned provider-ID association.
- Private governs Kite's own AI helpers. It cannot restrict other local applications or coding agents with filesystem access, manually copied content, or requests already started before tagging. Tags are local, not provider labels.
- The original screenshot was unavailable in the resumed session. The background change is based on identifying the checkerboard code; layout and interaction checks use headless GPUI.
- Idle RAM and a complete live visual pass are not measured here.

Changed files (paths relative to the repository):

| Area | Files |
| --- | --- |
| App shell and menus | `crates/app/src/app.rs`, `keys.rs`, `lib.rs`, `list.rs`, `sidebar.rs`, `tags.rs`, `theme.rs`, `thread.rs` |
| Compose and settings | `crates/app/src/compose.rs`, `crates/app/src/preferences/connections.rs`, `mod.rs`, `shortcuts.rs` |
| App integration tests | `crates/app/tests/common/mod.rs`, `compose.rs`, `layout.rs`, `preferences.rs`, `settings_accounts.rs`, `settings_ai.rs`, `shortcuts.rs`, `tags.rs` |
| Core and contracts | `crates/core/src/ai.rs`, `api.rs`, `auth.rs`, `engine.rs`, `model.rs`, `store.rs`, `sync/graph.rs` |
| Core integration tests | `crates/core/tests/ai_privacy.rs`, `engine.rs`, `graph.rs`, `store.rs` |
| Documentation | `docs/ARCHITECTURE.md`, `DESIGN.md`, `SPEC.md`, `CHANGE_REPORT.md` |
