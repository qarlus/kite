# AGENTS.md — Kite

Kite is a keyboard-first desktop email client in the spirit of Superhuman. The goal is to be **faster and smaller than Superhuman**. Read this whole file before you touch code.

## Read first
- `docs/SPEC.md`: what we build (features, keyboard map, accounts).
- `docs/DESIGN.md`: how it looks (tokens, layout, motion). The UI must match it.
- `docs/ARCHITECTURE.md`: how it fits together (IPC contract, data model, sync, file ownership).

## Stack
- **Tauri 2** shell. **Rust** core in `src-tauri/` handles sync, storage, auth and sending.
- **SolidJS + TypeScript** UI in `src/`. Plain CSS with custom properties. No CSS framework, no component library, no router, no state library.
- **SQLite** (rusqlite, bundled, FTS5) as the local source of truth. The UI never waits on the network.
- Package manager: **pnpm**. Rust edition 2021.

## Prime directives
1. **Speed is the feature.** Every user action renders from local state in under 16ms. Network work is async and optimistic.
2. **No bloat.** Each dependency must justify itself in the PR description. Prefer std and small, focused crates. Never add a dependency for something that takes under 50 lines.
3. **Minimal code.** No speculative abstractions, no dead code, no commented-out code, and no "utils" dumping grounds. Write comments only for non-obvious *why*.
4. **Budgets are tests.** `pnpm test:budget` fails the build if a size or perf budget is exceeded (see below). Do not raise a budget to make a test pass. Make the code smaller instead.
5. **Stay in your lane.** Only edit the files your task owns (see `docs/ARCHITECTURE.md#ownership`). If a contract change is needed, make the smallest change to `src-tauri/src/model.rs` and `src/types.ts` together and call it out in your report.

## Budgets (enforced)
| Budget | Limit |
|---|---|
| JS bundle (gzip, all chunks) | ≤ 45 KB |
| CSS (gzip) | ≤ 10 KB |
| Release binary (Windows exe) | ≤ 12 MB |
| Inbox render, 10k threads (mock IPC) | first paint ≤ 100 ms |
| `j`/`k` keypress → selection painted | ≤ 16 ms |
| FTS search, 50k messages | ≤ 20 ms (Rust test) |

## Tests: the only kinds we write
- **E2E UI** (`tests/e2e/*.spec.ts`, Playwright): this drives the real built UI in Chromium with Tauri IPC mocked through `tests/e2e/mock-ipc.ts`, which is injected via `addInitScript`. Mock code **never** ships in `src/`.
- **Rust integration tests** (`src-tauri/tests/*.rs`): use a real SQLite and real protocol code against local fakes (an HTTP mock for Graph, an in-process scripted server for IMAP/SMTP).
- **Budget tests** (`scripts/budget.mjs`, plus the perf assertions inside e2e and Rust tests).
- No unit tests of private helpers, no snapshot tests, no tests that only assert mocks were called. Each test must fail if a user-visible behaviour breaks.

## Commands
```sh
pnpm i
pnpm dev            # vite only (UI against mock in browser is NOT supported; use tauri dev)
pnpm tauri dev      # full app
pnpm build          # UI production build -> dist/
pnpm test:e2e       # playwright against dist/
pnpm test:budget    # size budgets
cd src-tauri && cargo test
cd src-tauri && cargo clippy -- -D warnings
```

## Definition of done (every task)
- `cargo clippy -- -D warnings`, `cargo test`, `pnpm build`, `pnpm test:e2e` and `pnpm test:budget` all pass.
- No new warnings. No `unwrap()` on user/network data in non-test Rust (use `?` and `anyhow`).
- Commit on your branch with a clear message. Report: what you did, which files you touched, any contract changes, and any new dependency with its justification.
