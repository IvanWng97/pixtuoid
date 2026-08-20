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
