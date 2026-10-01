# pixtuoid-scene — render+simulation engine crate guide

The **backend-agnostic render + simulation engine**: layout geometry,
pose/motion/pathfinding, the pixel pass (`render_to_rgb_buffer` — the shared
world render), the color-theme MODEL, pets, chitchat, frame cache, bundled
sprite pack. The three painters (`tui`, `floating`, `pixtuoid-web`) sit on top.
Module map: `ls src/` — each file's `//!` header is its annotation.
Cross-cutting rules: workspace [`AGENTS.md`](../../AGENTS.md).

## Screen-space compass (THE convention — read before reasoning about N/S)

Directions in this crate are **SCREEN-SPACE**, map-style (north = up), NOT
real-world headings. Pin this and stop re-deriving it:

- **North = −y = screen TOP** — the far wall, the floor-to-ceiling windows,
  the city skyline (the north wall band, `layout/compute.rs::top_margin`).
  "Behind" a piece.
- **South = +y = screen BOTTOM** — the near side, the FRONT, toward the
  viewer. This is the z-sort **"south row"** (`placement.rs` pins the z-sort
  row to the box's south row) and the **south-anchored** ground strip
  (`GroundAlign::End`): a sprite's front/base row.
- East = +x (right), West = −x (left).

A piece's approach set is canonical (facing-South) then rotated by live
`Facing` — `layout.desk_facing(i)` is the authority; never assume the
canonical set is the live one. (The compass stays screen-space even where
real-world geography disagrees — flipping it would invert the z-sort/"south
row" vocabulary across the crate for zero behavior change.)

## The corpus census

`just corpus-all` drives every local transcript through the real render seam;
what it reports and why it doesn't gate: `examples/corpus_check.rs`'s `//!` header.

## When refactoring

Changes to `derive_with_routing`, `MotionState`, or the pixel passes add or
update a frame-by-frame continuity guard (`motion/tests.rs`, `pose/tests.rs`,
the binary's `tui_renderer/harness`) — the flash/teleport/replay regressions
all came back as failing tests first.
