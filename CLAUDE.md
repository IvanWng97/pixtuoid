# CLAUDE.md

Instructions for Claude Code (or any AI coding agent) working in this repo.
This is the **workspace-level** guide — conventions, invariants, and rules that
apply everywhere. **Module-level detail and the crate-specific "sharp edges"
live in nested `CLAUDE.md` files**, which Claude Code auto-loads when you touch
files in those trees:

- [`crates/pixtuoid-core/CLAUDE.md`](crates/pixtuoid-core/CLAUDE.md) — the headless lib: sources, reducer/state, sprites, layout, physics, pose.
- [`crates/pixtuoid/CLAUDE.md`](crates/pixtuoid/CLAUDE.md) — the binary: install, runtime, cli, config, multi-floor, embedded pack.
- [`crates/pixtuoid/src/tui/CLAUDE.md`](crates/pixtuoid/src/tui/CLAUDE.md) — the terminal renderer: draw_scene, pixel painter, widgets, themes, motion/pose authority, pathfinding.

**Read the nested guide for the crate you're editing.** Many things that look
like a bug are documented, load-bearing design — the "Known sharp edges" section
in each nested file (indexed below) explains why.

## What this is

Terminal-native, multi-agent pixel-art visualizer for AI coding agents. Each running CC (Claude Code) session shows up as an animated half-block sprite in an ASCII office. Built in Rust as a Cargo workspace of three crates.

User-facing overview: [`README.md`](README.md).

(Design specs/plans are kept locally under `docs/superpowers/` but are not
versioned — see `.gitignore`.)

## Layout (workspace)

```
crates/
├── pixtuoid-core/      headless lib — no terminal deps (ratatui/crossterm forbidden here)
│                       source/ state/ sprite/ render/ layout/ physics.rs pose/ walkable.rs
│                       → see crates/pixtuoid-core/CLAUDE.md for module-level detail
├── pixtuoid/           binary — ratatui + crossterm + tokio + clap
│                       cli.rs config.rs runtime/ install/ tui/ sprites/ (default/robot/skeleton packs)
│                       → see crates/pixtuoid/CLAUDE.md and crates/pixtuoid/src/tui/CLAUDE.md
└── pixtuoid-hook/      tiny shim CC invokes — stdin JSON → Unix socket (Unix) / named pipe (Windows) via transport.rs, 200ms send bound
scripts/                crop-snapshot.py (visual verification),
                        gen-docs-images.py (regenerate ALL docs/images screenshots + demo.gif
                        from a release build — single source of truth; run via `just demo`),
                        replay-fixture.sh (replay a captured source rollout fixture into a
                        headless run via --codex-sessions-root, for eyeballing lifecycle),
                        check_upstream_drift.py (weekly CI: CC/Codex wire-format rename watch)
site/                   Astro marketing landing page → GitHub Pages (ivanwng97.github.io/pixtuoid).
                        Self-contained Node project; own CI (.github/workflows/site.yml) + deploy
                        (.github/workflows/pages.yml). `just site-{setup,dev,check,fmt,demos}`;
                        demo art is generated from the binary by scripts/gen-demos.sh.
                        → see site/README.md
```

> Note: the `sprites/` directory (default / robot / skeleton character packs) lives under
> **`crates/pixtuoid/`**, not `pixtuoid-hook/`. The default pack is embedded into the binary
> via `include_str!`; the robot/skeleton packs are loadable examples.

## Build & test

```
just build                                                           # debug build
just build --release                                                 # release build
just test                                                            # all tests (600+) — nextest if installed, else cargo test
cargo test -p pixtuoid --lib <filter>                                # fast iteration: one crate's unit tests only
cargo run --release --example snapshot -- /tmp/snap.png              # render TUI to PNG
./target/release/pixtuoid run --headless --projects-root ~/.claude/projects   # live test against real CC
```

The `test-renderer` feature is needed for the `e2e.rs` integration test; **`just test` injects it for you** (as does every recipe), so prefer it over a raw `cargo test`. `just test` runs `cargo nextest run` when `cargo-nextest` is installed (parallel execution, the same runner CI uses) and falls back to `cargo test` otherwise. While iterating on one crate, scope it (`cargo nextest run -p pixtuoid` or `cargo test -p pixtuoid --lib <filter>`) — seconds, vs a full-workspace run.

> **Don't chain `cargo clippy && cargo test`.** Clippy and test/nextest use *separate* build caches (clippy's rustc driver has a different fingerprint), so chaining them recompiles the whole workspace **twice**. Run the single gate `just preflight` (lint → clippy → hack → test, the exact CI order), or run one check at a time.

### Test organization (three tiers)

- **Unit tests** — `#[cfg(test)] mod tests` next to the code. For large modules this is a *sibling file* declared `#[cfg(test)] mod tests;` (e.g. `motion/tests.rs`, `pose/tests.rs`, `layout/tests.rs`, `pixel_painter/tests.rs`) so production stays readable; it keeps `use super::*` and full crate-internal access (no API widening).
- **Integration / public-contract** — `crates/<crate>/tests/*.rs` (separate crate, only `pub` API): `reducer.rs`, `e2e.rs`, `hook_socket.rs`, `jsonl_watcher.rs`. One deliberate exception: `socket_path_parity.rs` `#[path]`-includes the hook shim's `paths.rs` (source inclusion, not a dep) to pin shim↔daemon socket-path equality across crates without violating the no-core-dep-in-shim invariant; it's `exclude`d from the published tarball (the included file can't exist there).
- **Headless render harness** — `tui_renderer/harness.rs` (`#[cfg(test)] mod harness;`). Drives the *real* `TuiRenderer` through `render()` / `navigate_floor()` via ratatui `TestBackend` (no terminal). Output-first assertions: `buf()` (RgbBuffer pixels) + the `#[cfg(test)] frame_buffer()` ratatui-cell inspector; white-box seams (`floor_motion`, `floor_buf`, `inject_coffee`) only where an invariant isn't observable from output. NOT coverable headlessly (excluded in `codecov.yml`): the crossterm event loop + terminal lifecycle (`tui/mod.rs`, reads/writes the real TTY), the async runtime glue (`runtime/driver.rs`, tokio `block_on` + `ctrl_c` + socket bind), and `main.rs`.

Coverage: `just coverage` (writes lcov.info + JUnit XML — the exact command CI runs).

### Visual verification (Python venv)

```
python3 -m venv .venv
.venv/bin/pip install -r requirements-dev.txt
just build --release --example snapshot
./target/release/examples/snapshot --cols 192 --rows 80 /tmp/snap.png
.venv/bin/python3 scripts/crop-snapshot.py /tmp/snap.png --scale 3
```

> To regenerate **all** of `docs/images/` (screenshot, gallery-\*, themes-composite, demo.gif)
> from a release build, run **`just demo`** (→ `scripts/gen-docs-images.py`) — the single source
> of truth for the office images (render params, crop quadrants, themes-composite diagonal), so
> the screenshots never drift.

See `.claude/skills/beautify-decoration/SKILL.md` for the full iteration loop, self-critique checklist, and sprite-format pitfalls.

### Pre-commit / pre-push preflight

The `justfile` is the **single source of truth** for what each check runs —
`.github/workflows/ci.yml` and the git hooks call the same recipes, so there's
no CI-vs-local drift to maintain. Requires `just` (`brew install just`); the
checks also need a handful of cargo tools — `just setup-tools` installs them
(cargo-hack, cargo-nextest, cargo-machete, cargo-deny, cargo-semver-checks, cargo-edit).

```
just              # list recipes
just setup-tools  # install the dev tools the checks need (run once per clone)
just preflight    # full pre-push gate: lint (fmt+machete+deny, parallel) → clippy → hack → test
just fmt          # auto-format
```

(`hack` is `cargo hack --feature-powerset` — it catches code that only builds
with `test-renderer` on. `semver` and `coverage`/`smoke` are CI-only. The
semver gate checks **`pixtuoid-core` only** — the `pixtuoid` binary's lib
target is an internal-facing surface for examples/integration tests, not a
semver surface, so its `pub` paths may move without a major bump.
`check-windows` cross-checks the workspace for `x86_64-pc-windows-msvc` on every PR (compile-only, no linking).)

Run `just preflight` locally to avoid the round-trip of "push → wait for CI →
red → fix → push again."

`.githooks/pre-commit` runs `just fmt-check` only (sub-second).
`.githooks/pre-push` runs `just preflight` before pushing (honors `SKIP_PREFLIGHT=1`).

Activate hooks **once per clone**:

```
git config core.hooksPath .githooks
```

Bypass in an emergency with `git commit --no-verify` or `SKIP_PREFLIGHT=1 git push`.

### Cutting a release

`just bump X.Y.Z` is the one command. It rewrites **every** version number — the
workspace version, the `pixtuoid`→`pixtuoid-core` path-dep requirement, and
`Cargo.lock` (via `cargo set-version`, so the path-dep can't drift) — drafts the
in-app `release_notes()` arm from the commit log, runs `just preflight`, and
commits on a `release/vX.Y.Z` branch. It **stops before the tag**: pushing the
tag is what triggers the *irreversible* crates.io publish (`release.yml`), so
that stays a human step.

```
just bump 0.5.1                              # bump + draft notes + preflight → branch
# curate release_notes() to ~6 highlights → PR → merge, then:
git tag v0.5.1 && git push origin v0.5.1     # fires build + crates.io + homebrew
```

Needs cargo-edit (`just setup-tools`). See [`CONTRIBUTING.md`](docs/CONTRIBUTING.md#releasing).

## Conventions

- **TDD first.** Plan and existing tests are TDD-shaped — failing test → minimal impl → commit. Don't add code without a test that exercises it.
- **DRY, YAGNI.** No features beyond what v1 specifies. v2 items are deferred — adding them in v1 code is a regression.
- **No comments unless WHY.** Don't write comments that restate what the code does. Comment only when a future reader can't tell from the code why something is the way it is (a workaround, a non-obvious constraint, a surprising invariant).
- **Errors propagate via `anyhow::Result` in app code, `thiserror` in core if a typed error becomes load-bearing.** The hook listener and JSONL watcher log + continue on malformed input — they never panic.
- **No `unwrap()` in non-test code.** Tests can unwrap freely.
- **Match the surrounding shell:** scripts in this repo target zsh (interactive) or POSIX sh. `shellcheck` any `.sh` you touch.
- **macOS first.** BSD-flavored CLI, brew, launchd for daemons. The hook shim uses a Unix socket on Unix (`std::os::unix::net::UnixStream`) and a named pipe on Windows — transport selection is in `pixtuoid-hook/src/transport.rs`.
- **Keep docs current.** When a change alters module structure, architecture, developer workflow, or the public API surface, update the relevant `CLAUDE.md` (workspace or nested) and `README.md` in the same commit. Stale docs cost more than the 5 minutes to update them.
- **Track every deferred finding as a GitHub issue.** When a review finding, bug, or improvement is real but you consciously defer it (out of scope for the current PR, low-priority, needs broader design), `gh issue create` to capture it BEFORE moving on — don't let it live only in a chat message or a PR comment that scrolls away. The issue body should state the problem, why it was deferred, and a concrete fix sketch (link the PR/review that surfaced it). A deferred finding with no issue is a silently-dropped finding. (Verify the finding is real first — see "Don't blindly accept reviewer findings" below; never file an issue for a hallucinated/refuted one.)
- **Sprite changes require visual verification.** After editing any `.sprite` file: (1) rebuild the snapshot example, (2) render at `--cols 192 --rows 80`, (3) crop the relevant quadrant with `scripts/crop-snapshot.py --scale 3`, (4) read the cropped PNG and self-critique — does a stranger recognize the intended pose/object? Iterate until it reads at half-block scale. Then rebuild the release binary (`just build --release`). Commit messages should include iteration history (which designs were tried and why they were rejected). See `.claude/skills/beautify-decoration/SKILL.md` for the full checklist.

## Architecture invariants

These are load-bearing; don't break them without updating the spec.

1. **`pixtuoid-core` has no terminal dependencies.** No `ratatui`, no `crossterm`, no `stdout` writes. If you need one, the abstraction belongs behind the `Renderer` trait.
2. **Events flow through ONE channel** typed `mpsc::Sender<(Transport, AgentEvent)>`. The `Transport` tag is load-bearing — the reducer uses it for hook-wins dedup. Do not hardcode `Transport::Hook` on the consumer side; the producer (each Source impl) tags its own events.
3. **`Source` trait is the only seam for adding agent CLIs** (Codex / Cursor / Copilot). Don't bypass it. Per-source JSONL format knowledge lives in the source's own decoder fn (injected into `JsonlWatcher` via fn pointers), not in a shared decoder.
4. **`install-hooks` writes through symlinks.** `resolve_symlink` in `install/io.rs` is critical for stow-managed `~/.claude/settings.json`. Don't replace it with `fs::rename` on the symlink path.
5. **The hook shim must never block CC.** Always exit 0 silently on any error. The 200ms send bound is non-negotiable (Unix: socket write timeout; Windows: a watchdog thread bounding the whole connect+write phase).
6. **Walkable mask = ground footprint only.** This is a top-down view. Visual sprites can be wider/taller than their ground footprint (elevation effects, shadows, wall trim). The walkable mask must only block the ground-level projection — e.g., a 3px-wide wall visual has a 1px walkable mask because the wall's base is 1px. Characters walk right next to walls, not 3px away.

## Known sharp edges (index)

Don't be surprised by these. **Full explanation (the WHY) lives in the nested `CLAUDE.md` for the owning crate** — read it before "fixing" any of them.

**`pixtuoid-core`** (see [`crates/pixtuoid-core/CLAUDE.md`](crates/pixtuoid-core/CLAUDE.md)):
- CC hook payloads DO include `tool_use_id` (hook-wins dedup fires).
- CC hook `transcript_path` always points to the PARENT transcript → `active_tasks` suppresses subagent-leak; **liveness flows UP** (`refresh_lineage`) so a working subagent keeps its ancestors fresh and a long delegation isn't stale-swept.
- JSONL watcher gates historical/ended transcripts on EVERY first-sight path (initial seed, 250ms rescan, 60s poll, notify) — not just startup: `should_seed_at_eof` in `walk_jsonl` (1-hour mtime window + session-end tail scan → seed cursor at EOF, no `SessionStart`). Unifying this gate was the #85 fix.
- Agent removal needs a `SessionEnd`; abrupt exits (Ctrl-C / Codex) have none → fall back to the slow stale-sweep, which cascade-exits the parent's whole subagent subtree — but only once the subtree is genuinely silent (liveness-vs-readiness guards: `refresh_lineage` up-propagation + `has_waiting_ancestor` exemption for permission-blocked subagents).
- Subagent display names come from `attributionAgent` in JSONL.
- The subagent-dispatch tool is **`Agent`** in current CC (not `Task`); `make_tool_detail` maps both → `ToolDetail::Task`. Missing the name silently disables subagent-leak suppression + b1 completion.
- Codex subagents (`spawn_agent`) wire into the scope tree via the `SubagentStart`/`SubagentStop` HOOKS (distinct `agent_id` + parent `session_id`), since their rollout is flat (no `/subagents/` path); the reducer's `SessionStart` arm enriches a JSONL-first orphan's `parent_id`. Regression: `tests/codex_subagent_lifecycle.rs`.
- `AgentSlot.state_started_at` is `SystemTime` (process-local; v2-daemon-ready type).
- `ActivityState::Active` ≠ "tool currently executing" — Active→Idle is debounced (`ACTIVE_GRACE_WINDOW`).

**`pixtuoid` / `tui`** (see [`crates/pixtuoid/CLAUDE.md`](crates/pixtuoid/CLAUDE.md) and [`crates/pixtuoid/src/tui/CLAUDE.md`](crates/pixtuoid/src/tui/CLAUDE.md)):
- `draw_scene` is called through `TuiRenderer` (owns cross-frame state, returns the cached `Layout`).
- `recolor_frame` substitutes by RGB equality (each palette key must map to a unique RGB).
- Terminal cell aspect drives sprite design (~16×16 px ceiling; bundled character pack maxes at 8×12).
- Snap-back and exit walks are time-compressed to fit their GC windows; entry/wander are not.
- A walk leg's A\* polyline shape is frozen once per leg, not re-routed per frame (prevents the "flash").

## Things NOT to do

- Don't add `ratatui` / `crossterm` / terminal anything to `pixtuoid-core`.
- Don't write to `~/.claude/settings.json` directly. Always go through `install/io.rs::write_config_atomic` (advisory lock + atomic rename + symlink resolution).
- Don't add `println!` / `eprintln!` to any production path other than the headless summary and explicit user-facing CLI output. Use `tracing::{info, warn, error}` instead.
- Don't relax the hook shim's "always exit 0" contract. Blocking CC = breaking the user's primary workflow.
- Don't add `--no-verify` / hook-skipping flags to any git operations performed in this repo.
- Don't generate a README / CLAUDE.md / CHANGELOG / docs in PRs unless explicitly asked.
- Don't `git push` without explicit user confirmation, even after committing.
- Don't merge a PR without running the code review process (2+ agents: explorer/reviewer/architect). No exceptions — PR #23 was merged without review and had a critical path-traversal vulnerability.
- Don't blindly accept reviewer findings. Verify the premise before coding a fix — the reviewer may have incomplete context about design intent. Check the relevant "Known sharp edges" and existing comments first. If a fix contradicts an earlier design decision, trace the code path manually.

## Where to look (cross-cutting)

- "How does a CC tool call become a moving sprite?" → trace `runtime/driver.rs::run_async` → `SourceManager::spawn` → `ClaudeCodeSource::run` → `HookSocketListener::run` → `decoder::decode_hook_payload` → `reducer::Reducer::apply` → (reducer publishes `Arc<SceneState>` on a `watch` channel) → `TuiRenderer::render` → `render_to_rgb_buffer` (the terminal-agnostic pixel pass any PNG/GIF/web renderer reuses) → `draw_scene` (top-down, cubicle grid). The first half lives in `pixtuoid-core`, the render half in `pixtuoid/tui` — see those nested guides for the per-stage detail.
- **Architecture overview + the data-flow diagram → [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)** (rendered on the site at `/architecture`; the single source for the public/contributor-facing architecture narrative).

Area-specific "Where to look" entries (layout, sources, install, themes, motion, weather, pets, …) are in the nested `CLAUDE.md` for the owning crate/module.

## When refactoring

If you change anything in the channel type, `Source` trait, `AgentEvent` enum, or reducer signature, update **all four** test files that exercise them: `tests/reducer.rs`, `tests/e2e.rs`, `tests/hook_socket.rs`, `tests/jsonl_watcher.rs`, plus `runtime/driver.rs` on the binary side. The `AgentEvent::agent_id()` method in `source/mod.rs` needs a new arm too if you add a variant.

**Adding a new agent CLI**: the source module + ONE `SourceDescriptor` row in `source/registry.rs` (label prefix, decoders, hook keying, reducer caps — the per-source fact table) + the name in `source::REGISTERED_SOURCES` (forces a coalescing fixture via tests) **and** — for a source with a watchable transcript — wire it into `runtime/driver.rs::run_async` (the runtime spawns sources by hand — the registry only gates the conformance tests, not runtime wiring); a hook-only CLI (`line_decoder: None`) skips the `Source` trait and the runtime wiring, shipping a `hook.custom` decoder + an `install/` target instead. See the nested `crates/pixtuoid-core/CLAUDE.md` "multi-source decoding" entry.
