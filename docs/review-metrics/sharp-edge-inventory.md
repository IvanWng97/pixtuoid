# Sharp-edge inventory — the canonical, slug-anchored set

The full set of formal **"Known sharp edges"** bullets across the nested
`crates/*/CLAUDE.md` guides, each with a STABLE kebab-slug. This is the canonical
universe the review-history census's sharp-edge-citation leg counts against — so
the leg is a script run (`scripts/sharp_edge_inventory.py`), not a hand-count, and
the demotion clock is anchored to a committed artifact instead of being
re-derived ad hoc each census (the #386 follow-through).

**How it's used.** A REVIEW-LEDGER row that cites a documented sharp edge tags it
`[edge:<slug>]` in the mechanism column (per the ledger protocol step 5). The
census harvests those tags, counts citations per slug, and reports the uncited
ones. **Demotion rule:** a slug uncited across **two consecutive censuses**
(counting from census #4 — the first to use this committed inventory) is a
demotion candidate (demote, never kill). The `last cited` column seeds the clock:
`#2`/`#3` = the census that last cited it; `—` = not yet cited under the tracked
window.

**Drift guard.** `just sharp-edge-inventory` (and the CI hygiene job) asserts this
inventory stays in lockstep with the CLAUDE.md sharp-edge bullets — a per-file
count parity, so a sharp edge cannot be added or removed from a `CLAUDE.md`
without updating this file (the `supported_sources_manifest` bridge-test pattern).
It also flags any `[edge:<slug>]` in the ledger that doesn't resolve here (a typo
or an orphaned citation).

**`file` key → guide:** `core` = `crates/pixtuoid-core/CLAUDE.md` ·
`tests` = `crates/pixtuoid-core/tests/CLAUDE.md` ·
`scene` = `crates/pixtuoid-scene/CLAUDE.md` ·
`bin` = `crates/pixtuoid/CLAUDE.md` ·
`tui` = `crates/pixtuoid/src/tui/CLAUDE.md`.

A `(←old-slug)` note records a slug whose prefix moved when its edge changed
crates — the recolor / exit-compression / walk-leg edges moved `tui → scene` with
the engine extraction (#346/#349), so the census's old `tui-*` names are re-anchored
to `scene-*` here (the topic part is preserved so citation history stays traceable).

| slug | file | last cited | headline |
|---|---|---|---|
| `core-cc-tool-use-id-dedup` | core | — | CC hook payloads DO include `tool_use_id` |
| `core-cc-keys-on-session-uuid` | core | — | CC now keys on the session UUID, not the transcript path. |
| `core-transcript-path-points-at-parent` | core | — | CC hook `transcript_path` always points to the PARENT'S transcript |
| `core-jsonl-skips-historical-first-sight` | core | — | JSONL watcher skips historical transcripts — on EVERY first-sight path, not just startup. |
| `core-watch-backend-native-vs-poll` | core | — | Watch backend: native in prod, polling in tests. |
| `core-hook-from-unknown-id-registers` | core | — | A hook event from an UNKNOWN session id REGISTERS it — hooks are proof of life. |
| `core-abrupt-exit-stale-sweep` | core | — | Agent removal needs a `SessionEnd`; abrupt exits have none and fall back to the slow stale-sweep. |
| `core-resurrect-clean-correlation` | core | — | Resurrect-in-place starts from clean correlation state. |
| `core-codex-subagents-via-hooks` | core | — | Codex subagents (`spawn_agent`) are wired via the `SubagentStart`/`SubagentStop` HOOKS, not JSONL paths. |
| `core-subagent-name-from-attribution-agent` | core | — | Subagent display names come from `attributionAgent` in JSONL. |
| `core-state-started-at-systemtime-serialize` | core | #2 | `AgentSlot.state_started_at` is `std::time::SystemTime` |
| `core-active-not-tool-executing` | core | — | `ActivityState::Active` ≠ "tool is currently executing". |
| `core-waiting-resolves-on-posttooluse` | core | — | The reducer's permission `Waiting` resolves on the gated tool's PostToolUse. |
| `core-narrow-meeting-room-no-furniture` | core | — | A meeting room narrower than `MEETING_FURNITURE_MIN_W` (compute.rs) has NO sofa/table/seats — bare floor, BY DESIGN. |
| `core-occlusion-is-emergent` | core | — | Occlusion is EMERGENT — there is no `occludes_behind` field / synthetic cap any more (deleted). |
| `core-pantry-counter-shallow-strip` | core | #2 | Pantry counter blocks only a shallow `PANTRY_FOOTPRINT_DEPTH` south strip, not its full sprite height. |
| `tests-two-tests-stay-flat` | tests | — | Two tests stay FLAT and MUST NOT be moved into a grouped binary |
| `tests-multifile-binary-is-main-rs` | tests | — | A multi-file binary is `tests/<area>/main.rs`, NOT `tests/<area>.rs`. |
| `tests-conformance-dir-must-be-registered-source` | tests | — | `conformance.rs` (the harness) asserts every dir under `sources/fixtures/` is a registered source |
| `tests-insta-name-from-path` | tests | — | insta snapshot names = `<binary>__<module>__<explicit-name>` |
| `scene-recolor-by-rgb-equality` | scene | #2 | `recolor_frame` substitutes by RGB equality. (←tui-recolor-by-rgb-equality) |
| `scene-exit-compression-not-snapback` | scene | — | EXIT walks are time-compressed to fit the GC window; entry/wander/snap-back are not. (←tui-exit-compression-not-snapback) |
| `scene-walk-leg-frozen-polyline` | scene | — | A walk leg's A\* polyline shape is frozen once per leg, not re-routed per frame. (←tui-walk-leg-frozen-polyline) |
| `bin-terminal-cell-aspect` | bin | — | Terminal cell aspect drives sprite design. |
| `bin-max-desks-no-default` | bin | #3 | `--max-desks` has no hard default. |
| `bin-reinstall-noop-backup-append` | bin | — | Re-install is a SEMANTIC no-op, and backups APPEND their suffix. |
| `bin-two-presenters-one-source-core` | bin | — | Two surfaces bind a source, ONE core. |
| `bin-code-artifact-install-verify-coverage` | bin | — | Code-artifact targets: install writes ⊆ verify checks (#387). |
| `tui-draw-scene-via-tuirenderer` | tui | — | `draw_scene` is called through `TuiRenderer` |
| `tui-version-popup-url-rect-lockstep` | tui | — | The version popup's URL click-rect (`version_popup_url_rect`) derives its offsets from the SAME `PANEL_PAD_*` consts the painter insets by |
