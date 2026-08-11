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

Annotated tree in [`LAYOUT.md`](LAYOUT.md) — grep it for a filename.

<!-- layout:start · generated from LAYOUT.md by `just gen-guides` — edit the tree there, not this skeleton -->
```
src/                (the pixtuoid-scene crate root; default pack at …
├── anim.rs         centralized easing curves + eased_progress(start …
├── audio/          the ambient-audio ENGINE (#633) — mod.rs is the …
├── layout/             zone-based office geometry (terminal-agnostic; moved from …
├── physics.rs          pure walk-pace physics (no terminal/router deps) …
├── overlay.rs      backend-agnostic UI overlay MODEL …
├── burn.rs         burn tier (model gate × effort split, USER-PINNED) …
├── board.rs        backend-agnostic NEON WALL-BOARD model + shared …
├── footer.rs       backend-agnostic STATUS-FOOTER model — the sibling of …
├── pose/           pose derivation, pure-vs-routed split FILE-level …
├── localclock.rs   TEST-ONLY local wall-clock instants: at_hour / at_hour_min …
├── motion/         per-agent walk-timing state, split production vs tests:
├── pathfind/       Router trait + AStarRouter with selective cache …
├── floor/          FloorCtx (per-floor render state), render_floor (THE …
├── frame_cache.rs  FrameCache — per-agent recolored-sprite cache keyed …
├── theme/          color theme MODEL — one file per theme, Theme struct in …
├── pet.rs          PetKind (Cat, Dog) + per-kind static data; Pet{kind,name} …
├── creatures.rs    ambient wandering-creature BEHAVIOR (office pet + OpenClaw …
├── chitchat.rs     venue-keyed group/solo speech bubbles (VenueKey::Room vs …
├── token_meter.rs  token meter (#632) — burn.rs's sibling: RAW counters live …
├── embedded_pack.rs  include_str! the default character pack at compile time …
├── cutaway/        the ENRICHED orthographic cutaway PROFILE — the sibling …
├── render_scale.rs THE layout-space ↔ buffer-space seam. Every layout …
└── pixel_painter/  the SHARED world render (render_to_rgb_buffer) — TWO …
```

- **Furniture drawables y-sort via `layout::z_sort_row`.** (the south base row, tied to the mask's `anchored_top_left` so the sprite and its blocked ground …
- **`floor::render_floor` is the shared headless frame seam (#423), and `floor::FloorSession` is the owned painter session over it.** One compiler-owned "scene → RgbBuffer" frame: prologue (buffer sizing, layout, router zone), the …
<!-- layout:end -->

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
- **`recolor_frame` substitutes by RGB equality.** Works because each recolor key maps to a unique RGB. The recolor key set is …
- **EXIT walks are time-compressed to fit the GC window; entry/wander/snap-back are not.** Walk duration is normally pure physics (`distance ÷ speed`), but an exiting slot races a removal …
- **A walk leg's A\* polyline shape is frozen once per leg, not re-routed per frame.** `route_walking_pose` snapshots the route into `MotionState.walk_path` keyed on `(from, to)` and …
- **A meeting room narrower than `MEETING_FURNITURE_MIN_W` (compute.rs) has NO sofa/table/seats — bare floor, BY DESIGN.** Below it the 16px sofa body leaves too little margin for the coarse 4×4 router to reach the …
- **Occlusion is EMERGENT — there is no `occludes_behind` field / synthetic cap any more (deleted).** An overhanging piece (plant canopy, booth column, TV monitor, whiteboard panel, pantry counter) …
- **Pantry counter blocks only a shallow `PANTRY_FOOTPRINT_DEPTH` south strip, not its full sprite height.** The counter is a ¾-view sprite centered on `pos`; only its south base contacts the floor (the …
- **splitmix64 is open-coded FOUR times in this crate on purpose** (`weather_state` in `background/sky.rs`, `strike_offset` in `background/mod.rs` …
- **`paint_glass_wall_h` and `paint_glass_wall_v` (`pixel_painter/wall.rs`) stay SEPARATE — don't hoist a `paint_glass_strip(axis, alphas)` helper.** They share a 5-branch tone/alpha ladder (mullion > seam > first-edge > last-edge > mid) but …
- **`epoch_ms(now)` is the ABSOLUTE wall-clock ms (~1.7e12) — reduce it BEFORE any `as f32`.** An f32 has 24 mantissa bits, so at that magnitude its ULP is ~131 s: `epoch_ms(now) as f32` …
- **The sun/moon disc keeps a "real low window":** it is only visible at LOW altitude (dawn/dusk) and clips above the glass near its arc apex …
- **Day-over-night light invariant:** the moon casts no direct beam (diffuse-fill only — both `beam_strength` and `time_of_day_look`'s …
- **The weather VEILS are lit by the emitter, and their cross-weather ordering is NOT an invariant.** `skyline_haze` (behind the glass) and the Fog/Overcast/Smog `wash_glass` arms scale with …
- **An EXCLUSIVE spot is single-occupancy, enforced where the destination is CHOSEN — not where it's drawn.** `waypoint_index_for_cycle(id, cycle_n, n)` is occupancy-BLIND (a pure hash of agent × cycle), so …
- **The free-standing whiteboard OVERLAPS a north-facing desk's seat in the committed stills, and that is an ACCEPTED look, not a bug to fix.** Since the board snaps to an inter-pod aisle and this branch made that aisle every north pod …
- **`desk_facings` is index-parallel to `home_desks` and stays that way — the consolidation into one `Vec<Desk>` was weighed and declined.** Both are `pub` on a published crate, and nothing in the type system pins their lengths, so a …
- **A back-turned desk whose south front has no reachable approach is DEMOTED to viewer-facing, and that rung never fires in any swept layout.** `compute` re-probes every `Facing::North` desk after the walkable mask exists and flips it South …
- **Two narrow-band connectivity guards keep the office ONE region (#566), both graceful DECOR degradation — not bugs.** (1) The lounge couch's east seat can seal the elevator `door_threshold`'s own column when the …
- **The free-standing whiteboard stands ONLY in an inter-pod aisle, and is ABSENT rather than relocated when the band holds a single pod row.** Its `usable_h / 3` anchor is a HINT, not a slot: it knows nothing of the desk grid, so unsnapped …
- **A desk chair's z key TIES with its occupant's and the order is decided by INSERTION, not by the key.** `enqueue_desk_chairs` runs after `enqueue_characters` and `sort_by_key` is stable, which is the …
- **Every wander destination is filtered by `ReachSet`, but the ±4px `jitter_dest` perturbation is applied AFTER that filter — a small residual.** `pick_aimless_dest` now requires `is_walkable(p) && reachable.reaches(p)` (the same conjunct …
- **The GENERATED night pad renders NO sub-bass — lane 5 (the BASS stem) owns that register; only the frozen v4 night-pad anchor still bakes its sub into the pad.** `night_pad_core` takes `sub: Option<(&[u8; 4], f32)>`: the `#[cfg(test)]` anchor passes …
- **Every ARTIFICIAL FLOOR light scales by `indoor_scale`; the self-lit WALL fixtures deliberately do not.** An emptied floor reads dark because its lights go out, not because the floor takes a second …
- **The hour's cast on objects runs as TWO `wash_since` passes over a snapshot diff, and foreground EMITTERS are deliberately inside the second one.** `paint_frame` washes twice: once around the floor fixtures (corridor runner, mats, water cooler …
- **The north wall band is SOLID for its whole width — the elevator doorway is a hole in the WALL, not in the ground — and a character sprite overlapping that wall is the intended look, not a bug.** `build_walkable_mask` blocks `0..wall_band_h()` across `buf_w` and takes no `door` input at all …
- **Layout variety is spent out of budgets that already exist — a scattered plant can only stand where a DISPLACED one already could.** `settle_plant`'s ladder was always "slide inward 1 px at a time up to `MAX_PLANT_NUDGE_PX`, else …
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
- How does the neon wall board work?
- How do pets work?
- How does the neon wall-board work (the `pixtuoid v… ★ Star / mood / uptime` panel)?
- How does the status footer work (the `n/total · ●A ◐W · Bash×2 · ⬢gw   ♩ [q]uit` bottom row)?
- How does the coffee run work?
- How do atmosphere / ambient effects work?
- How does the theme system work?
- How does weather work?
- How does the sun/moon sky-light work?
- Where do the lounge aquarium / soft-goods mats live?
- How does the meeting room come alive (sitting + group talk)?
- How does the thinking pose work?
- How do the corridor appliances work?
- How do the phone booth and standing desk work?
- How does per-agent motion state work?
- What is the elastic wander timeline?
- How do multi-floor offices work (the per-floor engine state)?
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
