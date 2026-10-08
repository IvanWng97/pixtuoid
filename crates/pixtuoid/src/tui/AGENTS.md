# pixtuoid/tui — terminal renderer agent guide

The **terminal painter**: ratatui `App` + `TuiRenderer` (its inherent `render`
flush). Owns the half-block flush, mouse hit-testing, and the crossterm event
loop + terminal lifecycle; the panels it paints, their UI models and the key
dispatch are `crate::panels`, which the window shares. The screen-space compass every N/S claim here rests on is in the
[scene guide](../../../pixtuoid-scene/AGENTS.md). Module map: `ls` this
directory — each file's `//!` header is its annotation. Cross-cutting rules:
workspace [`AGENTS.md`](../../../../AGENTS.md).

## When refactoring

Changes to `flush_classic`, the widgets, or the dispatch precedence add or update
a harness test (`tui_renderer/harness` drives the real `TuiRenderer` through a
ratatui `TestBackend`, output-first). Don't reach back into `floating/` from
here.
