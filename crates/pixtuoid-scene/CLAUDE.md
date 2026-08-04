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
Worked example — the **home desk** (`DESK_APPROACH = {n:true, s:false,
e:true, w:true}`): approached from **North (far) + East + West**, NEVER
South — the monitor faces the viewer at the south front, so the seat opens
away from it. (We keep north=up even though a real sunny-window office in the
northern hemisphere would face its windows SOUTH — the compass is
screen-space, and flipping it would invert the entire z-sort/"south row"
vocabulary across 400+ sites for zero behavior change.)

## Layout

```
src/                (the pixtuoid-scene crate root; default pack at ../sprites/default/, embedded via its own build.rs)
├── anim.rs         centralized easing curves + eased_progress(start, duration_ms, easing, now) free function —
│                   used by floor slide, A* walk path ease, and version popup entrance/dismissal animations
├── audio/          the ambient-audio ENGINE (#633) — mod.rs is the backend-agnostic MODEL (below); the
│                   PURE synth stack MOVED here from the binary (web-audio) so native rodio AND wasm WebAudio
│                   build the SAME buffers: dsp.rs (radix-2 FFT + bands + spectral-envelope noise shaping +
│                   warp_resample + splitmix64 NoiseStream + centroid_hz), score.rs (the FROZEN day/night
│                   lofi const tables + checksums), synth.rs (the ratified per-voice recipes + fingerprint
│                   pins), mixer.rs (LoopStem + Mixer gain ramps + typing/drop schedulers + master_amp),
│                   engine.rs (THE shared per-tick authority `AudioEngine::tick(dt, frame: Option<AudioFrame>)
│                   -> TickCommands {gains, plays, swap}` — mixer + schedulers + pick + TrackSwitch behind ONE
│                   seam, so the native run_loop AND the wasm WebAudioDriver::tick are thin device/JSON shells
│                   over the SAME mixing/crossfade/scheduling, not two hand-synced copies; the BUILD stays
│                   caller-side [`swap` SIGNALS it, per the TrackSwitch sharp edge], dt is a clamped PARAMETER
│                   [MAX_DT_S] so both shells are gap-immune; OneShotPool + AssetBank::sample live in bank.rs). NO
│                   audio-device deps (pure math; the rodio/cpal ban still holds — `just arch`). All but score.rs
│                   are `#[doc(hidden)] pub` (workspace-internal, overlay/board pattern); score.rs is PRIVATE —
│                   every item in it is `pub(super)`, so a `pub mod` would only publish an empty path. The binary keeps only
│                   the DEVICE shell (sink/spawn/run_loop — the clock, mute/volume atomics, bed BUILD,
│                   sink forwarding). mod.rs MODEL — the sound twin of overlay/board: StemLevels
│                   (owner-ratified tier gains: empty/moderate/busy × pad/sparkle/keys/drums/texture/rain/typing;
│                   rain scales on pixel_painter::precipitation_level) + OneShot events + AudioCueTracker
│                   (cross-frame edge emitter: door chime capped 1/frame, printer/vending off the SAME
│                   occupancy edges as the #567 anims; the elevator-ding + audio-glug cues were
│                   owner-CUT in the dogfood round — the 2s VISUAL glug bubble stays, unvoiced).
│                   NO audio deps in this crate (the rodio/cpal ban is in `just arch`);
│                   the binary's audio/ gateway is the consumer, WebAudio can ride the same model later.
│                   Since Phase 2 (musical stems) every StemLevels lane is AUDIBLE — the binary
│                   synthesizes the frozen lofi compositions at startup and loops all six beds.
│                   ALL-GENERATIVE SOUNDTRACK (owner decision — all music is generated at
│                   runtime): TrackId {GenDay(seed), GenNight(seed)} + pure select_track(is_day,
│                   precip, track_epoch) ride AudioFrame — night hours (the SAME
│                   pixel_painter::hour_is_day sun window the lighting renders) or any rain pick
│                   the night MOOD; the compose seed is the audio::track_epoch block
│                   (TRACK_EPOCH_SECS = 600 — 10-min song cadence, owner-tuned for short
│                   agent sessions), so every block is a new song, deterministic on
│                   native/wasm/tests, and the block id change drives the engine's crossfade
│                   with no new state. TrackBeds::build composes + renders via gen_beds. The
│                   FROZEN takes (Day/Day2/Day3/Night tables + synth recipes) left the runtime
│                   and are #[cfg(test)] TEST ANCHORS: their fingerprint pins guard the shared
│                   cores the generator renders through — don't delete them as dead code.
│                   compose.rs — the theory-constrained COMPOSER (generative lofi): pure
│                   compose(mood, seed) -> GeneratedScore over a vetted progression grammar
│                   (8 day templates — 6 diatonic + 2 owner-adopted chromatic (V7/vi, borrowed
│                   iv) — + 4 night, ×transpose), melody rules (strong-beat chord
│                   tones / diatonic passing / bounded resolved leaps / two-phrase form with
│                   peak + loop-closing resolution), humanized groove templates; synth::gen_beds
│                   renders it through the SAME cores the frozen takes use (day_pad_core/
│                   night_pad_core/events_stem_core/drums_core/night_texture_core — the frozen
│                   fns are thin delegations, pins prove byte-fidelity). Quality gate is
│                   STATISTICAL: examples/lofi_audition renders N seeds for a blind owner
│                   batch (--solo <lane> isolates a stem); the seed-sweep property suite
│                   (compose/tests.rs) pins the rules. compose::LeadVoice is the
│                   INSTRUMENT registry (EpVel + Pluck): lanes are busy-ness roles,
│                   instruments are timbres within a lane — synth::lead_voice_fn is the
│                   one dispatch; the voice draw sits LAST in the seed stream so adding
│                   voices never redraws a blessed seed's notes. The add-an-instrument
│                   checklist lives in .claude/skills/procedural-lofi (the generator loop)
├── layout/             zone-based office geometry (terminal-agnostic; moved from pixtuoid-core —
│                       the engine owns its geometry; `Layout` = compat alias for SceneLayout;
│                       the WalkableMask VOCABULARY it stamps stays in core, coherence-bound to Grid):
│                       mod.rs (SceneLayout struct, Bounds, Point, Size, WallSegment, constants, accessors) + tests.rs sibling,
│                       compute.rs (compute_with_seed + private helpers: pod grid, pod decor, waypoints),
│                       rooms/ (#557: ONE seam per enclosed room — meeting.rs [MeetingRoom {bounds, trio:
│                         Option<MeetingTrio>}; Vec index == room_id, bare rooms keep their slot so the
│                         id can't shift] + pantry.rs [PantryRoom {bounds, counter_size, kitchen_island};
│                         COMPACT_COUNTER + pantry_counter_y_pct + content_fit_h, the inverse of the
│                         island y-clamps] + walls.rs [the wall's GEOMETRY home — owns BOTH (a) REQUEST-BASED
│                         segment derivation: rooms declare enclosure runs + doors, the resolver merges shared
│                         boundaries — the dense pair's wall renders ONCE and SOLID (no inter-meeting door,
│                         owner policy; each room keeps its own centered corridor door), trims V runs below H
│                         wall bodies, cuts DOOR_GAP gaps; AND (b) the linear-furniture geometry: WALL_THICK_H/V
│                         + WALL_TOP_OVERHANG_PX, WallDef, wall_segment_rect (blocked footprint via the shared
│                         ground_rect), stitch_vertical_wall (joints) — mask.rs only STAMPS these, the painter's
│                         wall.rs paints the SAME joints].
│                         The meeting/pantry split negotiates via MeetingRoom::trio_fit_h vs
│                         PantryRoom::content_fit_h — all-or-nothing donation, see compute.rs),
│                       decor.rs (role enums WaypointKind/PodDecor/PlantKind/WallDecor + Facing — carry NO
│                         dimensions, each .furniture()-maps to the unified Furniture geometry enum; the ONE
│                         table furniture_def(Furniture) + desk_furniture_def() — single source for EVERY
│                         point-footprint piece's FOOTPRINT [blocked ground] AND visual [sprite size],
│                         occupies_pos, dwell, ApproachSides. Includes the singleton/per-room bodies
│                         [MeetingSofaBody/MeetingTable/KitchenIsland/FloorLamp/LoungeSideTable].
│                         Only PLANT_FOOTPRINT [6×3, shared by the TWO 6px-pot plants: Ficus + Tall;
│                         Flower (2×2) and Succulent (3×2) carry their own shallow south-anchored pot
│                         strips] + DESK_APPROACH/desk_walk_anchor
│                         live alongside; walls use the linear WALL_THICK_H/V [owned by rooms::walls, not this table]),
│                       placement.rs (Anchor {Center,TopLeft} + anchored_top_left + z_sort_row — the ONE
│                         convention for WHERE a box sits relative to its `pos` [footprint origin = sprite
│                         origin] and its y-sort row; shared by mask.rs AND the tui renderer so the blocked
│                         ground, the blitted sprite, and the z-key can't drift. Anchor is passed per
│                         PLACEMENT SITE, not stored on FurnitureDef — Whiteboard is Center as pod decor but
│                         TopLeft as wall decor),
│                       mask.rs (build_walkable_mask — stamps each obstacle via stamp_ground: the footprint
│                         offset inside the visual box by the row's ground_x/ground_y GroundAlign, placed at
│                         anchored_top_left [no inline origin math]; stamps wall footprints via
│                         rooms::walls::wall_segment_rect + its own WALL_ROUTING_MARGIN_X routing pad; pantry south-strip +
│                         meeting-furniture-too-narrow gate are the documented exceptions; a TopLeft
│                         south-strip CENTERS the narrower ground footprint under the sprite's visual
│                         width — the wall-decor whiteboard's 10px wheel span sits at sprite cols 2-11,
│                         not hugged to the west edge),
│                       approach.rs (stand_point [obstacle render anchor] + approach_point [A*'s goal] +
│                         seated_foot_cell [the seat's render-anchor inverse] — the walkable cell an agent
│                         stands/approaches at, on the reachable allowed side nearest its desk, filtered by
│                         FurnitureDef.approach (ApproachSides) AND ReachSet),
│                       reach.rs (ReachSet — coarse-cell BFS over the WalkableMask mirroring the tui A* grid
│                         coarsening; reaches(p) ⇒ A* routable, so approach_point never targets a walled-off cell),
│                       placement_sweep.rs (#[cfg(test)] — the GENERATIVE placement-invariant harness: a
│                         sizes×seeds sweep asserting table-derived invariants [in-buffer, in-container,
│                         no-ground-overlap, no-wall-overlap, mask≡pieces parity (a stamped-nowhere
│                         ground = agents walk through the piece), pixel-BFS connectivity, the capacity law,
│                         every-kind-placed] over EVERY placed piece. Its `pieces()` destructures SceneLayout
│                         with NO `..` — a new furniture field fails compilation until wired in or exempted
│                         with a WHY; geometry comes from the SAME mask::ground_rect the collision mask
│                         stamps, never a second copy. Add a new furniture kind → register it there)
├── physics.rs          pure walk-pace physics (no terminal/router deps): WalkIntent, WalkProfile,
│                       walk_profile (trapezoidal/triangular kinematics), walk_progress (t_x1000),
│                       walk_arrived, speed_mult, pause_ms_for; constants: V_CRUISE_COMMUTE=0.36,
│                       V_CRUISE_SNAPBACK=0.65, V_CRUISE_WANDER=0.25, WALK_ACCEL=6.5e-4,
│                       WALK_ACCEL_SNAPBACK=2.0e-3, SPEED_MULT_MIN/MAX, PAUSE_MS_MIN/MAX
├── overlay.rs      backend-agnostic UI overlay MODEL: LabelElement{anchor_px,text,tone,hovered} + build_overlay
│                   (one name-badge per visible agent — text disambiguated/truncated, tone from activity, anchor
│                   from character_anchor); owns truncate_label/disambig_suffix. The tui + floating painters
│                   consume it identically (single source of truth so the two surfaces can't drift)
├── burn.rs         burn tier (model gate × effort split, USER-PINNED): TOP_MODELS prefix table
│                   (claude-fable/claude-mythos/gpt-5.6-sol — source-verified slugs) × MAX_EFFORTS
│                   ({ultra,ultrathink,xhigh,max}) → BurnTier{Normal,Premium,Top}; fresh_effort =
│                   the ONE EFFORT_TTL_SECS freshness rule (tier + dossier share it), and it ALSO drops
│                   CC's decoder-synthesized ultra_exit exit-sentinel so the internal token never
│                   reaches the dossier — the one source-specific special-case in this otherwise
│                   source-agnostic table (string = core-owned claude_code::ULTRA_EXIT_LABEL,
│                   referenced not re-hardcoded). Interpretation
│                   lives HERE; the RAW strings live on AgentSlot (core). Consumed by
│                   pixel_painter's paint_character_at (ember 'H' recolor + Top flame crown) and
│                   the binary's tooltip. Unknown model → Normal (fail-quiet, never flames).
│                   BACK-VIEW ember slab is DELIBERATE (user-ratified): walking_back/back_couch
│                   sprites are ~6 rows of 'H', so Premium+ reads as a solid red back — accepted
│                   (transient pose; spotting an expensive agent from behind is the point). Don't
│                   "fix" with per-row recolor.
├── board.rs        backend-agnostic NEON WALL-BOARD model + shared scene-stats — the sibling of overlay.rs for
│                   the wall panel (brand+★ / mood pulse / uptime·floor·gateway). Owns StateCounts + scene_stats/
│                   bucket_slot/per_floor_counts/gateway_rollup/compact_hms (relocated from the binary's tui
│                   widgets so floating + wasm share them), plus BoardTone/BoardSegment/BoardModel and
│                   build_board(counts, uptime_secs, floor, gateway) → the three model rows. Carries TONE not a
│                   resolved color (like overlay); tone_rgb(tone, theme) HERE is the ONE tone→theme-role map,
│                   each painter only converts the Rgb to its surface type: tui (wall_board.rs board_tone_color →
│                   ratatui), floating (offscreen.rs pack_xrgb → AA), wasm (lib.rs board_hex → DOM span). Width
│                   via chars().count() (mood glyphs ▲●○ are single-column). Panel interior geometry is the
│                   pixel_painter NEON_PANEL_INNER_{X,Y,W,H} consts (2,2,28,6).
│                   (font.rs — the old 8×8 bitmap font over the font8x8 crate — was DELETED with its dep:
│                   every text surface renders anti-aliased Monaspace Neon via the BINARY's `aa_text`,
│                   incl. the snapshot example's cell rasterizer; the "8×8 stand-in" look is fully retired)
├── footer.rs       backend-agnostic STATUS-FOOTER model — the sibling of board.rs/overlay.rs for the bottom
│                   status line. PURE build_footer(&FooterInputs, budget) → FooterModel{segments} owns the
│                   WHOLE tier ladder (death-tier preempt with the ▲N alarm pinned through truncation →
│                   full/medium/minimal width-fit → the ♩/floor/keys suffix), emitting toned FooterSegments
│                   already right-flushed to budget. The budget contract is TOTAL — every STATS path exits
│                   through fit_tiers/finish_tier (the keys-only rung degrades the TAIL: full hints →
│                   keys_alert whole → clip), and the death tier right-flushes by hand after clipping its own
│                   tail to the budget. Both are pinned to exactly `budget` columns at every width by
│                   every_footer_path_is_exactly_budget_wide, so a painter never has to clip. RungKind (the relocated binary StateKind vocab:
│                   glyph●◐○◌/letter/word/ALL/count — re-exported to the binary AS StateKind so tooltip/
│                   dashboard are byte-unchanged) + FooterTone{Neutral/Rung/Tool/Gateway/Warning}; carries
│                   TONE not a color (like board/overlay), footer_tone_rgb(tone, theme) HERE is the ONE
│                   tone→theme-role map both painters convert: tui (widgets/footer.rs to_color → ratatui),
│                   floating (offscreen.rs pack_xrgb → AA band). The one scene read (footer_tool_tally) is a
│                   free feeder so build_footer is pure. death `source_warning` arrives PRE-merged as
│                   Option<&str> (the binary's doctor::footer_warning owns the death>drift merge; scene never
│                   names the native-gated SourceDeath → stays wasm-clean). Width via chars().count() (whole
│                   vocabulary ·×↑↓●◐○◌⬢▲♩⚠… single-column — binary pin test).
├── pose/           pose derivation, pure-vs-routed split FILE-level (core/pose merged in here):
│                   mod.rs (the ROUTED authority: PoseHistory, derive_with_routing, snap-back; snapshots A*
│                   path length once at walk-start → freezes WalkProfile → drives t_x1000 per-frame via
│                   physics::walk_progress; the leg's A* polyline *shape* is also frozen once per leg via
│                   walk_path, re-snapshotted only when (from,to) changes and only for cornered routes
│                   >2 points, so per-frame overlay churn can't reroute an in-flight walker (no flash);
│                   octile_distance is pub(crate); re-exports the whole pure surface from `pure`),
│                   pure.rs (the STATELESS state→pose derivation, ex-core::pose — derive/derive_state_only/
│                   idle-wander knobs, a function of the snapshot inputs only: no routing, no per-frame
│                   history; pure/tests.rs sibling),
│                   tests.rs (#[cfg(test)] mod tests: unit + frame-by-frame continuity guards for the routed half)
├── motion/         per-agent walk-timing state, split production vs tests:
│                   mod.rs (MotionState: entry/exit/snap_back/wander/walk_path fields — exit and
│                   snap_back are WalkLeg{started_at,profile,from} structs (named fields, was a
│                   3-tuple carrying the from-Point); entry stays a 2-tuple (SystemTime, WalkProfile);
│                   walk_path = frozen per-leg A* polyline via WalkPathSnapshot; the wander
│                   timeline values are ONE WanderState struct (cycle_n/phase/phase_started_at/
│                   target/last_advanced_at — was nine flat wander_*/last_advanced_at fields; the trip
│                   dest+dest_kind+dest_wp_idx+seat are now one WanderTarget{dest, kind: WanderKind}
│                   where WanderKind = Named{wp_idx,kind,seat}|Aimless); WanderPhase enum Seated/WalkingOut(WalkProfile)/AtWaypoint(WalkProfile)/WalkingBack(WalkProfile) — the three walk variants CARRY their frozen profile (#574 folded the old `Option<WalkProfile>` field in, making "in a walk leg with no profile" unrepresentable);
│                   octile_path_len; walking_position (the pure per-segment walk lerp — pose history
│                   records with it, pixel_painter re-imports it: sim-side home so pose never imports
│                   from the render layer); advance_wander drives the elastic wander timeline, idempotent
│                   per now via wander.last_advanced_at; owned as HashMap<AgentId, MotionState> on FloorCtx.motion),
│                   tests.rs (#[cfg(test)] mod tests)
├── pathfind/       Router trait + AStarRouter with selective cache invalidation
├── floor/          FloorCtx (per-floor render state), render_floor (THE shared headless frame seam, #423 —
│                   returns Option<FloorFrame> {layout, occupied_waypoints}: the sim's occupancy rides out
│                   with the layout so a windowed painter can feed the audio cue tracker, #633),
│                   FloorSession (THE owned painter session: PerFloor {FloorCtx+RgbBuffer} + PerOffice
│                   {CoffeeState + chitchat + AudioObserver} — render() runs the dual eviction + render_floor
│                   and keeps last_layout AND last_occupied (cleared on an unlayoutable size); the
│                   appliance-cue occupancy/kind feed flows through FloorSession::audio_frame() (the audio
│                   twin of board()/overlay(), backed by the PRIVATE last_occupied+last_layout — the old
│                   public occupied_waypoints()/waypoint_kind() getters are GONE) into the shared
│                   AudioObserver: the office-wide cue tracker + floor-reprime latch that composes the
│                   per-frame AudioFrame. It runs EVERY frame; only DELIVERY is mute-gated, so re-enabling
│                   audio fires no cue volley for what arrived while muted (the kind lookup is the shared
│                   free fn floor::waypoint_kind_of). observe()
│                   is the headless sim tick, no pixels; a NEW painter starts here, not by hand-rolling
│                   the bundle), CoffeeState (per-office cup/steam bookkeeping), FloorTransition,
│                   LightingState, build_floor_scene (projects one floor's agents into ProjectedSlot
│                   pairs — the slot with its UNTOUCHED global desk_index + the floor-LOCAL desk typed
│                   FloorLocalDeskIndex; project_floor_scene performs the documented local→global
│                   re-host into the uniform single-floor scene, so single_floor_local identity reads
│                   stay honest; see its doc comment + core's GlobalDeskIndex/FloorLocalDeskIndex docs
│                   in state/mod.rs)
├── frame_cache.rs  FrameCache — per-agent recolored-sprite cache keyed (agent_id, anim, frame_idx, flip_x, glow_tint, burn — glow_tint is the theme-derived per-tool monitor-glow color, burn the `BurnTier` ember-hair gate, so each variant caches + self-invalidates separately);
│                   owned per-FloorCtx, flushed on theme change (set_theme) so recolors update immediately;
│                   per-agent entries also drop when the outfit seed changes (note_outfit_seed —
│                   a cwd backfill retints the outfit mid-life, the stale-outfit class)
├── theme/          color theme MODEL — one file per theme, Theme struct in mod.rs
│                   mod.rs (struct defs + ALL_THEMES registry), normal.rs, cyberpunk.rs,
│                   dracula.rs, tokyo_night.rs, catppuccin.rs, gruvbox.rs
│                   (the theme-PICKER UI — the [t] preview overlay — lives in tui/widgets/theme_picker.rs, not here)
├── pet.rs          PetKind (Cat, Dog) + per-kind static data; Pet{kind,name} (a configured office pet) + Pet::defaulted; select_pet_for_floor(u64,&[Pet])->Option<&Pet>; PetState (heart-anim interaction). The pet MODEL; its roaming BEHAVIOR lives in creatures.rs
├── creatures.rs    ambient wandering-creature BEHAVIOR (office pet + OpenClaw gateway mascot): pet_position / mascot_position (stateless — a pure fn of now + presence + seed) + the shared roaming toolkit (visit-spot geometry + the no-flash walk_between). SIM, NOT render — pixel_painter consumes these positions and PAINTS the creatures. Split out of pixel_painter/drawable.rs; ONE file until a 2nd pet/mascot pays for a per-entity split (the pet & mascot deliberately share the roaming toolkit)
├── chitchat.rs     venue-keyed group/solo speech bubbles (VenueKey::Room vs ::Waypoint)
├── token_meter.rs  token meter (#632) — burn.rs's sibling: RAW counters live on the slot
│                   (AgentSlot::{tokens_used, last_usage: Option<UsageObservation>}, core), ALL
│                   interpretation here. token_tier (×TIER_FACTOR=8 geometric ladder off
│                   TIER_BASE_TOKENS=250K → 250K/2M/16M, MAX_TIER=3 cap; two-population
│                   calibration in the module doc), sheet_fall_dist (the falling-sheet window:
│                   delta ≥ SHEET_MIN_DELTA_TOKENS=25K within SHEET_FALL_MS, integer ease-in —
│                   never `epoch_ms as f32`), compact_tokens (the dossier Σ format). The paint
│                   half is drawable.rs::paint_token_stack (paper tower on the desk's right
│                   wing, 2px/ream, T3 teeters 1px east; theme colors FurnitureColors::{paper,
│                   paper_shade} ×6 themes); tier-0 renders byte-identical to the pre-meter
│                   desk (default-on safety, test-pinned)
├── embedded_pack.rs  include_str! the default character pack at compile time (from this crate's own
│                   ../sprites/default/, watched by pixtuoid-scene's build.rs for rerun-if-changed) →
│                   sprite::format::load_pack_from_strings; --pack-dir merges OPTIONAL_FURNITURE over it
├── cutaway/        the ENRICHED orthographic cutaway PROFILE — the sibling renderer, not a
│                   fidelity knob on the classic one. The brief models two renderers over ONE
│                   shared scene frame, and that frame already exists: `pixel_painter::sim.rs`'s
│                   `sim_step` returns an owned immutable `SimFrame`, so this module becomes its
│                   SECOND reader and no new seam is invented. shade.rs = the vocabulary the
│                   visual mock ratified BEFORE any of it was written: `Ramp {lit, base, shade}`
│                   (one key light from the north windows, every material carries one — uniformity
│                   is why the room reads lit from one direction) + `slab` (top-lit mass; a 1-row
│                   mass is all `base`, since either edge tone would make a 1px detail read as a
│                   highlight/shadow instead of the material) + `dither_band` (4x4 Bayer). The
│                   dither is NOT a workaround: an indexed palette cannot blend, so a gradient IS
│                   a dither — and it costs TWO palette slots instead of a dozen, which is what
│                   keeps theme recolour an index swap and SIXEL free of a quantisation pass.
│                   paint.rs = `render_cutaway(&SimFrame, layout, pack, theme, scale, buf)` —
│                   floor + desks + cast, DELIBERATELY partial (walls/rooms/effects change no
│                   answer yet). Two decisions worth not re-deriving: (1) a desk sorts on its TOP
│                   SURFACE's south edge, a deliberate DIVERGENCE from classic (which sorts on the
│                   desk's visual base so the monitor hides the occupant) — the reference draws the
│                   head OVER the surface, because in a cutaway the occupant sits at the near side;
│                   (2) the desk BLITS the pack's `desk` sprite at classic's exact anchor
│                   (`desk.y - 1`, its monitor-bezel raise) and derives the cutaway's front face by
│                   SAMPLING the sprite's own base row. The desk's brown lives in the PACK
│                   (`"D" = #8b5a2b`), NOT the theme — `furniture.wood_top` is a different material
│                   that reads nearly identical to the carpet in tokyo-night — and sampling means a
│                   custom `--pack-dir` desk gets a matching front face for free.
│                   (3) a desk-seated pose is RE-PROJECTED: `CharacterPlacement.seat_desk`
│                   carries the desk the sim seated an agent at, because `anchor` is already
│                   projected FOR CLASSIC (it raises the sprite so the monitor overhangs and hides
│                   the lower body) and a second profile cannot recover the desk from it. Classic
│                   ignores the field; the cutaway anchors at the desk's own row so the head reads
│                   OVER the surface. That field is what "one simulation, two projections" has to
│                   mean in practice — without it the shared frame silently belongs to one painter.
│                   (4) MIXED DENSITY: `densest_art(pack, name, scale)` prefers a `<name>@<N>x`
│                   pack variant (see the core guide) over block-scaling the base — picking the
│                   densest one that DIVIDES the render scale and blitting it at the remainder,
│                   so 4x art still HALVES the upscale at 8x instead of being discarded for not
│                   matching exactly. Its direction is the point: richer art REMOVES the upscale
│                   rather than fighting it, so the asset work lands one piece at a time instead
│                   of as a flag day, and a pack with no variants renders byte-identically. The
│                   size check here is only the render-time BACKSTOP for an unvalidated pack —
│                   `validate_pack_animations` is where an author is meant to find out. The one
│                   arithmetic trap: `paint_desk` sizes the front face off `art.width() *
│                   blit_at`, NOT `art.width() * scale` — variant art is already in buffer
│                   units, and multiplying twice puts the front face a whole desk below the
│                   surface it belongs to (pinned by
│                   `the_drawn_size_is_the_same_whichever_density_the_art_came_from`).
│                   The bundled `desk@4x` also records two ART decisions made against the
│                   RENDERED OFFICE rather than the sprite: grain runs ALONG the boards (random
│                   dots read as dirt) and screen chrome is the monitor-frame grey, NOT a bright
│                   tone — the cutaway says "occupied" with the `lit` glow it paints OVER the
│                   screen, so bright baked content made all 12 desks read as staffed.
│                   Visual check: `cargo run --release --example cutaway_snapshot`
├── render_scale.rs THE layout-space ↔ buffer-space seam. Every layout coordinate is a buffer
│                   pixel today, so the office's SIZE and its RESOLUTION are ONE axis — doubling
│                   the buffer builds a room with 4× the desks rather than drawing the same room
│                   sharper (measured: 25 desks at 192×160, 1554 at 1536×1280). `RenderScale`
│                   splits them: layout keeps computing in LOGICAL units (capacity, desk
│                   assignment and the walkable mask untouched), the painter multiplies on its
│                   way to pixels. `RenderScale::ONE` is the classic path, byte-identical to the
│                   pre-seam behaviour. `floor::floor_capacity_scaled` is the seam-aware twin of
│                   `floor_capacity` — deliberately a SECOND fn, not a defaulted param: every
│                   existing caller means "buffer pixels ARE layout units" and must keep meaning
│                   it, so a painter adopting a scale opts in at its own call site. `FrameInputs`
│                   carries `scale` as a COMPILE-FORCED field (the `Target.post_install_hint`
│                   pattern): `size` alone is ambiguous once the spaces differ, so every painter
│                   states which it means. `render_floor` is where they part — the buffer sizes
│                   in PIXELS, the layout computes at `scale.logical(size)`. Density is the core
│                   blitter's `blit_frame_scaled` for art authored BELOW the render scale, and
│                   the pack's `<piece>@<N>x` variants for art authored AT it (`cutaway::paint::
│                   densest_art` is the picker); see the core guide
└── pixel_painter/  the SHARED world render (render_to_rgb_buffer) — TWO phases behind that one seam:
                    sim.rs (sim_step: advances every &mut sim store — router/overlay/history/motion/
                    light/chitchat, bundled as SimStores — and returns an OWNED immutable SimFrame:
                    poses, seated map, CharacterPlacements, indoor_scale, chitchat bubbles, coffee
                    carriers, occupied waypoints (the appliance busy-feedback observation). THE headless observation seam: a FloorSession / native-app consumer can
                    advance + observe the world with no pixel buffer anywhere. Takes pack — anchors
                    center on the pack's character width — but NOT theme: placements carry a
                    theme-free CharacterGlow the paint pass resolves to a color), then the paint
                    pass (paint_frame over &SimFrame; its only &muts are the pixel buffer + the
                    paint-local FrameCache — a render cache, deliberately not a sim store — so
                    painting can NOT move the world; pinned by paint_frame_is_pure_and_byte_identical
                    + sim_step_advances_motion_without_painting),
                    mod.rs (PixelCtx struct — the painters' construction surface; it now BORROWS the
                    per-floor `FloorCtx` as one `store` field (was seven flat fields router/overlay/history/
                    cache/motion/light + door_anim_max_ms), read as disjoint `store.router`/… projections;
                    `buf` stays a SEPARATE field (disjoint sibling on a PerFloor) — orchestrator + the
                    private PaintCtx), background/ (weather, sunset, skyline,
                    sky.rs, lighting.rs, celestial.rs [sun/moon disc + night stars, #469]),
                    ambient.rs (sun spot + dust motes + ceiling halos),
                    drawable.rs (y-sort Drawable enum + paint dispatch via `DrawableCtx` — the
                                  param BUNDLE {buf, pack, cache, now, theme, scale}: this was six
                                  positional params and the render scale would have made seven, the
                                  growth PixelCtx/PaintCtx already answered the same way. `blit_centered`
                                  is THE density-aware centring seam every centred sprite rides — it
                                  centres in LOGICAL space then converts ONCE (centring in buffer space
                                  halves the already-scaled width and drifts odd-width art off the
                                  footprint its mask stamped — a drift NO scale-1 test can see, pinned by
                                  `a_centred_blit_scales_both_its_position_and_its_art`); creature BEHAVIOR moved to crate::creatures — only the Pet/GatewayMascot PAINT arms + paint_mascot_bubbles stay here), effects.rs (glow/z's/steam/dust/bubble),
                    palette.rs (agent palette + recolor + tool_glow_tint), anchors.rs (breath, character_anchor —
                    pub(crate); walk position re-imported from motion::walking_position;
                    the per-pose anchor fns take `sprite_w` — the pack's character width,
                    resolved ONCE per frame [8 bundled / 10 robot] — so a non-8-wide pack centers correctly),
                    furniture.rs (meeting table, area rug + entry/bar mats, side table,
                                  kitchen island, fish tank, meeting chair, corridor
                                  appliances: vending machine/printer/coat rack),
                    wall.rs (the wall's RENDER half — frosted-glass room dividers: paint_glass_wall_h/v +
                             paint_door_jamb_h/v + enqueue_room_walls_h/v; the GEOMETRY half — thickness
                             consts, WallDef/wall_segment_rect, stitch_vertical_wall — lives in layout::rooms::walls),
                    seat.rs (SeatView orientation single-source + seat_sprite + settle_seat_view + paint_character_at),
                    debug_overlay.rs (#[cfg(debug_assertions)] mask/approach/route overlay — the `w` toggle),
                    tests.rs (sibling unit suite, extracted from mod.rs)
```

> Furniture drawables y-sort via `layout::z_sort_row` (the south base row, tied to the mask's
> `anchored_top_left` so the sprite and its blocked ground can't drift); `center_pin_south_offset` remains
> only as the offset primitive for shadow/halo placement. See `layout::placement`.

> **`floor::render_floor` is the shared headless frame seam (#423), and
> `floor::FloorSession` is the owned painter session over it.** One
> compiler-owned "scene → RgbBuffer" frame: prologue (buffer sizing, layout,
> router zone), the pixel pass (itself two-phase: `sim_step` → paint, see
> `pixel_painter/sim.rs` above), and the bookkeeping epilogue (`CoffeeState`
> carrier persistence + the door-anim clamp). It also single-sources the
> label-overlay (`overlay`) and wall-board (`board`) derivations: `render`
> captures the frame's `Layout` internally so `overlay` builds against the SAME
> geometry the sprite pass used — a mismatched layout/route pair is
> unrepresentable, not merely discouraged, and the web + floating painters drop
> their duplicated `last_layout` capture. The persistent bundle every
> painter used to hand-roll — {FloorCtx, RgbBuffer, CoffeeState, chitchat} +
> the dual `evict_missing` protocol — is now the `FloorSession` type
> (`PerFloor` + `PerOffice` halves): each painter drifted on exactly that
> convention once (the floating window never evicted — a slow leak; the web
> hero shipped without eviction — the loop-2 teleport), so `render()` runs the
> dual eviction itself and a painter can no longer forget it. Its scene MUST
> be the FULL live scene — eviction against a PROJECTED scene would wipe every
> other floor's state, which is why `render_floor` itself still never evicts
> (the caller-side rule; the session IS the caller for single-floor painters).
> Consumers: the desktop window (`floating::offscreen::OfficeRenderer`) and
> the web hero (`pixtuoid-web::Office`) each own one `FloorSession`; the TUI
> composes the halves (`Vec<PerFloor>` + one `PerOffice` — coffee/chitchat are
> office-wide so a cup survives floor navigation) and stays on `render_floor`
> directly for the floor-slide (`TuiRenderer::render_transition`, which
> renders projected scenes). `FloorSession::observe` is the headless twin —
> eviction + prologue + `sim_step` + epilogue with NO pixel buffer (the
> sim/paint split's observation seam; `SimFrame`/`CharacterPlacement`/
> `CharacterGlow` are pub for its callers, `sim_step`/`SimStores` stay
> crate-internal). The ONE deliberate raw-`render_to_rgb_buffer` consumer is
> `tui::renderer::draw_scene` (the terminal half-block flush): it needs the
> full `PixelPassResult` (pet/mascot positions, chitchat bubbles) and holds
> only immutable coffee borrows mid-flush, routing its bookkeeping through the
> pub `frame_epilogue` seam (the same `CoffeeState`/door-anim step the session
> runs — no longer hand-copied, closing the #423 drift class). A FOURTH painter starts
> from `FloorSession`, not by re-rolling the bundle. The terminal flush +
> widgets + footer live in the binary's `tui/`; the chunky-upscale blit is the
> binary's `floating/`; the RGBA blit to canvas is `pixtuoid-web`'s. Keep
> `pixtuoid-scene` flush-free.

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

- **Agent OUTFIT (shirt+pants) is keyed on the normalized `cwd`, not `agent_id`** (Team Palette): same working directory → same outfit, so the office reads as a color-coded org-chart. Hair/skin stay `agent_id`-seeded for individual distinctness; `unknown_cwd`/empty-cwd falls back to the `agent_id` seed. The old WARM/COOL personality split for outfit selection was intentionally dropped (it was an `agent_id` artifact) — the outfit now spans the full 16-preset pool. The `examples/snapshot` fixture deliberately assigns VARIED cwds so the gallery shows grouping; don't collapse it back to one cwd.
- **`recolor_frame` substitutes by RGB equality.** Works because each recolor key maps to a unique RGB. The recolor key set is `pixtuoid_core::sprite::format::RECOLOR_KEYS` (`B/H/S/P`) — the SINGLE source of truth `recolor_frame` (`pixel_painter/palette.rs`) iterates AND `validate_recolor_palette` guards, so the substitution and the guard can't drift (add a 5th recolor key there, once). **The uniqueness invariant is ENFORCED at pack load** (`validate_recolor_palette` in `sprite/format.rs::build_pack` bails on a collision) for the embedded AND `--pack-dir` custom packs — it is no longer just "documented, be careful." If you genuinely need two recolor keys to share a color, swap to a palette-key-indexed approach instead. (Core validates / scene consumes — the dep direction is kept.)
- **EXIT walks are time-compressed to fit the GC window; entry/wander/snap-back are not.** Walk duration is normally pure physics (`distance ÷ speed`), but an exiting slot races a removal deadline: the reducer GCs it after `EXIT_GRACE_WINDOW = 4500 ms`, so when the exit's physics duration exceeds the window `derive_with_routing` scales `elapsed` so `walk_progress` reaches 1000 by the window edge — without this the sprite would **vanish mid-corridor** when the deadline fires (a real regression, fixed with a test). Don't delete the exit compression as "redundant." **Snap-back is NO LONGER compressed** (it used to be, to fit a 900 ms window): it now runs by **pure physics** with a brisk SnapBack profile (`physics::V_CRUISE_SNAPBACK` faster cruise + `WALK_ACCEL_SNAPBACK` ≈ 3× accel — an "urgent rush back"), and `SNAP_BACK_MS = 900 ms` is now just the **ARM window** (only fire a snap-back for a recent flip), NOT a render cap — so a far snap-back renders to completion as a real ≈ 1.3 s walk instead of a hard-compressed 900 ms dash. Entry has no cap (nothing GCs an entering agent) and must stay uncompressed so far desks genuinely take longer. **A CANCELLED walkout re-enters through the door, and only once it ARRIVED.** `exiting_at` is revocable (the reducer's `resurrect_in_place`) but `created_at` is not re-stamped, so the entry branch's spawn-window gate can never re-arm on its own; once the walkout has arrived the sprite has stopped rendering, so `PoseHistory` holds nothing recent for the snap-back either — both overrides declined and the agent popped onto its chair with no walk at all. `derive_with_routing` therefore takes the spent leg (`MotionState::exit`) on the non-exiting path and re-arms entry from `now`. An IN-FLIGHT walkout re-arms entry too, but **from the sprite's live position** (`PoseHistory::recent`), not the door — starting at the door would jump the walker backwards. That is why `MotionState::entry` is a `WalkLeg` carrying a `from` like its two siblings, rather than the bare `(SystemTime, WalkProfile)` it was. **The snap-back is NOT the mechanism for either half and never was** — `resurrect_in_place` leaves the slot `Idle`, so the wander dispatch returns (usually `SeatedThinking`, since the SessionStart just refreshed `last_event_at`) long before the snap-back is consulted, which is `Active`/`Waiting`-only. What makes the entry leg the right seam is ORDER: the ENTRY branch runs BEFORE that dispatch, so an armed leg is what actually renders. An earlier revision of this entry claimed the snap-back covered the in-flight case; it measured as a 67–126 px teleport. The `take()` is load-bearing on its own: a retained arrived leg would be replayed by the NEXT exit and the sprite would vanish on its first frame. `exit_elapsed_ms` is shared by the exit render and this check so an already-arrived leg can't read as in-flight.
- **A walk leg's A\* polyline shape is frozen once per leg, not re-routed per frame.** `route_walking_pose` snapshots the route into `MotionState.walk_path` keyed on `(from, to)` and reuses it until the endpoints change. This is NOT redundant with `AStarRouter`'s own cache: the router cache is *invalidated* by per-frame occupancy-overlay churn (another agent toggling a waypoint obstacle), and without the freeze a mid-leg re-route remaps the frozen-progress `t` onto a differently-shaped polyline → the sprite **jumps** (the "flash") and the frame pays a fresh A\* cost (the periodic stutter). Only cornered routes (>2 points) are frozen; straight 2-point walks re-route each frame (cheap, and self-healing if A\* transiently fell back to a straight `[from,to]`). The accepted trade-off: a frozen walker won't dodge an agent that steps into its path mid-leg (rare, cosmetic, legs are seconds). Don't "simplify" the `walk_path` check away as duplicate caching. The occupancy overlay is still built from the *stateless* pure `pose::derive` (for cache-signature stability) — that intentional divergence from the scene motion timeline is what made the freeze necessary.
- **A meeting room narrower than `MEETING_FURNITURE_MIN_W` (compute.rs) has NO sofa/table/seats — bare floor, BY DESIGN.** Below it the 16px sofa body leaves too little margin for the coarse 4×4 router to reach the seats, so an idle agent sent to sit would TELEPORT (find_path None → straight-line fallback). The room Bounds/walls/door still exist; only the unroutable furniture is dropped (same degradation the dense floor uses when too short). So "meeting room exists" no longer implies "meeting slots exist" — don't "fix" the missing sofa at small sizes. Guarded by `pathfind::tests::every_wander_waypoint_is_routable_on_the_coarse_grid` (coarse-grid reachability across seeds × sizes — stronger than the pixel-BFS connectivity sweep).
- **Occlusion is EMERGENT — there is no `occludes_behind` field / synthetic cap any more (deleted).** An overhanging piece (plant canopy, booth column, TV monitor, whiteboard panel, pantry counter) has its footprint **south-anchored to the sprite base** by its DECLARED `ground_y: GroundAlign::End` field (the general top-down collision-box model: the blocked rect is a `footprint`-sized rect offset inside the `visual` box by `ground_x`/`ground_y`, each a `GroundAlign::{Start,Center,End}` resolving to a pixel offset from `visual − footprint` at stamp time — drift-free). ONE `mask::stamp_ground` formula covers south strips (`End` — the walk-behind shape: the tall sprite overhangs the shallow strip so a walker parks behind it, occluded by z-sort. The DESK is this: a shallow `DESK_FOOT_H` (2px) footprint pinned to the sprite base by `ground_y: End`, so the monitor + surface overhang NORTH and an agent approaching from the top row walks behind the monitor, occluded by the desk's own y-sort — the same emergent occlusion as the plant canopy. This shallow footprint is also what RELAXED `INTER_POD_AISLE_Y`'s floor from 20 to 16: the old full-body-`Start` desk blocked the whole north approach zone), centered pieces (`Center` — sofa body + floor lamp; note `Center` is center-ON-pos `v/2 − f/2`, NOT center-in-box `(v−f)/2`: they differ by 1px at opposite parity, which the floor lamp exposed and the walkable golden caught) — so the old `visual.h > footprint.h` per-site inference and its three bypass sites are gone. (`Start` = top-anchored footprint is currently UNUSED — no piece has its ground contact at the sprite TOP — but it's kept as the third align for completeness; the desk was `Start` until it went walk-behind. No `Inset(n)` interior-band variant: three aligns cover every row; add one only if a real piece ever needs a footprint pinned mid-box — none does.) A walker then parks DEEP behind the overhang and the piece's own y-sorted sprite paints over their lower body. The old `paint_furniture_back_floor` floor-snapshot cap is gone. **Two intentionally decoupled footprints:** the MASK uses the shallow `footprint` (occlusion + collision); `approach_point` derives the USER's park distance from the full `visual` (`approach_clearance_extent` — the SINGLE extent source, which `stand_point` delegates through) so someone *using* the furniture parks clear of the whole sprite, not inside it. This is the industry Y-sort pattern (base-collision + sprite-occludes); **don't re-add a cap** — make the sprite overhang its shallow footprint instead. Pinned by `pixtuoid_scene::pixel_painter::tests::every_pod_occludes_via_overhang` (in the `pixtuoid-scene` engine crate, not core).
- **Pantry counter blocks only a shallow `PANTRY_FOOTPRINT_DEPTH` south strip, not its full sprite height.** The counter is a ¾-view sprite centered on `pos`; only its south base contacts the floor (the receding cabinet tops overhang, invariant #6). `mask.rs` south-anchors a 3px strip; a character behind it is occluded by the counter's own y-sorted sprite, couch-style — the same emergent occlusion as every overhanging piece (above). `stand_point` parks the USER clear of the full `visual`, not this shallow strip (the runtime-sized pantry was the original mask-vs-approach decouple, now generalised to all overhanging obstacles).
- **splitmix64 is open-coded FOUR times in this crate on purpose** (`weather_state` in `background/sky.rs`, `strike_offset` in `background/mod.rs`, `dust_mote_positions` in `ambient.rs`, `cwd_outfit_seed` in `pixel_painter/palette.rs` — each the canonical 3-stage finalizer, shifts 30/27/31) beside the canonical `core::id::splitmix64` (`#[doc(hidden)] pub` since the physics/pose personality slicers moved into this crate and finalize through it cross-crate), plus a SEPARATE family of simpler **constant-as-multiplier position/twinkle mixes** — `ambient.rs`, `pose::is_aimless_cycle`, the skyline `city_dot_lit`/`city_dot_twinkle`, and the night-star `star_exists`/`star_twinkle` in `background/celestial.rs` (a single-shift or lone golden-ratio multiply — NOT splitmix64's 3-stage finalizer; don't miscount them as splitmix64 clones). DELIBERATE either way: each is an independent noise source over a disjoint input domain — no two sites need equal output, so no parity test exists or is wanted; the real gate is the gen-check pixel diff on the rendered output. Don't unify them into one shared fn.
- **`paint_glass_wall_h` and `paint_glass_wall_v` (`pixel_painter/wall.rs`) stay SEPARATE — don't hoist a `paint_glass_strip(axis, alphas)` helper.** They share a 5-branch tone/alpha ladder (mullion > seam > first-edge > last-edge > mid) but diverge at every load-bearing point: the H strip has a north back-cap PLUS the face (`rows = GLASS_CAP_PX + WALL_THICK_H_PX`, origin `cap_top`), the V strip is edge-on with no cap (`WALL_THICK_V_PX`, origin `x_left`); the seam/mullion run along DIFFERENT axes (x vs y) with the edge on the OTHER axis; and the alphas are deliberately tuned per orientation (face-on H seam .55 / edge .82 vs edge-on V .60 / .85 — a wall SHOWING its face reads at a different opacity than one seen edge-on). A unifying `paint_glass_strip` would take an along-range + a cross-range + a coordinate-swap closure + a 5-alpha set — an interface nearly as wide as the two-loop body it hides (a textbook SHALLOW module / mirror-blit-param anti-pattern), for a shared piece that is only the branch SKELETON, not the numbers. Adjudicated against in the architecture-deepening sweep-3 (the sibling of the splitmix64 + `Frame::mirror` "don't unify the mirror pair" calls); revisit only if a THIRD wall orientation ever appears.
- **`epoch_ms(now)` is the ABSOLUTE wall-clock ms (~1.7e12) — reduce it BEFORE any `as f32`.** An f32 has 24 mantissa bits, so at that magnitude its ULP is ~131 s: `epoch_ms(now) as f32` rounds every frame within ~2 min to one value, freezing whatever animation reads it (the neon-pulse + dust-mote freeze, arc #3). The idiom for a time-driven animation is to reduce to a bounded phase with an INTEGER op FIRST — `elapsed_ms % CYCLE_MS`, `now_ms / cycle_ms`, `phase_ms % PERIOD` — and only then cast; or, when the smooth continuous value is wanted (`neon_pulse`, dust drift), do the arithmetic in **f64** and cast only the final position/brightness to f32. The relative `anim::elapsed_ms(now, since)` sites are safe by construction (small values). A pinning test MUST exercise a WALL-CLOCK-scale `now` (~1.7e12 ms), not a small test epoch (`UNIX_EPOCH + 5s`), or it never sees the freeze.
- **The sun/moon disc keeps a "real low window":** it is only visible at LOW altitude (dawn/dusk) and clips above the glass near its arc apex (absent at solar midday / lunar midnight — see `HORIZON_FRAC`/`ARC_RISE_FRAC` in `background/celestial.rs`); it's gated to the ONE window its centre is currently over, so it hides behind the inter-window wall pillar between panes and behind the right-side elevator door at the dusk extreme; and thick cloud (Overcast/Rain/Storm) hides it uniformly below `MIN_DISC_VIS`, regardless of altitude. Don't "fix" any of these as a bug — they're the intended day/night + weather gating.
- **Day-over-night light invariant:** the moon casts no direct beam (diffuse-fill only — both `beam_strength` and `time_of_day_look`'s `direct_eff` zero it for `Body::Moon`), and `MOON_PEAK_LUM` + the weather-keyed `city_bounce` night floor are calibrated so a solar noon of ANY weather stays brighter than the brightest moonlit night — guarded by `solar_noon_outshines_the_brightest_night` (`background/sky.rs`). Don't recalibrate one side (moon luminance, `city_bounce`, or a weather's `atmo` channels) without re-running that test. **That guard asserts on `time_of_day_look().darkness` = the INTERIOR illuminance, and is structurally blind to anything painted on the GLASS afterwards** — the weather veils land after the sky pass (see the next entry), which is how a foggy midnight pane once rendered 1.8× brighter than a stormy noon one while `darkness` stayed correct. Any future sky/weather pin asserts on the RENDERED buffer: `no_weather_flattens_the_glass_day_night_contrast` / `no_night_pane_outshines_the_clear_solar_noon_pane` (`background/tests.rs`) are the pane-side twins, swept over `ALL_THEMES` × every weather × every NIGHT hour (`hour_is_day` is the boundary, so the sweep can't drift from a hand-written hour list). **A pane pin states a CONTRAST floor, not a bare `night < noon` ordering** — the broken veils left every weather's night pane just barely under its own noon pane (0.865..0.956 of it, worst cyberpunk Fog at 01:00), so an ordering-only assertion passed on the defective painter; `MAX_NIGHT_PANE_FRACTION` is what actually reds. Midnight is deliberately NOT the sampled instant either: the pre-dawn twilight tint renders brighter than the moon's apex hour on every theme.
- **The weather VEILS are lit by the emitter, and their cross-weather ordering is NOT an invariant.** `skyline_haze` (behind the glass) and the Fog/Overcast/Smog `wash_glass` arms scale with `veil_lum` — `NIGHT_VEIL_FLOOR + (1 - floor) × sky.emitter_lum` — so a white-out fog is white at noon and a dim city-glow murk at midnight. The day term is the emitter's OWN luminance and deliberately NOT `atmo`/`darkness`: those already carry the weather (so does the veil colour), and folding them in would darken a stormy noon veil twice. The `NIGHT_VEIL_FLOOR` is the city-light scatter that keeps fog reading as fog after dark — don't scale the veil to zero at night (`fog_still_glows_over_the_midnight_sky` is the counter-pin), and don't "fix" fog rendering BRIGHTER than clear at the same hour: a lit white-out genuinely out-shines a blue sky, and at night low cloud genuinely glows. The only ordering the repo asserts on the pane is per-weather day-over-night (as a CONTRAST floor — see the previous entry) plus "no NIGHT-hour pane out-shines the CLEAR solar-noon pane"; how bright a snowy/stormy noon pane renders is a theme-palette choice (cyberpunk's day sky is deliberately dark), so a "heavier weather is never brighter" pin would be asserting taste, not physics.
- **An EXCLUSIVE spot is single-occupancy, enforced where the destination is CHOSEN — not where it's drawn.** `waypoint_index_for_cycle(id, cycle_n, n)` is occupancy-BLIND (a pure hash of agent × cycle), so N agents could target one chair. `pose::SpotClaims` + `motion::spot_claims` are the exclusion: an agent out on a trip to an `exclusive` waypoint holds it, and `resolve_wander_target` linear-probes forward to the next unclaimed waypoint. A multi-seat venue's seats are CONTIGUOUS in `layout.waypoints` (`compute_waypoints` pushes a sofa's 3 / a table's 2 in one run), so the probe lands on a neighbouring seat of the SAME venue first — "that chair's taken, I'll take the next one" — and only leaves the room once the venue is full. A SINGLE-slot exclusive spot (phone booth, standing desk) has no sibling to probe into — it's a size-1 venue, instantly full — so a contested one just defers the second agent to its next hashed waypoint (an unrelated spot), which is correct: one booth, one caller. With no claims the probe resolves to `waypoints[first]` byte-identically to the pre-claim behaviour. **The gate is `furniture_def(kind.furniture()).exclusive`, never a hand-listed kind set** — that's what makes a FUTURE exclusive kind inherit the behaviour the day it sets the flag, with no second list to sync (`no_exclusive_waypoint_kind_ever_steps_aside` ranges `WaypointKind::ALL` to force it). **`exclusive` is a SEPARATE field from `occupies_pos` on purpose (the wrong-abstraction split):** `occupies_pos` is a render/approach fact (sprite ON `pos`, has a `seated_foot_cell`); `exclusive` is a CAPACITY fact. They coincide for seats but DIVERGE for the phone booth + standing desk — stand-beside singles that render at a side cell yet hold exactly one occupant (`occupies_pos: false`, `exclusive: true`). The invariant `occupies_pos ⇒ exclusive` (a seat is never shareable) is pinned by `furniture_def_invariants_hold_for_every_row`. Queue spots (pantry counter / vending / printer / snack shelf) are `exclusive: false` — they share and step aside. Two gates are load-bearing: (1) the claim needs `phase != Seated`, because the bootstrap / stale-resume path re-seats an agent at its desk WITHOUT clearing `wander.target` — the PHASE, not the kind, is the honest "actually out at the spot" signal; (2) `derive_with_routing` RELEASES the claim on the non-wander path, because `advance_wander` stops running once a slot goes Active and its target would otherwise freeze mid-trip, holding a spot it snapped away from for the whole active burst (an unbounded leak — the ≤`THINKING_WINDOW_SECS` hold on the SeatedThinking path is bounded, and the exiting agent keeps its spot for the ≤`EXIT_GRACE_WINDOW` walkout, both deliberate). Consequently `waypoint_rank_offset_x` returns 0 for every exclusive spot: its ±6/±9 offsets are the pre-per-seat-waypoint FOSSIL (one `Couch` waypoint once spread 3 sitters, hence ±6 == `SEAT_DX`), and ±9 coincidentally equals `MEETING_CHAIR_TABLE_DX` (two unrelated 9s) — it parked the second sitter ON the meeting table, the "two agents on one chair" report. The offset survives ONLY for the shareable queue spots. Don't re-add a render-time seat de-collision; an exact overlap is the honest render of a claim bug, a plausible-looking sideways slide is not.
- **Two narrow-band connectivity guards keep the office ONE region (#566), both graceful DECOR degradation — not bugs.** (1) The lounge couch's east seat can seal the elevator `door_threshold`'s own column when the cubicle band splits to EXACTLY 30 px wide; `compute_with_seed` gates the lounge OFF unless the couch's derived east ground (`couch_x + max SEAT_DX + Couch half-footprint + WAYPOINT_STAMP_PAD_PX`) is at-or-west of `door_threshold.x` (`couch_clears_door` — the east-side twin of `LOUNGE_MIN_BAND_W`), so the couch degrades away at `band.width == 30` (the `couch_sprite_center: None` case). This is why `door` is computed ABOVE the lounge gate now. (2) A scatter plant `settle_plant` relocated onto an obstacle's AISLE row can plug the SOLE inter-pod drain at a single-pod-column band (buf 59-60 × tall), sealing the appliance strip off from the door: after the mask is built, a flood-fill from `door_threshold` (`unreachable_walkable_cells`) finds any pocket and drops the aisle-resident scatter plants (`plant_ground_in_bounds`), rebuilding once (last resort: every scatter plant; a `debug_assert` fires if a pocket somehow survives — a non-plant seal cause). The discrete `SWEEP_SIZES` grid structurally SKIPPED these widths, so `narrow_band_connectivity_boundary_scan` (a step-1 sweep of the `NARROW_BAND` 32-76 widths) now pins both. Don't "restore" the couch/plant at these sizes. The connectivity PREDICATE is now the ROUTER's, not a pixel flood's: `unreachable_walkable_cells` is 4-connected at PIXEL granularity, but A\* runs on the coarse 4x4 grid (`cell_walkable` needs >= `COARSE_CELL_WALKABLE_MIN` of a cell's 16 px open), so a <=3px channel is pixel-connected and coarse-IMPASSABLE — the office read as ONE region at pixel granularity and TWO at router granularity (measured: 286 of 13,770 narrow layouts, widths 32-39 x heights 116-148 and 192-220, production floors 4 and 6, up to 3 of 6 desks whose `desk_approach_cell` returned the sentinel and whose every leg straight-lined through the pantry wall). `compute_with_seed`'s `severed` predicate therefore asks for BOTH: no pixel pocket AND every emitted home desk still holding a reachable `approach_point`. The DEGRADATION is unchanged — the same three decor rungs clear every measured case, so no desk is ever dropped and capacity is untouched. Pinned by ONE predicate, `placement_sweep::assert_home_desk_approaches_are_routable` (the coarse twin of `assert_walkable_connected`), run at three resolutions because each catches a different slice: `every_home_desk_approach_is_routable_from_the_door` sweeps `SWEEP_SIZES` × `SWEEP_SEEDS` AND × the PRODUCTION `floor_seed(0..MAX_FLOORS)` (disjoint but for seed 0 — the measured floors 4/6 live only on the latter), and `narrow_band_connectivity_boundary_scan` re-runs it at the step-1 width resolution its pixel twin already had (with the coarse half disabled, severed desks appear at widths {32,33,34,35,37,38,39} and only 34/38 are in `SWEEP_SIZES`). `every_wander_destination_is_routable_from_its_desk` is NOT a pin for this class — measured green with the coarse half disabled, both when it routed from the door and after it was moved onto the production leg origin; it pins its own class (a waypoint approach `ReachSet` filter) and reds when that filter is removed. **The spawn point itself is pinned SEPARATELY, and not by any routing assertion**: `door_threshold.y = top_margin + DOOR_THRESHOLD_CLEARANCE_PX` must land at-or-south of the floor line, never on the `wall_band_h()..top_margin` carpet apron the straddling wall decor stamps into (`placement_sweep::the_spawn_threshold_stands_on_the_floor_not_the_wall_apron`, both seed axes). Routing can't see this: move the spawn 8 px north and it stays `is_walkable` at 264/264 swept layouts (the wall band's blocked rows END at `wall_band_h()`), while `find_path` and `ReachSet::from_mask` both SNAP a displaced seed back into the component — so every connectivity/routability guard above stays green on a spawn that has walked into the wall decor's strip.
- **Every wander destination is filtered by `ReachSet`, but the ±4px `jitter_dest` perturbation is applied AFTER that filter — a small residual.** `pick_aimless_dest` now requires `is_walkable(p) && reachable.reaches(p)` (the same conjunct `approach_point` applies to every NAMED destination), because a pixel-walkable cell whose coarse 4x4 cell fails `cell_walkable` makes `find_path` return `None` and `route()` fall back to the straight `[from,to]` line — the agent glides through furniture for the whole out-leg AND back-leg. But `route_jittered` routes to `jitter_dest(id, to)`, so a reachable goal can still be nudged up to 4px into an unroutable pocket. That is ACCEPTED: the jitter is a lockstep contract (the rendered shape, the measured profile length and the router-cache key all read the SAME jittered goal — see `route_jittered`), so a dest-side re-filter there would need all three to move together. Don't 'fix' it by clamping inside `jitter_dest` alone.

## Where to look

Answers live in [`WHERE-TO-LOOK.md`](WHERE-TO-LOOK.md), so a session
pays for the entry it needs instead of all of them. Grep it for the
question:

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

## When refactoring

The render path is exercised by the headless harness (the binary's
`tui/tui_renderer/harness`, ~100 headless integration tests) plus dense `motion/tests.rs` +
`pose/tests.rs` unit suites in THIS crate with a real A\* router and overlay
churn. Changes to `derive_with_routing`, `MotionState`, or the pixel passes
should add or update a frame-by-frame continuity guard — the
flash/teleport/replay regressions documented above all came back as failing
tests first. The crate must stay terminal- and window-free (invariant #1, now
COMPILER-enforced by the crate boundary + `just arch`): if you reach for
`ratatui`/`crossterm`/`winit`/`softbuffer`, you CAN'T add it to
`pixtuoid-scene/Cargo.toml` — the code belongs in the binary's painter (`tui/`
or `floating/`), not here.
