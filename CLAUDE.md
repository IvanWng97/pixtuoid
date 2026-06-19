# CLAUDE.md

Instructions for Claude Code (or any AI coding agent) working in this repo.
(`AGENTS.md` is a symlink to this file for the cross-tool standard; a Windows
checkout without `core.symlinks` materializes it as a one-line pointer — read
this file.)
This is the **workspace-level map** — conventions, invariants, and rules that
apply everywhere. **Module-level detail and the crate-specific "sharp edges"
live in nested `CLAUDE.md` files**, auto-loaded when you touch those trees:

- [`crates/pixtuoid-core/CLAUDE.md`](crates/pixtuoid-core/CLAUDE.md) — the headless lib: sources, reducer/state, sprites, layout, physics, pose.
  - [`crates/pixtuoid-core/tests/CLAUDE.md`](crates/pixtuoid-core/tests/CLAUDE.md) — the integration-test layout (8 test binaries: six grouped + two flat publish-excluded; parity twins) + add-a-CLI test steps.
- [`crates/pixtuoid-scene/CLAUDE.md`](crates/pixtuoid-scene/CLAUDE.md) — the backend-agnostic render+sim engine CRATE (`pixtuoid-core ← pixtuoid-scene ← pixtuoid`): pixel painter (render_to_rgb_buffer), layout, pose/motion authority, pathfinding, the theme MODEL, weather/ambient, pets, chitchat, frame_cache, embedded_pack.
- [`crates/pixtuoid/CLAUDE.md`](crates/pixtuoid/CLAUDE.md) — the binary: install, runtime, cli, config, multi-floor, embedded pack.
  - [`crates/pixtuoid/src/tui/CLAUDE.md`](crates/pixtuoid/src/tui/CLAUDE.md) — the terminal painter (over the `pixtuoid-scene` crate): draw_scene flush, harness, widgets, the theme-PICKER ui, Sources panel, dashboard, hit_test, version popup.

**Read the nested guide for the crate you're editing.** Many things that look
like a bug are documented, load-bearing design — the "Known sharp edges"
section in each nested file (indexed below) explains why.

## What this is

Terminal-native, multi-agent pixel-art visualizer for AI coding agents. Each
running CC (Claude Code) session shows up as an animated half-block sprite in
an ASCII office. Rust workspace of four crates. User-facing overview:
[`README.md`](README.md). (Design specs live locally under
`docs/superpowers/`, unversioned.)

## Layout (workspace)

```
crates/                 DAG: pixtuoid-core ← pixtuoid-scene ← pixtuoid (+ standalone pixtuoid-hook)
├── pixtuoid-core/   headless lib — no terminal deps (ratatui/crossterm forbidden)
│                    source/ state/ sprite/ render/ layout/ physics.rs pose/ walkable.rs
├── pixtuoid-scene/  backend-agnostic render+sim ENGINE crate — terminal AND window-free BY CRATE
│                    BOUNDARY (no ratatui/crossterm/winit/softbuffer in its Cargo.toml; just arch enforces)
│                    pixel_painter/ (render_to_rgb_buffer) layout/ pose/ motion/ pathfind/ floor/ theme/
│                    pet/ chitchat/ frame_cache/ anim/ overlay/ font/ embedded_pack/ (default pack at
│                    sprites/default/, own build.rs); depends on pixtuoid-core
├── pixtuoid/        binary — ratatui + crossterm + winit + tokio + clap; depends on pixtuoid-scene
│                    cli.rs config.rs runtime/ install/ tui/ floating/ (two thin painters over the
│                    pixtuoid-scene crate; neither depends on the other) sprites/ (skeleton embedded via
│                    include_str!, robot --pack-dir-loadable)
└── pixtuoid-hook/   tiny shim CC invokes — stdin JSON → Unix socket / Windows named
                     pipe (transport.rs), 200ms send bound
scripts/             gen-media.py + media.json (the ONE manifest-driven driver for ALL
                     docs/images + site demos + CI visual baselines → `just gen-media`),
                     crop-snapshot.py (visual verify), gen-readme.mjs (README sections
                     from site/src/*.json), compare-screenshots.py (`just gen-check`),
                     replay-fixture.sh (replay a captured rollout headlessly),
                     openclaw-live-e2e.sh (zero-cost HERMETIC daemon live-e2e: drives the real
                     shim with crafted OpenClaw envelopes on an isolated socket → asserts the lobster's
                     idle/busy/degraded/down via the headless `daemons=` line, incl. #317 degraded
                     + #318 mid-attach pid-adopt→kill→down),
                     openclaw-cc-backend-e2e.sh (NON-hermetic: starts a REAL `openclaw gateway run`
                     + one `openclaw agent` turn on the claude-cli backend → proves the gateway
                     the lobster AND its backend `cc·<workspace>` coding sprite coexist live; real
                     account/gateway footprint, NOT a CI test),
                     check_upstream_drift.py (weekly wire-format watch),
                     review-metrics.py (review-economics collector)
site/                Astro landing page → GitHub Pages; self-contained Node project,
                     own CI; `just site-{setup,dev,check,fmt}` → see site/README.md
integrations/raycast/  Raycast extension (TypeScript, self-contained Node project; NOT Rust):
                     `Manage Sources` (connect/disconnect over `pixtuoid sources|connect|disconnect
                     --json`) + `Start Floating` commands. A thin shell over the CLI `--json`
                     contract — does NOT bundle the binary; resolves it via login-shell PATH +
                     a binary-path preference. Own CI (.github/workflows/raycast.yml: tsc + eslint;
                     `ray build`/`ray lint` need the macOS app, run before store publish). See its README.
```

## Build & test

```
just build [--release]                                  # build
just test                                               # all tests (1,400+), nextest if installed
cargo test -p pixtuoid --lib <filter>                   # fast iteration: one crate's unit tests
cargo run --release --example snapshot -- /tmp/snap.png # render TUI to PNG
./target/release/pixtuoid run --headless --projects-root ~/.claude/projects  # live vs real CC
```

The `test-renderer` feature is needed by `e2e.rs`; every `just` recipe
injects it — prefer `just test` over raw `cargo test`. While iterating,
scope to one crate (seconds vs a full-workspace run).

> **Don't chain `cargo clippy && cargo test`** — they use separate build
> caches and recompile the workspace twice. Run `just preflight` (lint →
> clippy → hack → test, the exact CI order) or one check at a time.

**Test organization (three tiers):** unit tests next to the code (large
modules use a sibling `#[cfg(test)] mod tests;` file — keeps `use super::*`
without API widening); integration tests in `crates/<crate>/tests/` —
pixtuoid-core's suite is 8 binaries (six capability-grouped + two
deliberately flat publish-excluded) with `#[cfg(windows)]` parity twins, all mapped in
[`crates/pixtuoid-core/tests/CLAUDE.md`](crates/pixtuoid-core/tests/CLAUDE.md);
the headless render harness (`tui_renderer/harness.rs`) drives the real
`TuiRenderer` through ratatui `TestBackend` — see the tui guide. Coverage:
`just coverage`. Decoder never-panic fuzz vs a real session corpus:
`just fuzz <jsonl-dir>` (on-demand, not in CI).

### Visual verification

```
just build --release --example snapshot
./target/release/examples/snapshot --cols 192 --rows 80 /tmp/snap.png
.venv/bin/python3 scripts/crop-snapshot.py /tmp/snap.png --scale 3   # venv: requirements-dev.txt
```

A PR that **intentionally** changes the office's look must run `just gen`
and commit the regenerated `docs/images/` (incl. the `reference-*.png` CI
baselines) plus `site/public/demos/` in the same change, or the smoke job's
`just gen-check` pixel-diff goes red. Full iteration loop + sprite pitfalls:
`.claude/skills/beautify-decoration/SKILL.md`.

### Preflight, hooks, release

The `justfile` is the single source of truth for every check — CI and the
git hooks call the same recipes (no local-vs-CI drift). `just setup-tools`
installs the needed cargo tools once per clone.

```
just preflight    # full pre-push gate: lint (fmt+machete+deny+arch) → clippy → hack → test
just fmt          # auto-format
git config core.hooksPath .githooks   # activate hooks once per clone
```

Never pipe `preflight` through `tail`/`head` — the exit code becomes the
pipe's and a real failure reads as green; redirect to a file and `echo $?`.
CI-only gates: semver (pixtuoid-core only — the binary's lib target is not a
semver surface), coverage/smoke, gen-check, gen-readme-check, npm-check,
check-windows (cross-lint for msvc on every PR).

**Release:** `just bump X.Y.Z` rewrites every version number, drafts
`release_notes()`, runs preflight, and commits on a release branch — it
stops before the tag; pushing the tag is the irreversible crates.io publish
and stays a human step. See
[`CONTRIBUTING.md`](docs/CONTRIBUTING.md#releasing).

## Conventions

- **TDD first.** Failing test → minimal impl → commit. Don't add code without a test that exercises it. Non-trivial changes (new feature/config key/seam, sharp edge, or spanning ≥3 files) plan against [`.github/prompts/impl-plan.prompt.md`](.github/prompts/impl-plan.prompt.md) first — it front-loads the review's failure classes, and its answers fill the review's change-specific slots.
- **DRY, YAGNI.** No features beyond what v1 specifies; v2 items are deferred.
- **No comments unless WHY.** Comment only what a future reader can't tell from the code (a workaround, a non-obvious constraint, a surprising invariant).
- **Errors propagate via `anyhow::Result` in app code, `thiserror` in core** if a typed error becomes load-bearing. The hook listener and JSONL watcher log + continue on malformed input — they never panic.
- **No `unwrap()` in non-test code.** Tests can unwrap freely.
- **Layer-internal items stay `pub(crate)`, not `pub`.** `unreachable_pub` is `warn` in `[workspace.lints.rust]` and CI's `just clippy` (`-D warnings`) makes it a hard gate — a `pub` item in a private module tree fails the build. Reserve bare `pub` for genuinely cross-crate API (and in `pixtuoid-core`, only those reach the semver surface). The lint is the mechanical enforcement of "the install/uninstall entry points are `pub(crate)`, `crate::sources` is the only caller" and every other inter-layer seam.
- **No scan-the-history logic.** Keep persistent state (a set, a map, a bool) updated as events arrive; never derive state by scanning backward through time.
- **Match the surrounding shell** (zsh interactive / POSIX sh); `shellcheck` any `.sh` you touch. **macOS first**: BSD CLI, brew, launchd.
- **Keep docs current.** A change that alters module structure, architecture, workflow, or public API updates the relevant `CLAUDE.md` + `README.md` in the same commit.
- **Every review adjudication leaves a trace** in [`docs/REVIEW-LEDGER.md`](docs/REVIEW-LEDGER.md) (premise-anchored protocol in its header; economics in `docs/review-metrics/`). A finding refuted as "deliberate design" MUST cite an existing sharp edge or add one in the same change.
- **Track every deferred finding as a GitHub issue** BEFORE moving on — problem, why deferred, fix sketch. A deferred finding with no issue is a silently-dropped finding. (Verify it's real first — see "Don't blindly accept reviewer findings".)
- **Sprite changes require visual verification** — render, crop, read the PNG, self-critique until it reads at half-block scale; commit messages carry the iteration history. Full checklist: `.claude/skills/beautify-decoration/SKILL.md`.
- **Periodic context-file audits also distill memory**: each `/revise-claude-md`-style audit sweeps recent session memories for promote-to-repo candidates (the memory layer of [`docs/KNOWLEDGE-BASE.md`](docs/KNOWLEDGE-BASE.md)).

## Architecture invariants

These are load-bearing; don't break them without updating the spec.

1. **`pixtuoid-core` has no terminal dependencies.** No `ratatui`, no `crossterm`, no `stdout` writes. If you need one, the abstraction belongs behind the `Renderer` trait. **`pixtuoid-scene` (the render+sim engine) is ALSO terminal- AND window-free** — and now COMPILER-enforced by the crate boundary: `ratatui`/`crossterm`/`winit`/`softbuffer` aren't in its `Cargo.toml`, so reaching for one won't compile. `just arch` covers BOTH crates. Terminal/window code lives in the `pixtuoid` binary's painters (`tui/`, `floating/`).
2. **Events flow through ONE channel** typed `mpsc::Sender<(Transport, AgentEvent)>`. The `Transport` tag is load-bearing — the reducer uses it for hook-wins dedup. Do not hardcode `Transport::Hook` on the consumer side; the producer tags its own events.
3. **`Source` trait is the only seam for adding a transcript-bearing agent CLI.** Per-source format knowledge lives in the source's own decoder fn, not a shared decoder. A **hook-only** CLI (Reasonix) is the documented exception — see `crates/pixtuoid-core/CLAUDE.md` "multi-source decoding".
4. **Hook install writes through symlinks.** `install::install_target`/`uninstall_target` (driven by the in-TUI Sources panel `s` — there is no `install-hooks` CLI) go through `resolve_symlink` in `install/io.rs`, critical for stow-managed `~/.claude/settings.json`; on Windows `write_config_atomic` keeps a bounded rename-retry (sharing violations are a platform reality).
5. **The hook shim must never block CC.** Always exit 0 silently on any error; the 200ms send bound is non-negotiable (watchdog thread on BOTH platforms). The watchdog hard-exits, so `send_line` has NO in-process tests — all shim coverage is child-process level.
6. **Walkable mask = ground footprint only.** Visual sprites can be wider than their footprint; the mask blocks only the ground-level projection, so characters walk right next to walls.

## Known sharp edges (index)

Don't be surprised by these — and don't "fix" them. One line each here; the
full WHY lives in the nested `CLAUDE.md` for the owning crate.

**`pixtuoid-core`** ([full entries](crates/pixtuoid-core/CLAUDE.md)):
- CC hook payloads DO include `tool_use_id` (hook-wins dedup fires).
- CC hook `transcript_path` points at the PARENT transcript; subagent-leak is suppressed via `active_tasks`, and liveness flows UP (`refresh_lineage`). CC's `SubagentStart`/`SubagentStop` hooks decode (`decode_cc_hook_custom`).
- The JSONL watcher gates historical/ended transcripts on EVERY first-sight path: liveness probe first (CC pid registry / Codex open-rollout FDs), `should_seed_at_eof` fallback. Content NEVER drives lifecycle. The probe also powers ongoing liveness: the `ProofOfLife` sweep exemption, the negative vouch, and the ms-scale `exit_watch` rung.
- A hook event for an unknown session id registers it (hooks are proof of life), normally with real `Identity`; JSONL events never synthesize.
- Abrupt exits have no `SessionEnd` → stale-sweep cascade, guarded by the liveness-vs-readiness exemptions.
- Subagent display names come from `attributionAgent`; the dispatch tool is **`Agent`** (mapped with legacy `Task` → `ToolDetail::Task`); `Workflow` is deliberately NOT mapped.
- Codex subagents wire via the SubagentStart/Stop hooks (flat rollout, no path nesting).
- Subagent clean-exit ladder: b1 drain / SubagentStop hooks / child-ledger re-links / the un-claim side-channel.
- `AgentSlot.state_started_at` is `SystemTime` (process-local; the whole `SceneState` tree is `Serialize`/`Deserialize` for debug dumps + the snapshot golden, NOT a stable wire contract — the v2-daemon consumer is closed out-of-scope, #279/#280/#281); `ActivityState::Active` ≠ "tool executing" (debounced via `ACTIVE_GRACE_WINDOW`).

**`pixtuoid-scene` engine + `pixtuoid` painters `tui`/`floating`** ([scene engine crate](crates/pixtuoid-scene/CLAUDE.md), [binary](crates/pixtuoid/CLAUDE.md), [tui painter](crates/pixtuoid/src/tui/CLAUDE.md)). The backend-agnostic render+sim engine is its OWN crate `pixtuoid-scene` (`render_to_rgb_buffer`, layout, pose/motion, pathfind, theme model, pets, chitchat, …), sitting between `pixtuoid-core` and the binary; `tui` and `floating` (in the `pixtuoid` binary) are sibling thin painters over it.
- `draw_scene` is called through `TuiRenderer` (owns cross-frame state, returns the cached `Layout`) — it's the terminal flush in the binary's `tui::renderer`, delegating the world render to `pixtuoid_scene::pixel_painter::render_to_rgb_buffer`.
- `recolor_frame` (`pixtuoid_scene::pixel_painter::palette`) substitutes by RGB equality (palette keys must map to unique RGBs).
- Terminal cell aspect drives sprite design (~16×16 px ceiling; bundled pack maxes at 8×12).
- EXIT walks are time-compressed to fit the GC window; snap-back runs pure physics (`SNAP_BACK_MS` is only the ARM window); entry/wander are uncompressed (`pixtuoid_scene::pose`/`pixtuoid_scene::motion`).
- A walk leg's A\* polyline is frozen once per leg, not re-routed per frame (`pixtuoid_scene::motion`).

## Things NOT to do

- Don't add `ratatui` / `crossterm` / terminal anything to `pixtuoid-core`.
- Don't write to `~/.claude/settings.json` directly — go through `install/io.rs` (`write_config_atomic`, or `lock_config` + `ConfigLock::write_atomic` for read-merge-write).
- Don't add `println!` / `eprintln!` to production paths (headless summary and explicit CLI output excepted) — use `tracing`.
- Don't relax the hook shim's "always exit 0" contract. Blocking CC = breaking the user's primary workflow.
- Don't add `--no-verify` / hook-skipping flags to git operations in this repo.
- Don't generate a README / CLAUDE.md / CHANGELOG / docs in PRs unless explicitly asked.
- Don't `git push` without explicit user confirmation, even after committing.
- Don't leave stale `Closes #N` in commit/squash bodies or PR text on a re-scope — GitHub fires the keyword from either place, and conditional phrasing still fires.
- Don't merge a PR without the **two-lens review**: 2+ agents, lenses differentiated (correctness/grounding + design/blast-radius), briefs from [`.github/prompts/pr-review.prompt.md`](.github/prompts/pr-review.prompt.md). No exceptions — PR #23 merged unreviewed with a critical path-traversal vulnerability.
- Don't blindly accept reviewer findings. Verify the premise before coding a fix — check the relevant sharp edges and existing comments first; if a fix contradicts an earlier design decision, trace the code path manually.
- **Don't assert on a path's STRING form with a hardcoded separator.** `Path::join` / `to_string_lossy()` emit `\` on Windows, so `assert_eq!(p.to_string_lossy(), "/home/u/claw")` passes on Unix and fails ONLY on `windows-test` (a CI-only catch — local macOS preflight is blind to it). Keep path helpers RETURNING `PathBuf` (not `String`) and compare `PathBuf` (structural, component-wise), or build the expected with the SAME `.join()` the impl uses. `PathBuf` is the cross-platform abstraction — stay in it, don't round-trip to `String` for comparison. (Resolution-POLICY differences — `HOME` vs `USERPROFILE`, `%APPDATA%` vs `~/.config` — are a SEPARATE class no path lib fixes: each CLI resolves differently, so `dirs`/`shellexpand` give the generic answer = the bug; mirror each CLI instead, see `platform::home_first_dir`/`resolve_user_config_dir`.)

## Where to look

- "How does a CC tool call become a moving sprite?" → `runtime/driver.rs::run_async` → `SourceManager::spawn` → source → decoder → `reducer::Reducer::apply` → `watch` channel → `TuiRenderer::render` → `pixtuoid_scene::pixel_painter::render_to_rgb_buffer` (the world render) → `tui::renderer::draw_scene` (the terminal flush). First half in `pixtuoid-core`; the world render in the `pixtuoid-scene` crate; the terminal flush in `pixtuoid`'s `tui`.
- Architecture overview + data-flow diagram: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md). Area-specific entries (layout, sources, install, themes, motion, weather, pets, …) are in the nested guides.

## When refactoring

If you change the channel type, `Source` trait, `AgentEvent` enum, or reducer
signature, update **all four** test areas (`tests/reducer/`, `tests/e2e.rs`,
`tests/transport/socket.rs`, `tests/watcher/`) plus `runtime/driver.rs`; a
new `AgentEvent` variant also needs an `agent_id()` arm.

**Adding a new agent CLI**: source module + one `SourceDescriptor` row in
`source/registry.rs` + the name in `REGISTERED_SOURCES` + runtime wiring in
`runtime/driver.rs::run_async` (transcript-bearing CLIs only; hook-only CLIs
ship a `hook.custom` decoder + an `install/` target instead) + a row in
`site/src/sources.json` (bridge-tested against `REGISTERED_SOURCES`). Full
steps: `crates/pixtuoid-core/CLAUDE.md` "multi-source decoding" + the tests
guide.
