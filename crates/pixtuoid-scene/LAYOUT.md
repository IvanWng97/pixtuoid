# pixtuoid-scene — annotated layout

The navigable skeleton is in [`CLAUDE.md`](CLAUDE.md); this is the same tree with each entry's full annotation.

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
│                   (owner-ratified tier gains: empty/moderate/busy × pad/sparkle/keys/drums/texture/bass/rain/typing;
│                   rain scales on pixel_painter::precipitation_level) + OneShot events + AudioCueTracker
│                   (cross-frame edge emitter: door chime capped 1/frame, printer/vending off the SAME
│                   occupancy edges as the #567 anims; the elevator-ding + audio-glug cues were
│                   owner-CUT in the dogfood round — the 2s VISUAL glug bubble stays, unvoiced).
│                   NO audio deps in this crate (the rodio/cpal ban is in `just arch`);
│                   the binary's audio/ gateway is the consumer, WebAudio can ride the same model later.
│                   Since Phase 2 (musical stems) every StemLevels lane is AUDIBLE — the binary
│                   synthesizes the frozen lofi compositions at startup and loops all seven beds.
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
│                   (10 day templates — 6 diatonic + 4 chromatic (V7/vi, borrowed iv, two A7
│                   jazz turnarounds) — + 8 night, ×transpose), melody rules (strong-beat chord
│                   tones / diatonic passing / bounded resolved leaps / two-phrase form with
│                   peak + loop-closing resolution), humanized groove templates; synth::gen_beds
│                   renders it through the SAME cores the frozen takes use (day_pad_core/
│                   night_pad_core/events_stem_core/drums_core/night_texture_core — the frozen
│                   fns are thin delegations, pins prove byte-fidelity). Quality gate is
│                   STATISTICAL: examples/lofi_audition renders N seeds for a blind owner
│                   batch (--solo <lane> isolates a stem); the seed-sweep property suite
│                   (compose/tests.rs) pins the rules. compose::LeadVoice is the
│                   LEAD-instrument registry (EpVel + Pluck + Kalimba + Vibraphone, mood-pooled):
│                   lanes are busy-ness roles,
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
│                         strips] + DESK_APPROACH/desk_walk_anchor_facing
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
│                   fidelity knob on the classic one, and NOT converging on it. It is being
│                   built toward an OWNER-SUPPLIED REFERENCE RENDER (a dark room of workers seen
│                   from behind); classic is the shipping look, this is the target look, and they
│                   are not the same picture. So parity with classic is owed ONLY where the
│                   SIMULATION speaks — which desk an agent sits at, which side of it, where they
│                   walk — because that is one world observed twice. Everything else (materials,
│                   light, desk art, density) is this profile's own, judged against the reference,
│                   and "make it match classic" is the WRONG repair for a difference there.
│                   It is deliberately NOT user-reachable yet: `render_cutaway`'s only caller is
│                   `pixtuoid/examples/cutaway_snapshot`, `run` paints classic unconditionally, and
│                   `--graphics` is a `doctor`-only capability report (see the binary guide). It
│                   stays that way until the reference look is actually reached — a half-built
│                   profile behind a flag is a worse release than no profile.
│                   The brief models two renderers over ONE
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
│                   paint.rs = `render_cutaway(frame, layout, pack, theme, scale, now, cache, buf)`
│                   -> `Vec<CutawayLabel>` (badge anchors; the engine cannot draw text, so the
│                   painter renders them). Floor + wall band + rooms + desks + furniture + cast;
│                   DELIBERATELY partial in EFFECTS only (weather/glow/steam/pet stay classic's).
│                   order.rs = the draw ORDER: a dependency graph over `Span::behind` + a
│                   topological sort, NOT a sort key. With today's predicate the two are
│                   EQUIVALENT (the relation is derived from a total order on base rows, so it
│                   cannot cycle) — the graph earns its place as `check_order`, which turns every
│                   pairwise geometric fact into an assertion, and as the shape ELEVATION will
│                   need. The one thing it cannot fix: a LONG object has no meaningful base row,
│                   so wall runs are SPLIT into `WALL_SEG_H` segments (no predicate substitutes
│                   for splitting). Decisions worth not re-deriving: (1) every piece's sort row
│                   comes from `layout::anchored_top_left` — the SAME anchoring the walkable mask
│                   and the classic painter use — so the box a piece sorts by cannot drift from
│                   the box it blits into; the desk needs NO divergence for the occupant to read
│                   over its surface, that falls out once both are measured at their true base
│                   rows; (2) the desk BLITS the pack's `desk` sprite at classic's exact anchor
│                   (`desk.y - 1`, its monitor-bezel raise) and derives the cutaway's front face by
│                   SAMPLING the sprite's own base row. The desk's brown lives in the PACK
│                   (`"D" = #8b5a2b`), NOT the theme — `furniture.wood_top` is a different material
│                   that reads nearly identical to the carpet in tokyo-night — and sampling means a
│                   custom `--pack-dir` desk gets a matching front face for free.
│                   (3) a desk-seated pose is NOT re-projected. `CharacterPlacement.seat_desk`
│                   carries the desk the sim seated an agent at, because `anchor` is already
│                   projected and a second profile cannot recover the desk from it — but it is the
│                   CHAIR and the suppressed contact shadow that read it, never the seat side.
│                   Which side of their desk someone sits on is a SIM fact (`layout.desk_facings`,
│                   through `seated_anchor_facing`), so both profiles get it from the same place.
│                   `cutaway_seat_anchor` used to override it onto the desk's own row for EVERY
│                   occupant, which was right while the sim seated everyone far-side and this
│                   profile wanted them near; once a pod's two rows started facing opposite ways
│                   the override began contradicting the sim, and it was DELETED rather than made
│                   facing-aware (the row it hardcoded is exactly what the shared anchor yields for
│                   a back-turned desk). Note the direction of that repair: it removed a
│                   divergence about WHERE SOMEONE IS, which is the only class parity is owed on.
│                   Diverging on how the desk is DRAWN is expected and fine — classic ships
│                   `desk_north.sprite` (raised monitor) and this profile has no equivalent yet,
│                   which is a to-do against the REFERENCE, not a parity bug.
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
│                   VARIANT ART IS EMBEDDED FOR EVERY TARGET, INCLUDING WASM, WHERE NOTHING
│                   CAN SELECT IT — `pixtuoid-web` paints at `RenderScale::ONE` by design (the
│                   chunky look), and only `densest_art` reads variants. Budgeting the art
│                   phase: `gen-wasm-check` gates the GZIP size, because raw and served bytes
│                   disagree by more than 2x on this payload and sprite text compresses far
│                   better than the wasm code around it — a variant that looks expensive on
│                   disk is cheap over the wire.
│                   Run the recipe for the live raw/gzip figures rather than trusting one
│                   written here; the cap and its reasoning are in the justfile above it. What
│                   the CDN actually transfers is a SEPARATE check the recipe cannot make —
│                   `curl -sIL -H 'Accept-Encoding: gzip, br, zstd'` answered gzip as of
│                   2026-08, with brotli an open GitHub feature request, not a published
│                   contract; if it ever lands, this gate becomes a ~20% overestimate.
│                   Also: `pack.toml` declares the variant, so dropping the `include_str!`
│                   alone makes `build_pack` fail on a missing frame — the loader would have to
│                   skip variant animations too.
│                   The bundled `desk@4x` also records two ART decisions made against the
│                   RENDERED OFFICE rather than the sprite: grain runs ALONG the boards (random
│                   dots read as dirt) and screen chrome is the monitor-frame grey, NOT a bright
│                   tone — the cutaway says "occupied" with the `lit` glow it paints OVER the
│                   screen, so bright baked content made all 12 desks read as staffed.
│                   Visual check: `cargo run --release --example cutaway_snapshot`
├── render_scale.rs THE layout-space ↔ buffer-space seam. Every layout coordinate is a buffer
│                   pixel today, so the office's SIZE and its RESOLUTION are ONE axis — doubling
│                   the buffer builds a room with 4× the desks rather than drawing the same room
│                   sharper (measured: 25 desks at 192×160, 1722 at 1536×1280). `RenderScale`
│                   splits them: layout keeps computing in LOGICAL units (capacity, desk
│                   assignment and the walkable mask untouched), the painter multiplies on its
│                   way to pixels. `RenderScale::ONE` is the classic path, byte-identical to the
│                   pre-seam behaviour. `floor::floor_capacity_scaled` is the seam-aware twin of
│                   `floor_capacity` — deliberately a SECOND fn, not a defaulted param: every
│                   existing caller means "buffer pixels ARE layout units" and must keep meaning
│                   it, so a painter adopting a scale opts in at its own call site. The scale is an
│                   explicit PARAMETER of the one entry point that has one (`render_cutaway`), not
│                   a field on `FrameInputs`/`PixelCtx`/`DrawableCtx`: those describe the classic
│                   pass, where buffer pixels ARE layout units, and a field they all default to ONE
│                   is a question every painter answers identically. `render_floor` is where they part — the buffer sizes
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
                                  param BUNDLE {buf, pack, cache, now, theme}: this was five
                                  positional params, the growth PixelCtx/PaintCtx already answered
                                  the same way. `blit_centered`
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

**Furniture drawables y-sort via `layout::z_sort_row`.** (the south base row, tied to the mask's `anchored_top_left` so the sprite and its blocked ground can't drift); `center_pin_south_offset` remains only as the offset primitive for shadow/halo placement. See `layout::placement`.

**`floor::render_floor` is the shared headless frame seam (#423), and `floor::FloorSession` is the owned painter session over it.** One compiler-owned "scene → RgbBuffer" frame: prologue (buffer sizing, layout, router zone), the pixel pass (itself two-phase: `sim_step` → paint, see `pixel_painter/sim.rs` above), and the bookkeeping epilogue (`CoffeeState` carrier persistence + the door-anim clamp). It also single-sources the label-overlay (`overlay`) and wall-board (`board`) derivations: `render` captures the frame's `Layout` internally so `overlay` builds against the SAME geometry the sprite pass used — a mismatched layout/route pair is unrepresentable, not merely discouraged, and the web + floating painters drop their duplicated `last_layout` capture. The persistent bundle every painter used to hand-roll — {FloorCtx, RgbBuffer, CoffeeState, chitchat} + the dual `evict_missing` protocol — is now the `FloorSession` type (`PerFloor` + `PerOffice` halves): each painter drifted on exactly that convention once (the floating window never evicted — a slow leak; the web hero shipped without eviction — the loop-2 teleport), so `render()` runs the dual eviction itself and a painter can no longer forget it. Its scene MUST be the FULL live scene — eviction against a PROJECTED scene would wipe every other floor's state, which is why `render_floor` itself still never evicts (the caller-side rule; the session IS the caller for single-floor painters). Consumers: the desktop window (`floating::offscreen::OfficeRenderer`) and the web hero (`pixtuoid-web::Office`) each own one `FloorSession`; the TUI composes the halves (`Vec<PerFloor>` + one `PerOffice` — coffee/chitchat are office-wide so a cup survives floor navigation) and stays on `render_floor` directly for the floor-slide (`TuiRenderer::render_transition`, which renders projected scenes). `FloorSession::observe` is the headless twin — eviction + prologue + `sim_step` + epilogue with NO pixel buffer (the sim/paint split's observation seam; `SimFrame`/`CharacterPlacement`/ `CharacterGlow` are pub for its callers, `sim_step`/`SimStores` stay crate-internal). The ONE deliberate raw-`render_to_rgb_buffer` consumer is `tui::renderer::draw_scene` (the terminal half-block flush): it needs the full `PixelPassResult` (pet/mascot positions, chitchat bubbles) and holds only immutable coffee borrows mid-flush, routing its bookkeeping through the pub `frame_epilogue` seam (the same `CoffeeState`/door-anim step the session runs — no longer hand-copied, closing the #423 drift class). A FOURTH painter starts from `FloorSession`, not by re-rolling the bundle. The terminal flush + widgets + footer live in the binary's `tui/`; the chunky-upscale blit is the binary's `floating/`; the RGBA blit to canvas is `pixtuoid-web`'s. Keep `pixtuoid-scene` flush-free.
