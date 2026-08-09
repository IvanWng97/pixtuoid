# pixtuoid-core — agent guide

The **headless library**: no terminal dependencies (`ratatui`/`crossterm`/`stdout`
are forbidden here — see workspace invariant #1). Owns the source/decoder seam,
the reducer/state machine, sprite parsing, and the shared grid/walkability
vocabulary. The sim geometry (layout, pose derivation, walk physics) lives in
`pixtuoid-scene` — the engine owns its geometry; only the coherence-bound
`walkable.rs` stays here (see [`SHARP-EDGES.md`](SHARP-EDGES.md)). The scene engine (`pixtuoid-scene`)
and the binary (`pixtuoid`) sit on top of this. See the workspace
[`CLAUDE.md`](../../CLAUDE.md) for cross-cutting rules.

## Layout

Annotated tree in [`LAYOUT.md`](LAYOUT.md) — grep it for a filename.

```
src/
├── source/             Source trait, hook+jsonl decoders, listeners …
├── state/              SceneState + Reducer (event coordinator: Transport-tagged …
├── sprite/             .sprite parser, pack.toml loader, blit_frame blitter …
├── platform.rs         cross-platform home-dir resolution (user_home() …
├── grid.rs             Grid<T> — a width×height row-major Vec<T> with checked …
├── harness.rs          `harness` FEATURE (non-default, dev-only — absent from …
├── id.rs               AgentId + from_parts/from_transcript_path (moved out of …
├── walkable.rs         WalkableMask = Grid<bool> (static obstacle mask) + …
└── tests/              one integration test per concern
```

- **Burn-tier plumbing (model flame):** what `ModelInfo` carries, and each source's model/effort channel.
- **Token-meter plumbing (#632, the desk paper tower):** what counts as FRESH spend, per source.
- **Focus-jump plumbing (#focus-jump):** who stamps `_pid`, and the `FocusChannel` capability that routes it.

## Known sharp edges (don't be surprised by these)

Full entries in [`SHARP-EDGES.md`](SHARP-EDGES.md) — grep it for the phrase.

- There is NO core `render` module and no core render trait.
- `walkable.rs` is coherence-bound to this crate — it did NOT move with the sim-geometry cluster.
- A daemon's runtime identity is the SOURCE's own wire fact, and for OpenClaw that is the resolved gateway PORT — not the profile, not the pid.
- `GatewayDown` CREATES an absent instance; `PidExited` never does — the asymmetry is the point, not a hole in the creation-polarity rule.
- A `gatewayPort`-LESS OpenClaw envelope falls back to ONE documented legacy instance instead of being rejected — deliberately.
- The two pid→fan-out shells (`HookPidWatch` in `hook/pid_watch.rs`, the daemon's `PresenceExitWatch` in `daemon/native.rs`) stay SEPARATE — do NOT hoist a generic `PidFanout<K>`.
- The `native` (default) feature gates the ASYNC SOURCE RUNTIME, not the decoders — and the gates are MODULE-level, not item scatter.
- The per-CLI home resolvers MIRROR each CLI's own; the axes deliberately NOT mirrored are listed here
- CC hook payloads DO include `tool_use_id`
- That hook-wins dedup is ONE-directional — a JSONL-first tool inflates `tool_call_count` by 1 (cosmetic).
- The `active_tasks` insert in `track_active_tasks` is not slot-gated, and that's HARMLESS — don't "fix" it with a `contains_key` guard.
- The JSONL watcher's DECODED-event send is blockable, unlike the hook path's `CONN_TIMEOUT` — a deliberate asymmetry.
- CC now keys on the session UUID, not the transcript path.
- CC hook `transcript_path` always points to the PARENT'S transcript
- JSONL watcher skips historical transcripts — on EVERY first-sight path, not just startup.
- Watch backend: native in prod, polling in tests.
- A hook event from an UNKNOWN session id REGISTERS it — hooks are proof of life.
- Agent removal needs a `SessionEnd`; abrupt exits have none and fall back to the slow stale-sweep.
- A subagent registered AFTER its parent's cascade escapes it, and is reaped only by its OWN stale sweep — deliberate, because a parent's `exiting_at` is NOT a terminal verdict.
- Resurrect-in-place starts from clean correlation state.
- Codex subagents (`spawn_agent`) are wired via the `SubagentStart`/`SubagentStop` HOOKS, not JSONL paths.
- Subagent display names come from `attributionAgent` in JSONL.
- `AgentSlot.state_started_at` is `std::time::SystemTime`
- `ActivityState::Active` ≠ "tool is currently executing".
- The reducer's permission `Waiting` resolves on the gated tool's PostToolUse.

## Where to look

Answers live in [`WHERE-TO-LOOK.md`](WHERE-TO-LOOK.md), so a session
pays for the entry it needs instead of all of them. Grep it for the
question:

- How does the per-agent state machine work?
- Why is the subagent's sprite the right one and not the parent?
- How does multi-source decoding work?
- Why don't old idle sessions show on startup?

## Keeping the decode mapping current (upstream drift)

Moved to [`UPSTREAM-DRIFT.md`](UPSTREAM-DRIFT.md) — read it before touching this area.

## When refactoring

The channel type, `Source` trait, `AgentEvent` enum, and reducer signature are workspace-wide contracts — see the root [`CLAUDE.md`](../../CLAUDE.md) "When refactoring" for the full list of test files to update and the add-a-CLI checklist.
