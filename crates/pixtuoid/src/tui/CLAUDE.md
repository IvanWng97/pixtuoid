# pixtuoid/tui — terminal renderer agent guide

The **terminal painter**: ratatui `App` + `TuiRenderer` (its inherent `render`
flush). Owns the half-block flush, the widgets, mouse hit-testing, the version
popup, the crossterm event loop + terminal lifecycle, and the per-session UI
models. The engine's deep "why" lives in the
[scene guide](../../../pixtuoid-scene/CLAUDE.md); `tui` and `floating` are
sibling painters — neither depends on the other. Cross-cutting rules:
workspace [`CLAUDE.md`](../../../../CLAUDE.md).

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

Grep [`WHERE-TO-LOOK.md`](WHERE-TO-LOOK.md) for the question:

<!-- lookup:start · generated from WHERE-TO-LOOK.md by `just gen-guides` — edit the entry there, not this list -->
- How is the office rendered (pixel pass → terminal)?
- How does the version popup render?
- How does the agent dashboard work?
- How does the footer instrument work?
- How does the Sources panel work?
- How does first-run onboarding work?
<!-- lookup:end -->

## When refactoring

Changes to `draw_scene`, the widgets, or the dispatch precedence add or update
a harness test (`tui_renderer/harness` drives the real `TuiRenderer` through a
ratatui `TestBackend`, output-first). Don't reach back into `floating/` from
here.
