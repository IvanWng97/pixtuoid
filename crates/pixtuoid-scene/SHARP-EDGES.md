# pixtuoid-scene — known sharp edges

Indexed one line each in [`CLAUDE.md`](CLAUDE.md). These look like bugs and are deliberate design — read the entry before "fixing" one: the edge, the WHY, one authority pointer (pinning test / in-code comment / issue). Adjudication history lives in the cited issue/PR, not here.

- **Agent OUTFIT (shirt+pants) is keyed on the normalized `cwd`, not `agent_id`** (Team Palette): same working directory → same outfit; hair/skin stay `agent_id`-seeded. The `examples/snapshot` fixture assigns VARIED cwds so the gallery shows grouping — don't collapse it to one cwd.

- **`recolor_frame` substitutes by RGB equality** — sound because each recolor key maps to a unique RGB, enforced at pack load (`validate_recolor_palette`) over the one key set `RECOLOR_KEYS`. If two keys ever need to share a color, switch to palette-key indexing.

- **EXIT walks are time-compressed to fit the GC window; entry/wander/snap-back are not.** An exiting slot races `EXIT_GRACE_WINDOW` (4.5s), so `derive_with_routing` scales `elapsed` — delete the compression and the sprite vanishes mid-corridor. A CANCELLED walkout re-enters via the door once ARRIVED and from its live position IN-FLIGHT — the entry branch re-arms the leg itself. The `take()` of the spent exit leg is load-bearing: a retained leg replays on the NEXT exit.

- **A walk leg's A\* polyline shape is frozen once per leg, not re-routed per frame.** A mid-leg re-route remaps frozen progress onto a different polyline — teleport + fresh A\* per frame. Only cornered routes (>2 points) freeze; straight walks re-route each frame. Accepted: a frozen walker won't dodge a mid-leg obstruction. Don't "simplify" the `walk_path` check away as duplicate caching.

- **A meeting room narrower than `MEETING_FURNITURE_MIN_W` has NO sofa/table/seats — bare floor, BY DESIGN.** Below it the coarse router can't reach the seats (a seated trip would teleport). Guarded by `every_wander_waypoint_is_routable_on_the_coarse_grid`.

- **Occlusion is EMERGENT — there is no `occludes_behind` field / synthetic cap (deleted).** A piece's blocked rect is its `footprint` offset inside the `visual` box via ONE formula (`mask::ground_rect`); an overhanging sprite occludes via its own y-sort. **Don't re-add a cap** — make the sprite overhang its footprint. Pinned by `every_pod_occludes_via_overhang`.

- **splitmix64 is open-coded FOUR times in this crate on purpose** (sky weather, lightning, dust motes, outfit seed) — each an independent noise source over a disjoint domain; no two sites need equal output. The real gate is the gen-check pixel diff. Don't unify them.

- **`paint_glass_wall_h` and `paint_glass_wall_v` stay SEPARATE — don't hoist a `paint_glass_strip(axis, alphas)` helper.** They share only the tone-ladder SKELETON and diverge at every load-bearing point; the unifier's interface would be as wide as the body it hides. Revisit only if a THIRD wall orientation appears.

- **`epoch_ms(now)` is the ABSOLUTE wall-clock ms (~1.7e12) — reduce it BEFORE any `as f32`.** At that magnitude an f32 ULP is ~131s: a direct cast freezes any animation reading it. Reduce with an INTEGER op first (`% CYCLE_MS`) or stay in f64. A pinning test MUST use a wall-clock-scale `now`, or it never sees the freeze.

- **The sun/moon disc keeps a "real low window":** low-altitude only, clipped near apex, gated to the ONE window its centre is over, hidden under thick cloud. All intended gating (`background/celestial.rs`).

- **Day-over-night light invariant:** the moon casts no direct beam; `solar_noon_outshines_the_brightest_night` guards the calibration — re-run it before retuning moon luminance, `city_bounce`, or an `atmo`. It reads `darkness` and is BLIND to pane-side painting, so pane pins assert a CONTRAST FLOOR on the RENDERED buffer (`MAX_NIGHT_PANE_FRACTION`), not a bare ordering. See `background/tests.rs`.

- **The weather VEILS are lit by the emitter, and cross-weather brightness ordering is NOT an invariant.** Veils scale with `veil_lum` (folding in `atmo`/`darkness` would darken a stormy noon twice). Lit fog genuinely out-shines clear sky (`fog_still_glows_over_the_midnight_sky` is the counter-pin); "heavier weather is never brighter" would assert taste, not physics.

- **An EXCLUSIVE spot is single-occupancy, enforced where the destination is CHOSEN — not where it's drawn.** `SpotClaims` + a forward probe; the gate is `furniture_def(..).exclusive`, never a hand-listed kind set (`no_exclusive_waypoint_kind_ever_steps_aside`). Load-bearing: the claim needs `phase != Seated`, and `derive_with_routing` RELEASES on the non-wander path (else an Active burst leaks the spot). Don't re-add render-time seat de-collision: an exact overlap is the honest render of a claim bug.

- **The free-standing whiteboard OVERLAPS a north-facing desk's seat in the committed stills — an ACCEPTED look, not a bug.** The board's aisle band IS the seat band; the ground strip clears the seat cell; the owner accepted the crop. Don't nudge the board's x without asking.

- **`desk_facings` is index-parallel to `home_desks` and stays that way — consolidation into one `Vec<Desk>` was weighed and declined.** Both are `pub` on a published crate (a second breaking change later); reads funnel through `desk_facing`, both vectors come from ONE `compute_pod_desks` call. Guards: the `debug_assert_eq!` in `desk_facing` + `every_desk_has_a_facing`. If ever consolidated, do it while the minor is unreleased.

- **A back-turned desk with no reachable south approach is DEMOTED to viewer-facing — and that rung never fires in any swept layout.** A net for a future placer; a pod whose rows face the same way is a `pod_row_facing` bug, not this. Deleting it is still wrong: `assert_home_desk_approaches_are_routable` panics on a stranded desk rather than silently seating one off its routed side.

- **Two narrow-band connectivity guards keep the office ONE region (#566), both graceful DECOR degradation — not bugs.** The couch can seal the door column; a relocated plant can plug the sole drain. **A per-rung mutation won't red** — the rungs are a redundancy LADDER on one predicate; only removing the WHOLE branch reds the tests (the negative control to use). The predicate is the ROUTER's, not a pixel flood's; the spawn point is pinned separately (`the_spawn_threshold_stands_on_the_floor_not_the_wall_apron`).

- **Both layout floors are DERIVED per axis and deliberately UNIFORM across variants** — `MIN_LAYOUT_W`/`_H` solve the band formulas against the WIDEST variant column: one number the painter can say out loud, where a per-variant gate would flip office↔notice as the user changes floors. Pinned by `every_floor_variant_seats_a_desk_at_the_minimum_layout_size`.

- **The size gate cannot be replaced by a search over the real placer — below it `compute_with_seed` is out of contract, and RELEASE fails silently.** Below the floor a `debug_assert!` fires and a subtraction underflows — DEBUG-only; release instead returns a desk-less layout, sometimes with a sealed pocket. Failing silently is worse than crashing, so `min_layout_size()` derives from the placer's own band formulas, pinned by `min_band_w_is_the_placers_own_first_desk_boundary`.

- **The free-standing whiteboard stands ONLY in an inter-pod aisle, and is ABSENT rather than relocated when the band holds a single pod row.** Unsnapped, its anchor sealed the west lane (`free_standing_whiteboard_survives_the_west_aisle_it_used_to_seal`); the bands are deliberately NOT "every strip no pod occupies" — north margin and south remainder are spoken for.

- **A desk chair's z key TIES with its occupant's and the order is decided by INSERTION, not by the key.** `enqueue_desk_chairs` runs after `enqueue_characters`, sort stable — reorder or use an unstable sort and the chair paints UNDER the sitter with every test green. The `const _` beside `DESK_WALK_Y_OFF_BACK` guards the one edit that could clamp a sprite off its chair.

- **Every wander destination is filtered by `ReachSet`, but the ±4px `jitter_dest` perturbation applies AFTER that filter — an accepted residual.** The jitter is a lockstep contract (rendered shape, profile length, router-cache key read the same goal); a re-filter must move all three together. Don't clamp inside `jitter_dest` alone.

- **The GENERATED night pad renders NO sub-bass — the BASS stem owns that register; only the frozen v4 anchor bakes its sub in.** Re-adding one doubles the low end and pushes the chords out of headroom; deleting the anchor's sub invalidates the v4 pins. Related: `gen_bed`'s `_ =>` arm ships a wired lane SILENT (add the lane to `generated_beds_are_finite_phase_locked_and_in_the_sound_world` in the same change); `LoopStem::Rain` stays LAST in `ALL` (wasm handoff indexes rain at `TRACK_STEMS.len()`).

- **Every ARTIFICIAL FLOOR light scales by `indoor_scale`; the self-lit WALL fixtures deliberately do not.** An emptied floor reads dark because its lights go out — the old `EMPTY_FLOOR_DIM_BOOST` was deleted for faking that. A FIFTH floor emitter added without the factor is invisible to the suite — extend the set at `desk_light` and pin it in `an_emptied_floor_takes_both_desk_emitters_down_with_the_level`.

- **The hour's cast runs as TWO `wash_since` passes over a snapshot diff, and foreground EMITTERS are deliberately inside the second one.** The grouping axis is PAINT ORDER; the exclusions differ on purpose (`paint_shadow`/`paint_ambient` already take `look` — folding them in applies the hour twice). The desk lamp's halo IS washed, the floor lamp's is not — measured and accepted. Don't move the desk lamp above the snapshot: it would paint under a seated occupant.

- **The north wall band is SOLID for its whole width — the elevator doorway is a hole in the WALL, not the ground — and a sprite overlapping the band is the intended look** (feet-anchored, invariant #6). Re-cutting a channel (#902) is invisible to every routing guard; the tooth is `no_cell_of_the_north_wall_band_is_walkable`, because `walkable_target` samples straight off the mask — a cut cell is a legal draw, and a cat walked up the channel.

- **Layout variety is spent out of budgets that already exist — a scattered plant can only stand where a DISPLACED one already could.** The scatter only reorders `settle_plant`'s inward ladder (`every_piece_ground_stays_in_its_container` catches a free nudge). `compute_pod_decor` draws from a SHUFFLED BAG per pass with a rotate-on-collision walk between passes — don't collapse the walk into one shuffle (the pass SEAM is what the adjacency rule is about). Pinned by `no_two_adjacent_aisle_slots_share_a_kind`.

- **The painter's canvas nudge can seat a sprite ON furniture — the accepted side of a real conflict (#912).** `keep_sprite_on_canvas` moves a sprite inward without re-checking the mask; the residual is small and size-shaped. The resolution is one-directional: the SIM keeps the guarantee (`a_wandering_mascot_always_stands_on_walkable_ground`), the PAINT layer takes the compromise — a drawn overlap costs pixels; a moved sim position propagates into the hover box, z-sort and the next leg's origin.

- **A character is clamped to the canvas TWICE, against two DIFFERENT frame sizes — deliberately.** `sim::resolve_characters` clamps the sprite on the pack's real frame; `anchors::character_anchor` clamps the badge + hit box on the default `CHARACTER_SPRITE_W`/`_H` (a custom pack's size isn't threaded to the label path). Clamping only the sprite strands the badge off the drawn pixels. The label half has its own pin, `a_character_badge_is_never_anchored_off_the_canvas` (#916).
