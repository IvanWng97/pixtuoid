# pixtuoid (binary) — agent guide

The **TUI binary**: `ratatui` + `crossterm` + `winit` + `tokio` + `clap`. Wires
sources → reducer → renderer, owns the CLI subcommands, hook installation, config
persistence, and multi-floor orchestration. The backend-agnostic render +
simulation **engine** (layout, pose/motion/pathfinding, the pixel pass, the theme
model, pets, chitchat) is its OWN dependency crate `pixtuoid-scene` (it used to be
an in-binary module) — see [`../pixtuoid-scene/CLAUDE.md`](../pixtuoid-scene/CLAUDE.md);
the DAG is `pixtuoid-core ← pixtuoid-scene ← {pixtuoid, pixtuoid-web}`. This
binary's two thin painters **over the `pixtuoid-scene` crate** are the terminal
renderer `src/tui/` ([`src/tui/CLAUDE.md`](src/tui/CLAUDE.md)) and the
`floating/` desktop window (neither depends on the other); the wasm `<canvas>`
painter is the SIBLING crate `pixtuoid-web`. Cross-cutting rules: workspace
[`CLAUDE.md`](../../CLAUDE.md); headless-lib detail:
[`../pixtuoid-core/CLAUDE.md`](../pixtuoid-core/CLAUDE.md).

## Layout

Module map: `ls src/` — each file's `//!` header is its annotation.
(`sprites/` holds the robot + skeleton packs, NOT under pixtuoid-hook.)

## Known sharp edges (don't be surprised by these)

Full entries in [`SHARP-EDGES.md`](SHARP-EDGES.md) — grep it for the phrase.

<!-- edges:start · generated from SHARP-EDGES.md by `just gen-guides` — edit the entry there, not this line -->
- **Windows focus-jump borrows the foreground thread's input state; the two better-known bypasses are refused on purpose.** `AttachThreadInput` + one retry; the verdict is `GetForegroundWindow`, not the BOOL (a …
- **`--graphics off` conflates protocol capability with the cutaway's profile decision — deliberately.** Nothing paints the cutaway yet (`render_cutaway`'s only caller is `examples/cutaway_snapshot`) …
- **Capacity GROWTH strands an already-allocated overflow agent; render conservatively, count completely.** `AgentSlot.floor_idx` freezes at allocation while `floor_of` recomputes from current capacities …
- **Terminal cell aspect drives sprite design.** The half-block ▀ technique assumes ~1:2 cell aspect; sprites larger than ~16×16 px break on …
- **`--max-desks` applies to `run` only, and floating's capacity publish is `store`, not `fetch_max` — don't harmonize.** `run`: capacity auto-computes from terminal size; an explicit cap clamps via `fetch_max` …
- **The floating pipeline boots in `resumed`, not `floating::run` — the seed needs the PHYSICAL `window.inner_size()`.** Config size is LOGICAL (HiDPI-stable) and buffer size is not monotone in scale factor, so no …
- **Re-install is a SEMANTIC no-op, and backups APPEND their suffix.** `MergeOutcome.changed` compares parsed configs, not bytes; `backup_once` appends `.pixtuoid.bak` …
- **Daemon presence is ANNOUNCE-only; don't "fix" invisibility with a poll.** `gateway_start` fires once per gateway process start, so a gateway pixtuoid never heard stays …
- **Connecting OpenClaw is not the last step — a RUNNING gateway must restart.** Upstream marks `plugins.load` reload-kind `restart`, so `connect` legitimately succeeds with no …
- **`connect openclaw` binds ONE state dir, and that install covers EVERY gateway of that profile (1:N, verified live).** Upstream resolves the dir WITHOUT reading `OPENCLAW_PROFILE`, so a gateway under another profile …
- **OpenClaw's config is JSON5, so pixtuoid REFUSES to rewrite a non-strict document instead of "fixing" it.** A `serde_json` round-trip silently deletes a human's comments — so would adding a JSON5 parser …
- **`disconnect` reserves `Err` for the persist-abort and folds hook-removal failure into `Ok` — never a silent clean "disconnected".** One core (`crate::sources::{connect,disconnect}`), two presenters (Sources panel, CLI) that both …
- **`OutcomeRow` `{id, outcome, message?}` is a PUBLISHED wire — no flag-day edits.** Installed Raycast copies parse the wire independently of the binary's version (the folded→split …
- **Code-artifact targets: install writes ⊆ verify checks, CONTENT included (#387).** The artifact that BAKES the shim path is read back and stat'd (`check_shim_binary`) — a moved …
- **A decoder arm is NOT evidence the event arrives — `*_EVENTS` in `install/<cli>.rs` is the other half of the wire, and nothing links the two.** A decode arm without its install-list row rendered a permission-parked CC session as WORKING …
- **`doctor`'s `<cli> --version` probes are GATED on presence — several CLIs bootstrap their state dir on ANY invocation.** An unconditional sweep manufactured the presence it reports (and defeated HOME-isolated e2e). …
<!-- edges:end -->

## Where to look

Answers live in [`WHERE-TO-LOOK.md`](WHERE-TO-LOOK.md), so a session
pays for the entry it needs instead of all of them. Grep it for the
question:

<!-- lookup:start · generated from WHERE-TO-LOOK.md by `just gen-guides` — edit the entry there, not this list -->
- How do hooks get installed?
- Where do runtime errors / config warnings surface?
- How does config persistence work?
- How do multi-floor offices work?
<!-- lookup:end -->

