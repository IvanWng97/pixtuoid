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

Module map: `ls src/` — each file's `//!` header is its annotation. The
render-seam digest (`render_floor`/`FloorSession`) lives in
[`WHERE-TO-LOOK.md`](WHERE-TO-LOOK.md) under "How is the office rendered".

## The corpus census (`examples/corpus_check.rs`)

`just corpus-all` (or `--example corpus_check -- <source>`) drives every local
transcript through the real decode→reduce pipeline and asks
`FloorSession::observe` for a non-empty `SimFrame.characters` — the headless
"a sprite would be painted" seam. It REPORTS rather than gates (corpora are
partly historical); the hard failures are a decode `Err` or a PANIC on bytes
the source itself wrote. Detail: the example's own `//!` header.

## Known sharp edges (don't be surprised by these)

Full entries in [`SHARP-EDGES.md`](SHARP-EDGES.md) — grep it for the phrase.

<!-- edges:start · generated from SHARP-EDGES.md by `just gen-guides` — edit the entry there, not this line -->
- **Agent OUTFIT (shirt+pants) is keyed on the normalized `cwd`, not `agent_id`** (Team Palette): same working directory → same outfit; hair/skin stay `agent_id`-seeded. The …
- **`recolor_frame` substitutes by RGB equality** — sound because each recolor key maps to a unique RGB, enforced at pack load …
- **EXIT walks are time-compressed to fit the GC window; entry/wander/snap-back are not.** An exiting slot races `EXIT_GRACE_WINDOW` (4.5s), so `derive_with_routing` scales `elapsed` …
- **A walk leg's A\* polyline shape is frozen once per leg, not re-routed per frame.** A mid-leg re-route remaps frozen progress onto a different polyline — teleport + fresh A\* per …
- **A meeting room narrower than `MEETING_FURNITURE_MIN_W` has NO sofa/table/seats — bare floor, BY DESIGN.** Below it the coarse router can't reach the seats (a seated trip would teleport). Guarded by …
- **Occlusion is EMERGENT — there is no `occludes_behind` field / synthetic cap (deleted).** A piece's blocked rect is its `footprint` offset inside the `visual` box via ONE formula …
- **splitmix64 is open-coded FOUR times in this crate on purpose** (sky weather, lightning, dust motes, outfit seed) — each an independent noise source over a …
- **`paint_glass_wall_h` and `paint_glass_wall_v` stay SEPARATE — don't hoist a `paint_glass_strip(axis, alphas)` helper.** They share only the tone-ladder SKELETON and diverge at every load-bearing point; the unifier's …
- **`epoch_ms(now)` is the ABSOLUTE wall-clock ms (~1.7e12) — reduce it BEFORE any `as f32`.** At that magnitude an f32 ULP is ~131s: a direct cast freezes any animation reading it. Reduce …
- **The sun/moon disc keeps a "real low window":** low-altitude only, clipped near apex, gated to the ONE window its centre is over, hidden under …
- **Day-over-night light invariant:** the moon casts no direct beam; `solar_noon_outshines_the_brightest_night` guards the calibration …
- **The weather VEILS are lit by the emitter, and cross-weather brightness ordering is NOT an invariant.** Veils scale with `veil_lum` (folding in `atmo`/`darkness` would darken a stormy noon twice). Lit …
- **An EXCLUSIVE spot is single-occupancy, enforced where the destination is CHOSEN — not where it's drawn.** `SpotClaims` + a forward probe; the gate is `furniture_def(..).exclusive`, never a hand-listed …
- **The free-standing whiteboard OVERLAPS a north-facing desk's seat in the committed stills — an ACCEPTED look, not a bug.** The board's aisle band IS the seat band; the ground strip clears the seat cell; the owner …
- **`desk_facings` is index-parallel to `home_desks` and stays that way — consolidation into one `Vec<Desk>` was weighed and declined.** Both are `pub` on a published crate (a second breaking change later); reads funnel through …
- **A back-turned desk with no reachable south approach is DEMOTED to viewer-facing — and that rung never fires in any swept layout.** A net for a future placer; a pod whose rows face the same way is a `pod_row_facing` bug, not …
- **Two narrow-band connectivity guards keep the office ONE region (#566), both graceful DECOR degradation — not bugs.** The couch can seal the door column; a relocated plant can plug the sole drain. …
- **Both layout floors are DERIVED per axis and deliberately UNIFORM across variants** — `MIN_LAYOUT_W`/`_H` solve the band formulas against the WIDEST variant column: one number the …
- **The size gate cannot be replaced by a search over the real placer — below it `compute_with_seed` is out of contract, and RELEASE fails silently.** Below the floor a `debug_assert!` fires and a subtraction underflows — DEBUG-only; release …
- **The free-standing whiteboard stands ONLY in an inter-pod aisle, and is ABSENT rather than relocated when the band holds a single pod row.** Unsnapped, its anchor sealed the west lane …
- **A desk chair's z key TIES with its occupant's and the order is decided by INSERTION, not by the key.** `enqueue_desk_chairs` runs after `enqueue_characters`, sort stable — reorder or use an unstable …
- **Every wander destination is filtered by `ReachSet`, but the ±4px `jitter_dest` perturbation applies AFTER that filter — an accepted residual.** The jitter is a lockstep contract (rendered shape, profile length, router-cache key read the …
- **The GENERATED night pad renders NO sub-bass — the BASS stem owns that register; only the frozen v4 anchor bakes its sub in.** Re-adding one doubles the low end and pushes the chords out of headroom; deleting the anchor's …
- **Every ARTIFICIAL FLOOR light scales by `indoor_scale`; the self-lit WALL fixtures deliberately do not.** An emptied floor reads dark because its lights go out — the old `EMPTY_FLOOR_DIM_BOOST` was …
- **The hour's cast runs as TWO `wash_since` passes over a snapshot diff, and foreground EMITTERS are deliberately inside the second one.** The grouping axis is PAINT ORDER; the exclusions differ on purpose …
- **The north wall band is SOLID for its whole width — the elevator doorway is a hole in the WALL, not the ground — and a sprite overlapping the band is the intended look** (feet-anchored, invariant #6). Re-cutting a channel (#902) is invisible to every routing guard …
- **Layout variety is spent out of budgets that already exist — a scattered plant can only stand where a DISPLACED one already could.** The scatter only reorders `settle_plant`'s inward ladder …
- **The painter's canvas nudge can seat a sprite ON furniture — the accepted side of a real conflict (#912).** `keep_sprite_on_canvas` moves a sprite inward without re-checking the mask; the residual is …
- **A character is clamped to the canvas TWICE, against two DIFFERENT frame sizes — deliberately.** `sim::resolve_characters` clamps the sprite on the pack's real frame …
<!-- edges:end -->

## Where to look

Grep [`WHERE-TO-LOOK.md`](WHERE-TO-LOOK.md) for the question:

<!-- lookup:start · generated from WHERE-TO-LOOK.md by `just gen-guides` — edit the entry there, not this list -->
- Where does a furniture's footprint / visual size / approach side / dwell come from?
- Why doesn't a bigger buffer just render the office sharper?
- Which side does an agent approach furniture from?
- How is the office rendered (pixel pass)?
- How do agent name-badge labels work?
- How do the room dividers render (frosted-glass partitions)?
- How does the theme system work?
- How does weather / sky-light work?
- How does the meeting room come alive?
- How does walk-pace physics work?
- What is the elastic wander timeline?
- How does the gateway mascot (the OpenClaw lobster) work?
- How does the lofi soundtrack get composed (and how do I add an instrument / a stem)?
<!-- lookup:end -->

## When refactoring

Changes to `derive_with_routing`, `MotionState`, or the pixel passes add or
update a frame-by-frame continuity guard (`motion/tests.rs`, `pose/tests.rs`,
the binary's `tui_renderer/harness`) — the flash/teleport/replay regressions
all came back as failing tests first. Terminal/window code belongs in the
binary's painters, not here (invariant #1).
