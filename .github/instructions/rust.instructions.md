---
applyTo: "**/*.rs"
description: "Rust coding standards for the pixtuoid workspace"
---

# Rust standards — pixtuoid

These apply to all Rust in this Cargo workspace. The authoritative source is the
root `CLAUDE.md` and the nested `crates/*/CLAUDE.md` — read those for the
architecture invariants and the "known sharp edges" (many things that look like a
bug are documented, load-bearing design). This file is the condensed
coding-standard slice.

## Errors & panics

- **No `unwrap()` / `expect()` in non-test code.** Tests may unwrap freely.
- App/binary code propagates errors via `anyhow::Result`. Core (`pixtuoid-core`)
  reaches for `thiserror` only when a typed error becomes load-bearing.
- The hook listener and JSONL watcher **log and continue** on malformed input —
  they never panic.
- The hook shim must **always exit 0 silently** on any error — blocking Claude
  Code breaks the user's primary workflow. The 200 ms write timeout is
  non-negotiable.

## Crate boundaries (load-bearing)

- `pixtuoid-core` has **no terminal dependencies** — never add `ratatui`,
  `crossterm`, or `stdout`/`println!` there. Terminal concerns live behind the
  `Renderer` trait.
- Events flow through **one** channel typed `mpsc::Sender<(Transport, AgentEvent)>`.
  Don't hardcode `Transport::Hook` on the consumer side — each `Source` tags its
  own events.
- The `Source` trait is the only seam for adding agent CLIs. Don't bypass it.

## Logging

- Use `tracing::{info, warn, error}` — **not** `println!`/`eprintln!`. The only
  exceptions are the headless summary and explicit user-facing CLI output.

## Tests (TDD-first)

- Write the failing test before the implementation. Don't add code without a test
  that exercises it.
- Unit tests: `#[cfg(test)] mod tests` next to the code, or a sibling `tests.rs`
  declared `#[cfg(test)] mod tests;` for large modules (keeps production readable
  without widening the API).
- Integration / public-contract tests live in `crates/<crate>/tests/*.rs` (they
  see only the `pub` API).
- Run the suite with `just test` (nextest). Scope with
  `cargo nextest run -p <crate> <filter>` while iterating. **Don't chain
  `cargo clippy && cargo test`** — they use separate build caches, so chaining
  recompiles the workspace twice. Run `just preflight` or one check at a time.

## Style

- **Comments explain WHY, not what** — only where a reader can't tell from the
  code (a workaround, a non-obvious constraint, a surprising invariant).
- DRY, YAGNI — no features beyond the current scope.
- Match the surrounding code's naming, idiom, and comment density.
- **Keep docs current** — a change to module structure, the public API, or a
  developer workflow updates the relevant `CLAUDE.md` / `README.md` in the *same*
  commit.
- Verify locally with `just preflight` (lint → clippy → test, the exact CI order)
  before pushing.
