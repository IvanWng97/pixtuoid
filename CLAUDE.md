# CLAUDE.md

Instructions for Claude Code (or any AI coding agent) in this repo.
(`AGENTS.md` symlinks here.) This is the workspace map; per-crate detail lives
in nested guides, auto-loaded when you touch their tree:

- [`crates/pixtuoid-core/CLAUDE.md`](crates/pixtuoid-core/CLAUDE.md) — headless lib: sources/decoders, reducer/state, sprites, grid/walkable. (+ [`tests/CLAUDE.md`](crates/pixtuoid-core/tests/CLAUDE.md): the 9-binary test layout, add-a-CLI test steps.)
- [`crates/pixtuoid-scene/CLAUDE.md`](crates/pixtuoid-scene/CLAUDE.md) — backend-agnostic render+sim engine: pixel painter, layout, walk/pose/pathfind, theme model, weather, pets, chitchat.
- [`crates/pixtuoid/CLAUDE.md`](crates/pixtuoid/CLAUDE.md) — the binary: install, runtime, cli, config, multi-floor. (+ [`src/tui/CLAUDE.md`](crates/pixtuoid/src/tui/CLAUDE.md): the terminal painter.)
- [`integrations/raycast/CLAUDE.md`](integrations/raycast/CLAUDE.md), [`site/CLAUDE.md`](site/CLAUDE.md) — the non-Rust `--json` consumers; their gates are `tsc`/`eslint` / `just site-check`, not cargo.

**What the guides hold.** A nested `CLAUDE.md` says what its crate IS. The
three that also carry operational content — `pixtuoid-core/tests/`, Raycast,
site — keep it INLINE there. Everything else a change needs — the constraint that looks
like a bug, the WHY, the test that pins it — is on the declaration it
constrains: read the item's doc comment before changing it.

**A third consumer lives outside this repo**: homebrew-core's `pixtuoid`
formula asserts exact CLI output (`test do`) and needs `pixtuoid man` /
`completions <shell>` on clean stdout — breakage surfaces in THEIR CI on an
autobump we never see, while our suite stays green (it asserts the same
strings as goldens). The asserted rows are marked "homebrew-core contract" at
`validate.rs`, `sources_cli.rs`, `claude_code.rs`; release-side consequences
in [`CONTRIBUTING.md`](docs/CONTRIBUTING.md#releasing).

## What this is

Terminal-native, multi-agent pixel-art visualizer for AI coding agents: each
running CC session is an animated sprite in an ASCII office. Rust workspace,
five crates. Overview: [`README.md`](README.md).

## Layout

```
crates/   DAG: pixtuoid-core ← pixtuoid-scene ← {pixtuoid, pixtuoid-web}  (+ standalone pixtuoid-hook)
├── pixtuoid-core/   headless lib — no terminal deps; `native` feature gates the async
│                    source runtime (no-default-features = wasm32-clean decode/reduce)
├── pixtuoid-scene/  render+sim engine — terminal- AND window-free BY CRATE BOUNDARY
├── pixtuoid/        binary — two thin painters over pixtuoid-scene: `tui/`, `floating/`
├── pixtuoid-web/    third painter — wasm canvas, publish=false; a SITE BUILD INPUT
│                    (`just gen-wasm` → committed site/public/wasm/)
└── pixtuoid-hook/   tiny shim CC invokes — stdin JSON → socket/named pipe
scripts/  gen-media.py (the ONE driver for committed art), e2e tiers (lib/), drift watch
policy/   Conftest/OPA structural contracts for CI observability
site/     Astro landing page; integrations/raycast/  Raycast extension
```

## Build & test

```
just build [--release] · just test (nextest)         # scope to one crate while iterating
cargo test -p <crate> --lib <filter>                 # fast loop
just preflight                                       # pre-push gate: lint → clippy → hack → test (CI order)
cargo run --release --example snapshot -- /tmp/snap.png   # render TUI to PNG
```

- Don't chain `cargo clippy && cargo test` (two build caches) — `just preflight` or one at a time. Never pipe preflight through `tail`/`head` (exit code eaten).
- Touched `--json` / `SourceStatus` / `OutcomeRow` / the source roster → `just gen-contract` (regenerates schemas + Raycast types).
- Renamed a decoded/registered wire name → `just gen-drift-surface`, commit both `crates/*/drift-surface.json` — the crate's own test fails on a stale fragment; regenerate, don't hand-edit.
- Look-changing PR → `just gen`, commit `docs/images/` + `site/public/demos/`; a scene/web change ALSO needs `just gen-wasm` + commit `site/public/wasm/` (`gen` deliberately excludes it, and nothing catches a skip).
- Real wire bytes ride ONE pipeline: `pixtuoid_core::harness::Drive` (dev-only `harness` feature). A driver keyed off anything but the source's registry row registers NOTHING.
- Fixtures are RECORDED, never composed (`just capture-fixture` — BILLED); every scenario declares `provenance.json`. Rules: [`fixtures/README.md`](crates/pixtuoid-core/tests/sources/fixtures/README.md). `just corpus-all` censuses local corpora; `just fixture-age` is advisory/local.
- Visual verification for sprite work: snapshot example → `scripts/crop-snapshot.py` → READ the PNG; loop in `.claude/skills/beautify-decoration/SKILL.md`.
- CI-only gates preflight can't see: semver · api-surface · doc-check · coverage/smoke · gen-check · gen-readme-check · npm-check · check-windows · insta snapshots. Details: [`CONTRIBUTING.md`](docs/CONTRIBUTING.md#ci-gates).
- On-demand advisory (never gates): `just mutants`, `just coverage`, `just bench`, CodSpeed.
- Hooks: `git config core.hooksPath .githooks` once per clone; `just setup-tools` installs cargo tools (incl. rust-analyzer — without it the agent LSP degrades to grep).
- Release: `just bump X.Y.Z` stops before the tag; pushing the tag IS the publish (crates.io + npm + homebrew autobump) and stays a human step. [`CONTRIBUTING.md`](docs/CONTRIBUTING.md#releasing).

## Workflow

Non-trivial work runs as an arc — pick → grill the design → design gate →
spec → build (TDD) → self-review → merge gate → wrap. Per-step detail:
[`CONTRIBUTING.md`](docs/CONTRIBUTING.md#the-arc-loop). The merge gate is the
`two-lens-review` skill: 2+ differentiated lenses + green CI + every bot
finding dispositioned, under that skill's **convergence contract** (churn
budget, two-fix-round cap, HIGH-only blocking). **A human merges.**

Repo skills (committed): `two-lens-review`, `beautify-decoration`,
`add-source`, `add-theme`, `procedural-lofi`.

## Conventions

- **TDD first** — failing test → minimal impl → commit. **DRY, YAGNI** — nothing beyond the current spec.
- **Comments: WHY only.** Only what the code can't say (workaround, constraint, invariant). Every sentence must add information the earlier ones don't — delete each after the first; if nothing is lost, cut it. First sentence is the whole answer. Fn-body comments ≤2 lines — a longer rationale isn't trimmed, it MOVES onto the declaration. The rules are semantic, none demands brevity — a comment that passes them stays at whatever length it earned. Measurements belong in commit messages, not comments. **Name the authority, never restate its value** — `` × [`MAX_CONCURRENT_CONNS`] slots ``, not `× 128 slots`: a restated value drifts silently while a name greps, and where rustdoc documents the item (pub in a lib; everything in the bin) the intra-doc link also turns a rename into a `doc-check` red (the magic-number rule, applied to prose).
- **No magic numbers** — reuse the existing authority (a dep's const, our registry/theme/layout value), else ONE named `const` at the narrowest covering scope; prefer a type (enum/newtype) for a related set. Two copies of one value is a latent drift bug — if a copy must cross a boundary, pin the pair with a test. Self-evident `0`/`1`/`2`, indices, and test fixtures stay inline.
- **Errors**: `anyhow::Result` in app code, `thiserror` in core; hook listener + JSONL watcher log-and-continue, never panic. **No `unwrap()` outside tests.**
- **Visibility**: layer-internal stays `pub(crate)` (`unreachable_pub` is a hard gate); every `pub` item in a published crate carries a doc comment (`missing_docs`); `#[doc(hidden)] pub` = mechanism-not-contract escape hatch.
- **No scan-the-history** — keep state updated as events arrive; never derive it by scanning backward.
- **Shell**: match the surrounding shell; `shellcheck` + `shfmt` (`just shfmt-fix`) any `.sh` you touch. macOS-first (BSD CLI, brew).
- **Docs current in the same commit** as any structure/API/workflow change.
- **External-surface claims are fetched, not remembered** — cite the `path:line` you fetched THIS session or add a `check_upstream_drift.py` row; the population is the whole upstream repo (`gh api .../git/trees/<ref>?recursive=1`), not one plausible file (#938).
- **A refuted review finding produces a MECHANISM, or nothing** — a test, a compile-time constraint, or a CI gate; refuting never produces prose, because prose has no failure mode. Only an EXTERNAL fact (another CLI's wire bytes, an OS semantic) earns a comment, on the declaration it constrains. **A real finding this change introduced is fixed in-scope or forces a re-scope; a pre-existing one is SURFACED to the owner in one line (four terminal states, defined once in `pr-review.prompt.md`). Agents never file issues.**
- **Path asserts compare `PathBuf` structurally**, never `to_string_lossy()` with a hardcoded separator — string asserts pass on Unix and fail only in `windows-test`. Resolution POLICY (HOME vs USERPROFILE, %APPDATA% vs `~/.config`) is per-CLI: mirror each CLI's own resolver (`platform::home_first_dir`).

## Architecture invariants (load-bearing)

1. **`pixtuoid-core` and `pixtuoid-scene` have no terminal/window deps** (compiler-enforced by crate boundary; `just arch`). New render targets are thin painters over `pixtuoid_scene::floor::render_floor` / `pixel_painter::render_to_rgb_buffer`.
2. **Agent events flow through ONE channel** `mpsc::Sender<(Transport, AgentEvent)>`; the `Transport` tag drives hook-wins dedup — producers tag their own events. Daemon presence rides a separate `AgentId`-free channel (`PresenceMsg { key: DaemonInstanceKey, delta }`) and never enters `Reducer::apply`.
3. **`Source` trait is the only seam** for a transcript-bearing CLI; per-source format knowledge lives in that source's decoder. Exceptions: hook-only CLIs (Reasonix) and the shared ACP wire standard (`source/acp.rs`, reused by grok) — see core's guide.
4. **Hook install writes through symlinks** (`resolve_symlink` in `install/io.rs`) — critical for stow-managed configs; Windows keeps the bounded rename-retry.
5. **The hook shim never blocks CC** — always exit 0 silently; the 200 ms send bound is watchdog-enforced on both platforms. Shim coverage is child-process level only.
6. **Walkable mask = ground footprint only**; sprite size never moves a sim position — fitting the frame is the painter's job (`keep_sprite_on_canvas`), not the sim's (#912).

## Sharp edges (cross-crate ownership)

Don't "fix" documented design. Ownership by crate: **core** owns session
lifecycle/identity (registration, dedup, first-sight, liveness ladder,
subagent parenting, feature boundaries) · **scene** owns look/motion (palette
recolor by RGB equality, walk timing, footprints, sky/light invariants,
reachability) · **binary** owns install/runtime wiring (config rewriting,
desk growth, boot order, doctor, daemon announce-only) · **tui** owns the
flush (popup geometry, hit-test ladders, key dispatch). Terminal cell aspect
drives sprite design: the half-block ▀ technique assumes ~1:2 cells, so sprites
past ~16×16 px break on taller-cell terminals; bundled character sprites max at
8×12 px.

## Things NOT to do

- No `ratatui`/`crossterm`/terminal anything in `pixtuoid-core` or `pixtuoid-scene`.
- No direct `~/.claude/settings.json` writes — go through `install/io.rs` (`write_config_atomic` / `ConfigLock`).
- No `println!`/`eprintln!` on production paths (headless summary + CLI output excepted) — `tracing`.
- Never relax the shim's always-exit-0 contract; never add `--no-verify`/hook-skipping flags.
- No new `.md` files, READMEs, CHANGELOGs, or docs unless the owner explicitly asks — the owner reviews every doc change directly, so propose the diff rather than adding a generator or a cap. No `git push` without explicit user confirmation.
- No stale `Closes #N` on a re-scope (fires from commit body or PR text, even conditional).
- No merging without the two-lens review (PR #23 merged unreviewed with a path traversal). Don't blindly accept reviewer findings — verify the premise against the declaration's own doc comment first.

## Where to look

- Tool call → sprite: `runtime/driver.rs::run_async` → `SourceManager::spawn` → source → decoder → `reducer::Reducer::apply` → watch channel → `TuiRenderer::render` → `pixtuoid_scene::pixel_painter::render_to_rgb_buffer` → `tui::renderer::draw_scene`.
- Architecture + data flow: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md). Rust+site+Raycast spanning change: [`docs/PARALLEL-DELIVERY.md`](docs/PARALLEL-DELIVERY.md). What to run when: [`CONTRIBUTING.md`](docs/CONTRIBUTING.md#the-running-order).
- Refactoring the channel type, `Source` trait, `AgentEvent`, or reducer signature touches all four test areas (`tests/reducer/`, `tests/e2e.rs`, `tests/transport/socket.rs`, `tests/watcher/`) + `runtime/driver.rs`; a new `AgentEvent` variant needs an `agent_id()` arm.
- Adding an agent CLI: source module + `SourceDescriptor` row (`source/registry.rs`) + `runtime/driver.rs` wiring + `site/src/sources.json` row; full checklist in [`CONTRIBUTING.md`](docs/CONTRIBUTING.md#adding-a-new-agent-cli); `add-source` skill drives it.
