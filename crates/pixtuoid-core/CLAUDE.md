# pixtuoid-core — agent guide

The **headless library**: the source/decoder seam, the reducer/state machine,
sprite parsing, grid/walkability. Sim geometry lives in `pixtuoid-scene`; only
the coherence-bound `walkable.rs` stays here. Module map: `ls src/` — each
file's `//!` header is its annotation. Test layout and the add-a-CLI test steps:
[`tests/CLAUDE.md`](tests/CLAUDE.md). Cross-cutting rules: workspace
[`CLAUDE.md`](../../CLAUDE.md).
