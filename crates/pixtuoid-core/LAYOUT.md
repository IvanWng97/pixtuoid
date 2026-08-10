# pixtuoid-core — annotated layout

The navigable skeleton is in [`CLAUDE.md`](CLAUDE.md); this is the same tree with each entry's full annotation.

```
src/
├── source/             Source trait, hook+jsonl decoders, listeners, SourceManager
│                       mod.rs (AgentEvent + agent_id(), Transport + the module-level
│                       `native` gates), native.rs (the `native`-only async transport seam: TaggedSender/
│                       TaggedReceiver + the Source trait [async via Send-bounded RPITIT] + its object-safety
│                       twin DynSource [boxed-future, blanket-impl'd — SourceManager's Box<dyn> currency;
│                       source authors never name it] — behind ONE gate in mod.rs, re-exported from
│                       `crate::source` so the pre-split paths didn't move),
│                       registry.rs (THE per-source fact table: SourceDescriptor{label_prefix, verified_version,
│                       version_probe, kind: SourceKind} — one row per source; doc(hidden), not public API.
│                       SourceKind is the TYPED discriminator: Agent{transcript: Option<Transcript{line_decoder,
│                       id_from_path, path_filter, cwd_extractor}>, hook{id_key,custom}, caps} vs
│                       Daemon{presence_decoder} — the Transcript bundle makes the four-fn pairing structural
│                       (transcript-bearing = every fn, hook-only = None; a half-populated row is unrepresentable).
│                       Consumers read via accessors is_daemon()/line_decoder()/id_deriver()/path_filter()/
│                       cwd_extractor()/hook()/caps()/presence_decoder() — never the enum shape. What earns a
│                       column is name-keyed DISPATCH (a caller that does NOT own the source picking by name),
│                       not consumer count: `id_from_path` (the ONE transcript→session-id derivation) and
│                       `path_filter` (WHICH .jsonl are this source's transcripts) are each read by BOTH the
│                       watcher (JsonlWatcher::new's defaults) and the offline harness. presence_decoder_for + daemon_sources
│                       drive the registry-driven daemon demux + per-daemon sweep, so a 2nd daemon = one row),
│                       daemon.rs (the SHARED, daemon-AGNOSTIC presence layer: DaemonPresenceUpdate vocabulary +
│                       DaemonInstanceKey{source, instance} + PresenceMsg{key,delta} + DecodedPresence{instance,
│                       updates} + PresenceTtl + apply_presence/sweep_presence_ttl/mark_presence_down; the
│                       instance-tagged PresenceExitWatch + PresenceSender live in the
│                       once-gated daemon/native.rs, re-exported — every daemon's state machine,
│                       keyed by (source name, DaemonInstanceId) in SceneState::daemons so N daemons AND N
│                       concurrent INSTANCES of one daemon (OpenClaw supports several isolated gateways per
│                       host) each own their state; the per-daemon WIRE decode — INCLUDING what makes two
│                       instances different — stays in the daemon's own module, e.g. openclaw.rs (the resolved
│                       gateway PORT). sweep/mark_down stay SOURCE-scoped and walk that source's instances (a
│                       disconnect is one Sources-panel row = every gateway); TTL decay is instance-LOCAL.
│                       The instant abrupt-down (PidExited) rung arms
│                       on GatewayUp{pid} (restart-rebind) AND on PidSeen{pid} (#318 FIXED): the plugin stamps
│                       _pid on EVERY event, so a MID-ATTACH / reconnect-while-alive adopts current_pid off the
│                       next event (apply_presence's None-only adopt; driver arms the watch on PidSeen too).
│                       The 4th state DaemonState::Degraded (#317): agent_end failure → RunFailed →
│                       Degraded (sickly-red sluggish the lobster), healed by the next clean RunEnded / new RunStarted /
│                       GatewayUp restart. `success:false` alone is NOT that failure: upstream builds it as
│                       `!aborted && !promptError` (both construction sites, shipped 2026.7.1), so a user CANCELLING a
│                       turn reports it too — and Degraded is sticky (the TTL sweep deliberately never heals it), so an
│                       abort would latch "model error" until the next run. The plugin forwards `errored` — the mere
│                       PRESENCE of upstream's `error`, as a bare boolean because the string can embed prompt content —
│                       and the decoder degrades only on `!success && errored`. An ABSENT `errored` defaults to TRUE
│                       (an older on-disk plugin keeps the pre-change behaviour rather than becoming un-degradable —
│                       the same legacy direction as the gatewayPort-less envelope). `DaemonPresence` STORES only the orthogonal axes `liveness:
│                       DaemonLiveness{Up{degraded}|Down}` + `in_flight_runs` (#460 — a run_key→last-observation
│                       MAP, so each run expires on its OWN clock: the daemon-wide `last_seen` is refreshed by
│                       ANY event, so a dropped agent_end used to latch Busy for the gateway's whole life on a
│                       gateway that kept serving other traffic): Busy/Idle are NOT
│                       stored — they're PROJECTED by `DaemonPresence::display_state()` (in state/mod.rs), the ONE
│                       place the Degraded>Busy>Idle priority lives (degraded checked BEFORE the run set, so a
│                       fan-out-with-one-failure renders Degraded not Busy). apply_presence mutates axes; every
│                       renderer reads `display_state()`/`is_busy()`, never a stored `state` — so Busy can't drift
│                       from the run set (the 4-site hand-sync is gone)),
│                       decoder.rs (shared utils + decode_hook_payload, a registry-driven dispatcher;
│                       short-circuits is_daemon() → zero AgentEvents),
│                       drift.rs (structured decode-drift breadcrumbs: unknown_event/missing_field/
│                       unknown_dispatch/shape_drift, each a tracing::warn! on the stable target
│                       `pixtuoid::drift` — see "Keeping the decode mapping current" defense #2.
│                       Their free-form values are made display-safe AT EMISSION via
│                       decoder::display_safe (Cc + the Cf bidi set + the MAX_DECODED_FIELD_CHARS
│                       cap): in every non-TUI mode `tracing` writes to RAW stderr, a terminal sink
│                       no cell buffer clips and no presenter sanitizes),
│                       hook/ (HookSocketListener facade — mod.rs shared generic handle_conn over AsyncRead [ONE framing/decode path for both transports] + the registry-driven DAEMON demux (presence_decoder_for → source-tagged sibling channel, then `continue` — never the agent arms); router.rs (HookRouter — the Source that OWNS the one shared socket every source's hooks ride; it is the #246 tee PRODUCER + the daemon-presence demux host + the HookPidWatch attach; being a Source, its fatal bind surfaces via spawn_with_health #157 for free); unix.rs UnixListener [liveness arbitration is an EXCLUSIVE advisory lock on a sibling `<sock>.lock`, held for the daemon's lifetime and NEVER unlinked — bind try-locks it: a live owner ⇒ the typed SocketBusy, which HookRouter::run catches and degrades the hook plane to a quiet Ok(()) (warn, NO SourceDeath — hooks stay with the owning instance; the transcript watchers run as independent tasks); lock acquirable ⇒ the previous owner is dead ⇒ the socket file is residue and is reclaimed. connect() errnos NEVER decide liveness for a lock-holding owner (a backlog-saturated LIVE daemon yields ECONNREFUSED on macOS / EAGAIN on Linux — the kernel note in pixtuoid-hook/tests/shim.rs); the belt-and-braces post-lock probe only protects a lock-LESS live listener (older pixtuoid mid-upgrade / squatter), and INSIDE that probe non-WouldBlock errnos still decide reclaim — so a saturated lock-less owner can be stolen from during the mixed-version window, an accepted residual that ages out once every daemon holds the lock. Every OTHER bind error stays fatal → SourceDeath. The socket binds at a temp name, chmods 0600, then atomically renames onto the final path — NO process-global umask mutation (it raced other tokio workers' file creation)], windows.rs named-pipe accept loop — a second instance's CreateNamedPipeW fails ACCESS_DENIED against the owner's `first_pipe_instance` and maps to the SAME typed SocketBusy → same hook-plane degradation; pipe uses owner-only SDDL `D:P(A;;GA;;;OW)` [umask-0700 equivalent; the kernel copies the descriptor at each CreateNamedPipe]; a failed connect is NOT a reusable instance, so windows.rs recreates the server after each connect error; pid_watch.rs (HookPidWatch — the hook-only sources' `_pid`→ExitWatch abrupt-exit rung; see the multi-source-decoding entry)), jsonl/ (the JsonlWatcher as a directory module — mod.rs: JsonlWatcher + builders/test seams + the run() select loop + WatchCtx/SourceDecoders + the public re-exports [crate::source::jsonl::{JsonlWatcher, ProbeSnapshot, ChildEndUnclaims, …} paths are unchanged]; walk.rs: walk_jsonl + the unified first-sight gate should_seed_at_eof + scan_root/emit_first_sight/scan_pending_tasks + detect_parent_id/extract_cwd; liveness.rs: the whole probe ladder — ProbeSnapshot/LivenessProbe, the `ProbeLadder` OWNER (the vouch hysteresis + the pid→ids bindings + rebind migration under one struct, a PURE `fold`→`ProbeOutcome` failure-detector — functional core) applied by its imperative shell refresh_probe_snapshot, the instant-exit `pid_died`, emit_session_exit, revouch_gated_files, emit_proof_of_life; unclaim.rs: ChildEndUnclaims + drain_child_end_unclaims [#246]; health.rs: FailureLatch; tests.rs: the sibling unit-test module),
│                       claude_code.rs / codex.rs / antigravity.rs / copilot.rs / omp.rs / grok.rs (per-source PURE decode + label
│                       fns; each mixed module's runtime half — its impl Source, watcher wiring, probes — lives
│                       in a once-gated `native` sub-module, source/<cli>/native.rs, re-exported from the parent
│                       so `source::<cli>::*Source` paths are unchanged;
│                       ClaudeCodeSource is now a PURE JsonlWatcher — the shared hook socket lifted to HookRouter,
│                       so CC keeps only its projects_root + the child_end_unclaims CONSUMER + the liveness probe),
│                       cc_probe.rs (the CC `~/.claude/sessions/<pid>.json` registry probe: live_cc_session_ids
│                       [re-exported at `claude_code::live_cc_session_ids`] + RegistryParse/parse_registry_entry +
│                       the pid-start identity check + cc_sessions_dir),
│                       reasonix.rs / codewhale.rs / opencode.rs / cursor.rs / hermes.rs / kimi.rs (HOOK-ONLY: hook-payload decoder,
│                       no Source impl — no watchable JSONL (kimi's `wire.jsonl` exists but is deliberately unwatched: explicitly unstable format);
│                       codewhale is cwd-keyed because its session_id is
│                       inconsistent across events, opencode is ses_*-keyed via a TS plugin, cursor + hermes are session_id-keyed (hermes with a cwd fallback),
│                       kimi is session_id-keyed off a CC-SHAPED envelope so it rides the shared hook arms),
│                       manager.rs (SourceManager::spawn / with_source / spawn_with_health —
│                       publishes SourceDeath on a watch channel so the binary can surface a
│                       fatal source exit in the TUI footer, #157; plain data, invariant #1 holds)
├── state/              SceneState + Reducer (event coordinator: Transport-tagged dedup, the
│                       cross-slot active_tasks/gated_before_waiting correlation, the sweeps) +
│                       fsm.rs (Layer-A per-agent transitions) + scope.rs (Layer-B parent↔subagent tree) +
│                       correlation.rs (the SEVEN reducer-private correlation maps as one Correlation
│                       struct owned by the reducer — entry types, TTL consts [re-exported at the
│                       state::reducer:: paths], freshness predicates, one gc(now); intra-layer
│                       bookkeeping extraction, DECISIONS stay in reducer/mod.rs);
│                       GlobalDeskIndex / FloorLocalDeskIndex newtypes encode the two desk-index
│                       spaces (AgentSlot.desk_index is GLOBAL; the typed bridge to a floor's
│                       home_desks is floor_local_desk(), or the documented single_floor_local()
│                       identity for a single-floor scene)
├── sprite/             .sprite parser, pack.toml loader, blit_frame blitter, Pack::merge_from
│                       + blit_frame_scaled — the integer nearest-neighbour twin, the FALLBACK
│                       for art authored at a lower density than the buffer is painted at
│                       (a 1x pack fills a scaled render; a pack authored AT the render scale
│                       blits through blit_frame untouched, so richer art REMOVES the upscale
│                       rather than fighting it). Not `image::imageops::resize`: that pulls a
│                       large dep into the headless core for a doubled loop AND cannot
│                       composite transparency into an existing buffer, which is the job.
│                       `scale: NonZeroU16` makes a paint-nothing zero unrepresentable; the
│                       layout↔buffer MEANING stays up in `pixtuoid_scene::render_scale`.
│                       DENSITY VARIANTS are the other half: a pack may ship `<piece>@<N>x`
│                       (`density_variant_name`/`split_density_variant`/`DENSITY_VARIANT_SEP`)
│                       — the same piece drawn on an N-times grid, following the prevailing
│                       asset convention (`@2x`/`@3x` Apple, `scale-200` Windows). The SCALE is
│                       in the name because a name saying only "denser" can't express a pack
│                       shipping BOTH a 2x and a 4x variant, and leaves the file's meaning
│                       dependent on whichever render scale measures it. Variants are legal by
│                       DERIVATION from OPTIONAL_FURNITURE_ANIMATIONS, never their own rows
│                       (`is_optional_furniture_animation`) — a second list forgotten fails
│                       quietly in its least visible direction: the variant loads for the
│                       bundled pack but `merge_from` never inherits it, so only `--pack-dir`
│                       users drop back to the upscale. `merge_from` therefore iterates what
│                       the BASE pack HAS (the density axis is open — the registry names
│                       PIECES, not the grids each may be drawn on). The name CLAIMS a density
│                       and the frame size PROVES it: `validate_pack_animations` reports a
│                       `DensityMismatch` as a hard ERROR, because otherwise the claim is only
│                       ever tested by whichever renderer looks for that density — silently, at
│                       paint time, on someone else's terminal. `1x` and `+4x` are rejected:
│                       this string is a lookup KEY, and two spellings of one density is two
│                       files a renderer picks between arbitrarily
├── platform.rs         cross-platform home-dir resolution (user_home(), USERPROFILE-first on
│                       Windows — HOME is unset there and Git Bash's HOME is POSIX-form) +
│                       codex_home() (honors CODEX_HOME when it points at an existing dir, else
│                       <home>/.codex — codex's own precedence; routes BOTH CodexSource::
│                       default_paths and the installer's config.toml path, so they can't disagree) +
│                       home_first_dir() (HOME-FIRST then USERPROFILE on Windows — the OPPOSITE of
│                       user_home(); the shared resolver for the CLIs that hand-roll $HOME-first home
│                       resolution: CodeWhale (config::effective_home_dir) + OpenClaw (infra/home-dir.ts
│                       resolveRawOsHomeDir). A HOME-exporting Windows shell (Git Bash/MSYS2) would
│                       otherwise have the installer write hooks where the CLI never reads →
│                       installed-but-no-sprite. Installer-only consumers; every OTHER CLI uses a
│                       USERPROFILE-first stdlib home so they correctly use user_home())
├── grid.rs             Grid<T> — a width×height row-major Vec<T> with checked get/set/get_or
│                       (the ONE y*w+x indexing + edge-clamp; WalkableMask = Grid<bool>, ReachSet wraps one;
│                       sprite::Frame = newtype(Grid<Pixel>) + sprite::RgbBuffer = newtype(Grid<Rgb>), Deref to
│                       Grid so .width/.height/as_slice are transparent — but RgbBuffer keeps its OWN inherent
│                       unchecked get/put (shadow Grid's checked get via Deref) for the blit hot path; don't
│                       "simplify" them to Grid::get)
├── harness.rs          `harness` FEATURE (non-default, dev-only — absent from the published crate): the ONE
│                       offline decode→reduce driver every test/tool that feeds real wire bytes rides
│                       (Drive::transcript_at/transcript/hooks → Driven{scene, events, lines, unparseable,
│                       seed_events, decode_errors, panics, reached}). Its four shells are tests/sources/conformance.rs,
│                       pixtuoid/tests/wire_to_pixels.rs, examples/decoder_fuzz.rs and
│                       pixtuoid-scene/examples/corpus_check.rs. Every line runs under catch_unwind (the
│                       never-panic contract, now inherent rather than fuzz-only); failures report the line's
│                       SHAPE, never its content
├── id.rs               AgentId + from_parts/from_transcript_path (moved out of source/mod.rs in the
│                       #350 smell-audit; source/mod.rs keeps AgentEvent + agent_id())
│                       + normalize_path_key (moved here from source/decoder.rs — it's an identity-key
│                       canonicalization shared with the pixtuoid-scene palette's cwd outfit key, so the
│                       render layer doesn't depend on the decoder layer for it)
├── walkable.rs         WalkableMask = Grid<bool> (static obstacle mask) + OccupancyOverlay (dynamic per-frame).
│                       STAYED here when the sim-geometry cluster moved to pixtuoid-scene: the mask is an
│                       ALIAS whose obstacle ops are an inherent `impl Grid<bool>`, and the orphan rule pins
│                       that impl to the crate owning Grid — see SHARP-EDGES.md
└── tests/              one integration test per concern
```

**Burn-tier plumbing (model flame):** `AgentEvent::ModelInfo { model, effort }` carries RAW wire strings (interpret-at-paint — the tier tables live in `pixtuoid-scene::burn`): CC assistant lines' `message.model` (per turn, filters `<synthetic>`) + TWO CC effort channels — the HOOK payloads' documented `effort.level` (low..max verbatim, on every tool-context hook; ultracode reports as `xhigh` — the PRIMARY channel, decoded in the shared arms) and the transcript's periodic `ultra_effort_enter`/`ultrathink_effort` attachment markers (no wire value → decoder-synthesized "ultra"/"ultrathink"; the `ultra_effort_exit` twin synthesizes the NON-max `ULTRA_EXIT_LABEL` ("ultra_exit"), so last-seen-wins kills the flame instantly, the TTL only backstops a missed exit; the sentinel is display-suppressed by `burn::fresh_effort`, so the dossier never renders it) — plus the SessionStart hook's optional `model` field; Codex `turn_context` model+effort verbatim; copilot per-tool `data.model` (attributed to the ACTING agent); opencode `session.created` `info.model.id`; omp assistant messages' bare `model` (#545 — the separate `provider` field and the provider-prefixed `model_change` entry are deliberately NOT decoded: the bare field matches TOP_MODELS' prefix vocabulary and every turn re-stamps it). The reducer caches `slot.model` (last-seen-wins — a mid-session `/model` switch tracks) + `slot.effort: EffortObservation{value, seen_at}` (re-stamped per sighting; the scene's EFFORT_TTL_SECS turns Codex's per-turn field and CC's periodic marker into ONE freshness semantic). Unknown id = no-op (a model line never registers a session); legitimate on BOTH transports (wire data, not liveness). Bounded residual: recent/live-probed files replay from the TOP on first sight, so a historical effort marker reads fresh for up to EFFORT_TTL_SECS (10 min) after attach before decaying — cosmetic, accepted. Both fields serde-skipped.

**Token-meter plumbing (#632, the desk paper tower):** `AgentEvent::Usage { fresh_tokens }` carries a per-reading FRESH-spend delta (new input + cache WRITES + output — cache READS are re-served context, ~95% of the raw total, excluded at decode; zero readings skipped). The ModelInfo posture throughout: interpret-at-paint (tier ladder + sheet-fall window live in `pixtuoid-scene::token_meter`), unknown id = no-op (usage never registers a session), counters-only in the reducer (`tokens_used` saturating + `last_usage: Option<UsageObservation>` — no liveness/`last_event_at` refresh). JSONL-only (no hook carries usage → never enters hook-wins dedup). Wires: CC assistant `message.usage` (sidechain lines included — the meter is the SESSION's total burn, live-calibrated 2026-07); Codex `token_count` `info.last_token_usage` (the per-turn reading, NEVER the cumulative `total_token_usage` twin — the reducer accumulates deltas; codex `input` INCLUDES the cached share so fresh input = input − cached, saturating; reasoning is additive); omp assistant `message.usage` (per-turn; `input` EXCLUDES cache — fixture-verified `totalTokens = input + output + cacheRead + cacheWrite` — so fresh = input + cacheWrite + output); copilot's ONE usage wire is the `session.shutdown` summary (#645) — decoded as a final Usage AFTER the SessionEnd in the same vec. The flash-safety is the PAINTER's exiting filter (an exiting desk paints no tower/sheet), not the event order — the counter lands on the slot either way (cascade_exit keeps the slot for the GC window); SessionEnd-first just makes the one theoretically observable intermediate frame an already-exiting slot (defense-in-depth). Payoff: an honest dossier Σ on the walk-out hover. `tokenDetails.input` already EXCLUDES cache reads (arithmetic pinned by the copilot shutdown usage test). The other 8 sources genuinely carry no usage wire; their desks never grow paper. Attach-time residual (deliberate): an OVERSIZED (>1 MiB) in-flight transcript first-sights with NO backlog replay (#204's bounded startup — identity from a head read, cursor at EOF), so for such a late attach `tokens_used` counts only post-attach burn — the tower reads "burn on my watch". Accepted: full replay would read N×10 MB files at boot, and the normal flow (the watcher running as sessions start) is complete from birth. Caught by the #632 live dogfood against a 13 MB session; do not "fix" by replaying the backlog. Both slot fields serde-skipped at zero/None (`tokens_used` flat like `tool_call_count`; the last reading bundled as `UsageObservation{delta, seen_at}` — the `EffortObservation` pattern, a half-stamped reading unrepresentable).

**Focus-jump plumbing (#focus-jump):** the shim fills `_pid` (an ancestor walk past the runner's shell) into the hook envelope WHEN ABSENT — opencode's plugin and CodeWhale's env-mode supply their own, which win; the daemon's `handle_conn` peeks `_pid` UNCONDITIONALLY (an exit-watch backend failing to init — pre-5.3 Linux — must not take the focus pid cache down with it; only the `HookPidWatch` BIND needs the watch) and stamps it onto the batch's `Identity` events (`patch_identity_pids` — the per-source decoders never see the key). The stamp is a `PidIdentity` — pid + the kernel start MARKER read at peek time (`source::pid_start_marker`: macOS `pbi_start_tvsec`, Linux `/proc` stat field-22 raw ticks — EQUALITY-only, no epoch conversion, which is why #220's macOS-only limitation doesn't apply) — so the binary's click re-reads the marker and REFUSES a recycled/dead pid (#527; markerless stamps skip the check, additive). The reducer caches it on `AgentSlot.pid` (fill at registration, refresh per Identity, `Some` never downgraded), serde-skipped so the scene golden doesn't churn. WHICH channel a source rides is the registry's **`FocusChannel` capability** (`ShimStamp`/`PluginStamp`/`TranscriptProbe`/`Unsupported`, a field of `SourceKind::Agent` so daemons structurally can't carry one) — the ONE data-driven truth shared by the stamp gate (`patch_identity_pids` stamps iff `accepts_stamp()`), the click-time probe dispatch (`focus::resolve_pid`), and the doctor report. It is DATA-only on purpose: the const table compiles to wasm, so the native-only probe FNS stay in the binary, pinned to the enum by the `transcript_probe_sources_all_have_a_resolve_arm` lockstep test. `TranscriptProbe` (CC/Codex) is never stamped — their getppid is the hook-command parent, never recycle-guarded, and a stamped stale pid would shadow the probe in `resolve_pid`. Their channel is the recycle-guarded probes, exposed as the two pub point-query seams `source::cc_pid_for_session` (projects root → sibling sessions registry) and `source::codex_pid_for_session` (rollout UUID). **Windows now resolves a pid too — by WALKING, not by getppid** (#528): a raw getppid there names the transient `cmd /C` the hook runner interposed, so the shim walks past it instead (`pixtuoid-hook`'s `cli_pid` — one Toolhelp32 snapshot, skip `cmd`/`powershell`/ `pwsh`, stop at the first ancestor that is the CLI; a parent already absent from the snapshot yields NO stamp rather than a maybe-recycled pid). That stamp is safe to trust because the marker landed with it: `pid_start_marker` reads the creation `FILETIME` on Windows, so the #527 click-time recycle check is live there rather than inert. A plugin-stamped pid (opencode's `process.pid`) still wins where it exists — the peek needs no exit-watch, whose backend is absent on Windows/pre-5.3 Linux, which is also why an abrupt exit can't set `exiting_at` on those platforms and the click-time marker re-read is the guard that carries it. Two limits are worth stating exactly, because the obvious phrasing of each is wrong. The skip list is by NAME, so a runner interposing some OTHER shell still stamps a transient pid — that degrades to the pre-#528 behaviour rather than a wrong window, but the thing that saves it is the WALK, not the marker: a dead pid owns no window, so `focusable` is false and there is nothing to activate. Do NOT restate that as "the marker refuses it" — Windows documents a process HANDLE as valid after termination, so a marker read is not a liveness proof there the way a macOS/Linux one is; the guard this arm actually earns is the RECYCLED-pid half (a reused pid belongs to a process with its own creation time). Two residuals neither half catches. A stamp the daemon could NOT read a marker for — the named process already gone when it decoded the line, or an elevated CLI it may not open — skips the identity check entirely, so that pid is cached unguarded; the walk from a recycled one normally dead-ends on a non-focusable chain, but a wrong window is reachable. And a parent that exits and has its pid recycled between the shim's snapshot and the daemon's marker read — a window bounded by socket delivery and daemon scheduling, NOT by the shim's send bound — is stamped from the impostor and matches itself.
