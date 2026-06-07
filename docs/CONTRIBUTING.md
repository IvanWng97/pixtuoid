# Contributing to pixtuoid

Thanks for your interest! PRs are welcome — especially **new themes** and
**`Source` adapters** for other agent CLIs (Copilot, Cursor, OpenCode).

Before you start, read [`CLAUDE.md`](../CLAUDE.md) at the repo root (and the nested
`crates/*/CLAUDE.md` for the crate you touch). It holds the architecture
invariants, "known sharp edges", and conventions that are load-bearing here —
many things that look like bugs are documented, intentional design.

## Build & test

Requires a recent stable Rust toolchain and [`just`](https://github.com/casey/just)
(`brew install just`). On Linux you also need `lld` (`apt install lld`) —
`.cargo/config.toml` links x86_64-linux builds with it, matching CI. The
`justfile` is the single source of truth for what each check runs — CI and the
git hooks call the same recipes.

```bash
just              # list recipes
just preflight    # full pre-push gate: lint (fmt + machete + deny) → clippy → hack → test
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

## Releasing

Recipes are grouped by intent — run `just --list` to see them:

| To… | Run | What it touches |
| --- | --- | --- |
| **cut a release** | `just bump X.Y.Z` | every version number (workspace + the `pixtuoid`→`pixtuoid-core` path-dep + `Cargo.lock`) · drafts the in-app release notes · `just preflight` · commits on `release/vX.Y.Z` |
| **regenerate doc art** | `just demo` | `docs/images/*` (screenshots + `demo.gif`) from a release build |

`just bump` rewrites every version number in one shot via `cargo set-version`
(so the path-dep requirement can't drift — the classic missed edit), drafts the
`release_notes()` arm from the commit log since the last tag, runs the full
gate, and lands it on a release branch. It **stops before the tag** — pushing
the tag is what fires the *irreversible* crates.io publish, so a human owns that:

```bash
just setup-tools                            # once per clone — installs cargo-edit (+ the rest)
just bump 0.5.1                             # bump + draft notes + preflight → branch release/v0.5.1
# curate the drafted release_notes() bullets to ~6 highlights, then PR → review → merge, then:
git tag v0.5.1 && git push origin v0.5.1    # fires release.yml → build + crates.io + homebrew
```

## Conventions (the short version — see [`CLAUDE.md`](../CLAUDE.md) for the full set)

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

Step by step. The registration steps (4–6) are test-forced — skipping one
fails `just test`; steps 1–3 and 7–9 are on you (7, the runtime wiring, is the
historically-missed one):

1. **Verify the wire format against the CLI's actual source/releases first.**
   Where does it write transcripts, what does a line look like, does it have
   hooks, what identifies a session? Pin every fact to an upstream file/version
   in your comments — wire formats change without notice (`Task` → `Agent` did),
   and a guessed format decodes nothing (see the "Keeping the decode mapping
   current" section in `crates/pixtuoid-core/CLAUDE.md`).
2. **Write the source module** — `crates/pixtuoid-core/src/source/<name>.rs`
   with a `SOURCE_NAME` const, a `LineDecoder` fn (one JSONL line → `Vec<AgentEvent>`),
   a label deriver, and unit tests for every event mapping. Per-source format
   knowledge lives HERE, not in shared code.
3. **Implement the `Source` trait** (the watcher lifecycle). Your impl is a
   plain `async fn`:

   ```rust
   impl Source for MyCliSource {
       fn name(&self) -> &str { "my-cli" }
       async fn run(self: Box<Self>, tx: TaggedSender) -> anyhow::Result<()> {
           // watch + decode + tx.send(...) until the session universe ends
       }
   }
   ```

   (The trait itself declares `run` as `-> impl Future<Output = …> + Send` —
   the explicit form is what carries the `Send` bound `tokio::spawn` needs,
   so a non-`Send` future in your impl is a compile error, not a runtime
   surprise. `SourceManager` boxes sources via the object-safe `DynSource`
   twin; the blanket impl means you never name it.)

   **Hook-only CLI** (no watchable transcript — e.g. one that full-rewrites
   its session file per turn)? Skip the `LineDecoder`, the `Source` trait, and
   step 7: set `line_decoder: None` in the registry row, put the format
   knowledge in a `hook.custom` decoder (it must claim EVERY event — see the
   contract on `HookDecoding::custom`), and do step 8 (install target) instead
   — its hooks ride the shared socket.

4. **Add ONE `SourceDescriptor` row** in `crates/pixtuoid-core/src/source/registry.rs`
   — label prefix (2 chars), the line decoder, hook keying (`IdKey` + an
   optional custom hook decoder), and truthful capability flags (`has_exit_signal`,
   `resurrects_on_prompt`, `delegations_are_hook_silent`). Lifecycle policy
   derives from the flags; you do **not** edit the reducer.
5. **Add the name to `source::REGISTERED_SOURCES`** — a bridge test pins
   table↔list equality, and the conformance suite then REQUIRES a fixture.
6. **Drop a sanitized real-capture fixture** under
   `crates/pixtuoid-core/tests/fixtures/sources/<name>/<scenario>/`
   (transcript + hook payloads as applicable — see the fixtures README for the
   provenance/sanitization rules), then `cargo insta review` to accept the
   golden snapshot. The harness asserts all of a session's events coalesce to
   ONE `AgentId` — the duplicate-sprite bug class.
7. **Wire it into `runtime/driver.rs::run_async`** (`crates/pixtuoid/src/runtime/driver.rs`) —
   the runtime spawns sources by hand; the registry gates tests, not spawning.
8. **If the CLI has hooks**, add an `install/` target (`Target` registry row +
   a `merge_install`/`merge_uninstall` pair + a registered-events↔decoder-arms
   guard test) so `pixtuoid install-hooks --target <name>` wires the shim.
9. **Docs in the same PR**: README "Supported Tools" row, the nested
   `crates/pixtuoid-core/CLAUDE.md` entry, and — if the upstream is open
   source — a `scripts/check_upstream_drift.py` check so a silent rename
   pages us weekly.

See "Adding a new agent CLI" in [`CLAUDE.md`](../CLAUDE.md) and
`crates/pixtuoid-core/CLAUDE.md` for the deeper wiring detail (and the four
test files that must be updated together if you touch the shared contracts).

## License

By contributing, you agree your contributions are licensed under the same terms
as the project (see the **License** section of the [README](../README.md)).
