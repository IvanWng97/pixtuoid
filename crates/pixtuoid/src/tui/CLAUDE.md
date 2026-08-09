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

Annotated tree in [`LAYOUT.md`](LAYOUT.md) — grep it for a filename.

```
tui/
├── renderer.rs     draw_scene<B: Backend> orchestrator (DrawCtx struct …
├── mod.rs          crossterm event loop + terminal lifecycle (raw mode / …
├── ui_state.rs     UiState — run_tui's per-surface UI state as ONE struct …
├── widgets/        ratatui widget paint fns, split into sub-modules:
├── hit_test/       mouse hit-test: agent hover, coffee machine click …
├── tui_renderer/   TuiRenderer + its inherent `render` flush, split …
├── connection/     Sources panel UI-side model ONLY (no ratatui) …
├── dashboard/      agent-dashboard PURE model (no ratatui): mod.rs …
└── welcome/        first-run onboarding PURE model (no ratatui): mod.rs …
```

## Known sharp edges (don't be surprised by these)

Full entries in [`SHARP-EDGES.md`](SHARP-EDGES.md) — grep it for the phrase.

- `draw_scene` is called through `TuiRenderer`
- The 6 borderless popups share ONE geometry authority — `panel::PanelGeometry`.
- The board's ★ Star click target is `wall_board::star_hit_rect`, the same phantom-launch class
- The CLICK ladder (`tui/mod.rs`) and the HOVER ladder (`renderer.rs`) share the `agent > coffee > pet` ordering AND the underlying hit-tests, but a unified `resolve_pointer_hit(...) -> PointerHit` enum is NOT worth hoisting.
- Every modal overlay paints on EVERY frame — including the footer-only ones.
- Every crossterm key event passes `should_dispatch_key` first
- The `w` walkable/approach/route debug overlay is dev-only
- `run_tui` is the `block_on` ROOT future, not a tokio worker.

## Where to look

Answers live in [`WHERE-TO-LOOK.md`](WHERE-TO-LOOK.md), so a session
pays for the entry it needs instead of all of them. Grep it for the
question:

- How is the office rendered (pixel pass → terminal)?
- How does the neon wall board work?
- How does the theme PICKER UI work?
- How are popups framed?
- How does the version popup render?
- How does the `?` help overlay work?
- How does the agent dashboard work?
- How does the footer instrument work?
- How does the Sources panel work?
- How does first-run onboarding work?
- How does the hover tooltip (the agent DOSSIER) work?
- How does the coffee machine Easter egg work?
- How do furniture hover tooltips work?
- How does the pet tooltip / click-to-pet work?
- How does multi-floor navigation work (the painter side)?

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
