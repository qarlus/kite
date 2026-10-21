# AGENTS.md — Kite

Kite is a keyboard-first, fully native desktop email client in the spirit of Superhuman. The goal is to be **faster and smaller than Superhuman**. Read this whole file before you touch code.

## Read first
- `docs/SPEC.md`: what we build (features, keyboard map, accounts).
- `docs/DESIGN.md`: how it looks (tokens, layout, motion, icons). The UI must match it.
- `docs/ARCHITECTURE.md`: how it fits together (crates, the `Mail` trait, threading model, file ownership).

## Stack
- **Rust only.** It's a Cargo workspace:
  - `crates/core` (`kite-core`): storage, sync backends, auth, engine, AI. No UI code.
  - `crates/app` (`kite`): the GPUI desktop app.
- **UI:** GPUI via **`gpui-kit` =0.7.3**, which bundles GPUI (`gpui-pre` 0.3.9), `gpui-component` (60+ styled components) and Lucide icons. Import everything through `gpui_kit::*`. Never depend on `gpui`/`gpui-pre` directly.
  - API reference: read the sources under `~/.cargo/registry/src/*/gpui-kit-0.7.3`, `gpui-component-0.7.3` and `gpui-pre-0.3.9`, especially their `examples/` and `tests/`. Do not guess APIs from memory: GPUI changes often, and older snippets online are usually wrong.
- **HTML mail:** Blitz (`blitz-html`/`blitz-dom`/`blitz-paint` =0.3.0-beta.4) rasterised with `anyrender_vello_cpu` into a GPUI image (`crates/app/src/html.rs`). If Blitz gets something wrong, patch it (or work around it in our injected stylesheet) rather than switching engines.
- **SQLite** (`rusqlite`, bundled, FTS5) is the local source of truth. The UI never waits on the network.
- **Async:** GPUI's executor on the UI side. The engine runs its own tokio runtime on a background thread. They talk only through the `Mail` trait and `Event` channel.
- **TLS:** the OS stack via `native-tls` (Schannel / Security.framework). Never use rustls/ring.

## Prime directives
1. **Speed is the feature.** Every user action paints from local state within one frame (≤ 16 ms). Network work is async and optimistic.
2. **No bloat.** Each dependency must justify itself in your report. Prefer std and small, focused crates. Never add a dependency for something that takes under 50 lines. Turn off default features you don't need.
3. **Minimal code.** No speculative abstractions, no dead code, no commented-out code, and no "utils" dumping grounds. Write comments only for non-obvious *why*.
4. **Budgets are tests.** Do not raise a budget to make a test pass. Make the code smaller or faster instead.
5. **Stay in your lane.** Only edit the files your task owns (see `docs/ARCHITECTURE.md#ownership`). If a contract change is needed (`model.rs`, `api.rs`, `sync/mod.rs`), make the smallest change and call it out in your report.

## Budgets (enforced)
Timing budgets are checked locally before every merge (shared CI runners are too noisy for them).

| Budget | Limit | Where |
|---|---|---|
| Release binary (Windows exe) | ≤ 24 MB | `crates/app/tests/budget.rs`, and `release.yml` on every release |
| Inbox first frame, 10k threads | ≤ 50 ms | UI test, release |
| `j`/`k` → selection repainted | ≤ 4 ms per frame | UI test, release |
| Blitz render, typical email | ≤ 15 ms | `crates/app/tests/html.rs`, release |
| FTS search, 50k messages | ≤ 20 ms | `crates/core/tests/store.rs` |
| Idle RAM, 10k threads | ≤ 120 MB | manual check before release |

## Tests: the only kinds we write
- **UI integration tests** (`crates/app/tests/*.rs`): use `#[gpui_kit::test]` and `gpui_kit::test::{TestWindowExt, …}` (see `gpui-kit-0.7.3/TESTING.md`). They open the real Kite views in a headless window, press real keys (`cx.simulate_keystrokes`) and assert on what the user sees (`window.find(id).label()`, focus, selection) and on calls reaching the `Mail` fixture. Test fixtures live in `crates/app/tests/common/` and never ship.
- **Core integration tests** (`crates/core/tests/*.rs`): use a real SQLite and real protocol code against local fakes (an HTTP fake for Graph, an in-process scripted IMAP/SMTP server).
- **Budget tests**, as in the table above.
- No unit tests of private helpers, no snapshot tests, no tests that only assert mocks were called. Each test must fail if a user-visible behaviour breaks. Name elements with `.id("…").test_support().aria_label("…")` so tests can find them.

## Commands
```sh
cargo run -p kite                      # run the app
cargo test --workspace                 # all tests
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --release       # perf budgets are only asserted in release
cargo build --release -p kite          # target/release/kite(.exe)
# Faster perf-budget loop (thin LTO); confirm with --release before merging:
cargo test --profile perf -p kite --test perf --test html && cargo test --profile perf -p kite-core --test store
```
The first build compiles GPUI and Stylo (~3–5 min). After that, builds are incremental. `.cargo/config.toml` uses `rust-lld` on Windows for fast links.

## Definition of done (every task)
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass with no new warnings.
- No `unwrap()`/`expect()` on user or network data in non-test code (use `?` and `anyhow`).
- Commit on your branch with a clear message. Report: what you did, which files you touched, any contract changes, and any new dependency with its justification.
