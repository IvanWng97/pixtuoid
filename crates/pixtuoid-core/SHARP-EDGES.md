# pixtuoid-core — known sharp edges

Indexed one line each in [`CLAUDE.md`](CLAUDE.md). These look like bugs and are deliberate design — read the entry before "fixing" one: the edge, the WHY, one authority pointer (pinning test / in-code comment / issue). Adjudication history lives in the cited issue/PR, not here.

- **Only a wire surface we actually READ may raise a drift alarm — don't add a `KNOWN_*` allowlist of names we ignore.** `drift::unknown_event` has no dedup, so such a list must mirror an upstream vocabulary forever or a harmless upstream addition alarms every user (#935). Detect by SHAPE where the payload allows (`an_unnamed_type_is_a_turn_only_when_it_carries_the_payload`); WHICH sources also owe an upstream watch, and why a breadcrumb cannot cover some of them, is decided in `source/drift.rs`'s header — not here, because that answer changes with the decoders.

- **`walkable.rs` is coherence-bound to this crate — it did NOT move with the sim-geometry cluster.** `WalkableMask` aliases `Grid<bool>` with inherent obstacle ops; the orphan rule pins an inherent impl to the crate owning the type. Wrapper struct and extension trait were adjudicated against — see the comment on the impl.

- **A daemon's runtime identity is the SOURCE's own wire fact, and for OpenClaw that is the resolved gateway PORT — not the profile, not the pid.** The profile is install-scope and 1:N with gateways; the pid churns the mascot per restart. `DaemonInstanceId` (stable) vs `DaemonPresence::current_pid` (rebound per incarnation) stay separate so a late exit receipt for a replaced process no-ops.

- **`GatewayDown` CREATES an absent instance; `PidExited` never does — the asymmetry is provenance, not a hole.** `gateway_stop` is a first-hand wire report; `PidExited` is synthesized locally, and creating on it resurrects a phantom (fbe26049) — hence the `current_pid == Some(pid)` guard. Don't harmonize the arms.

- **`SessionEnded` alone among the presence arms does NOT resurrect a Down gateway — don't harmonize it with its siblings.** The recorded clean shutdown is `gateway_stop` then `session_end` 2 ms later, so "any event ⇒ up" would undo the stop just announced; a real `session_start` still resurrects. Pinned both ways by `session_end_after_a_gateway_stop_leaves_it_down`.

- **A `gatewayPort`-LESS OpenClaw envelope falls back to ONE documented legacy instance instead of being rejected — deliberately.** Old installed plugins stamp no port; a hard reject would vanish every existing user's lobster. The fallback fires `drift::missing_field`; a PRESENT-but-unusable port still rejects. Don't tighten the absent arm without solving stale-plugin refresh.

- **The two pid→fan-out shells (`HookPidWatch`, the daemon's `PresenceExitWatch`) stay SEPARATE — do NOT hoist a generic `PidFanout<K>`.** The deep primitive is already shared (`source/exit_watch.rs::ExitWatch`); each shell is ~12 lines differing at every load-bearing point. The comment in `daemon/native.rs` records the call.

- **`HookPidWatch` arms the exit watch only when one agent reports the SAME pid twice IN A ROW — the one-event lag IS the guard.** A per-invocation wrapper shell can never be corroborated; acting on first sight synthesized a `SessionEnd` after EVERY hook (#896). **opencode deliberately arms on sight** — its `_pid` is the plugin's own `process.pid`. Pinned by `a_batch_yields_one_bind_target_per_agent`.

- **The `native` (default) feature gates the ASYNC SOURCE RUNTIME, not the decoders — and the gates are MODULE-level.** `default-features = false` leaves the wasm32-clean decode/reduce core; decoders + registry stay always-compiled. The `harness` feature keeps `Drive` out of the shipped lib; core dev-depends on ITSELF by path (rust-analyzer#20836) — don't "clean it up" into a normal dependency, that ships the harness.

- **The per-CLI home resolvers MIRROR each CLI's own; the axes deliberately NOT mirrored are listed in #880** — each audited against the shipped artifact (an unmirrored axis is fail-silent). Don't "fix": copilot's `--config-dir` + XDG dirs; hermes profiles (upstream `hermes-agent#18594`); CC NFC / grok `dunce` normalization. Whitespace overrides read as unset; a RELATIVE override warns and passes through. Re-run the probe matrix when a closed CLI majors.

- **The hook-wins dedup is asymmetric BY EVENT KIND (#150).** A hook End suppresses both JSONL kinds; a hook Start never suppresses a JSONL End — when the best-effort PostToolUse hook drops, that End is the only completion signal, and eating it leaks `active_tasks` all session. Kind lives in the map VALUE; kind-in-the-key regresses `late_batched_jsonl_pair_after_delivered_hook_end_is_fully_dropped`.

- **That dedup is also ONE-directional — a JSONL-first tool inflates `tool_call_count` by 1, cosmetically.** The count feeds only the hover tooltip; a symmetric reverse-dedup costs a hot-path map write for a tooltip metric. Don't also count in the JSONL arm — that IS the double-count.

- **The `active_tasks` insert in `track_active_tasks` is not slot-gated, and that's HARMLESS — don't add a `contains_key` guard.** An orphan entry eats nothing (every reader is slot-guarded); `tick` reaps it within ~1s; gated vs ungated is byte-identical empirically (#612).

- **The JSONL watcher's DECODED-event send is blockable, unlike the hook path's `CONN_TIMEOUT` — deliberate.** Dropping a decoded delta would desync the cursor's tail-follow; worst case is delayed detection, self-recovering. The upstream PATH channel IS bounded via try_send-drop (#585). Don't add a send-timeout on the decoded side.

- **CC keys on the session UUID, not the transcript path.** Hook `IdKey::SessionId` == watcher `cc_id_from_path` (filename stem) == `detect_parent_id`'s path component — cwd-split-safe. Route new CC keying sites through `cc_id_from_path`, Antigravity sites through `id::normalize_path_key` (#197).

- **CC hook `transcript_path` always points to the PARENT'S transcript**, even when a subagent acts — the reducer suppresses hook activity inside a `Task`; child attribution arrives via the per-subagent JSONL. CC's `SubagentStart`/`SubagentStop` hooks register/end the child (#241) — Stop is the ONLY end signal a Workflow-fleet subagent gets, keyed on `cc_id_from_path(agent_transcript_path)`.

- **The subagent-dispatch tool is detected SEMANTICALLY — by `subagent_type` in the input — with the name `Agent` only as fallback.** CC renamed `Task`→`Agent` undocumented (v2.1.63), silently breaking name-only matching; an unrecognized name still works and logs `unknown_dispatch`. Don't revert to name-only matching.

- **Liveness flows UP the subagent tree** (`scope::refresh_lineage`): activity refreshes the actor and every ancestor, and `has_waiting_ancestor` exempts a child blocked under a Waiting parent. Don't drop the up-refresh from the suppress block: one >10-min delegation would evict the working subtree.

- **The JSONL first-sight gate runs on EVERY path a file is first seen — seed, rescan, poll, notify — not just startup (#85).** `walk_jsonl::should_seed_at_eof`: outside 1h OR an end marker in the tail ⇒ cursor parks at EOF, no `SessionStart`; symlinked entries refused wholesale. Gating only the initial seed once let a rescan resurrect a stale session — that unification IS the #85 fix.

- **"Recent" is the source's own ACTIVITY clock where it has one, not the file mtime.** CC writes metadata sidecars into OTHER, dead sessions' transcripts (bumping mtime), so CC's `ActivityRecency` verdict is three-state: `At(secs)`, `Unknown` (keep mtime), `SidecarOnly` — which GATES, because parseable-but-turn-less is EVIDENCE of a metadata write (an `Option` collapsed this and re-admitted the ghost). An unrecognized line is sidecar evidence UNLESS it carries the turn payload.

- **A liveness-probe vouch bypasses the gate's RECENCY half, NEVER its terminator half.** A vouch answers "is the process alive", not "is this session over" (omp's fd vouch fires for a tool merely READING an old transcript); a structural end marker outranks any proxy. Probes attach only under that CLI's first-party layout, so fixture replays keep pure-mtime behavior.

- **The exit ladder is instant (ms) > negative vouch (~60–120s) > ProofOfLife TTL + stale sweeps — and a probe FAILURE changes nothing.** `None` snapshot = enumeration failed → freeze state; `Some(empty)` = healthy "nothing alive"; missing from two healthy snapshots ≥60s apart ⇒ synthesized `SessionEnd` + un-claim (a resume self-heals). ProofOfLife only refreshes the reap exemption. Pinned by the `tests/watcher/liveness.rs` + `tests/reducer/liveness.rs` suites.

- **An OVERSIZED pending span (>1 MiB) is skipped to EOF, never replayed — but it still registers (#204) and rescues in-flight Task dispatches (#222).** The tail is scanned for UNMATCHED Task dispatches, which re-emit; everything else is discarded and the terminator wins. Pinned by `oversized_attach_dispatch_outside_window_is_missed`.

- **A hook event from an UNKNOWN session id REGISTERS it — hooks are proof of life; JSONL events for unknown ids stay no-ops.** A hook only comes from a live process; a transcript line can be a historical replay, so JSONL never synthesizes (end-ish events included). `decode_hook_payload` attaches `Identity` ahead of activity so registration lands with real source/session_id/cwd. Pinned by the `hook_identity_*` suites in `tests/reducer/lifecycle.rs`.

- **A refused (desk-exhausted) hook registration must NOT record its `tool_use_id`** — the stale record would dedup-eat the JSONL `ActivityStart` that finally registers the session. Pinned by `refused_hook_registration_does_not_poison_dedup_for_the_later_jsonl_copy`.

- **A child's end is remembered by tombstone (5s) + child ledger (#244/#246), riding TWO deliberately different clocks.** Late parented re-starts are gated 90s; the entry is RETAINED 300s so a parentless revival ADOPTS the remembered parent — one shared clock re-registered a >90s-idle multi-turn child as an orphan. The multi-turn Codex child has no upstream SessionStart at turn N+1, so `jsonl/unclaim.rs` RELEASES (never removes) the claim on hook `SessionEnd{as_child}` and the next append re-registers + re-links. Residuals: `tests/reducer/child_ledger.rs` + `tests/watcher/unclaim.rs`.

- **CC has no durable JSONL exit record — and user-controllable content must never drive lifecycle.** The old content matcher false-positived on messages QUOTING `/exit`; `cc_session_ended` matches only structural markers (CC writes none today — the drift watch flags a change). The `SessionEnd` hook is the one clean-exit signal, best-effort; Ctrl-C falls to the exit ladder.

- **Codex Idle reaps at 5 min; CC keeps 30 — don't add a short CC reaper.** Registry-derived (`SourceCaps::short_idle_reap()` = `!has_exit_signal && resurrects_on_prompt`): safe only where the false-positive self-heals on the next prompt; CC's clean exits already signal via the hook; a vouched slot is exempt (#220). Pinned by `codex_idle_agent_reaps_faster_than_claude_idle`.

- **The b1 Task-drain cascade is grace-DEFERRED (2.5s, #151), and the chained-dispatch linger is unbounded — don't restore immediacy.** A parallel second dispatch is only visible via its lagged JSONL copy; an immediate cascade would evict that LIVE subtree unrecoverably. Any Task insert inside the grace cancels it; residuals accepted in #151.

- **A subagent registered AFTER its parent's cascade escapes it — deliberate, because `exiting_at` is NOT a terminal verdict.** REFUSING the registration breaks live-parent-false-exit re-adopt; INHERITING `exiting_at` is the b1 hazard verbatim. A durable fix must key on something outliving the slot (the child ledger); tracked, not patched here.

- **Resurrect-in-place starts from clean correlation state.** A `SessionStart` on an EXITING root cancels the walkout AND evicts the dead life's correlation entries (a leftover tuid would suppress every hook of the new life); the proof-of-life vouch deliberately survives.

- **Codex subagents are wired via the `SubagentStart`/`SubagentStop` HOOKS, not JSONL paths.** A Codex child's rollout is FLAT under `sessions/`, so the hooks are the only parent-link carrier. `SessionStart` ENRICHES an orphan's `parent_id` — never re-parents, refuses cycles (`scope::would_create_cycle`, #238). Wire pinned in `tests/sources/codex/mod.rs`.

- **`AgentSlot.state_started_at` is process-local `SystemTime` (pose timing — don't wall-clock-anchor it), and `SceneState`'s serde derive is NOT a wire contract** (#279: debug dumps + the insta golden only).

- **`ActivityState::Active` ≠ "tool is currently executing".** `ActivityEnd` only ARMS pending-idle; `tick` realizes Idle after `ACTIVE_GRACE_WINDOW` (1.5s). Visible Idle lags real Idle by up to ~2.5s — don't depend on instant flips.

- **grok leader-mode sessions are DELIBERATELY UNVOUCHED (#826) — the #638 socket vouch was deleted, not repaired.** `--leader` runs the agent in a shared process while `active_sessions.json` records only TUI-client pids, so a client detach reads as exit; session→leader ownership lives only in the leader's memory — no correct in-tree fix exists (upstream #839). Leader sessions ride the pre-#638 path (mtime gate + short-idle reap + prompt resurrect).

- **grok fires OTHER CLIs' hooks with GROK envelopes — the cross-fire guard drops them QUIETLY.** Duplicates arrive tagged `claude-code`/`cursor`; `decode_hook_payload` drops any `hookEventName` payload whose tag isn't grok, at trace level — else CC/cursor would Err per grok tool call.

- **grok's `spawn_subagent` defaults `background=true`, where `post_tool_use` fires at SPAWN — so `ToolDetail::Task` is minted ONLY for explicit `background:false`.** Treating a background spawn as a Task lets the b1 cascade evict the LIVE child ~2.5s after spawn (End == spawn). `rawInput` carries CLIENT-form keys — `spawn_is_blocking`'s both-keys read is load-bearing, not defensive.

- **The permission `Waiting` resolves on the gated tool's PostToolUse — and the gate (`gated_before_waiting`) has SEVEN clear sites whose conditionality is load-bearing.** Task-drain clears only the matching tuid; suppress-restore clears only a Task tuid; the RE-NOTIFY clear is skipped while already `Waiting` — CC follows `PermissionRequest` with the idle `Notification`, and clearing there loses the tool whose PostToolUse resolves the wait (`a_second_waiting_keeps_the_gate_the_first_one_remembered`). Keep the sites in sync if you touch eviction.

- **omp names its slots from the PATH and the TITLE, not the cwd** — omp subagents run IN-PROCESS and repeat the parent's cwd, so the default deriver gives a whole fan-out identical labels. `omp_derive_label` (wired via `with_label_deriver`) uses a nested transcript's STEM (`tasks[].name`); a root keeps the cwd basename because its stem is a timestamp+uuid. The auto-generated title then distinguishes concurrent roots through two required carriers: the decoder's `title`/`title_change` arm handles live lines and small first-sight replays; `omp_head_title` (wired via `with_head_label`) handles the bounded first-sight head. Without the head carrier, oversized roots skip the backlog and revivals read only the tail, so neither reaches line 1. **The empty-title guard is load-bearing in both carriers**: omp creates that fixed-width slot empty and leaves subagent titles empty forever; an unguarded rename blanks roots at birth and wipes subagent dispatch names.
