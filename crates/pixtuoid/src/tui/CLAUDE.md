# pixtuoid/tui — terminal renderer agent guide

The **terminal painter**: ratatui `App` + `TuiRenderer` (its inherent `render`
flush). Owns the half-block flush, the widgets (footer/wall-display/theme-picker
UI/tooltip/help/dashboard/Sources panel), mouse hit-testing, the version popup,
the crossterm event loop + terminal lifecycle, and the per-session UI models
(dashboard, Sources/connection). This is where the `pixtuoid-scene` engine's pixel
buffer becomes terminal cells.

**The render + simulation ENGINE is its OWN crate
[`pixtuoid-scene`](../../../pixtuoid-scene/CLAUDE.md)** (the DAG is
`pixtuoid-core ← pixtuoid-scene ← pixtuoid`) — layout, pose/motion/pathfinding,
the pixel pass (`render_to_rgb_buffer`), the color theme MODEL, pets, chitchat,
frame_cache, embedded_pack. `tui` and `floating` are sibling thin painters over
the `pixtuoid-scene` crate; neither depends on the other. When a painter entry
below references engine internals (`pixtuoid_scene::pixel_painter`,
`pixtuoid_scene::theme`, `pixtuoid_scene::motion`, …) follow the link to the
[scene guide](../../../pixtuoid-scene/CLAUDE.md) for the deep "why."

Parent guides: binary [`../../CLAUDE.md`](../../CLAUDE.md); workspace
[`../../../../CLAUDE.md`](../../../../CLAUDE.md); headless lib
[`../../../pixtuoid-core/CLAUDE.md`](../../../pixtuoid-core/CLAUDE.md); the engine
[`../../../pixtuoid-scene/CLAUDE.md`](../../../pixtuoid-scene/CLAUDE.md).

## Layout

Module map: `ls tui/` — each file's `//!` header is its annotation.

## Known sharp edges (don't be surprised by these)

Full entries in [`SHARP-EDGES.md`](SHARP-EDGES.md) — grep it for the phrase.

<!-- edges:start · generated from SHARP-EDGES.md by `just gen-guides` — edit the entry there, not this line -->
- **The 6 borderless popups share ONE geometry authority — `panel::PanelGeometry`.** `compute(..) → outer()/inner()/cell_rect()` is PURE: the painter fills `inner()` and every …
- **The board's ★ Star click target is `wall_board::star_hit_rect` — same phantom-launch class.** Paint and hit-rect derive from the SAME `BOARD_W`/`BOARD_STAR`; the `debug_assert` in …
- **The CLICK ladder (`tui/mod.rs`) and HOVER ladder (`renderer.rs`) share the `agent > coffee > pet` ordering and the hit-test primitives, but a unified resolver is NOT worth hoisting.** Different layers, different target sets, and the click FUSES test+act in `focus_clicked_agent` …
- **Every modal overlay paints on EVERY frame — including footer-only frames.** Below `renderer::min_terminal_size()` rendering falls to `draw_footer_only_frame` but every key …
- **Every crossterm key event passes `should_dispatch_key` first**: Windows delivers Press AND Release (and Repeat) per keystroke, so only `KeyEventKind::Press` …
- **The `w` walkable/approach/route debug overlay is dev-only** — its dispatch arm is `#[cfg(debug_assertions)]`-gated, so release builds silently ignore `w`. …
- **`run_tui` is the `block_on` ROOT future, not a tokio worker — `block_in_place` there is INERT.** The loop body runs on the `block_on` thread, which owns no worker core (removed as proven inert …
<!-- edges:end -->

## Where to look

Answers live in [`WHERE-TO-LOOK.md`](WHERE-TO-LOOK.md), so a session
pays for the entry it needs instead of all of them. Grep it for the
question:

<!-- lookup:start · generated from WHERE-TO-LOOK.md by `just gen-guides` — edit the entry there, not this list -->
- How is the office rendered (pixel pass → terminal)?
- How does the version popup render?
- How does the agent dashboard work?
- How does the footer instrument work?
- How does the Sources panel work?
- How does first-run onboarding work?
<!-- lookup:end -->

## When refactoring

The terminal render path is exercised by the headless harness
(`tui_renderer/harness`, ~100 tests — 99 as of #435) driving the real `TuiRenderer` through a
ratatui `TestBackend` — output-first (`buf()` pixels + `frame_buffer` cells).
Changes to `draw_scene`, the widgets, or the dispatch precedence should add or
update a harness test. The engine-side authority (`derive_with_routing`,
`MotionState`, the pixel passes) lives in the `pixtuoid-scene` crate — see
[`../../../pixtuoid-scene/CLAUDE.md`](../../../pixtuoid-scene/CLAUDE.md)'s "When
refactoring." Don't reach back into `floating/` from here: `tui` and `floating`
are sibling painters that share the `pixtuoid-scene` crate, not each other.
