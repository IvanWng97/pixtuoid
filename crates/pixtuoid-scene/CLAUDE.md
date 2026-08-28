# pixtuoid-scene — render+simulation engine crate guide

The **backend-agnostic render + simulation engine**: layout geometry,
pose/motion/pathfinding, the pixel pass (`render_to_rgb_buffer` — the shared
world render), the color-theme MODEL, pets, chitchat, frame cache, embedded
sprite pack. Terminal- AND window-free by crate boundary (workspace invariant
#1); the three painters (`tui`, `floating`, `pixtuoid-web`) sit on top and
none depends on another. Cross-cutting rules: workspace
[`CLAUDE.md`](../../CLAUDE.md).

## Screen-space compass (THE convention — read before reasoning about N/S)

Directions in this crate are **SCREEN-SPACE**, map-style (north = up), NOT
real-world headings. Pin this and stop re-deriving it:

- **North = −y = screen TOP** — the far wall, the floor-to-ceiling windows,
  the city skyline (`mask.rs`'s `north wall band`). "Behind" a piece.
- **South = +y = screen BOTTOM** — the near side, the FRONT, toward the
  viewer. This is the z-sort **"south row"** (`placement.rs`: *the z-sort row
  IS the south row of the box*) and the **south-anchored** ground strip
  (`GroundAlign::End`): a sprite's front/base row.
- East = +x (right), West = −x (left).

A piece's approach set is canonical (facing-South) then rotated by live
`Facing` — `layout.desk_facing(i)` is the authority; never assume the
canonical set is the live one. (The compass stays screen-space even where
real-world geography disagrees — flipping it would invert the z-sort/"south
row" vocabulary across 400+ sites for zero behavior change.)

## Layout

Module map: `ls src/` — each file's `//!` header is its annotation.

## The corpus census (`examples/corpus_check.rs`)

`just corpus-all` (or `--example corpus_check -- <source>`) drives every local
transcript through the real decode→reduce pipeline and asks
`FloorSession::observe` for a non-empty `SimFrame.characters` — the headless
"a sprite would be painted" seam. It REPORTS rather than gates (corpora are
partly historical); the hard failures are a decode `Err` or a PANIC on bytes
the source itself wrote. Detail: the example's own `//!` header.

## When refactoring

Changes to `derive_with_routing`, `MotionState`, or the pixel passes add or
update a frame-by-frame continuity guard (`motion/tests.rs`, `pose/tests.rs`,
the binary's `tui_renderer/harness`) — the flash/teleport/replay regressions
all came back as failing tests first. Terminal/window code belongs in the
binary's painters, not here (invariant #1).
