# GitHub Copilot instructions — pixtuoid

pixtuoid is a terminal-native, multi-agent pixel-art visualizer for AI coding
agents — a Cargo workspace of three Rust crates (`pixtuoid-core` headless lib,
`pixtuoid` binary, `pixtuoid-hook` shim).

**Read [`CLAUDE.md`](../CLAUDE.md) first** (and the nested `crates/*/CLAUDE.md` for
the crate you touch). It holds the architecture invariants and "known sharp
edges" — much of what looks like a bug is documented, load-bearing design.
Path-scoped Rust standards: [`.github/instructions/rust.instructions.md`](instructions/rust.instructions.md).
Workflow + how to add a theme / agent-CLI `Source`: [`CONTRIBUTING.md`](../docs/CONTRIBUTING.md).

## Architecture invariants (never break these)

1. `pixtuoid-core` has **no terminal dependencies** — no `ratatui`, `crossterm`, or `stdout`/`println!`. Terminal concerns live behind the `Renderer` trait.
2. Events flow through **one** channel typed `mpsc::Sender<(Transport, AgentEvent)>`; the `Transport` tag is load-bearing (hook-wins dedup). Each `Source` tags its own events — don't hardcode `Transport::Hook` on the consumer side.
3. The **`Source` trait** is the only seam for adding a transcript-bearing agent CLI (hook-only CLIs like Reasonix instead ship a hook decoder + `install-hooks` target).
4. `install-hooks` writes through symlinks (`resolve_symlink`) — don't replace with `fs::rename`.
5. The hook shim must **never block Claude Code** — always exit 0 silently; the 200 ms write timeout is non-negotiable.
6. Walkable mask = **ground footprint only** (top-down view); visual sprites may be wider/taller.

## Conventions

- **No `unwrap()`/`expect()` in non-test code.** `anyhow::Result` in app code, `thiserror` in core. The hook listener and JSONL watcher log-and-continue; they never panic.
- **TDD first** — failing test → minimal impl. **DRY, YAGNI.**
- Use `tracing::{info, warn, error}`, not `println!`/`eprintln!`.
- **Comments explain WHY, not what.**
- **Keep docs current** — a module/API/workflow change updates the relevant `CLAUDE.md` / `README.md` in the *same* commit.
- Verify with `just preflight` (fmt → clippy → test) before pushing. Don't chain `cargo clippy && cargo test` — they use separate build caches (double rebuild).

## Build & test

```bash
just preflight                         # full gate: lint → clippy → test
just test                              # the suite (cargo-nextest)
cargo nextest run -p <crate> <filter>  # fast iteration on one crate
```
