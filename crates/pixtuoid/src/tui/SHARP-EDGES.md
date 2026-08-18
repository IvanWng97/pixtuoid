# pixtuoid tui painter — known sharp edges

Indexed one line each in [`CLAUDE.md`](CLAUDE.md). These look like bugs and are deliberate design — read the entry before "fixing" one. An entry is the edge + the WHY + one authority pointer; adjudication history lives in the cited issue/PR.

- **The 6 borderless popups share ONE geometry authority — `panel::PanelGeometry`.** `compute(..) → outer()/inner()/cell_rect()` is PURE: the painter fills `inner()` and every click-target reads `cell_rect()` off the SAME compute, so paint and click cannot drift (the phantom-browser-launch class, killed structurally). `PANEL_PAD_X/Y` is PRIVATE to `panel.rs`. The flat offset drop shadow is a pinned USER PREFERENCE — don't "restore" a gradient/penumbra (`borderless_panel_casts_a_flat_offset_shadow`).

- **The board's ★ Star click target is `wall_board::star_hit_rect` — same phantom-launch class.** Paint and hit-rect derive from the SAME `BOARD_W`/`BOARD_STAR`; the `debug_assert` in `paint_wall_display` pins the pairing. It replaced `hit_test_branding`, which fired on all of cols 1..31.

- **The CLICK ladder (`tui/mod.rs`) and HOVER ladder (`renderer.rs`) share the `agent > coffee > pet` ordering and the hit-test primitives, but a unified resolver is NOT worth hoisting.** Different layers, different target sets, and the click FUSES test+act in `focus_clicked_agent` — a resolver would be a param-heavy shim as wide as the body it hides. Adjudicated in sweep-3; the ordering stays synced by the load-bearing comment in the click arm.

- **Every modal overlay paints on EVERY frame — including footer-only frames.** Below `renderer::min_terminal_size()` rendering falls to `draw_footer_only_frame` but every key handler stays live, so overlay state travels as ONE `renderer::OverlayFrame` through all THREE paint paths and panels self-guard on size via `PanelGeometry` (first run opens onboarding INTO the footer-only path). The footer row is reserved inside the geometry (`panel::RESERVED_FOOTER_ROWS`); `renderer::FOOTER_ROWS` is the one row-count every reserver reads.

- **Every crossterm key event passes `should_dispatch_key` first**: Windows delivers Press AND Release (and Repeat) per keystroke, so only `KeyEventKind::Press` dispatches — otherwise toggle keys double-fire there. Unix only emits Press, so the gate is a no-op locally.

- **The `w` walkable/approach/route debug overlay is dev-only** — its dispatch arm is `#[cfg(debug_assertions)]`-gated, so release builds silently ignore `w`. Don't "fix" a report that `w` does nothing in an installed binary.

- **`run_tui` is the `block_on` ROOT future, not a tokio worker — `block_in_place` there is INERT.** The loop body runs on the `block_on` thread, which owns no worker core (removed as proven inert in #603; it does NOT panic — that's `current_thread`-only). The loop's one-shot blocking I/O is an ACCEPTED ms-scale inline stall; `spawn_blocking` was rejected (non-`Send` borrows would force a pending-op state machine and widen `focus_slot`'s #527 pid-recycle guard window).
