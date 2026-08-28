# pixtuoid-core — agent guide

The **headless library**: the source/decoder seam, the reducer/state machine,
sprite parsing, grid/walkability. No terminal deps (workspace invariant #1).
Sim geometry lives in `pixtuoid-scene`; only the coherence-bound `walkable.rs`
stays here. Cross-cutting rules: workspace [`CLAUDE.md`](../../CLAUDE.md).

## Layout

Module map: `ls src/` — each file's `//!` header is its annotation.

## When refactoring

The channel type, `Source` trait, `AgentEvent` enum, and reducer signature are workspace-wide contracts — see the root [`CLAUDE.md`](../../CLAUDE.md) "When refactoring" for the full list of test files to update and the add-a-CLI checklist.
