# Contributing to pixtuoid

Thanks for your interest! PRs are welcome — especially **new themes** and
**`Source` adapters** for other agent CLIs (Copilot, Cursor, OpenCode).

Before you start, read [`CLAUDE.md`](CLAUDE.md) at the repo root (and the nested
`crates/*/CLAUDE.md` for the crate you touch). It holds the architecture
invariants, "known sharp edges", and conventions that are load-bearing here —
many things that look like bugs are documented, intentional design.

## Build & test

Requires a recent stable Rust toolchain and [`just`](https://github.com/casey/just)
(`brew install just`). The `justfile` is the single source of truth for what each
check runs — CI and the git hooks call the same recipes.

```bash
just              # list recipes
just preflight    # full pre-push gate: lint (fmt + machete + deny) → clippy → test
just fmt          # auto-format
just test         # the whole suite (cargo-nextest if installed, else cargo test)
```

While iterating on one crate, scope it for a much faster loop (seconds vs a full
workspace run):

```bash
cargo nextest run -p pixtuoid <filter>      # or: cargo test -p pixtuoid --lib <filter>
```

> **Don't chain `cargo clippy && cargo test`** — clippy and test use *separate*
> build caches, so chaining recompiles the whole workspace twice. Run
> `just preflight` (the exact CI order), or one check at a time.

### Git hooks

Activate once per clone:

```bash
git config core.hooksPath .githooks
```

`pre-commit` runs `just fmt-check` (sub-second); `pre-push` runs `just preflight`.
Run `just preflight` locally first to avoid the push → CI-red → fix round-trip.

## Conventions (the short version — see [`CLAUDE.md`](CLAUDE.md) for the full set)

- **TDD first** — failing test → minimal impl. Don't add code without a test that exercises it.
- **DRY, YAGNI** — no features beyond what the current scope specifies.
- **No `unwrap()` in non-test code.** Errors propagate via `anyhow::Result` (app code) / `thiserror` (core). The hook listener and JSONL watcher log-and-continue on malformed input — they never panic.
- **Comments explain WHY, not what** — only where a future reader can't tell from the code.
- **Keep docs current** — a change to module structure, the public API, or developer workflow updates the relevant `CLAUDE.md` / `README.md` in the **same commit**.
- **macOS-first** — BSD-flavored CLI; `shellcheck` any `.sh` you touch.
- **Sprite changes need visual verification** — see `.claude/skills/beautify-decoration/SKILL.md`.

## Architecture invariants (don't break these)

1. `pixtuoid-core` has **no terminal dependencies** (no `ratatui`/`crossterm`/`stdout`).
2. Events flow through **one** channel typed `mpsc::Sender<(Transport, AgentEvent)>`; the `Transport` tag is load-bearing (hook-wins dedup).
3. The **`Source` trait** is the only seam for adding agent CLIs.
4. `install-hooks` writes through symlinks (`resolve_symlink`) — don't replace with `fs::rename`.
5. The hook shim must **never block CC** — always exit 0 silently; the 200 ms write timeout is non-negotiable.
6. Walkable mask = **ground footprint only** (top-down view); visual sprites may be wider/taller.

## Pull requests

- Every PR is reviewed by **2+ agents** (explorer / reviewer / architect) before merge — no exceptions.
- AI-authored PRs get the `needs-human-verify` label and a human visual check before merge.
- Track every consciously-deferred finding as a GitHub issue (`gh issue create`) before moving on.

### Handy `gh` commands

```bash
gh pr checks --watch                         # live CI status (vs. polling)
gh pr merge --auto --squash --delete-branch  # auto-merge once checks pass
gh issue develop <number> --checkout         # a branch linked to an issue (auto-closes on merge)
gh run rerun --failed                        # rerun only the failed CI jobs
```

Useful extensions: `gh-poi` (prune merged local branches), `gh-dash` (PR/issue
TUI), `gh skill` (install Agent Skills, incl. into `.claude/skills/`).

## Adding a new agent CLI

Implement the `Source` trait, add it to `source::REGISTERED_SOURCES` (which forces
a coalescing fixture + label test via the conformance suite), and wire it into
`runtime::run_async` (the runtime spawns sources by hand):

```rust
#[async_trait]
pub trait Source: Send + 'static {
    fn name(&self) -> &str;
    async fn run(self: Box<Self>, tx: TaggedSender) -> anyhow::Result<()>;
}
```

Per-source JSONL format knowledge lives in the source's own decoder fn (injected
into `JsonlWatcher` via fn pointers), not a shared decoder. See "Adding a new
agent CLI" in [`CLAUDE.md`](CLAUDE.md) and `crates/pixtuoid-core/CLAUDE.md` for the
full wiring (and the four test files that must be updated together).

## License

By contributing, you agree your contributions are licensed under the same terms
as the project (see the **License** section of the [README](README.md)).
