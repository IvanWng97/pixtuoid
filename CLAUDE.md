# CLAUDE.md

Instructions for Claude Code (or any AI coding agent) working in this repo.
(`AGENTS.md` is a symlink to this file for the cross-tool standard; a Windows
checkout without `core.symlinks` materializes it as a one-line pointer — read
this file.)
This is the **workspace-level map** — conventions, invariants, and rules that
apply everywhere. **Module-level detail and the crate-specific "sharp edges"
live in nested `CLAUDE.md` files**, auto-loaded when you touch those trees:

- [`crates/pixtuoid-core/CLAUDE.md`](crates/pixtuoid-core/CLAUDE.md) — the headless lib: sources, reducer/state, sprites, the grid/walkable vocabulary.
  - [`crates/pixtuoid-core/tests/CLAUDE.md`](crates/pixtuoid-core/tests/CLAUDE.md) — the integration-test layout (9 test binaries: five grouped + four flat, three of them publish-excluded; parity twins) + add-a-CLI test steps.
- [`crates/pixtuoid-scene/CLAUDE.md`](crates/pixtuoid-scene/CLAUDE.md) — the backend-agnostic render+sim engine CRATE (`pixtuoid-core ← pixtuoid-scene ← pixtuoid`): pixel painter (render_to_rgb_buffer), layout, walk physics, pose (pure + routed) / motion authority, pathfinding, the theme MODEL, weather/ambient, pets, chitchat, frame_cache, embedded_pack.
- [`crates/pixtuoid/CLAUDE.md`](crates/pixtuoid/CLAUDE.md) — the binary: install, runtime, cli, config, multi-floor, embedded pack.
  - [`crates/pixtuoid/src/tui/CLAUDE.md`](crates/pixtuoid/src/tui/CLAUDE.md) — the terminal painter (over the `pixtuoid-scene` crate): draw_scene flush, harness, widgets, the theme-PICKER ui, Sources panel, dashboard, hit_test, version popup.

The NON-Rust **consumers** of the `--json` contract have their own guides (their
gates are `tsc`/`eslint` / `just site-check` (+ `just site-e2e`, the Playwright
runtime-contract smoke suite), NOT cargo — the Rust house rules
above don't apply there):
- [`integrations/raycast/CLAUDE.md`](integrations/raycast/CLAUDE.md) — the Raycast TS extension.
- [`site/CLAUDE.md`](site/CLAUDE.md) — the Astro landing page.

**Read the nested guide for the crate you're editing.** Many things that look
like a bug are documented, load-bearing design — the "Known sharp edges"
section in each nested file (indexed below) explains why.

## What this is

Terminal-native, multi-agent pixel-art visualizer for AI coding agents. Each
running CC (Claude Code) session shows up as an animated half-block sprite in
an ASCII office. Rust workspace of five crates. User-facing overview:
[`README.md`](README.md). (Design specs live locally under
`docs/superpowers/`, unversioned.)

## Layout (workspace)

```
crates/                 DAG: pixtuoid-core ← pixtuoid-scene ← {pixtuoid, pixtuoid-web} (+ standalone pixtuoid-hook)
├── pixtuoid-core/   headless lib — no terminal deps (ratatui/crossterm forbidden)
│                    source/ state/ sprite/ render/ grid.rs walkable.rs (walkable STAYS here:
│                    its ops are an inherent `impl Grid<bool>`, orphan-rule-pinned to Grid's crate)
│                    `native` (default) feature gates the async source runtime (tokio/notify,
│                    hook/jsonl/manager/probes, the Source-trait seam source/native.rs + each
│                    source's runtime half source/<cli>/native.rs — MODULE-level gates with
│                    parent re-exports, not item-level cfg scatter) — `default-features = false`
│                    leaves the pure decode/reducer core, which compiles to wasm32
├── pixtuoid-scene/  backend-agnostic render+sim ENGINE crate — terminal AND window-free BY CRATE
│                    BOUNDARY (no ratatui/crossterm/winit/softbuffer in its Cargo.toml; just arch enforces)
│                    pixel_painter/ (render_to_rgb_buffer) layout/ physics.rs pose/ (pure + routed,
│                    file-level split) motion/ pathfind/ floor/ theme/ pet.rs creatures.rs chitchat.rs
│                    frame_cache.rs anim.rs overlay.rs board.rs burn.rs embedded_pack.rs (default pack at
│                    sprites/default/, own build.rs); depends on pixtuoid-core (forwards `native`)
├── pixtuoid/        binary — ratatui + crossterm + winit + tokio + clap; depends on pixtuoid-scene
│                    cli.rs config/ runtime/ install/ focus/ (click-to-focus: pid→ancestor→activate) tui/ floating/ (two thin painters over the
│                    pixtuoid-scene crate; neither depends on the other) sprites/ (skeleton embedded via
│                    include_str!, robot --pack-dir-loadable)
├── pixtuoid-web/    the THIRD painter — wasm-bindgen `<canvas>` painter over pixtuoid-scene
│                    (default-features off → no tokio anywhere), publish = false: a SITE BUILD
│                    INPUT (`just gen-wasm` → committed site/public/wasm/), not a crates.io
│                    artifact. `Office` handle: new(seed) / step(now_ms,w,h) / frame_ptr/len;
│                    a looped scripted timeline (src/script.rs) drives the REAL Reducer (+ the OpenClaw
│                    lobster via the real apply_presence lane, and a visitor-facing Office.hire()
│                    — the install Copy click walks a capped extra coworker in, #434), so the
│                    hero's lifecycle/motion/render behave exactly like the app (the EVENT STREAM
│                    is authored; the state machine + pixel pass are the app's). Time is a
│                    PARAMETER — the engine never reads the clock on wasm.
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
site/                Astro landing page → GitHub Pages; self-contained Node project,
                     own CI; `just site-{setup,dev,dev-bg,dev-stop,check,fmt,e2e}` → see site/README.md
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

Prefer `just test` (nextest if installed) over raw `cargo test`. While
iterating, scope to one crate (seconds vs a full-workspace run).

> **Don't chain `cargo clippy && cargo test`** — they use separate build
> caches and recompile the workspace twice. Run `just preflight` (lint →
> clippy → hack → test, the exact CI order) or one check at a time.

**Test organization (three tiers):** unit tests next to the code (large
modules use a sibling `#[cfg(test)] mod tests;` file — keeps `use super::*`
without API widening); integration tests in `crates/<crate>/tests/` —
pixtuoid-core's suite is 9 binaries (five capability-grouped + four
flat, three of them deliberately publish-excluded) with `#[cfg(windows)]` parity twins, all mapped in
[`crates/pixtuoid-core/tests/CLAUDE.md`](crates/pixtuoid-core/tests/CLAUDE.md);
the headless render harness (`tui_renderer/harness`) drives the real
`TuiRenderer` through ratatui `TestBackend` — see the tui guide. Coverage:
`just coverage`. Decoder never-panic fuzz vs a real session corpus:
`just fuzz <jsonl-dir>` (on-demand, not in CI). Mutation testing (do the
assertions have teeth?): `just mutants` — diff-scoped (`cargo-mutants
--in-diff` vs origin/main), config in `.cargo/mutants.toml`; in CI it is its own
**on-demand** workflow (`mutants.yml`, `workflow_dispatch` from the Actions tab),
NOT per-PR — run it there or locally on reducer/decoder/layout changes (a
surviving mutant is a hint, not a gate). Property-based invariants use
`proptest` (e.g. `walkable.rs`).

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
installs the needed cargo tools once per clone (including the `rust-analyzer`
component — `rust-toolchain.toml` pins only `rustfmt`+`clippy`, so without it the
editor / AI-agent LSP silently degrades to grep).

```
just preflight    # full pre-push gate: lint (fmt+machete+deny+arch+shfmt+actionlint+links) → clippy → hack → test
just fmt          # auto-format
git config core.hooksPath .githooks   # activate hooks once per clone
```

Never pipe `preflight` through `tail`/`head` — the exit code becomes the
pipe's and a real failure reads as green; redirect to a file and `echo $?`.
CI-only gates: semver (pixtuoid-core + pixtuoid-scene — the binary's lib target is not a
semver surface), coverage/smoke, gen-check, gen-readme-check, npm-check,
check-windows (cross-lint for msvc on every PR), snapshots (`cargo insta` —
fails on a pending OR orphan `.snap`, the rot plain `cargo test` can't see).

**Release:** `just bump X.Y.Z` rewrites every version number, drafts
`release_notes()`, runs preflight, and commits on a release branch — it
stops before the tag; pushing the tag is the irreversible crates.io publish
and stays a human step. See
[`CONTRIBUTING.md`](docs/CONTRIBUTING.md#releasing).

## Development workflow (the arc loop)

Non-trivial work runs as an **arc**: design → build → gate → wrap. This is
the ONE portable description of the loop — follow it whatever tool or machine
you're on, because the richer aids below (skills, memory) are NOT
repo-committed and won't exist on a fresh checkout or in a non-Claude tool.

1. **Pick** — an issue (GitHub is the tracker; `gh issue list`) or backlog item.
2. **Grill the design** — decide the open questions ONE at a time, each with a
   recommended answer, before writing code. (A big arc introducing new
   seams/vocabulary grills against the domain docs first.)
3. **Spec** — synthesize the grilled decisions into `docs/superpowers/specs/`
   (LOCAL, git-ignored — the working design record, not the tracker). Also
   plan against [`.github/prompts/impl-plan.prompt.md`](.github/prompts/impl-plan.prompt.md).
4. **Mock gate (taste/visual work only)** — ratify the AFTER visual BEFORE any
   code (the `beautify-decoration` skill's "The visual-iteration loop").
5. **Build** — TDD (see Conventions): failing test → minimal impl → commit.
6. **Self-review** — a standards+spec pass before pushing. Not the merge gate.
7. **Merge gate (non-negotiable)** — the **two-lens review** (2+ differentiated
   lenses on the diff) + green CI + the online review bot's `Findings: 0` at
   HEAD, checked atomically. See "Things NOT to do" and the running order under
   "Where to look". **A human merges.**
8. **Wrap** — retro; record durable lessons.

**Skills.** Repo skills live in [`.claude/skills/`](.claude/skills/) (committed,
so they travel with the repo): `two-lens-review` (the merge gate),
`beautify-decoration` (the visual mock loop), `add-source` / `add-theme`
(scaffold + test-teeth for a new CLI / palette), `procedural-lofi` (synthesize
a new ambient sound — the reference-fingerprint → freeze pipeline). Claude Code
auto-surfaces them by description; other tools read this file (`AGENTS.md`) and
run the loop above as prose.

**Bootstrap on a fresh machine / other tool.** `git clone` gives you the repo
skills + all `just` gates immediately. The day-to-day *loop* skills
(`grilling`, `to-spec`, `tdd`, `code-review`, `diagnosing-bugs`) are a
PERSONAL, non-committed layer — install [mattpocock/skills](https://github.com/mattpocock/skills)
if you want the Claude Code implementations; otherwise this section IS the loop.
Do NOT run its `setup-matt-pocock-skills` here — it scaffolds a `CONTEXT.md` +
`docs/adr/` doc convention that would compete with our richer nested `CLAUDE.md`
+ sharp-edges system (neither exists in this repo, and we don't want a second,
rotting one), plus a fixed triage-label vocabulary separate from our existing
issue labels (e.g. `bug` / `enhancement` / `upstream-drift` / `needs-human-verify`).

## Conventions

- **TDD first.** Failing test → minimal impl → commit. Don't add code without a test that exercises it. Non-trivial changes (new feature/config key/seam, sharp edge, or spanning ≥3 files) plan against [`.github/prompts/impl-plan.prompt.md`](.github/prompts/impl-plan.prompt.md) first — it front-loads the review's failure classes, and its answers fill the review's change-specific slots.
- **DRY, YAGNI.** No features beyond what v1 specifies; v2 items are deferred.
- **No comments unless WHY.** Comment only what a future reader can't tell from the code (a workaround, a non-obvious constraint, a surprising invariant).
- **No magic numbers — reuse an authoritative source, else ONE named `const` (single source of truth).** A numeric (or sentinel-string) literal whose value *carries domain meaning* (timeout, size cap, threshold, ratio/factor, pixel offset, protocol constant) must never be an anonymous inline literal. Handle it in this priority order:
  1. **Reuse an existing authority.** If the stdlib or a third-party crate already exposes the value or a type that carries it, USE that — don't re-hardcode what a dependency owns (it silently drifts when they bump it): `libc::FD_SETSIZE` not `1024`, a crate's provided default/`Duration` constant, an enum's `::default()`, `std::mem::size_of`, etc. Likewise if OUR code already defines the value (a `Theme` field, a layout/registry const, a `SourceDescriptor` row), read it from there — never copy it.
  2. **Else name it ONCE** — the single source of truth. For a lone value, a `const NAME: T = …;` (SCREAMING_SNAKE_CASE) at the narrowest scope that covers all its use-sites — **fn-local** when only one function reads it, module-level otherwise — with a WHY comment. For a *set* of related discrete values, or a value guarding an invariant, prefer a **type over loose consts** — a Rust `enum` or a newtype (as this repo already does with the desk-index / `Grid` newtypes) makes illegal values unrepresentable, not merely named. Either way, every other site *references or derives from* the one definition, never a second copy of the literal: the version-popup click-rect derives its offsets from the SAME `PANEL_PAD_*` the painter insets by; a test computes `200.0 * SHADOW_FACTOR` instead of hardcoding `84`. **Two copies of the same magic value is a latent drift bug**, so when the value genuinely can't be centralized (it crosses a crate/config/wire boundary), still pin the copies together with a test or a `debug_assert!` that they match, and comment the pairing.
  3. **Exceptions stay inline** — don't over-constify readable code into a wall of one-use consts: self-evident `0`/`1`/`2` (incl. `* 2` for half-block sub-pixels), array indices, local loop bounds tied to a nearby collection, log/trace/error string literals, and test fixtures.

  **No lint enforces any of this** — the Rust team declined a general magic-number lint as too noisy (rust-clippy #1539 / #2342); clippy's `unreadable_literal` only enforces digit grouping (`1_000_000`), not naming. So it's a review-practice, not a gate — e.g. the truecolor read loop (`term.rs`) shipped inline `1024`/`64` and had to be lifted to `MAX_DECRQSS_RESPONSE_BYTES`/`DECRQSS_READ_CHUNK` after the fact.
- **Errors propagate via `anyhow::Result` in app code, `thiserror` in core** if a typed error becomes load-bearing. The hook listener and JSONL watcher log + continue on malformed input — they never panic.
- **No `unwrap()` in non-test code.** Tests can unwrap freely.
- **Layer-internal items stay `pub(crate)`, not `pub`.** `unreachable_pub` is `warn` in `[workspace.lints.rust]` and CI's `just clippy` (`-D warnings`) makes it a hard gate — a `pub` item in a private module tree fails the build. Reserve bare `pub` for genuinely cross-crate API (and in `pixtuoid-core`, only those reach the semver surface). The lint is the mechanical enforcement of "the install/uninstall entry points are `pub(crate)`, `crate::sources` is the only caller" and every other inter-layer seam.
- **No scan-the-history logic.** Keep persistent state (a set, a map, a bool) updated as events arrive; never derive state by scanning backward through time.
- **Match the surrounding shell** (zsh interactive / POSIX sh); `shellcheck` + `shfmt` any `.sh` you touch — run `just shfmt-fix` to format (both gated by `just lint` + the CI `hygiene` job). **macOS first**: BSD CLI, brew, launchd.
- **Keep docs current.** A change that alters module structure, architecture, workflow, or public API updates the relevant `CLAUDE.md` + `README.md` in the same commit.
- **A refuted finding cites (or adds) a sharp edge.** When you reject a review finding as "deliberate design," point at the relevant per-crate `CLAUDE.md` "Known sharp edges" entry — or add one in the same change. That keeps the context accurate for the next agent (the real payoff).
- **Track every deferred finding as a GitHub issue** BEFORE moving on — problem, why deferred, fix sketch. A deferred finding with no issue is a silently-dropped finding. (Verify it's real first — see "Don't blindly accept reviewer findings".)
- **Sprite changes require visual verification** — render, crop, read the PNG, self-critique until it reads at half-block scale; commit messages carry the iteration history. Full checklist: `.claude/skills/beautify-decoration/SKILL.md`.
- **Periodic context-file audits also distill memory**: each `/revise-claude-md`-style audit sweeps recent session memories for promote-to-repo candidates (the memory layer of [`docs/KNOWLEDGE-ENGINEERING.md`](docs/KNOWLEDGE-ENGINEERING.md)).
- **The lifecycle conventions above are PRACTICES, not a gate.** Two-lens review before merge, deferred→issue, docs-currency, no stray prod-`println!`, no direct `settings.json` write, no `--no-verify` — do them because they're right, not because a script blocks you. (The old `check_dod` mechanization + its `.dod/` attestation + the CI `definition-of-done` job were removed: a one-person gate run against oneself is ceremony, not enforcement. Real teeth live in the automated checks — `just preflight`, clippy, tests, the `claude-review` second lens.)

## Architecture invariants

These are load-bearing; don't break them without updating the spec.

1. **`pixtuoid-core` has no terminal dependencies.** No `ratatui`, no `crossterm`, no `stdout` writes. A NEW render target (window, canvas, PNG/GIF, …) plugs in as another thin painter over `pixtuoid_scene::floor::render_floor` / `pixel_painter::render_to_rgb_buffer` — THE seam every post-split painter (TUI flush, floating window, web hero) actually rides. (core once carried a `#[doc(hidden)]` `Renderer` trait that misled two design rounds; it was retired in #483 — its two impls rode it non-polymorphically, so they are now inherent `render` methods.) **`pixtuoid-scene` (the render+sim engine) is ALSO terminal- AND window-free** — and now COMPILER-enforced by the crate boundary: `ratatui`/`crossterm`/`winit`/`softbuffer` aren't in its `Cargo.toml`, so reaching for one won't compile. `just arch` covers BOTH crates. Terminal/window code lives in the `pixtuoid` binary's painters (`tui/`, `floating/`).
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
- Subagent display names come from `attributionAgent`; the dispatch tool is **`Agent`** (the one known name — the legacy `Task` name arm was dropped in 0.12.0; a pre-rename dispatch still carries `subagent_type`, THE semantic detection signal); `Workflow` is deliberately NOT mapped.
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
- Don't merge a PR without the **two-lens review**: 2+ agents, lenses differentiated (correctness/grounding + design/blast-radius), briefs from [`.github/prompts/pr-review.prompt.md`](.github/prompts/pr-review.prompt.md) — invokable via the `two-lens-review` skill. No exceptions — PR #23 merged unreviewed with a critical path-traversal vulnerability. (That skill's **whole-codebase scope** runs the periodic/pre-release AUDIT — the SAME shared factor taxonomy + verify contract + disposition, fanned out over the whole tree instead of a diff; `pr-review.prompt.md` is canonical for BOTH scopes, so a factor added once upgrades both.)
- Don't blindly accept reviewer findings. Verify the premise before coding a fix — check the relevant sharp edges and existing comments first; if a fix contradicts an earlier design decision, trace the code path manually.
- **Don't assert on a path's STRING form with a hardcoded separator.** `Path::join` / `to_string_lossy()` emit `\` on Windows, so `assert_eq!(p.to_string_lossy(), "/home/u/claw")` passes on Unix and fails ONLY on `windows-test` (a CI-only catch — local macOS preflight is blind to it). Keep path helpers RETURNING `PathBuf` (not `String`) and compare `PathBuf` (structural, component-wise), or build the expected with the SAME `.join()` the impl uses. `PathBuf` is the cross-platform abstraction — stay in it, don't round-trip to `String` for comparison. (Resolution-POLICY differences — `HOME` vs `USERPROFILE`, `%APPDATA%` vs `~/.config` — are a SEPARATE class no path lib fixes: each CLI resolves differently, so `dirs`/`shellexpand` give the generic answer = the bug; mirror each CLI instead, see `platform::home_first_dir`/`resolve_user_config_dir`.)

## Where to look

- "How does a CC tool call become a moving sprite?" → `runtime/driver.rs::run_async` → `SourceManager::spawn` → source → decoder → `reducer::Reducer::apply` → `watch` channel → `TuiRenderer::render` → `pixtuoid_scene::pixel_painter::render_to_rgb_buffer` (the world render) → `tui::renderer::draw_scene` (the terminal flush). First half in `pixtuoid-core`; the world render in the `pixtuoid-scene` crate; the terminal flush in `pixtuoid`'s `tui`.
- Architecture overview + data-flow diagram: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md). Area-specific entries (layout, sources, install, themes, motion, weather, pets, …) are in the nested guides.
- "How do I ship one change that spans the Rust lib + the site + the Raycast extension in parallel?" → [`docs/PARALLEL-DELIVERY.md`](docs/PARALLEL-DELIVERY.md) (the contract-first → fan-out → join model; the `--json` shape is the contract, the per-area `CLAUDE.md`/`AGENTS.md` scope each worker/agent). How lessons persist across agent runs so the next change is cheaper: [`docs/KNOWLEDGE-ENGINEERING.md`](docs/KNOWLEDGE-ENGINEERING.md).
- "Working an agent-driven change — what do I run, and when?" (each gate is detailed above; this is the running order) → **before code**, if non-trivial (new seam / ≥3 files), plan against [`.github/prompts/impl-plan.prompt.md`](.github/prompts/impl-plan.prompt.md) → **touched the `--json` / `SourceStatus` / `OutcomeRow` shape?** `just gen-contract` (regenerates BOTH committed schemas + the Raycast types) (else the Raycast `gen:contract` diff + `tsc` go red) → **before push** `just preflight` (lint → clippy → hack → test; never pipe through `tail`/`head` — it eats the exit code; the CI-only gates under "Build & test" — semver, gen-check — still run separately) → **before merge** the two-lens review (2+ agents, differentiated lenses; see "Things NOT to do") → **dogfood a source/lifecycle change** with `pixtuoid run --headless --projects-root ~/.claude/projects` vs live CC, or replay hermetically via `scripts/replay-fixture.sh` / `scripts/openclaw-live-e2e.sh`. Advisory backstops that surface risk but NEVER gate: `scripts/check_upstream_drift.py` (wire-format drift) and the `risk radar` PR workflow (`scripts/risk-radar.py` / `just risk-radar`) — deterministic path matching that posts the documented blast-radius escalations (shim never-panic audit, motion render-and-watch, reducer interaction-graph trace, …) as a sticky PR comment so prose-only escalation can't be silently skipped (#198).

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
guide — or invoke the `add-source` skill (which foregrounds the test-teeth steps
a diff-scoped edit misses). A new theme has an analogous `add-theme` skill.

**Adding a new ambient sound** (another lofi mood, a new one-shot cue, a
weather bed): the `procedural-lofi` skill is the reusable recipe — the
reference-fingerprint → shape-to-curve → freeze-a-take → synthesize-at-launch
pipeline the whole `#633` audio line was built on, with the parameter tables
(`LOFI-BIBLE.md`) + the numpy fingerprint/synth/freeze scripts bundled. The
in-tree synth home is `pixtuoid-scene/src/audio/` (see that crate's guide).
