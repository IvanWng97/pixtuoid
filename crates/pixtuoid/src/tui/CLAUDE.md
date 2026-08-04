# pixtuoid/tui — terminal renderer agent guide

The **terminal painter**: ratatui `App` + `TuiRenderer` (its inherent `render`
flush). Owns the half-block flush, the widgets (footer/wall-display/theme-picker
UI/tooltip/help/dashboard/Sources panel), mouse hit-testing, the version popup,
the crossterm event loop + terminal lifecycle, and the per-session UI models
(dashboard, Sources/connection). This is where the `pixtuoid-scene` engine's pixel
buffer becomes terminal cells.

**The render + simulation ENGINE is its OWN crate
[`pixtuoid-scene`](../../../pixtuoid-scene/CLAUDE.md)** (the DAG is
`pixtuoid-core ← pixtuoid-scene ← pixtuoid`) — layout, pose/motion/pathfinding,
the pixel pass (`render_to_rgb_buffer`), the color theme MODEL, pets, chitchat,
frame_cache, embedded_pack. `tui` and `floating` are sibling thin painters over
the `pixtuoid-scene` crate; neither depends on the other. When a painter entry
below references engine internals (`pixtuoid_scene::pixel_painter`,
`pixtuoid_scene::theme`, `pixtuoid_scene::motion`, …) follow the link to the
[scene guide](../../../pixtuoid-scene/CLAUDE.md) for the deep "why."

Parent guides: binary [`../../CLAUDE.md`](../../CLAUDE.md); workspace
[`../../../../CLAUDE.md`](../../../../CLAUDE.md); headless lib
[`../../../pixtuoid-core/CLAUDE.md`](../../../pixtuoid-core/CLAUDE.md); the engine
[`../../../pixtuoid-scene/CLAUDE.md`](../../../pixtuoid-scene/CLAUDE.md).

## Layout

```
tui/
├── renderer.rs     draw_scene<B: Backend> orchestrator (DrawCtx struct — borrows the per-floor FloorCtx
│                   as ONE `store` field + a separate `buf`, was seven flat store fields): calls pixtuoid_scene::pixel_painter::
│                   render_to_rgb_buffer for the world, then the half-block flush + widgets + footer
│                   (terminal lifecycle — raw mode + alternate screen — lives with the event loop in
│                   tui/mod.rs, #103); the floor-slide paints via the shared scene seam (floor::render_floor)
├── mod.rs          crossterm event loop + terminal lifecycle (raw mode / alternate screen) + key
│                   dispatch (the `w` debug-overlay toggle — dev-only, debug builds; `t` theme picker, `Tab` dashboard, `s`
│                   Sources panel, floor nav, version popup, quit chord) + connect_source/disconnect_source.
│                   run_tui is the EVENT LOOP only — each KeyAction's side effects are applied by
│                   apply_key_action(action, &mut KeyCtx) -> quit, which IS unit-tested (the loop is not);
│                   the per-surface UI state lives in ui_state.rs
├── ui_state.rs     UiState — run_tui's per-surface UI state as ONE struct (onboarding/version/help/
│                   pause/theme-picker/dashboard/connection + DriftScan): owns the open/close
│                   transitions, projects the dispatch-facing ModalState (`UiState::modal` — the one
│                   source of truth; help_open is owned HERE, the renderer's copy is a per-frame
│                   mirror like every other overlay), and computes the per-frame renderer mirrors
│                   (`build_frames(now, scene, health) -> RenderFrames`, pushed by `RenderFrames::
│                   apply_to`). Blocking panel I/O (`build_rows`, connect/disconnect, onboarding
│                   apply) stays at the loop as an accepted inline stall (block_in_place removed in #603 — inert on the block_on thread)
├── widgets/        ratatui widget paint fns, split into sub-modules:
│                   mod.rs (the shared HUD substrate: RE-EXPORTS scene_stats/StateCounts/per_floor_counts/
│                   gateway_rollup/compact_hms from `pixtuoid_scene::board` [the stats+board MODEL moved there
│                   in #aa-text so floating + wasm share it; the re-exports keep every footer/board call site
│                   unchanged]; the four-state vocabulary now lives ONCE in `pixtuoid_scene::footer::RungKind`
│                   (glyph/letter/word/ALL/count) and is RE-EXPORTED here as `StateKind` (tooltip/dashboard read
│                   the SAME channels — no parallel enum); the hue is the `state_color(kind, theme)` shim over
│                   `pixtuoid_scene::footer::footer_tone_rgb` (the old inherent `StateKind::color` + the
│                   `state_count` free fn are gone — call sites use `RungKind::count`); plus source_badge_span,
│                   display_width, marquee_window/marquee_or_truncate, shared helpers; re-exports panel::{borderless_panel, paint_panel, panel_inner_width, Overflow, PanelGeometry} — PANEL_PAD_* is PRIVATE to panel.rs),
│                   footer.rs (a THIN ADAPTER over `pixtuoid_scene::footer::build_footer`: FooterStats →
│                   FooterInputs marshalling + build_status_spans/build_status_summary render the shared model;
│                   the tier/policy logic + SegRole moved to scene::footer, source_warning_message stays here),
│                   wall_board.rs (the wall BOARD [paint_wall_display renders the
│                   `pixtuoid_scene::board::build_board` MODEL, mapping each segment's BoardTone → color via
│                   `board_tone_color`; BOARD_W=NEON_PANEL_INNER_W, BOARD_STAR re-read from scene::board +
│                   star_hit_rect]), elevator.rs (elevator indicator), theme_picker.rs (theme PICKER ui +
│                   theme_swatch), version_popup.rs (version popup paint + url-rect, REPO_URL/VERSION_POPUP_URL),
│                   tooltip.rs (hover DOSSIER, cat, coffee, furniture, labels —
│                   paint_label_widgets consumes the shared pixtuoid_scene::overlay::build_overlay MODEL, the floating
│                   window paints the SAME model its own way; chitchat bubbles), help.rs (paint_help_overlay),
│                   dashboard.rs (paint_dashboard — the
│                   agent-dashboard popup PAINTER), panel.rs (borderless_panel — the shared popup frame),
│                   connection.rs (paint_connection_panel — the Sources-panel PAINTER),
│                   welcome.rs (paint_welcome — the first-run onboarding overlay PAINTER; typewriter
│                   + staged roster reveal driven by OnboardingFrame.elapsed_ms)
├── hit_test/       mouse hit-test: agent hover, coffee machine click, furniture tooltips, pet
├── tui_renderer/   TuiRenderer + its inherent `render` flush, split production vs tests:
│                   mod.rs (TuiRenderer: `evict_missing` is the ONE seam for BOTH halves of the dual
│                   eviction — per-floor (cache/history/motion, every floor) AND office (coffee) — the same
│                   pairing pixtuoid_scene::floor::FloorSession::evict_missing writes; the office half used
│                   to sit inside `render`'s normal path, past the transition early-return, so a floor slide
│                   skipped it. Cross-frame state — Vec<pixtuoid_scene::floor::PerFloor> (each = FloorCtx
│                   [FrameCache/Router/PoseHistory/OccupancyOverlay + .motion + .door_anim_max_ms] + its RgbBuffer)
│                   + one pixtuoid_scene::floor::PerOffice (coffee + chitchat, office-wide) — the session halves
│                   the single-floor painters own as a FloorSession; plus Theme, cached Layout;
│                   #[cfg(test)] frame_buffer/floor_motion/floor_history/floor_buf/inject_coffee test seams),
│                   harness (#[cfg(test)] mod: ~100 headless integration tests (99 as of #435) driving the real
│                   render()/navigate_floor() path via ratatui TestBackend — output-first: buf() pixels + frame_buffer cells;
│                   white-box seams only where an invariant isn't output-observable.
│                   NOT coverable headlessly, excluded in codecov.yml — incl. two files
│                   outside this tree: tui/mod.rs (crossterm event loop + real TTY),
│                   runtime/driver.rs (tokio block_on + ctrl_c + socket bind), main.rs)
├── connection/     Sources panel UI-side model ONLY (no ratatui), historically named "connection": mod.rs
│                   (LiveInfo / live_view — the per-frame LIVE facet; ConnectionFrame / ConnectionUi;
│                   move_selection / format_{connect,disconnect}_result / FailedOp +
│                   format_failure, THE one producer of the panel's three failure
│                   sentences) + tests.rs. The row/status
│                   MODEL (ConnState / ConnectionRow / build_rows + the core_source join) was RELOCATED
│                   to crate::sources (the TUI-free CLI/model module) and is only re-exported here.
│                   The ratatui painter is widgets/connection.rs.
├── dashboard/      agent-dashboard PURE model (no ratatui): mod.rs (DashboardUi / DashboardRow /
│                   RowState / DashboardFolds; build_dashboard_rows tree builder; move_selection /
│                   reanchor_selection / resolve_floor / clamp_scroll; AUTO_COLLAPSE_THRESHOLD) +
│                   tests.rs. The ratatui painter is widgets/dashboard.rs.
└── welcome/        first-run onboarding PURE model (no ratatui): mod.rs (WelcomeUi/WelcomeRow checklist
                    [from_detected pre-checks the detected CLIs, move/toggle/decisions] + OnboardingFrame,
                    the bundled render snapshot {open,rows,selected,elapsed_ms}) + tests.rs. The ratatui
                    painter is widgets/welcome.rs; the apply is sources::apply_choices.
```

## Known sharp edges (don't be surprised by these)

- **`draw_scene` is called through `TuiRenderer`** (its inherent `render` flush), which owns the cross-frame state (per-floor `FloorCtx` {FrameCache/Router/OccupancyOverlay/PoseHistory/motion/light} + the `RgbBuffer`) and assembles a per-frame `DrawCtx` borrow that carries that `FloorCtx` as ONE `store` field (was the flat FrameCache/Router/OccupancyOverlay/PoseHistory list) + a separate `buf` (disjoint sibling), plus theme + mouse state. It delegates the world render to `pixtuoid_scene::pixel_painter::render_to_rgb_buffer` (the shared engine seam — see [`../../../pixtuoid-scene/CLAUDE.md`](../../../pixtuoid-scene/CLAUDE.md)), then does the terminal-only half-block flush + widgets + footer. `draw_scene` returns `Result<Option<Arc<Layout>>>` (a shared handle — `FloorCtx::frame_layout` memoizes an `Arc<Layout>` and hands out a refcount bump, not a per-frame deep copy of the mask + reach-set + layout Vecs) — the computed layout is cached on `TuiRenderer.cached_layout` so hit-test functions can use it without recomputing. During floor transitions, `cached_layout` is cleared to `None`.
- **The 6 borderless popups share ONE geometry authority — `panel::PanelGeometry`.** `compute(bounds, content_w, content_rows, title, scale) → outer()/inner()/cell_rect()` is PURE (no `TestBackend`): the painter fills `inner()` and any click-target reads `cell_rect()` off the SAME `compute`, so paint and click cannot drift. The version popup's URL click-rect (`version_popup_url_rect`) is now `cell_rect` off the shared `compute` — the phantom-browser-launch regression class is killed STRUCTURALLY and pinned by a pure arithmetic test. `PANEL_PAD_X/Y` is PRIVATE to `panel.rs` (nothing reverses the inset); `centered_in` + `version_popup_envelope` are DELETED; the `<4||<3 → None` guard + version's `.max(2)` floor are unified into `compute`. The 6 painters route through `panel::paint_panel` (auto-height from the ACTUAL `above`/`list`/`below` band lengths + `Overflow` list-windowing + the `⋮ N more ▾` cue via `window_range`), except the version popup which drives `PanelGeometry` directly (scale animation + `cell_rect`). `window_range` reuses `dashboard::clamp_scroll_idx` so every list panel slides identically.
- **The board's ★ Star click target is `wall_board::star_hit_rect`, the same phantom-launch class** — it derives the precise `★ Star` span from the SAME board geometry the L1 painter uses (`cell_x = scene.x+2`, `cell_y = scene.y+1`, right-flushed to `BOARD_W`, star text = `BOARD_STAR`) and clips to the scene (`None` on a too-narrow terminal). The caller (`tui/mod.rs`) still gates on `cached_layout().is_some()`. It REPLACED the loose `hit_test_branding` (fired on all of cols `1..31`). Keep the paint + the hit-rect deriving from the same `BOARD_W`/`BOARD_STAR` — the `debug_assert` in `paint_wall_display` pins the brand-fits invariant the pairing needs.
- **The CLICK ladder (`tui/mod.rs`) and the HOVER ladder (`renderer.rs`) share the `agent > coffee > pet` ordering AND the underlying hit-tests, but a unified `resolve_pointer_hit(...) -> PointerHit` enum is NOT worth hoisting.** Both resolve the agent through the SAME live-sprite `hit_test_agent` (the click via the thin `TuiRenderer::hit_test_agent_at` wrapper that calls it — FIND-22; the `#[cfg(test)]`-only `hit_test_from_tui` is the seated-anchor test locator, NOT a production path) and both call the same coffee/pet geometry — so the ordering + the individual hit-tests genuinely ARE shared. What blocks a clean resolver is everything ELSE: (1) the two live in different LAYERS with different data — the click is in the codecov-ignored crossterm event loop with `scene_rx`/`focus_roots`/`crossterm::size` in hand, hover is inside `draw_scene`; (2) the TARGET SETS differ — click: `★`→open-URL, agent→`focus_slot`, coffee→BMC, pet→`set_active_pet`, NO mascot/furniture; hover: agent/coffee/pet/**mascot**/**furniture** tooltips, NO star; and (3) the click FUSES test+act in `focus_clicked_agent` (it focuses INSIDE the hit, so a shared resolver can't own the agent arm). A `PointerHit` resolver would take the agent/star hits as caller-computed inputs and own only the ~6-way priority + the coffee/pet/mascot/furniture geometry across two layers — a param-heavy shim about as wide as the body it hides, for a marginal gain over the already-shared hit-tests. The ordering stays synced by the load-bearing comment in the click arm ("agent wins over coffee/pet, matching the hover ladder"). Adjudicated against in sweep-3 (the earlier draft wrongly claimed the agent PRIMITIVE diverges — it does not; corrected on review).
- **Every modal overlay paints on EVERY frame — including the footer-only ones.** The office needs a 32×31 terminal (`layout::compute_with_seed`'s `MIN_LAYOUT_W/H` over a `(rows-1)*2` buffer), well above `draw_scene`'s own 20×12 `MIN_SCENE_*` gate, so on the classic 80×24 the world can't lay out and both gates fall through to `draw_footer_only_frame`. Every modal's KEY HANDLER stays live there (`dispatch_key` knows nothing about terminal size), so an overlay suppressed on that path meant `?`/`s`/`Tab` toggled something invisible — and first run opens the onboarding modal into it. The overlay state therefore travels as ONE `renderer::OverlayFrame` through all THREE paint paths (`draw_scene`'s full frame, its two too-small early-returns, and `render_transition`'s both arms); the panels self-guard on size through `PanelGeometry`, so nothing needs a second threshold. All three hand `paint_overlays` the FULL terminal rect (a modal centers over the whole frame); **the footer row is kept off-limits inside the geometry instead, by `panel::RESERVED_FOOTER_ROWS`** — `compute` clamps `full_h` to `bounds.height - 1`. A panel taller than the terminal otherwise clamps to `bounds.height` and paints over the footer, the one persistent affordance (the version popup did exactly that at 32×31). Reserving the row in `compute` rather than shrinking each caller's `bounds` is deliberate: centering in a shorter box would move every panel that already FITS by a row on half the terminal heights, silently redrawing the committed `docs/images/dashboard.png` + `connection.png` + the site's `agent-tree` demos. **That clamp is only the card BODY's half of the rule** — `cast_drop_shadow` offsets the silhouette `SHADOW_OFFSET` rows DOWN, so a card stopping exactly one row short still landed its bottom band on the footer, and that band dims `fg` only: the live `[q]uit` text repainted at `SHADOW_FACTOR` over its still-lit bg (measured 200 → 84 on a synthetic cell; ~5.2:1 → ~1.55:1 contrast on a real frame), which the string assertion `last_row.contains("[q]uit")` cannot see. So the shadow clips ITSELF to `renderer::scene_rect(f.area())` — one clip covering BOTH card kinds, since the hover tooltips anchor inside `scene_rect` and sit at the same edge. Deliberately NOT a bigger `RESERVED_FOOTER_ROWS`: the reserve is symmetric (the clamp shrinks the box, `compute` still centers in the FULL `bounds`), so a clamped card's bottom is `bounds.bottom() - ceil(R/2)` and it would take `R = 2·SHADOW_OFFSET + 1 = 3` — costing two content rows and blanking every modal below a 6-row terminal — to move the band one row. The row count itself is `renderer::FOOTER_ROWS`, read by `scene_rect`, the panel reserve, and the shadow clip alike. The version popup rides the same rule and `TuiRenderer.popup.last_scale` is now the painted scale on every `Ok` frame — its click rect derives from the terminal BOUNDS, not the office layout, so paint and click agree at any size (the older "zero the hit-box on a footer-only frame" rule existed only because nothing was painted there).
- **Every crossterm key event passes `should_dispatch_key` first** (`tui/mod.rs`): Windows delivers Press AND Release (and Repeat) per keystroke, so only `KeyEventKind::Press` dispatches — otherwise toggle keys double-fire there (`p` pauses then instantly unpauses). Unix only emits Press, so the gate is a no-op locally.
- **The `w` walkable/approach/route debug overlay is dev-only** — its dispatch arm is `#[cfg(debug_assertions)]`-gated, so release builds silently ignore `w`. Don't "fix" a report that `w` does nothing in an installed binary.
- **`run_tui` is the `block_on` ROOT future, not a tokio worker.** `driver.rs` builds `new_multi_thread` and does inline `rt.block_on(run_async)`, which `.await`s `run_tui`; `reducer_task`/sources are the only `tokio::spawn` tasks (their own worker threads). So the loop body runs on the `block_on` thread, which owns no worker core — `tokio::task::block_in_place` there is INERT (the task-handoff path is skipped; it runs the closure inline with zero scheduling effect). It does NOT panic — that's `current_thread`-only (`pixtuoid-core`'s `jsonl/liveness.rs` documents that panic case for its `current_thread` test runtime, which has no `rt-multi-thread`). So do NOT wrap the loop's blocking I/O — the Sources-panel `build_rows`/connect/disconnect, onboarding apply/skip, `config::save` on `ThemeCommit`, `focus_slot` on a click — in `block_in_place`; it does nothing. That I/O is an ACCEPTED brief inline stall on one-shot, user-initiated actions; the `flock` is `try_lock` + auto-releasing so there's no unbounded hang, and `reducer_task`/sources keep running on their own threads (a stall freezes only render+input, never agent-event processing). `block_in_place` here was removed in #603 after it was proven inert. `spawn_blocking` was rejected: the closures borrow `&config_path`/`&mut ui` (not `Send + 'static`), so it would need a cross-frame pending-op state machine in the codecov-ignored loop AND would widen `focus_slot`'s #527 pid-recycle guard window — a real design change for no measurable UX gain on ms-scale one-shot stalls.

## Where to look

Answers live in [`WHERE-TO-LOOK.md`](WHERE-TO-LOOK.md), so a session
pays for the entry it needs instead of all of them. Grep it for the
question:

- How is the office rendered (pixel pass → terminal)?
- How does the neon wall board work?
- How does the theme PICKER UI work?
- How are popups framed?
- How does the version popup render?
- How does the `?` help overlay work?
- How does the agent dashboard work?
- How does the footer instrument work?
- How does the Sources panel work?
- How does first-run onboarding work?
- How does the hover tooltip (the agent DOSSIER) work?
- How does the coffee machine Easter egg work?
- How do furniture hover tooltips work?
- How does the pet tooltip / click-to-pet work?
- How does multi-floor navigation work (the painter side)?

## When refactoring

The terminal render path is exercised by the headless harness
(`tui_renderer/harness`, ~100 tests — 99 as of #435) driving the real `TuiRenderer` through a
ratatui `TestBackend` — output-first (`buf()` pixels + `frame_buffer` cells).
Changes to `draw_scene`, the widgets, or the dispatch precedence should add or
update a harness test. The engine-side authority (`derive_with_routing`,
`MotionState`, the pixel passes) lives in the `pixtuoid-scene` crate — see
[`../../../pixtuoid-scene/CLAUDE.md`](../../../pixtuoid-scene/CLAUDE.md)'s "When
refactoring." Don't reach back into `floating/` from here: `tui` and `floating`
are sibling painters that share the `pixtuoid-scene` crate, not each other.
