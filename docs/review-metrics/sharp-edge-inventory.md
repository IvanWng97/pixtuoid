# Sharp-edge inventory — the canonical, slug-anchored set

The full set of sharp edges the review-history census's sharp-edge-citation leg
counts against — a COMMITTED artifact with stable kebab-slugs, so the leg is a
script run (`scripts/sharp_edge_inventory.py`), not a hand-count, and the demotion
clock anchors here instead of being re-derived each census (the #386
follow-through).

**How it's used.** A REVIEW-LEDGER row that cites a documented sharp edge tags it
`[edge:<slug>]` in the mechanism column (ledger protocol step 5). The census
harvests those tags, counts citations per slug, reports the uncited.

**Three `kind`s of row:**
- **`edge`** — a formal `- **…**` bullet in a guide's "Known sharp edges" section.
  These are the count-parity'd set (see Drift guard).
- **`qa`** — a load-bearing invariant documented in a guide's "Where to look" /
  "Things NOT to do" prose rather than a bullet, but which the review corpus
  demonstrably cites (e.g. `floor_idx`-immutable, the per-CLI path-resolution
  rule — census #3 cited both). They are citeable + orphan-resolved but NOT
  count-parity'd (they have no bullet to count).
- **`alias`** — a retired slug kept so an OLD citation still resolves. The
  recolor / exit-compression / walk-leg edges moved `tui → scene` with the engine
  crate extraction (#346/#349), so the census's `tui-*` names alias the `scene-*`
  ones.

**Demotion clock.** The `last cited` value is a real clock position (the census
that last cited the slug), not mere provenance: a slug uncited from its seed
through two further consecutive censuses is a demotion candidate (demote, never
kill). `—` = not yet cited under the tracked window.

**Drift guard.** `just sharp-edge-inventory` (gated in CI hygiene) asserts the
`edge` rows stay in **per-file count parity** with the CLAUDE.md sharp-edge bullets
— a sharp edge can't be added/removed from a guide without updating this file (the
`supported_sources_manifest` bridge-test pattern). It also rejects any ledger
`[edge:<slug>]` that doesn't resolve to a row here. **Parity is by COUNT, not
content**: an in-place edit that repurposes a bullet without changing the count is
NOT caught here — the slug↔edge correspondence is by convention, re-verified when
the census re-reads each edge. So a bullet rewrite should still update this file.

**`file` key → guide:** `core` = `crates/pixtuoid-core/CLAUDE.md` ·
`tests` = `crates/pixtuoid-core/tests/CLAUDE.md` ·
`scene` = `crates/pixtuoid-scene/CLAUDE.md` ·
`bin` = `crates/pixtuoid/CLAUDE.md` ·
`tui` = `crates/pixtuoid/src/tui/CLAUDE.md` ·
`root` = `CLAUDE.md` (workspace; `qa` rows only).

| slug | file | kind | last cited | headline |
|---|---|---|---|---|
| `core-cc-tool-use-id-dedup` | core | edge | — | CC hook payloads DO include `tool_use_id` |
| `core-cc-keys-on-session-uuid` | core | edge | — | CC now keys on the session UUID, not the transcript path. |
| `core-transcript-path-points-at-parent` | core | edge | — | CC hook `transcript_path` always points to the PARENT'S transcript |
| `core-jsonl-skips-historical-first-sight` | core | edge | — | JSONL watcher skips historical transcripts — on EVERY first-sight path, not just startup. |
| `core-watch-backend-native-vs-poll` | core | edge | — | Watch backend: native in prod, polling in tests. |
| `core-hook-from-unknown-id-registers` | core | edge | — | A hook event from an UNKNOWN session id REGISTERS it — hooks are proof of life. |
| `core-abrupt-exit-stale-sweep` | core | edge | — | Agent removal needs a `SessionEnd`; abrupt exits have none and fall back to the slow stale-sweep. |
| `core-resurrect-clean-correlation` | core | edge | — | Resurrect-in-place starts from clean correlation state. |
| `core-codex-subagents-via-hooks` | core | edge | — | Codex subagents (`spawn_agent`) are wired via the `SubagentStart`/`SubagentStop` HOOKS, not JSONL paths. |
| `core-subagent-name-from-attribution-agent` | core | edge | — | Subagent display names come from `attributionAgent` in JSONL. |
| `core-state-started-at-systemtime-serialize` | core | edge | #2 | `AgentSlot.state_started_at` is `std::time::SystemTime` |
| `core-active-not-tool-executing` | core | edge | — | `ActivityState::Active` ≠ "tool is currently executing". |
| `core-waiting-resolves-on-posttooluse` | core | edge | — | The reducer's permission `Waiting` resolves on the gated tool's PostToolUse. |
| `core-narrow-meeting-room-no-furniture` | core | edge | — | A meeting room narrower than `MEETING_FURNITURE_MIN_W` (compute.rs) has NO sofa/table/seats — bare floor, BY DESIGN. |
| `core-occlusion-is-emergent` | core | edge | — | Occlusion is EMERGENT — there is no `occludes_behind` field / synthetic cap any more (deleted). |
| `core-pantry-counter-shallow-strip` | core | edge | #2 | Pantry counter blocks only a shallow `PANTRY_FOOTPRINT_DEPTH` south strip, not its full sprite height. |
| `tests-two-tests-stay-flat` | tests | edge | — | Two tests stay FLAT and MUST NOT be moved into a grouped binary |
| `tests-multifile-binary-is-main-rs` | tests | edge | — | A multi-file binary is `tests/<area>/main.rs`, NOT `tests/<area>.rs`. |
| `tests-conformance-dir-must-be-registered-source` | tests | edge | — | `conformance.rs` (the harness) asserts every dir under `sources/fixtures/` is a registered source |
| `tests-insta-name-from-path` | tests | edge | — | insta snapshot names = `<binary>__<module>__<explicit-name>` |
| `scene-recolor-by-rgb-equality` | scene | edge | #2 | `recolor_frame` substitutes by RGB equality. (←tui-recolor-by-rgb-equality) |
| `scene-exit-compression-not-snapback` | scene | edge | — | EXIT walks are time-compressed to fit the GC window; entry/wander/snap-back are not. (←tui-exit-compression-not-snapback) |
| `scene-walk-leg-frozen-polyline` | scene | edge | — | A walk leg's A\* polyline shape is frozen once per leg, not re-routed per frame. (←tui-walk-leg-frozen-polyline) |
| `bin-terminal-cell-aspect` | bin | edge | — | Terminal cell aspect drives sprite design. |
| `bin-max-desks-no-default` | bin | edge | #3 | `--max-desks` has no hard default. |
| `bin-reinstall-noop-backup-append` | bin | edge | — | Re-install is a SEMANTIC no-op, and backups APPEND their suffix. |
| `bin-two-presenters-one-source-core` | bin | edge | — | Two surfaces bind a source, ONE core. |
| `bin-code-artifact-install-verify-coverage` | bin | edge | — | Code-artifact targets: install writes ⊆ verify checks (#387). |
| `tui-draw-scene-via-tuirenderer` | tui | edge | — | `draw_scene` is called through `TuiRenderer` |
| `tui-version-popup-url-rect-lockstep` | tui | edge | — | The version popup's URL click-rect (`version_popup_url_rect`) derives its offsets from the SAME `PANEL_PAD_*` consts the painter insets by |
| `scene-floor-idx-immutable-snapshot` | scene | qa | #3 | `floor_idx` is set once at desk assignment and immutable thereafter; re-deriving it migrates agents to wrong desks (Q&A invariant, not a bullet). |
| `root-path-resolution-policy` | root | qa | #3 | Mirror each CLI's own authoritative config resolver — don't blanket-generalize (the "Things NOT to do" per-CLI mirroring rule). |
| `tui-recolor-by-rgb-equality` | scene | alias | — | retired → `scene-recolor-by-rgb-equality` (moved tui→scene, #346/#349). |
| `tui-exit-compression-not-snapback` | scene | alias | — | retired → `scene-exit-compression-not-snapback` (moved tui→scene, #346/#349). |
| `tui-walk-leg-frozen-polyline` | scene | alias | — | retired → `scene-walk-leg-frozen-polyline` (moved tui→scene, #346/#349). |
