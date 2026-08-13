# pixtuoid-scene — render+simulation engine crate guide

The **backend-agnostic render + simulation engine CRATE**: the office world
itself — the layer between `pixtuoid-core` (the headless lib) and the `pixtuoid`
binary's painters. The workspace DAG is `pixtuoid-core ← pixtuoid-scene ← {pixtuoid, pixtuoid-web}`.
`pixtuoid-scene` owns layout geometry, pose/motion/pathfinding (the per-agent
motion-timing authority + A\* router), the half-block-agnostic **pixel pass**
(`render_to_rgb_buffer` — the SHARED world render), the color-theme MODEL, pets,
chitchat, the frame cache, and the embedded sprite pack. It is **terminal- AND
window-free BY CRATE BOUNDARY** — `ratatui`/`crossterm`/`winit`/`softbuffer` are
NOT in `pixtuoid-scene/Cargo.toml`, so "no terminal/window dependency" is now a
COMPILER-enforced fact (not merely a lint), and `just arch` covers this crate too.
THREE thin painters layer on top — `tui` (ratatui half-block) and `floating`
(winit/softbuffer) **in the `pixtuoid` binary**, plus `pixtuoid-web` (the wasm
`<canvas>` painter, built with `default-features = false`) — and **none depends
on another**. This is where the headless `SceneState` becomes a pixel buffer;
the painters add the flush. Parent guides: workspace [`../../CLAUDE.md`](../../CLAUDE.md);
headless lib [`../pixtuoid-core/CLAUDE.md`](../pixtuoid-core/CLAUDE.md); the binary
[`../pixtuoid/CLAUDE.md`](../pixtuoid/CLAUDE.md); the terminal painter
[`../pixtuoid/src/tui/CLAUDE.md`](../pixtuoid/src/tui/CLAUDE.md).

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

`ApproachSides` states this once (`decor.rs`: *north = −y*); a piece's
approach set is canonical (facing-South) then rotated by live `Facing`.
Worked example — the **home desk**'s CANONICAL set (`DESK_APPROACH =
{n:true, s:false, e:true, w:true}`) opens AWAY from the monitor. Half a pod is
back-turned, so that set is rotated per desk: a viewer-facing desk is
approached N/E/W, a back-turned one S/E/W. `layout.desk_facing(i)` is the
authority — never assume the canonical set is the live one. (We keep north=up even though a real sunny-window office in the
northern hemisphere would face its windows SOUTH — the compass is
screen-space, and flipping it would invert the entire z-sort/"south row"
vocabulary across 400+ sites for zero behavior change.)

## Layout

Module map: `ls src/` — each file's `//!` header is its annotation. The
render-seam digest (`render_floor`/`FloorSession`) lives in
[`WHERE-TO-LOOK.md`](WHERE-TO-LOOK.md) under "How is the office rendered".

## The corpus census (`examples/corpus_check.rs`)

The one place the render layer answers "would the UI actually SHOW this?" for
real, uncurated bytes: `cargo run --release -p pixtuoid-scene --example
corpus_check -- <source> <root> [--json]` walks the `.jsonl` under a live
transcript tree that the source's registry `path_filter` admits (the same set
the watcher would), drives each file through `pixtuoid_core::harness::Drive` (the
shared decode→reduce pipeline, first-sight seed included), then asks
`FloorSession::observe` whether its `SimFrame.characters` is non-empty — the
documented headless seam, so a non-empty set IS "a sprite would be painted", no
pixel buffer or terminal involved. It REPORTS (corpus content is unbounded and
partly historical, so a non-registering file is not automatically a bug); the
hard failures are a decode `Err` or a PANIC on bytes the source itself wrote.
The provenance column — file mtime minus the newest turn the SESSION wrote — is
the ghost-session class made countable, and it lives in this shell rather than
the registry because it feeds a report, not a contract.

## Known sharp edges (don't be surprised by these)

Full entries in [`SHARP-EDGES.md`](SHARP-EDGES.md) — grep it for the phrase.

<!-- edges:start · generated from SHARP-EDGES.md by `just gen-guides` — edit the entry there, not this line -->
- **Agent OUTFIT (shirt+pants) is keyed on the normalized `cwd`, not `agent_id`** (Team Palette): same working directory → same outfit, so the office reads as a color-coded …
- **`recolor_frame` substitutes by RGB equality**, which works because each recolor key maps to a unique RGB — enforced at pack load …
- **EXIT walks are time-compressed to fit the GC window; entry/wander/snap-back are not.** An exiting slot races `EXIT_GRACE_WINDOW` (4.5s), so `derive_with_routing` scales `elapsed` to …
- **A walk leg's A\* polyline shape is frozen once per leg, not re-routed per frame.** The router's own cache is invalidated by per-frame occupancy churn; without the freeze a mid-leg …
- **A meeting room narrower than `MEETING_FURNITURE_MIN_W` has NO sofa/table/seats — bare floor, BY DESIGN.** Below it the coarse router can't reach the seats, so a seated trip would teleport (find_path …
- **Occlusion is EMERGENT — there is no `occludes_behind` field / synthetic cap (deleted).** A piece's blocked rect is its `footprint` offset inside the `visual` box by declared …
- **splitmix64 is open-coded FOUR times in this crate on purpose** (sky weather, lightning, dust motes, outfit seed) beside the canonical `core::id::splitmix64` …
- **`paint_glass_wall_h` and `paint_glass_wall_v` stay SEPARATE — don't hoist a `paint_glass_strip(axis, alphas)` helper.** They share only the 5-branch tone-ladder SKELETON and diverge at every load-bearing point (cap …
- **`epoch_ms(now)` is the ABSOLUTE wall-clock ms (~1.7e12) — reduce it BEFORE any `as f32`.** At that magnitude an f32 ULP is ~131s, so a direct cast freezes any animation reading it (the …
- **The sun/moon disc keeps a "real low window":** visible only at low altitude, clipped above the glass near apex, gated to the ONE window its …
- **Day-over-night light invariant:** the moon casts no direct beam, and `solar_noon_outshines_the_brightest_night` guards the …
- **The weather VEILS are lit by the emitter, and cross-weather brightness ordering is NOT an invariant.** Veils scale with `veil_lum` (emitter's own luminance + `NIGHT_VEIL_FLOOR` city scatter — folding …
- **An EXCLUSIVE spot is single-occupancy, enforced where the destination is CHOSEN — not where it's drawn.** `SpotClaims` + a forward linear-probe resolve contention (venue seats are contiguous in …
- **The free-standing whiteboard OVERLAPS a north-facing desk's seat in the committed stills — an ACCEPTED look, not a bug.** The board's aisle band IS the north pod rows' seat band, the ground strip clears the seat cell …
- **`desk_facings` is index-parallel to `home_desks` and stays that way — consolidation into one `Vec<Desk>` was weighed and declined.** Both are `pub` on a published crate (a second breaking change later), the read side is already …
- **A back-turned desk with no reachable south approach is DEMOTED to viewer-facing — and that rung never fires in any swept layout.** It is a net for a future placer, not live code (0 demotions across the sweep), so a pod whose …
- **Two narrow-band connectivity guards keep the office ONE region (#566), both graceful DECOR degradation — not bugs.** At specific narrow bands the lounge couch can seal the door column (gated off via …
- **Both layout floors are DERIVED per axis and deliberately UNIFORM across variants — a narrow-left-column floor that could seat desks below them is refused anyway.** `MIN_LAYOUT_W`/`_H` solve `band_w`/`band_h` against `DESK_BAND_MIN_W`/`_H` (the two terms …
- **The size gate cannot be replaced by a search over the real placer — below it `compute_with_seed` is out of contract, and RELEASE fails silently.** The early `None` is also the precondition guard for the whole body: below the floor the `#566` …
- **The free-standing whiteboard stands ONLY in an inter-pod aisle, and is ABSENT rather than relocated when the band holds a single pod row.** Its `usable_h / 3` anchor is a hint, not a slot — unsnapped it sealed the west lane, and …
- **A desk chair's z key TIES with its occupant's and the order is decided by INSERTION, not by the key.** `enqueue_desk_chairs` runs after `enqueue_characters` and the sort is stable — reorder the calls …
- **Every wander destination is filtered by `ReachSet`, but the ±4px `jitter_dest` perturbation applies AFTER that filter — an accepted residual.** The jitter is a lockstep contract (rendered shape, profile length, and router-cache key all read …
- **The GENERATED night pad renders NO sub-bass — the BASS stem owns that register; only the frozen v4 anchor still bakes its sub in.** Re-adding a baked sub doubles the low end AND (because `night_pad_core` peak-normalizes) pushes …
- **Every ARTIFICIAL FLOOR light scales by `indoor_scale`; the self-lit WALL fixtures deliberately do not.** An emptied floor reads dark because its lights go out, not via a second darkening (the old …
- **The hour's cast on objects runs as TWO `wash_since` passes over a snapshot diff, and foreground EMITTERS are deliberately inside the second one.** The grouping axis is PAINT ORDER; the two passes' exclusions differ on purpose (emitters + …
- **The north wall band is SOLID for its whole width — the elevator doorway is a hole in the WALL, not in the ground — and a sprite overlapping the band is the intended look.** `build_walkable_mask` blocks the full band and takes no door input; every entry/exit leg starts …
- **Layout variety is spent out of budgets that already exist — a scattered plant can only stand where a DISPLACED one already could.** The scatter changes only which step of `settle_plant`'s existing inward ladder is tried first; a …
- **The painter's canvas nudge can seat a creature ON furniture — the accepted side of a real conflict, not a leftover of #912.** `keep_sprite_on_canvas` moves a centre-anchored creature inward without re-checking the mask …
<!-- edges:end -->

## Where to look

Answers live in [`WHERE-TO-LOOK.md`](WHERE-TO-LOOK.md), so a session
pays for the entry it needs instead of all of them. Grep it for the
question:

<!-- lookup:start · generated from WHERE-TO-LOOK.md by `just gen-guides` — edit the entry there, not this list -->
- Where does a furniture's footprint / visual size / approach side / dwell come from?
- Why doesn't a bigger buffer just render the office sharper?
- How is the office laid out?
- How does walk-pace physics work?
- Which side does an agent approach furniture from?
- How is the office rendered (pixel pass)?
- How do agent name-badge labels work (the `cc·1a2b` text above each character)?
- How do the room dividers render (frosted-glass partitions)?
- How does the theme system work?
- How does weather work?
- How does the sun/moon sky-light work?
- How does the meeting room come alive (sitting + group talk)?
- How does per-agent motion state work?
- What is the elastic wander timeline?
- How does the gateway mascot (the OpenClaw lobster) work?
- How does the lofi soundtrack get composed and rendered (and how do I add an instrument / a stem)?
<!-- lookup:end -->

## When refactoring

The render path is exercised by the headless harness (the binary's
`tui/tui_renderer/harness`, ~100 headless integration tests) plus dense `motion/tests.rs` +
`pose/tests.rs` unit suites in THIS crate with a real A\* router and overlay
churn. Changes to `derive_with_routing`, `MotionState`, or the pixel passes
should add or update a frame-by-frame continuity guard — the
flash/teleport/replay regressions in [`SHARP-EDGES.md`](SHARP-EDGES.md) all came
back as failing tests first. The crate must stay terminal- and window-free (invariant #1, now
COMPILER-enforced by the crate boundary + `just arch`): if you reach for
`ratatui`/`crossterm`/`winit`/`softbuffer`, you CAN'T add it to
`pixtuoid-scene/Cargo.toml` — the code belongs in the binary's painter (`tui/`
or `floating/`), not here.
