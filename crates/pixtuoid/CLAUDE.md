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
- **Windows focus-jump borrows the foreground thread's input state, and the two BETTER-KNOWN bypasses are refused on purpose.** A console TUI doesn't own its host window, so `SetForegroundWindow` flashes the taskbar instead …
- **`--graphics off` answers TWO questions at once, and the second one is the cutaway's — but it is a `doctor` flag, and NOTHING paints the cutaway yet.** `render_cutaway`'s only caller is `examples/cutaway_snapshot`, so `graphics::resolve` reports a …
- **Capacity GROWTH strands an already-allocated overflow agent, and the HUD keeps counting it — render-conservatively, count-completely.** `AgentSlot.floor_idx` is frozen at allocation while `floor_of`/`floor_range` recompute from …
- **Terminal cell aspect drives sprite design.** The half-block ▀ technique assumes ~1:2 cell aspect; sprites larger than ~16×16 px break on …
- **`--max-desks` has no hard default, applies to `run` only, and floating's capacity publish is `store`, not `fetch_max` — don't harmonize.** Absent, per-floor capacity is auto-computed from terminal size (`FALLBACK_DESKS = 16` only …
- **The floating pipeline boots in `resumed`, NOT in `floating::run` — the seed needs the PHYSICAL `window.inner_size()`.** The `[floating]` config size is LOGICAL by design (HiDPI-stable persistence), and buffer size is …
- **Re-install is a SEMANTIC no-op, and backups APPEND their suffix.** `MergeOutcome.changed` compares the parsed/merged config, not bytes — a second connect skips the …
- **Daemon presence is ANNOUNCE-only — a gateway pixtuoid never heard announce is invisible until its next activity; don't "fix" with a poll.** `gateway_start` fires once per gateway process start, so connecting under a running gateway (or …
- **Connecting OpenClaw is NOT the last step — a RUNNING gateway must restart, and the presenters say so.** Upstream marks `plugins.load` reload-kind `restart` (verified in the shipped bundle), so …
- **`connect openclaw` binds ONE OpenClaw state dir — and that ONE install covers EVERY gateway of that profile (profile↔gateway is 1:N, verified live).** `gateway run --port` lets one state dir host N concurrent gateways off a single connect (which …
- **OpenClaw's config is JSON5, so pixtuoid REFUSES to rewrite a non-strict document instead of "fixing" it.** Our `serde_json` round-trip would silently delete a human's comments, so …
- **Two surfaces bind a source, ONE core.** `crate::sources::{connect,disconnect}` (persist + install/uninstall + rollback) is the single …
- **`OutcomeRow` is `{id, outcome, message?}` — and the wire is PUBLISHED; no more flag-day edits.** The split from the folded `failed: <msg>` string shipped while the store copy of the Raycast …
- **Code-artifact targets: install writes ⊆ verify checks, CONTENT included (#387).** opencode's TS plugin IS its `config_path` (schema check covers it); OpenClaw's JS plugin is …
- **A decoder arm is NOT evidence the event arrives — `*_EVENTS` in `install/<cli>.rs` is the other half of the wire, and nothing links the two.** CC's permission gate is `PermissionRequest`; `decoder.rs` has handled that arm since Codex …
- **`doctor`'s `<cli> --version` probes are GATED on presence — several CLIs bootstrap their state dir on ANY invocation.** An unconditional sweep in a pristine HOME created hundreds of entries in exactly the dirs …
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

