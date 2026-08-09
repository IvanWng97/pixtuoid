# pixtuoid tui painter — annotated layout

The navigable skeleton is in [`CLAUDE.md`](CLAUDE.md); this is the same tree with each entry's full annotation.

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

