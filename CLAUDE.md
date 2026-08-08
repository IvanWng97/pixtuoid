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

There is a THIRD consumer with no guide in this tree, because it lives in
someone else's repo: **homebrew-core**'s `pixtuoid` formula. Its `test do`
block asserts exact CLI output, and its `install` block requires `pixtuoid man`
and `pixtuoid completions <shell>` on clean stdout — those break core's BUILD.
The asymmetry is the dangerous part: Raycast and the site fail in OUR CI where
we see it; homebrew-core fails in THEIRS, on an `autobump: true` bump we
neither trigger nor get notified about, and our suite stays green because it
asserts those same strings as its own goldens. The contract is marked at each
source site — the exact asserted rows live in the "homebrew-core contract"
comments at `validate.rs`, `sources_cli.rs` and `claude_code.rs`, and the
release-side consequences (default-feature Linux builds, new `depends_on`, the
tag-is-a-publish rule) are in
[`CONTRIBUTING.md`](docs/CONTRIBUTING.md#releasing).

**Read the nested guide for the crate you're editing.** Many things that look
like a bug are documented, load-bearing design — the "Known sharp edges"
section in each nested file (indexed below) explains why.

**Two things about how these files load.** Each nested guide keeps its
"how does X work?" answers in a sibling `WHERE-TO-LOOK.md`, indexed by question
in the guide itself — so a session pays for the one answer it needs instead of
every answer the crate has. And a nested `CLAUDE.md` is re-read only when you
next touch a file in its tree: unlike this root file, it is **not** re-injected
after a `/compact`, so on a long arc re-open one file from the crate before
trusting your memory of its sharp edges.

## What this is

Terminal-native, multi-agent pixel-art visualizer for AI coding agents. Each
running CC (Claude Code) session shows up as an animated half-block sprite in
an ASCII office. Rust workspace of five crates. User-facing overview:
[`README.md`](README.md). (Design specs live locally under
`docs/superpowers/`, unversioned.)

## Layout (workspace)

```
crates/   DAG: pixtuoid-core ← pixtuoid-scene ← {pixtuoid, pixtuoid-web}  (+ standalone pixtuoid-hook)
├── pixtuoid-core/   headless lib — no terminal deps. Sources/decoders, reducer/state, sprites,
│                    grid + walkable. The `native` (default) feature gates the async source
│                    runtime; `default-features = false` leaves a wasm32-clean decode/reduce core.
├── pixtuoid-scene/  backend-agnostic render+sim ENGINE — terminal AND window-free BY CRATE
│                    BOUNDARY (ratatui/crossterm/winit/softbuffer are not in its Cargo.toml, so
│                    reaching for one won't compile; `just arch` covers it too). Pixel painter,
│                    layout, pose/motion/pathfind, theme MODEL, pets, chitchat, audio.
├── pixtuoid/        binary — ratatui + crossterm + winit + tokio + clap. TWO thin painters over
│                    pixtuoid-scene: `tui/` and `floating/`, and neither depends on the other.
├── pixtuoid-web/    the THIRD painter — wasm `<canvas>`, `publish = false`. A SITE BUILD INPUT
│                    (`just gen-wasm` → committed site/public/wasm/), not a crates.io artifact.
│                    Time is a PARAMETER — the engine never reads the clock on wasm.
└── pixtuoid-hook/   tiny shim CC invokes — stdin JSON → Unix socket / Windows named pipe.
scripts/             gen-media.py + media.json (the ONE driver for all committed art), the three
                     OpenClaw e2e tiers (see Build & test), check_upstream_drift.py, risk-radar.py.
policy/              policy-as-code: Conftest/OPA structural contracts + yq-extracted
                     action/workflow behavior tests (`policy/ci-observability/`).
site/                Astro landing page → GitHub Pages. Self-contained Node project, own CI.
integrations/raycast/  Raycast TS extension over the CLI `--json` contract. Own CI.
```

Per-crate module detail lives in that crate's nested guide — it is deliberately
NOT duplicated here.

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

**Touched the `--json` / `SourceStatus` / `OutcomeRow` shape, or the source
roster?** Run `just gen-contract` — it regenerates BOTH committed schemas and
the Raycast types; skip it and the Raycast `gen:contract` diff + `tsc` go red.

**Test tiers:** unit tests beside the code · integration tests in
`crates/<crate>/tests/` · the headless render harness driving the real
`TuiRenderer` through ratatui `TestBackend`. The 9-binary layout and the
add-a-CLI test steps are in
[`crates/pixtuoid-core/tests/CLAUDE.md`](crates/pixtuoid-core/tests/CLAUDE.md).

**Real wire bytes ride ONE pipeline.** Everything that feeds captured or live
transcript/hook bytes through the production path drives
`pixtuoid_core::harness::Drive` (core's dev-only `harness` feature, so it never
enters the published crate). **A driver that keys the first-sight seed any way
other than the source's registry row registers NOTHING** — a JSONL event for an
unknown id is a documented no-op. The four shells and the two on-demand tools
(`just fuzz`, the `corpus_check` census) are mapped in the tests guide; neither
tool runs in CI.

Mutation testing: `just mutants` (diff-scoped vs origin/main; in CI it is the
on-demand `mutants.yml`, NOT per-PR — a surviving mutant is a hint, not a
gate). Coverage: `just coverage`. Property invariants use `proptest`.

### Visual verification

```
just build --release --example snapshot
./target/release/examples/snapshot --cols 192 --rows 80 /tmp/snap.png
.venv/bin/python3 scripts/crop-snapshot.py /tmp/snap.png --scale 3   # venv: requirements-dev.txt
```

A PR that **intentionally** changes the office's look must run `just gen`
and commit the regenerated `docs/images/` (incl. the `reference-*.png` CI
baselines) plus `site/public/demos/` in the same change, or the smoke job's
`just gen-check` pixel-diff goes red. **`just gen` is `gen-icons gen-media
gen-readme` — it deliberately does NOT include `gen-wasm`** (that one needs
rustup's wasm32 std + `wasm-bindgen-cli` + `wasm-opt`, which `gen`'s Python/Node
tools don't), so a `pixtuoid-scene` or `pixtuoid-web` change must ALSO run `just
gen-wasm` and commit all of `site/public/wasm/`. Nothing catches a skip:
`gen-wasm-check` verifies only that the committed files match their own
`manifest.sha256` (a stale set is perfectly self-consistent), and the poster the
site crossfades OUT of — `site/public/demos/hero-wide.png`, a `wasm-still` job —
is built NATIVELY from source by `gen-media`, so it tracks the change while the
committed wasm the live hero actually runs does not. Full iteration loop + sprite
pitfalls: `.claude/skills/beautify-decoration/SKILL.md`.

### Preflight, hooks, release

The `justfile` is the single source of truth for every check — CI and the git
hooks call the same recipes (no local-vs-CI drift). `just setup-tools` installs
the needed cargo tools once per clone (including the `rust-analyzer` component —
`rust-toolchain.toml` pins only `rustfmt`+`clippy`, so without it the editor /
AI-agent LSP silently degrades to grep).

```
just preflight    # full pre-push gate: lint → clippy → hack → test (the exact CI order)
just fmt          # auto-format
git config core.hooksPath .githooks   # activate hooks once per clone
```

Never pipe `preflight` through `tail`/`head` — the exit code becomes the pipe's
and a real failure reads as green; redirect to a file and `echo $?`.

**CI-only gates** (these do NOT run in `preflight`): semver · api-surface ·
doc-check · coverage/smoke · gen-check · gen-readme-check · npm-check ·
check-windows · snapshots (`cargo insta` — fails on a pending OR orphan
`.snap`, the rot plain `cargo test` can't see). What each enforces, and the
workflow-security posture that `policy/ci-observability/` pins (zizmor,
Dependabot directories, the Claude reviewers' fail-visible arm, CodeQL's
semantic inputs), is in [`CONTRIBUTING.md`](docs/CONTRIBUTING.md#ci-gates).

**Release:** `just bump X.Y.Z` rewrites every version number, drafts the notes,
runs preflight, and commits on a release branch — it stops before the tag.
Pushing the tag is the irreversible publish (crates.io + npm, and it
auto-triggers a homebrew-core bump) and stays a human step. See
[`CONTRIBUTING.md`](docs/CONTRIBUTING.md#releasing).

## Development workflow (the arc loop)

Non-trivial work runs as an **arc**: design → build → gate → wrap. This is the
portable description — follow it whatever tool or machine you're on, because
the richer aids (loop skills, personal memory) are NOT repo-committed and won't
exist on a fresh checkout or in a non-Claude tool.

1. **Pick** — an issue (GitHub is the tracker; `gh issue list`) or backlog item.
2. **Grill the design** — decide the open questions ONE at a time, each with a
   recommended answer, before writing code.
3. **Design gate** (before build; NOT the step-8 merge review) — three lenses so
   slop dies in design: **best-practice search** (confirm the idiomatic way
   against real docs/source online, never memory) · **adversarial design
   review** (red-team the design itself, before code exists) · **deepening
   lens** (would deleting this concentrate complexity or just move it?).
4. **Spec** — synthesize into `docs/superpowers/specs/` (LOCAL, git-ignored) and
   plan against [`impl-plan.prompt.md`](.github/prompts/impl-plan.prompt.md).
5. **Mock gate** (taste/visual work only) — ratify the AFTER visual BEFORE code.
6. **Build** — TDD: failing test → minimal impl → commit.
7. **Self-review** — a standards+spec pass before pushing. Not the merge gate.
8. **Merge gate (non-negotiable)** — the **two-lens review** (2+ differentiated
   lenses on the diff) + green CI + the online review bot's `Findings: 0` at
   HEAD, checked atomically. **A human merges.**
9. **Wrap** — retro; record durable lessons.

Per-step detail, the fallback when the review bot can't post at HEAD, and the
bootstrap notes for a fresh machine or a non-Claude tool are in
[`CONTRIBUTING.md`](docs/CONTRIBUTING.md#the-arc-loop).

**Skills.** Repo skills live in [`.claude/skills/`](.claude/skills/) (committed,
so they travel with the repo); [`.agents/skills/`](.agents/skills/) aliases them
for Codex on symlink-capable checkouts. They are `two-lens-review` (the merge
gate), `beautify-decoration` (the visual mock loop), `add-source` / `add-theme`,
and `procedural-lofi`.

## Conventions

- **TDD first.** Failing test → minimal impl → commit. Don't add code without a test that exercises it. Non-trivial changes (new feature/config key/seam, sharp edge, or spanning ≥3 files) plan against [`.github/prompts/impl-plan.prompt.md`](.github/prompts/impl-plan.prompt.md) first — it front-loads the review's failure classes, and its answers fill the review's change-specific slots.
- **DRY, YAGNI.** No features beyond what v1 specifies; v2 items are deferred.
- **No comments unless WHY.** Comment only what a future reader can't tell from the code (a workaround, a non-obvious constraint, a surprising invariant). Three tests, in order — 1 and 3 are the known art; 2 is the one this repo kept failing:
  1. **A different abstraction level than the code.** If the reader could deduce it from the line below, delete it (Ousterhout, *A Philosophy of Software Design* ch. 13).
  2. **Every sentence carries information the earlier ones don't.** Rule 1 compares the comment to the CODE; this compares it to ITSELF. **The check: delete each sentence after the first, ONE AT A TIME. If nothing is lost — neither an action nor the ability to tell when the constraint stops applying — cut it.** The survivors are its *ideas*; report `N sentences, M ideas`.
  3. **First sentence is the whole answer.** A reader who stops there is not misled.

  All three cut; none demands brevity. A comment that passes all three stays at
  whatever length it earned — this repo's dense WHY is deliberate, and trimming a
  legitimate one is the worse error.

  Evidence, not decoration: in a code comment, a measurement or before/after count belongs in the COMMIT MESSAGE — an inline number nobody re-measures is the first thing to rot. An issue number STAYS when it names the failure mode the comment exists to prevent (`#793` in `check_upstream_drift.py`), and is not provenance.

- **No magic numbers — reuse an authoritative source, else ONE named `const` (single source of truth).** A numeric (or sentinel-string) literal whose value *carries domain meaning* (timeout, size cap, threshold, ratio/factor, pixel offset, protocol constant) must never be an anonymous inline literal. Handle it in this priority order:
  1. **Reuse an existing authority.** If the stdlib or a third-party crate already exposes the value or a type that carries it, USE that — don't re-hardcode what a dependency owns (it silently drifts when they bump it): `libc::FD_SETSIZE` not `1024`, a crate's provided default/`Duration` constant, an enum's `::default()`, `std::mem::size_of`, etc. Likewise if OUR code already defines the value (a `Theme` field, a layout/registry const, a `SourceDescriptor` row), read it from there — never copy it.
  2. **Else name it ONCE** — the single source of truth. For a lone value, a `const NAME: T = …;` (SCREAMING_SNAKE_CASE) at the narrowest scope that covers all its use-sites — **fn-local** when only one function reads it, module-level otherwise — with a WHY comment. For a *set* of related discrete values, or a value guarding an invariant, prefer a **type over loose consts** — a Rust `enum` or a newtype (as this repo already does with the desk-index / `Grid` newtypes) makes illegal values unrepresentable, not merely named. Either way, every other site *references or derives from* the one definition, never a second copy of the literal: the version-popup click-rect derives its offsets from the SAME `PANEL_PAD_*` the painter insets by; a test computes `200.0 * SHADOW_FACTOR` instead of hardcoding `84`. **Two copies of the same magic value is a latent drift bug**, so when the value genuinely can't be centralized (it crosses a crate/config/wire boundary), still pin the copies together with a test or a `debug_assert!` that they match, and comment the pairing.
  3. **Exceptions stay inline** — don't over-constify readable code into a wall of one-use consts: self-evident `0`/`1`/`2` (incl. `* 2` for half-block sub-pixels), array indices, local loop bounds tied to a nearby collection, log/trace/error string literals, and test fixtures.

  **No lint enforces any of this** (clippy's `unreadable_literal` only enforces digit grouping, not naming), so it is a review practice — e.g. the truecolor read loop (`term.rs`) shipped inline `1024`/`64` and had to be lifted to `MAX_DECRQSS_RESPONSE_BYTES`/`DECRQSS_READ_CHUNK` after the fact.
- **Errors propagate via `anyhow::Result` in app code, `thiserror` in core** if a typed error becomes load-bearing. The hook listener and JSONL watcher log + continue on malformed input — they never panic.
- **No `unwrap()` in non-test code.** Tests can unwrap freely.
- **Layer-internal items stay `pub(crate)`, not `pub`.** `unreachable_pub` is `warn` in `[workspace.lints.rust]`, and `just clippy` (`-D warnings`) makes it a hard gate — a `pub` item in a private module tree fails the build. Reserve bare `pub` for genuinely cross-crate API; in `pixtuoid-core` only those reach the semver surface.
- **Every `pub` item in a PUBLISHED crate carries a doc comment.** `missing_docs` is `warn` via `#![warn(missing_docs)]` in `pixtuoid-core` + `pixtuoid-scene`'s `lib.rs` (NOT `[workspace.lints]` — it's a public-API gate, scoped identically to the semver-checks + api-surface gates), and `just clippy` promotes it to a hard gate. Document *what it is* (unit / provenance / invariant), not filler. A `#[doc(hidden)] pub` item — the workspace-internal `overlay`/`board`/`footer` seam pattern — is exempt: that's the escape hatch for "public for mechanism, not contract".
- **No scan-the-history logic.** Keep persistent state (a set, a map, a bool) updated as events arrive; never derive state by scanning backward through time.
- **Match the surrounding shell** (zsh interactive / POSIX sh); `shellcheck` + `shfmt` any `.sh` you touch — run `just shfmt-fix` to format (both gated by `just lint` + the CI `hygiene` job). **macOS first**: BSD CLI, brew, launchd.
- **Keep docs current.** A change that alters module structure, architecture, workflow, or public API updates the relevant `CLAUDE.md` + `README.md` in the same commit.
- **A refuted finding cites (or adds) a sharp edge.** When you reject a review finding as "deliberate design," point at the relevant per-crate `CLAUDE.md` "Known sharp edges" entry — or add one in the same change. That keeps the context accurate for the next agent (the real payoff).
- **Track every deferred finding as a GitHub issue** BEFORE moving on — problem, why deferred, fix sketch. A deferred finding with no issue is a silently-dropped finding. (Verify it's real first — see "Don't blindly accept reviewer findings".)
- **Sprite changes require visual verification** — render, crop, read the PNG, self-critique until it reads at half-block scale; commit messages carry the iteration history. Full checklist: `.claude/skills/beautify-decoration/SKILL.md`.
- **Periodic context-file audits also distill memory**: each `/revise-claude-md`-style audit sweeps recent session memories for promote-to-repo candidates (the memory layer of [`docs/KNOWLEDGE-ENGINEERING.md`](docs/KNOWLEDGE-ENGINEERING.md)).
- **The lifecycle conventions above are PRACTICES, not a gate.** Two-lens review before merge, deferred→issue, docs-currency, no stray prod-`println!`, no direct `settings.json` write, no `--no-verify` — do them because they're right, not because a script blocks you. A one-person gate run against oneself is ceremony, not enforcement — real teeth live in the automated checks (`just preflight`, clippy, tests, the `claude-review` second lens).

## Architecture invariants

These are load-bearing; don't break them without updating the spec.

1. **`pixtuoid-core` has no terminal dependencies.** No `ratatui`, no `crossterm`, no `stdout` writes. A NEW render target (window, canvas, PNG/GIF, …) plugs in as another thin painter over `pixtuoid_scene::floor::render_floor` / `pixel_painter::render_to_rgb_buffer` — THE seam every post-split painter (TUI flush, floating window, web hero) actually rides. **`pixtuoid-scene` (the render+sim engine) is ALSO terminal- AND window-free** — and now COMPILER-enforced by the crate boundary: `ratatui`/`crossterm`/`winit`/`softbuffer` aren't in its `Cargo.toml`, so reaching for one won't compile. `just arch` covers BOTH crates. Terminal/window code lives in the `pixtuoid` binary's painters (`tui/`, `floating/`).
2. **Agent events flow through ONE channel** typed `mpsc::Sender<(Transport, AgentEvent)>`. The `Transport` tag is load-bearing — the reducer uses it for hook-wins dedup. Do not hardcode `Transport::Hook` on the consumer side; the producer tags its own events. Daemon PRESENCE is deliberately `AgentId`-free and rides its own sibling channel, carrying `PresenceMsg { key: DaemonInstanceKey, delta }` — so N daemons AND N concurrent instances of one daemon (two OpenClaw gateways) route to distinct `SceneState::daemons` entries, and presence never enters `Reducer::apply`.
3. **`Source` trait is the only seam for adding a transcript-bearing agent CLI.** Per-source format knowledge lives in the source's own decoder fn, not a shared decoder. TWO documented exceptions: a **hook-only** CLI (Reasonix); and a shared cross-vendor **WIRE STANDARD** — **ACP** (Agent Client Protocol) decode lives once in `source/acp.rs` (`KNOWN_ACP_TAGS` + `decode_session_update`), reused by any ACP-speaking source (grok today). ACP is a versioned multi-vendor protocol (a shared serde model), NOT per-source format; the per-source dispatch judgment (tool-detail / Task-detection, injected) + a source's OWN extension namespace (grok's `_x.ai/session/update`) stay bespoke. See `crates/pixtuoid-core/CLAUDE.md` "multi-source decoding".
4. **Hook install writes through symlinks.** `install::install_target`/`uninstall_target` (driven by the in-TUI Sources panel `s` — there is no `install-hooks` CLI) go through `resolve_symlink` in `install/io.rs`, critical for stow-managed `~/.claude/settings.json`; on Windows `write_config_atomic` keeps a bounded rename-retry (sharing violations are a platform reality).
5. **The hook shim must never block CC.** Always exit 0 silently on any error; the 200ms send bound is non-negotiable (watchdog thread on BOTH platforms). The watchdog hard-exits, so `send_line` has NO in-process tests — all shim coverage is child-process level.
6. **Walkable mask = ground footprint only.** Visual sprites can be wider than their footprint; the mask blocks only the ground-level projection, so characters walk right next to walls.

## Known sharp edges (index)

Don't be surprised by these — and don't "fix" them. One line each here; the
full WHY lives in the nested `CLAUDE.md` for the owning crate.

**`pixtuoid-core`** ([full entries](crates/pixtuoid-core/CLAUDE.md)):
- CC hook payloads DO include `tool_use_id` (hook-wins dedup fires).
- CC hook `transcript_path` points at the PARENT transcript; subagent-leak is suppressed via `active_tasks`, and liveness flows UP (`refresh_lineage`). CC's `SubagentStart`/`SubagentStop` hooks decode (`decode_cc_hook_custom`).
- The JSONL watcher gates historical/ended transcripts on EVERY first-sight path (`should_seed_at_eof`), and "recent" is the source's own ACTIVITY clock where it has one (CC only), not the file mtime — the same verdict guards the revive-on-append path; a liveness vouch (CC pid registry / Codex+omp open FDs / grok registry) exempts the RECENCY half only — a structural end marker still gates, and `revouch_gated_files` re-checks it. Content NEVER drives lifecycle. The probe also powers ongoing liveness: the `ProofOfLife` sweep exemption, the negative vouch, and the ms-scale `exit_watch` rung.
- A hook event for an unknown session id registers it (hooks are proof of life), normally with real `Identity`; JSONL events never synthesize.
- Abrupt exits have no `SessionEnd` → stale-sweep cascade, guarded by the liveness-vs-readiness exemptions.
- Subagent display names come from `attributionAgent`; the dispatch tool is **`Agent`** (the one known name — the legacy `Task` name arm was dropped in 0.12.0; a pre-rename dispatch still carries `subagent_type`, THE semantic detection signal); `Workflow` is deliberately NOT mapped.
- Codex subagents wire via the SubagentStart/Stop hooks (flat rollout, no path nesting).
- Subagent clean-exit ladder: b1 drain / SubagentStop hooks / child-ledger re-links / the un-claim side-channel.
- `AgentSlot.state_started_at` is `SystemTime` (process-local; the whole `SceneState` tree is `Serialize`/`Deserialize` for debug dumps + the snapshot golden, NOT a stable wire contract — the v2-daemon consumer is closed out-of-scope, #279/#280/#281); `ActivityState::Active` ≠ "tool executing" (debounced via `ACTIVE_GRACE_WINDOW`).
- Each CLI's home resolver is a MIRROR of that CLI's own (audited against the shipped artifact, not docs); the axes deliberately NOT mirrored — copilot's `--config-dir` and legacy XDG, hermes's out-of-band profiles, CC's NFC + empty-value split — are named per source, and a RELATIVE override is unmirrorable because each process resolves it against its own cwd. omp is the deepest (config-dir NAME bound under home Node-style, two profile vars, an agent-dir override with a drop rule, and an XDG redirect that FLATTENS `agent/` away, which is why `omp_sessions_dir` is its own fn). `SourceDescriptor.home_env` makes the next source answer this at compile time.
- A daemon's runtime identity is its SOURCE's wire fact — OpenClaw's resolved gateway PORT, never the profile/pid/session; the process incarnation is separate state, and no pid start-marker guard is needed there.
- A `gatewayPort`-less OpenClaw envelope (a stale installed plugin) falls back to ONE legacy instance + a drift breadcrumb, rather than vanishing the mascot; a present-but-invalid port is rejected.
- `GatewayDown` (a first-hand wire report) may create an absent instance; the locally-synthesized `PidExited` never may — the creation-polarity asymmetry is deliberate.

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

- "How does a CC tool call become a moving sprite?" → `runtime/driver.rs::run_async` → `SourceManager::spawn` → source → decoder → `reducer::Reducer::apply` → `watch` channel → `TuiRenderer::render` → `pixtuoid_scene::pixel_painter::render_to_rgb_buffer` (the world render) → `tui::renderer::draw_scene` (the terminal flush). First half in `pixtuoid-core`; the world render in `pixtuoid-scene`; the flush in `pixtuoid`'s `tui`.
- Architecture overview + data-flow diagram: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md). Area-specific answers (layout, sources, install, themes, motion, pets, …) live in each crate's `WHERE-TO-LOOK.md`, indexed by question in its `CLAUDE.md`.
- One change spanning the Rust lib + the site + the Raycast extension: [`docs/PARALLEL-DELIVERY.md`](docs/PARALLEL-DELIVERY.md). How lessons persist across agent runs: [`docs/KNOWLEDGE-ENGINEERING.md`](docs/KNOWLEDGE-ENGINEERING.md).
- **"What do I run, and when?"** — the running order (contract regen, preflight, the merge gate, dogfooding, the three OpenClaw e2e tiers, and the advisory backstops that surface risk but never gate): [`CONTRIBUTING.md`](docs/CONTRIBUTING.md#the-running-order).

## When refactoring

If you change the channel type, `Source` trait, `AgentEvent` enum, or reducer
signature, update **all four** test areas (`tests/reducer/`, `tests/e2e.rs`,
`tests/transport/socket.rs`, `tests/watcher/`) plus `runtime/driver.rs`; a
new `AgentEvent` variant also needs an `agent_id()` arm.

**Adding a new agent CLI**: source module + one `SourceDescriptor` row in
`source/registry.rs` (its `name` field IS the roster — `registered_source_names()`
projects `REGISTRY`) + runtime wiring in `runtime/driver.rs::run_async`
(transcript-bearing CLIs only; hook-only CLIs ship a `hook.custom` decoder + an
`install/` target instead) + a row in `site/src/sources.json` (bridge-tested
against `registered_source_names()`). The full 11-step checklist — which steps
are test-forced and which are on you — is in
[`CONTRIBUTING.md`](docs/CONTRIBUTING.md#adding-a-new-agent-cli). A new theme
and a new ambient sound have analogous `add-theme` / `procedural-lofi` skills.
