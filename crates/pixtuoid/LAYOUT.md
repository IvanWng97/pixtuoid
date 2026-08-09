# pixtuoid binary — annotated layout

The navigable skeleton is in [`CLAUDE.md`](CLAUDE.md); this is the same tree with each entry's full annotation.

```
src/
├── main.rs             entry point — arg-parse + dispatch + env glue ONLY (color/truecolor
│                       preflights, build_run_config, warn_broken_installs; config/install
│                       failure eprintlns pre-altscreen). The crash hook, logging bootstrap,
│                       and sources-CLI presenters are BIN-CRATE modules it declares
│                       (crash.rs / logging.rs / sources_cli.rs — pub(crate), same src/ dir
│                       as the lib but NOT in lib.rs; all three codecov-excluded like main.rs)
├── crash.rs            install_crash_hook — panic hook → terminal restore, timestamped
│                       backtrace appended to ~/.cache/pixtuoid/crash.log, pre-filled GitHub
│                       issue URL (percent-encode / char-boundary-truncate helpers unit-tested)
├── logging.rs          log routing (#157): logging::init installs the ONE tracing subscriber —
│                       TUI/floating mode ALWAYS file-logs at a warn floor
│                       ($PIXTUOID_LOG > $XDG_STATE_HOME/pixtuoid/log > ~/.cache/pixtuoid/log,
│                       one-deep rotation at 5MB by APPENDING .old; RUST_LOG, --log-level
│                       debug|trace, or $PIXTUOID_LOG raise verbosity — plain --log-level info
│                       is indistinguishable from the default and floors to warn); non-TUI
│                       modes log to stderr; log_file_path is the shared path authority
│                       (doctor dispatch + sources_cli + RunConfig.log_path read it).
│                       `open_private_append` is the ONE opener this log AND crash.rs's
│                       crash.log share: 0600 file under a 0700 dir on Unix (they carry
│                       transcript paths / AgentIds / at debug the agent's own tool
│                       commands — the settings.json rationale applies verbatim), with a
│                       best-effort fchmod so an older version's 0644 sink is tightened.
│                       The two mechanisms are SEPARATELY callable on purpose:
│                       create_owner_only_append (the race-free create-time mode) and
│                       install::io::tighten_to_owner_only (the upgrade fchmod, shared
│                       with the lock sidecar — one definition of the 0600 policy). Folded
│                       into one opener, reverting EITHER half was invisible to both mode
│                       tests, because the fchmod repaired what the create mode failed to set
├── cli.rs              clap subcommands (run / floating / validate-pack / init-pack / doctor / sources / connect /
│                       disconnect / setup / completions / man). The OLD install-hooks/uninstall-hooks CLI stays deleted
│                       (#284 removed the interactive ORCHESTRATION — plan_targets/interactive_pick); `connect <ids>`/
│                       `disconnect <ids>`/`sources [set <ids>]` are the SCRIPTABLE surface (Raycast/automation), a second
│                       presenter over crate::sources (see the scriptable-vs-interactive sharp edge); `setup [--yes]` is
│                       the headless onboarding twin (dry-run preview / apply); the in-TUI Sources panel (`s`) remains the
│                       INTERACTIVE one. `completions <shell>` (clap_complete) + hidden `man` (clap_mangen) emit to stdout
│                       from the SAME derived Cli tree as `--help` (homebrew `generate_completions_from_executable` / `man`
│                       capture them); main.rs dispatches both as plain arms (tracing → stderr, so stdout stays clean). Every
│                       PathBuf arg carries `value_hint` so completions path-complete (currently six: the four
│                       flattened SourceArgs flags + validate-pack's dir + init-pack's dest). Presenters live in
│                       sources_cli.rs (run_sources_list/run_sources_set/run_change/run_setup, codecov-excluded like doctor::run)
├── term.rs             truecolor preflight — does NOT guess from a $TERM name allowlist; ASKS the terminal (#397).
│                       `query_truecolor(timeout)` (the IO seam, cfg(unix), codecov-excluded): opens `/dev/tty`,
│                       raw-modes it (RAII `TermiosRestore`), writes the DECRQSS probe (`ESC[48;2;1;2;3m ESC P$qm ESC\\
│                       ESC[0m` — set unlikely 24-bit bg in the SEMICOLON form crossterm emits, query SGR back, reset),
│                       reads the reply via `libc::select` (NOT poll — macOS `poll()` returns POLLNVAL on tty/pty fds,
│                       found by PTY dogfood) until the `ESC\\`/BEL terminator or the budget, then `parse_decrqss_truecolor`
│                       (PURE, unit-tested):
│                       Some(true)=our RGB triple echoed back, Some(false)=valid-but-downsampled, None=`0$r`/empty/timeout.
│                       The pure policy pieces: `warn_zone(cmd_is_run_tui, is_tty, colorterm, suppress_env)` (the cheap
│                       pre-gate — only QUERY when this holds; truth-table tested) + `colorterm_is_truecolor` (an explicit
│                       positive that SKIPS the round-trip — the terminal declaring itself, not a guess) +
│                       `truecolor_warn_suppressed($PIXTUOID_NO_TRUECOLOR_WARN`, truthy `1`/`true`/`yes`/`on`) +
│                       `terminal_diagnostic_row(term, colorterm, probe)` (the `doctor` `terminal:` line; names HOW it was
│                       determined — COLORTERM / terminal query / downsamples / unknown). main.rs WARN-ONLY (never gates on
│                       Unix): `warn_zone(..) && query_truecolor(..) != Some(true)`, env/tty reads INLINED at the excluded
│                       call site. `doctor` runs the query ONLY when stdout is a tty (piped `doctor > file` neither emits
│                       escape codes nor probes — also why the test harness, output captured, never probes). Windows
│                       hard-gates VT separately (tui/mod); `query_truecolor` is a `None` stub there. `floating` is exempt
│                       (softbuffer = real RGB px). **Sharp edges:** a truecolor terminal that doesn't answer DECRQSS (rare)
│                       false-positives → the escape hatch covers it; a very-laggy reply past the 100ms budget could leak a
│                       few bytes to the TUI's stdin (accepted, rare). The query is the authority — there is NO $TERM/
│                       $TERM_PROGRAM allowlist to keep current (deleted on purpose; that was the "magic variable" smell).
│                       SEPARATE axis (color ON/OFF, not depth): `color_preflight(no_color, clicolor_force, term)` →
│                       `ColorPreflight` {Proceed / ForceColor / RefuseNoColor / RefuseDumbTerm}. The office is 24-bit with
│                       NO legible monochrome fallback, so when color is disabled we REFUSE the canvas + explain (mirrors the
│                       Windows VT hard-gate) instead of rendering block-soup. Precedence: `$TERM=dumb` first (can't render
│                       escapes at all — a force can't fix it), then NON-EMPTY `$NO_COLOR` (crossterm strips our SGR to a bare
│                       reset — VERIFIED empirically) UNLESS `$CLICOLOR_FORCE` (bixense `!= 0`) overrides it (precedence →
│                       `ForceColor`; main.rs MUST call `crossterm::style::force_color_output(true)` itself — crossterm
│                       honors `$NO_COLOR` but NOT `$CLICOLOR_FORCE`, also verified). Empty `$NO_COLOR` is ignored (matches
│                       crossterm — the thing that strips); `$FORCE_COLOR`/`$CLICOLOR` are deliberately NOT read (crossterm
│                       keys only on `$NO_COLOR`, so they'd no-op the render). Gated to the `run` TUI only (--headless/doctor/sources are plain
│                       text; floating = softbuffer). `color_status_row(pf)` is the `doctor` color line (reuses the SAME
│                       policy so the diagnostic matches `run`; doctor also SKIPS the DECRQSS probe under `$TERM=dumb`).
│                       **Sharp edge:** tmux (#4034) doesn't implement DECRQSS, so a truecolor tmux can false-positive the
│                       depth warn — `$PIXTUOID_NO_TRUECOLOR_WARN=1` covers it (tmux usually sets `$COLORTERM`, skipping the
│                       query entirely anyway).
├── setup.rs            first-run detection for onboarding: the PURE `is_first_run(cfg, path, load_degraded)` —
│                       `!load_degraded && (!path.exists() || cfg.sources.is_empty())`; a degraded load (malformed
│                       config, main passes `!cfg_warnings.is_empty()`) is NEVER a first run — don't replay
│                       onboarding over a real config. Matches resolve_connected's plain default (empty [sources] =
│                       nothing connected since 0.12.0), so onboarding IS the re-connect path for a pre-0.8 upgrader
│                       whose config predates the flags; unit-tested. `pub`
│                       because main.rs (a separate crate) computes RunConfig.first_run from it. The cinematic overlay
│                       lives in tui/welcome + widgets/welcome; the headless `setup [--yes]` presenter is sources_cli::run_setup
├── sources.rs          the TUI-free source-control CORE (detect/connect/disconnect/reconcile_to/status + the
│                       SourceStatus AND OutcomeRow serde DTOs = the two Raycast --json wire contracts, each pinned
│                       by a byte-shape test + a committed-schema golden → `just gen-contract`; OutcomeRow is
│                       {id, outcome, message?} — a bare token + optional failure detail, see SHARP-EDGES.md's entry). connect/disconnect
│                       are the PERSISTED half (save the [sources] flag + install/uninstall hooks + rollback) — the
│                       in-TUI panel (tui::connect_source/disconnect_source) delegates here and adds the one live-gate
│                       line (connected.set) a separate CLI process can't; reconcile_to = the declarative `sources set`
│                       (connected set = exactly the args). `apply_choices(cfg, &[(id,bool)])` = the onboarding apply
│                       (connect checked / disconnect unchecked), SCOPED to the ids passed so an unlisted source's
│                       flag is never written (the reason it's NOT the declarative reconcile_to); shares
│                       `apply_one` with reconcile_to. OWNS the source-status MODEL relocated from tui::connection
│                       (ConnState/ConnectionRow/build_rows*, re-exported back so the panel/harness are unchanged)
│                       + the folded-hook-removal VOCABULARY both presenters read back: the machine
│                       HOOK_REMOVAL_FAILED_PREFIX (`sources set`'s token) and its human twin
│                       HOOK_REMOVAL_FAILED_PHRASE (`pub` — main.rs's `disconnect` arm is a separate crate)
├── sources_cli.rs      the scriptable sources-CLI PRESENTERS over crate::sources (a bin-crate SIBLING of
│                       sources.rs — the core stays presenter-free): run_setup / run_sources_list /
│                       run_sources_set / the shared connect/disconnect run_change (+ emit_outcomes →
│                       Vec<OutcomeRow>, the `--json` batch envelope pinned by
│                       `outcome_envelope_is_the_id_outcome_raycast_contract`)
├── doctor.rs           `pixtuoid doctor` — read-only source self-diagnosis (connected? hooks
│                       installed? installed `<cli> --version` vs the registry's verified_version
│                       anchor → skew flag; + decode-drift counts scanned from the warn-floor log's
│                       `pixtuoid::drift` breadcrumbs). Pure scan_log_for_source/format_doctor_row/
│                       parse_version/version_status (tested; scan vs REAL fmt output); sanitizes
│                       untrusted sampled names (R0615-06). verified_version lives on SourceDescriptor.
│                       `read_log` is the ONE log-read authority both readers share (doctor's
│                       report + sources_cli's `health`): a MISSING log is the ordinary
│                       no-run-yet "no drift", every other error class returns a warning so
│                       an unreadable log can't read as a clean bill of health. That warning
│                       interpolates a `PIXTUOID_LOG`/`XDG_STATE_HOME` path and its two readers
│                       print to DIFFERENT terminals (doctor's stdout, sources_cli's tracing →
│                       raw stderr), so it is strip_control_chars'd where it is MINTED, not
│                       per presenter (R0615-06 — sanitizing per presenter is how the twin leaked).
│                       drifted_sources/footer_warning (also pure, tested) feed the LIVE footer nudge —
│                       run_tui throttle-scans the same log (≤15s) → ⚠ decode drift footer (see tui guide).
│                       **THE unified source-HEALTH module** (#309 health-consolidation): `SourceDiagnostics`
│                       { install: Option<SchemaVerifyResult>, drift } + `diagnose(src, log, cfg)` (install
│                       soundness via install::verify_target + drift scan) + `summary()` (⚠ install-broken
│                       > decode-drift) is the ONE rollup the Sources panel detail, the boot preflight
│                       (main.rs), AND `run` (the CLI report) all read — surfaces can't drift apart. Version
│                       skew stays report-ONLY (the <cli> --version probe is too costly for the interactive
│                       panel-open; advisory). doctor=health PROVIDER, ConnState=connection lifecycle it
│                       ANNOTATES (sub-state, not overlap). + the #526 focus-jump block (`focus_section`,
│                       pure + registry-bucketed: activation backend per OS — linux via the pure
│                       `linux_activation_backend` over the SAME env markers focus/linux.rs keys on —
│                       + CC/Codex probe-root presence via `source::cc_registry_dir` / codex
│                       default_paths; report-only, NO TUI notice — user-cut)
├── focus/              FOCUS-JUMP (click a sprite / dashboard `f` → the agent's terminal APP comes to the
│                       foreground; spec docs/superpowers/specs/2026-07-10). mod.rs: focus_slot (the ONE
│                       painter-agnostic dispatch entry — tui click/`f` today, the floating trigger later) →
│                       resolve_pid (slot.pid for stamp-channel sources — a `PidIdentity` (pid + kernel start
│                       marker) riding each hook Identity — else the registry `FocusChannel::TranscriptProbe`
│                       gate + the CC/Codex point queries `source::{cc,codex}_pid_for_session`, recycle-guarded;
│                       probe fns stay HERE, lockstep-tested against the registry enum — wasm const-table
│                       boundary; TWO click-time guards on the cached path: an EXITING slot is REFUSED,
│                       and the start marker is re-read via ProcessTable::start_time — mismatch/gone = recycled
│                       pid, refused, #527) + ancestor_walk (PURE over an
│                       injected ProcessTable, cycle-guarded, stops at pid≤1 — mock-table unit tests; KNOWN
│                       common miss #538: tmux/screen/zellij servers are daemonized → walk dead-ends at pid 1) +
│                       focus_agent (the ONE orchestration entry; activation injected so dispatch tests never
│                       touch the OS). Per-OS glue (codecov-ignored, winit-class): macos.rs `/bin/ps -o ppid=`
│                       per hop (NOT proc_pidinfo — it EPERMs at the setuid-root `login` in terminal chains;
│                       live-dogfood-caught) + NSRunningApplication activate (objc2-app-kit pinned to winit's
│                       stack, zero TCC); windows.rs Toolhelp32 + EnumWindows/SetForegroundWindow, retried
│                       once under an AttachThreadInput borrow of the foreground thread's input state
│                       (#528 — see the foreground-lock sharp edge); linux.rs /proc walk + ONE channel per env:
│                       sway/hyprland IPC by env marker (focusable asks the compositor tree for pid ownership,
│                       so the walk surfaces the terminal, not the agent) else EWMH _NET_ACTIVE_WINDOW via
│                       x11rb — i3 rides EWMH, NOT swaymsg (GNOME Wayland fails closed). ONE failure rule: every
│                       miss = tracing::debug + silent no-op — no fallback tiers, no info UI (user-directed).
│                       App-level only in v1 (no tab/pane precision — backlog). Windows shim-family sources
│                       now resolve a pid too (#528): the shim walks past its transient cmd.exe parent
│                       (pixtuoid-hook's cli_pid) and `pid_start_marker` reads a creation FILETIME there, so
│                       the #527 click-time recycle guard is live on Windows rather than inert.
├── config/             AppConfig persistence (~/.config/pixtuoid/config.toml), XDG-aware
├── runtime/            mod.rs (RunConfig, boot-capacity math, headless summarize — all unit-tested;
│                       ConnectedSources = the live `Arc<Mutex<HashSet<String>>>` connected-set,
│                       seeded from config::resolve_connected, mutated by the Sources panel toggle,
│                       read by the reducer task — recovers via into_inner on lock poison),
│                       driver.rs (tokio task wiring: source ── (Transport, AgentEvent) ──► reducer ──►
│                       renderer, compute_boot_capacities terminal-size query, Ctrl-C loop —
│                       untestable async glue, codecov-ignored, #103; exception: headless_loop
│                       takes its ctrl_c future as an injected seam, so its signal arms — incl.
│                       the registration-failure disarm — are unit-tested. The CONNECTION-GATE DECISION
│                       lives in the sibling `runtime::gate` module (`event_source` + `apply_gated_event` +
│                       `apply_gated_presence` [the daemon-presence twin] + `reconcile_sweep_tick` — covered AND
│                       mutation-tested, so a gate/reconcile drift reds a test, NOT hidden in this coverage-excluded
│                       shell; the tests drive the REAL fns, not a hand-copied mirror — #741/#751), DRIVEN by
│                       reducer_task (the presence arm keeps only `ew.watch` + publish): every incoming event is dropped
│                       if its source (resolved by the pure `event_source` off SessionStart/Identity, else the
│                       slot) is not in the connected-set; every sweep tick RECONCILES the scene toward the set via
│                       (idempotent) `Reducer::reconcile_connected(&cur)` — which evicts every slot whose
│                       source is the COMPLEMENT of the connected snapshot (NOT a registered-source list), so a
│                       panel disconnect walks characters out gracefully + live (no restart), the JSONL watcher
│                       still running can't keep a disconnected source visible, AND a blank-source slot that
│                       slipped the per-event gate is swept too. Stateless on purpose (no prev-set bookkeeping).
│                       LIVENESS-LADDER INTERACTIONS (all benign — a disconnect is an explicit user toggle, the
│                       same authority class as a SessionEnd, NOT content-driven lifecycle): a disconnected source
│                       is evicted by THIS 1-Hz reconcile, NOT the minutes-scale stale-sweep; reconcile's
│                       write-once `mark_exiting` is honored by the probe ladder (ProofOfLife/vouch SKIP exiting
│                       slots + never create/resurrect them — core sharp edge), so a vouched-but-disconnected
│                       source still exits; `cascade_exit` is source-agnostic (parent_id BFS) so a disconnect of
│                       a delegating parent takes its whole subtree while a DIFFERENT connected source's subtree
│                       is untouched. Reconnect = a fresh `SessionStart` resurrects-in-place once the old slot GCs.
│                       `build_source_set` is the ONE source-construction site: it mints the HookRouter (the
│                       Source that owns the shared hook socket — every CLI's hooks ride it), the transcript
│                       watchers (CC/Antigravity/Codex/Copilot/omp/grok), and the ONE shared ChildEndUnclaims handle (#246)
│                       — handed to the HookRouter (hook-tee PRODUCER) + ClaudeCodeSource & CodexSource & GrokSource (watcher
│                       CONSUMERS). Daemon presence (OpenClaw) rides a source-tagged sibling channel into
│                       SceneState::daemons; reducer_task's presence/sweep arms are registry-driven
│                       (daemon_sources()) so N daemons need no driver edit),
│                       pipeline.rs (#714 — `spawn_pipeline(socket_path, roots…, connected, boot_caps)
│                       -> Pipeline {scene_rx, health_rx, floor_caps, _source_handles}`: the ONE
│                       source→reducer spine BOTH painters boot through — presence chan + exit watch →
│                       build_source_set → event/scene/health chans → floor-caps atomics → reducer_task
│                       + SourceManager::spawn_with_health; was hand-mirrored across run_async and
│                       floating::run. boot_caps / socket_path / ConnectedSources stay CALLER-side (the
│                       documented divergences: TUI measures the terminal, floating its window pixels;
│                       both need socket/connected after boot). Needs an ambient tokio context
│                       (run_async's block_on / floating's rt.enter) — the RUNTIME is what keeps the
│                       spawned tasks alive (a dropped tokio JoinHandle DETACHES, never stops);
│                       `_source_handles` is an inert anchor for future abort/join.
│                       codecov-excluded + mutants-excluded like driver.rs)
├── init_pack.rs        extracts the embedded skeleton pack to a target dir for `init-pack`
├── validate.rs         the `validate-pack` presenter; pack.name/version are UNTRUSTED TOML strings (can
│                       embed ESC/OSC via \u escapes), so every printed line routes through
│                       strip_control_chars (same egress rule as the headless summary + doctor)
├── version.rs          pure version-popup boot logic
├── aa_text.rs          THE anti-aliased text rasterizer — every rasterized text surface rides it: the floating
│                       window's badges/board AND the snapshot example's terminal-cell text + --proof panel
│                       (the old 8×8 `pixtuoid_scene::font` + its font8x8 dep were DELETED — no bitmap stand-in
│                       anywhere). ONE face BY DESIGN: **Monaspace Neon** (GitHub Next, OFL) — the brand mono
│                       across the whole project (the site's `--font-mono` is the same family via
│                       @fontsource/monaspace-neon). Chosen over JetBrains Mono because it natively covers the
│                       office's FULL symbol vocabulary `★ ◐ ⬢ ▮ ▯ ↳ ◷ ▤` — JBM lacks all of those (verified;
│                       JetBrainsMono NERD Font does NOT help: its patches are all Private Use Area, a real
│                       terminal shows such symbols via system-font fallback), which had forced an interim
│                       JuliaMono-subset fallback face, then an interim JBM-native vocabulary (`✶ ◔ ◆ █ ░ └`)
│                       — both retired the same day Monaspace landed. `◷`/`▤` replaced the emoji-only `⏱`/`📁`
│                       tooltip prefixes. The `office_symbol_vocabulary_is_fully_covered` test is the gate: a
│                       NEW render glyph must be Monaspace-covered or the vocabulary changes — never a second
│                       face. Exposes has_glyph / text_width / line_height / blend_channel (the ONE
│                       coverage-blend curve all three surfaces wrap) / draw_text_at(s, x, top_y, px,
│                       put(x,y,coverage)) — a surface-agnostic coverage callback the caller blends
│                       (offscreen.rs `blend_xrgb`, snapshot `blend_px`/`mix_rgb`). Binary-only (ab_glyph is a
│                       runtime dep of THIS crate, not pixtuoid-scene — the engine stays font-impl-free; the
│                       OTF/CFF outlines rasterize fine through ab_glyph). The wasm/site painter does its own
│                       AA via DOM spans, not this. Snapshot cell text renders at CELL_FONT_PX=14.7 (Monaspace
│                       advance 7.96 ≤ the 8px cell; line_height rounds to the 16px cell — test-pinned).
├── audio/              ambient office sound (#633) — THE one consumer of pixtuoid_scene::audio's model and
│                       the only owner of rodio/cpal (behind the default-on `audio` cargo feature; Linux
│                       PREBUILTS ship without it — ALSA can't link into musl/cross builds — so Linux audio
│                       is from-source). mod.rs (AudioHandle: clone-cheap try_send gateway — disabled handle
│                       is inert everywhere, so callers never cfg; AssetBank = the ONE-SHOT pools, TrackBeds =
│                       the six TRACK-OWNED loop buffers (rain registers separately at spawn — weather is
│                       track-independent) HANDED OFF at registration/swap and dropped — RodioSink copies each
│                       into its own SamplesBuffer, so retaining them would double the bed RAM. Synthesis
│                       at spawn on a fixed seed, MEASURED ~2s release / >10s debug on M-series: frames
│                       try_sent in that window drop harmlessly (levels re-send every render frame) and MUTE
│                       rides an AtomicBool on the handle — NOT the droppable frame channel — so an m/p press
│                       mid-window can never be lost; run_loop = the device-agnostic thread body; rain at spawn,
│                       track beds on the first frame). The PURE synth stack (dsp/score/synth/mixer) AND the
│                       per-tick `AudioEngine` MOVED to `pixtuoid_scene::audio` (web-audio #633) so the native
│                       gateway here AND the wasm WebAudio painter run the SAME mixing/crossfade/scheduling —
│                       the binary imports `pixtuoid_scene::audio::{AudioEngine, dsp, synth, MAX_DT_S}`;
│                       AssetBank/TrackBeds + the device half stay HERE. `run_loop` is now a THIN SHELL over
│                       `AudioEngine`: the clamped-dt clock, the mute/volume atomics (`engine.set_muted/
│                       set_master`), the caller-side bed BUILD (on `TickCommands.swap`), and forwarding each
│                       tick's `{gains, plays}` to the sink (`bank.sample(pool, index)` resolves a play). The
│                       old `resync_after_stall` is GONE — the engine owns the (clamped) clock so a build stall
│                       can't burst the schedulers; only a post-build channel drain remains (`merge_backlog_levels`
│                       on the first frame keeps its events but adopts the backlog's freshest levels). Only
│                       sink.rs (AudioSink seam: NullSink for CI/no-device, RodioSink = rodio 0.22 Player
│                       glue, codecov-excluded winit-class) + spawn/run_loop remain binary-side behind the
│                       `audio` feature (the rodio dep). Audio NEVER blocks render: bounded channel,
│                       drop-on-backpressure. TUI feeds one AudioFrame per rendered frame, composed by the
│                       office-shared AudioObserver (pixtuoid_scene::floor, in PerOffice) via
│                       self.office.audio.frame(..) — runs every frame, only DELIVERY is mute-gated; m toggles
│                       mute. Audio is
│                       FLOOR-SCOPED (owner call): stems + door/appliance cues come from the floor
│                       being VIEWED (per_floor_counts + floor_idx-filtered ids; tracker re-primes on
│                       floor switch); rain stays global (weather, not agent activity). No elevator
│                       ding (owner-cut). Floating has FULL cue parity (#633 close-out): stems + door +
│                       appliance one-shots, scoped to its rendered floor — composed via
│                       `FloorSession::audio_frame()` through the SAME office-shared AudioObserver (backed by
│                       the session's private last_occupied + last_layout), floor-reprime automatic.
│                       [audio] config: ONE switch `muted` default TRUE (owner-cut the redundant enabled
│                       knob; `m` = the whole opt-in, persisted via save_audio_muted) + volume clamped [0,1];
│                       the system LAZY-SPAWNS on the first unmute (muted = zero cost: no device/thread/
│                       buffers) — run_tui swaps the fresh handle into the renderer; floating boot-spawns
│                       iff !muted AND has the SAME m/+/- runtime keys. TEARDOWN-ON-QUIT (the "music keeps
│                       playing after quit / killall coreaudiod" bug): the device runs on a spawned thread
│                       whose RodioSink Drop closes the OS output — detached, that Drop RACES process exit
│                       and on macOS strands CoreAudio (cpal's stream is !Send so it CAN'T live on the main
│                       thread lowfi-style — the device MUST stay off-thread, so the fix is to JOIN it, not
│                       relocate it). RAII owns this: **`AudioController` boot-spawns the device thread in
│                       `new()` and JOINs it in `impl Drop`** (via `AudioHandle::shutdown` — drop the sole
│                       sender → run_loop returns → bounded join by SHUTDOWN_JOIN_TIMEOUT, which must exceed
│                       a release synth build since run_loop is blind to the closed channel mid-build). Each
│                       painter holds ONE controller (TUI a run_tui local; floating a FloatingApp field),
│                       built AFTER that painter's fallible `?` boot steps — so no thread predates its
│                       Drop-owner and the compiler runs the teardown on EVERY exit (q/Ctrl-C/terminate/error/`?`;
│                       a release `panic=abort` is the one exit it skips), no hand-wired call to forget. Drop FLUSHES a pending debounced volume
│                       BEFORE the join, so a nudge-then-Ctrl-C persists too (#752, was `q`-branch-only). The join is bounded because
│                       CoreAudio device-close can itself block. The mute/volume TRANSITION is
│                       ONE authority — `audio::apply_audio_action(&mut AudioUi, action, paused, spawn)`
│                       (audio/mod.rs, unit-tested); the PERSIST protocol around it (mute-save-now, volume
│                       debounce→flash-expiry, exit-flush) is a SECOND authority BOTH painters OWN —
│                       `audio::AudioController` (new/apply/tick/set_paused/volume_flash/handle, exit-flush now
│                       folded into Drop;
│                       `now` injected → the debounce/flash is unit-tested). The TUI keeps ONE controller (was
│                       5 loop locals + a deleted `run_audio_action`); floating keeps one (was its own
│                       volume_flash/volume_dirty/flush_volume). BOTH painters now render the shared
│                       `pixtuoid_scene::footer` model, so the `♩`/`♩ N%` audio state lives in the footer's
│                       right suffix on BOTH — TUI-consistent (silent when muted; no separate overlay). Only
│                       the KEY→action decode is painter-specific: crossterm dispatch in
│                       tui/mod.rs, winit in floating/input.rs (the pure key-map, `m`/`+`=/`-`_, lowercase
│                       m only; winit's repeat flag swallows a held m — the TUI's crossterm path lacks it).
│                       window.rs stays thin winit glue; lazy spawn + persistence identical; audio feedback is
│                       the footer band's `♩`/`♩ N%` suffix (offscreen::paint_footer_into_surface — the
│                       standalone volume_flash overlay was retired when the footer landed). The KeyboardInput arm
│                       gates `is_synthetic: false` (winit replays held keys on focus-gain, X11/Windows —
│                       the focus-replay twin of the TUI's should_dispatch_key). Footer shows ♩ iff
│                       enabled && !effective-muted (m OR pause); onboarding carries the one-line m hint.
│                       +/- nudge volume (audio::VOLUME_STEP, THE shared step both painters read — audio/ is
│                       the sibling painters' one shared home, same for VOLUME_FLASH_MS + the transition
│                       fn; an AtomicU32-bits sibling of the mute atomic; mixer folds it per tick; persisted;
│                       footer flashes `♩ N%` ~1s — the lowfi volume-timer pattern; + from muted unmutes).
│                       RodioSink::open silences stderr around device open on Unix (ALSA prints raw lines;
│                       lazy spawn = mid-altscreen open, one line corrupts the TUI — lowfi issue #1).
│                       Volume→amplitude is mixer::master_amp = user² × BUS_TRIM(0.35): a squared perceptual
│                       curve under an ambient bus trim (dogfood: untrimmed linear was "too loud even at
│                       5%") — the ONE mapping site; the footer keeps showing the user's linear percent.
│                       Volume persist is DEBOUNCED to the ~1s flash expiry + the quit path (+/- is a
│                       repeatable key — per-repeat ConfigLock rounds were the bot MEDIUM); the +/- arm
│                       re-attempts the lazy spawn whenever unmuted-but-disabled ('+' is never a dead key).
│                       An EMPTY office now plays the quiet pad+sparkle+texture "radio on" floor (the
│                       ratified demo_1) — Phase 1's empty-silent behavior ended when the music landed.
│                       MOOD TRACKS (#644; ALL-GENERATIVE 2026-07-20): TrackId {GenDay(seed),
│                       GenNight(seed)} rides AudioFrame (scene's select_track over the
│                       lighting's OWN sun window + precipitation; seed = the audio::track_epoch
│                       block (600s — a new song every 10 minutes, owner-tuned for short agent
│                       sessions) and the block id change is an ordinary track switch); the
│                       engine's TrackSwitch holds the
│                       six TRACK_STEMS at 0, and when they reach silence `tick` returns `swap: Some(to)` →
│                       run_loop synthesizes that track's TrackBeds under the silence (~2s) and swap_loop's
│                       them (RodioSink drops+recreates the Player at gain 0), then ramps back — LATCHED per
│                       cycle (boundary flapping can't thrash synths); rain is weather, never swapped. Track beds register on the FIRST frame (it names the
│                       right mood — booting Day at night would synth a track just to fade it away).
│                       The night MOOD keeps the Lofi Girl anchor (sub-bass floor on the dedicated
│                       bass lane — the frozen v4 anchor alone still bakes it into its pad —
│                       kick+hat-only groove, duck-baked texture, phase-locked) — now COMPOSED
│                       per track-epoch by scene's compose.rs rather than replayed from the frozen v4
│                       take; bus glue is deliberately NOT runtime (rodio has no insert) — the
│                       listen gate renders the honest no-glue approximation.
├── fonts/              MonaspaceNeon-SemiBold.otf + OFL-Monaspace.txt (the ONE bundled face; vendored VERBATIM
│                       from githubnext/monaspace v1.400 static — unmodified, so the OFL Reserved-Font-Name
│                       clause is never triggered)
├── install/            multi-target (Claude + Codex + Reasonix + CodeWhale + opencode + Cursor + Hermes + OpenClaw + grok + Kimi) hook install via the `Target` registry:
│                       mod.rs (install_target/uninstall_target = structured core → InstallReport/UninstallReport,
│                         driven SOLELY by the in-TUI Sources panel's connect/disconnect (no CLI orchestration —
│                         plan_targets/interactive_pick/run_install/run_uninstall + inquire were deleted with the
│                         install-hooks CLI); has_hooks(t, cfg) is `pub(crate)` — its callers are doctor (diagnose's verify
│                         gate + run's per-source hooks_installed report row) and the onboarding-skip freeze
│                         (`sources::skip_freeze`, which probes it to keep a pre-0.12 upgrader's hooks); 0.12.0 dropped
│                         resolve_connected's install-state migrate inference),
│                       target.rs (Target trait + TARGETS = [CLAUDE, CODEX, REASONIX, CODEWHALE, OPENCODE, CURSOR, HERMES, OPENCLAW, GROK, KIMI];
│                         each Target carries a `verify_schema` fn-ptr — the #309 install-soundness check, per-source
│                         format-local like merge_install/uninstall),
│                       verify.rs (the READ-ONLY #309 install-schema verifier: SchemaParse/SchemaVerifyResult/ShimRef +
│                         shared read helpers shell_shim_ref (4 shell targets) / flat_json_verify (reasonix+cursor) /
│                         assemble; the two baked code templates keep their placeholder quoted so both files
│                         remain valid source before rendering (default-setup CodeQL parses them);
│                         install::verify_target(t, config) = the I/O wrapper that reads the config +
│                         calls verify_schema + stats the shim + (for `extra_artifacts` targets like OpenClaw)
│                         stats each wholly-owned plugin file for existence — a missing one is a HARD break, the
│                         silent-dead class the config check is blind to (#332; paths are hook-path-independent so a
│                         placeholder arg yields the install locations without resolving the binary). ONLY call when has_hooks(t, cfg) — the load-bearing gate
│                         (an uninstalled config verifies "broken"; a disconnect removes hooks → has_hooks=false →
│                         never called → never a false broken)),
│                       merge.rs (the install-WRITE shared helpers, split OUT of verify.rs so the read/write
│                         halves live apart: parse_json_or_empty/parse_toml_or_empty (empty ⇒ {}), hook_path_str
│                         (the ONE non-UTF-8-path rejector), bake_hook_path (opencode/openclaw plugin templater),
│                         and flat_json_merge_install/uninstall — the sentinel-keyed per-event merge Reasonix/Cursor/
│                         Claude ride (the entry SHAPE rides in the caller's make_entry closure, so Claude's nested
│                         entry fits the same core)),
│                       claude.rs / codex.rs / reasonix.rs / codewhale.rs / opencode.rs (+ bundled opencode_plugin.ts) /
│                         cursor.rs / hermes.rs (hook-only, GLOBAL ~/.hermes/config.yaml) / openclaw.rs (+ bundled openclaw_plugin.js —
│                         its plugin stamps the resolved `gatewayPort` on every forwarded hook (the mascot's
│                         instance identity) and returns an EXPLICIT `{outcome:"pass"}` from the awaited
│                         `before_agent_run` gate; contract-tested for real by scripts/openclaw-plugin.test.mjs
│                         under `just npm-check`) / grok.rs / kimi.rs (GLOBAL `<KIMI_CODE_HOME>/config.toml`,
│                         default `~/.kimi-code/config.toml`) (per-target hook_command + config path;
│                         claude.rs: Unix = bare shell-form, Windows = exec-form absolute .exe;
│                         reasonix = GLOBAL ~/.reasonix/settings.json, FLAT {match,command,timeout-ms}
│                         entries — project-scope is trust-gated; match omitted = every tool;
│                         codewhale = ~/.codewhale/config.toml [hooks] (enabled=true) + a `hooks` array of
│                         {event, command} entries. Env-mode events (session/tool/end) bake ` --event <name>`
│                         (CodeWhale sets no event env var; shim builds from DEEPSEEK_*); the subagent observer
│                         events (subagent_spawn/complete) use the PLAIN stdin-forward command (no --event) —
│                         CodeWhale pipes a full JSON payload with the child agent_id. `_pixtuoid` sentinel idempotency.
│                         opencode = a TS PLUGIN (the FIRST install target that writes CODE, not a config block):
│                         opencode auto-discovers `<config>/plugins/*.ts` (plural, canonical), so we DROP `<opencode-config>/plugins/pixtuoid.ts`
│                         (no opencode.jsonc edit). The plugin (bundled `opencode_plugin.ts`, shim abs-path baked in
│                         JSON-escaped) pipes lifecycle/tool/permission
│                         EventV2 to the shim on stdin; merge_install
│                         renders the whole file (it's wholly ours), uninstall writes a sentinel-free no-op stub
│                         (write-only orchestrator can't delete), detect on the `@pixtuoid-opencode-plugin` sentinel),
│                       hook_cmd/ (mod.rs / unix.rs / windows.rs — the shared per-platform hook-command builders,
│                         incl. `windows::windows_bare_hook_command`'s 8.3 short-name / cmd-unsafe-path guard),
│                       io.rs (resolve_symlink + the ONE config-write authority: ConfigLock —
│                         an RAII advisory-lock guard taken BEFORE the read and held across
│                         read+merge+backup+write (lost-update TOCTOU); its pinned symlink
│                         resolution is the ONE identity for the whole round — read/backup/
│                         remove_backup go through ConfigLock::read/::backup_once/::remove_backup,
│                         never a re-resolve of the link — and ConfigLock::write_atomic
│                         (fsync + atomic rename, PRESERVES the target's Unix mode / creates new
│                         files 0600 — settings.json can carry API keys; Windows: rename wrapped
│                         in 3×50ms retry for sharing-violation tolerance). write_config_atomic
│                         = lock_config + write_atomic for single-shot writers; NEVER re-call it
│                         while holding a ConfigLock — same-process flock self-deadlocks. The
│                         .lock file is deliberately never unlinked, and even a no-op
│                         re-install creates it: the lock must be taken BEFORE the read
│                         that detects "nothing changed"; open_lock_sidecar creates it
│                         0600 + O_NOFOLLOW — BOTH halves of the hook socket-lock's parity,
│                         since flock(2) grants an exclusive lock through a read-only fd,
│                         so a 0644 sidecar lets a co-located user wedge every install AND
│                         every config save — and lock_config then tighten_to_owner_only's
│                         an upgrader's pre-existing 0644 one. owner_only_create /
│                         tighten_to_owner_only are the ONE definition of that 0600 policy
│                         (create_hardened_tmp + logging's sinks read it too), kept as two
│                         SEPARATE fns so each half has its own test — see the logging entry.)
├── floating/           `pixtuoid floating` — the frameless, always-on-top DESKTOP WINDOW (winit + softbuffer,
│                       binary-only; pixtuoid-core stays window-free, invariant #1). ALL floating-only source
│                       lives here: mod.rs (run + PipelineBoot/LivePipeline: boots the SAME
│                       `runtime::pipeline::spawn_pipeline` spine as the TUI (#714 — the old hand-mirrored
│                       wiring is gone; only the window seed, socket resolution and ConnectedSources stay
│                       local), but from `window::resumed` — `run` has only the LOGICAL [floating] config
│                       size and the seed needs the REAL window's PHYSICAL px (#803) — spawned on
│                       a bg runtime, NEVER block_on [winit owns the main thread]; an EventLoopProxy bridges
│                       scene changes → redraw), offscreen.rs (OfficeRenderer — owns one
│                       pixtuoid_scene::floor::FloorSession, the scene-owned painter session over the shared
│                       render_floor seam (#423; eviction is structural — render() runs it); moved here from tui/ as it's floating-only; the testable unit;
│                       also OfficeRenderer::{labels + paint_labels_into_surface, board + paint_wall_board_into_surface}
│                       — agent name badges from the shared pixtuoid_scene::overlay model AND the neon wall board
│                       from pixtuoid_scene::board, both rendered as anti-aliased Monaspace Neon via crate::aa_text
│                       (NOT the old 8px pixtuoid_scene::font — that pixelated), blitted at NATIVE surface res
│                       POST-upscale with a near-black drop-shadow so the crisp caption reads over the chunky office;
│                       ALSO the whole window→capacity chain, kept here because window.rs is measured by neither
│                       codecov nor cargo-mutants: window_buffer_geometry (window→office-buffer projection) →
│                       floor_caps_for_buffer (the ONE per-floor derivation) → boot_capacities_for_window (the
│                       resumed seed) and sync_floor_caps (the per-redraw store + its resize memo)),
│                       window.rs (FloatingApp ApplicationHandler: renders the office at a DOWNSCALED buffer
│                       [~window/SCALE, OFFICE_TARGET_H≈180] then nearest-neighbor UPSCALES into the surface —
│                       a 1:1 blit renders 8×12 sprites unreadably tiny; ~30fps tick WHILE agents OR a live gateway
│                       daemon (the OpenClaw lobster — a time-driven wandering mascot in scene.daemons, Idle/Busy/
│                       Degraded) are present, else a ~1fps IDLE_AMBIENT tick (keeps the clock/weather/pet alive
│                       without burning CPU on an empty office — was a full 0fps freeze; the tick itself lives in
│                       cadence.rs, and the redraw REQUEST is gated on the same deadline as the wait — see below);
│                       restored [floating] position is validated against
│                       the live monitors (off-every-screen → OS-default placement, not unrecoverable off-screen);
│                       left-press drag / corner resize; m/+/- audio keys (the pure half in input.rs — see
│                       the audio/ entry); persists [floating] geometry (+ any pending volume) on close;
│                       calls offscreen::sync_floor_caps each redraw so floor_caps track the rendered layout's
│                       home-desk count and no agent is stranded off-screen (the memo that decides WHETHER to
│                       republish lives with the publish, not here — this file is measured by nothing);
│                       macOS Accessory + shadow, #[cfg(windows)] skip-taskbar; opacity = honest v1
│                       no-op, winit has none + softbuffer is opaque → wgpu/native deferred),
│                       geometry.rs (the pure window/monitor rect math extracted OUT of window.rs so it's
│                       unit-testable: window_visible_on_monitors = the off-screen-recovery AABB overlap +
│                       empty-monitor-list guard; near_resize_corner = the drag-vs-resize hit-test),
│                       input.rs (the PURE winit key → audio::AudioAction map; the mute/volume TRANSITION
│                       itself is shared with the TUI in audio::apply_audio_action — see the audio/ entry),
│                       cadence.rs (the PURE animation throttle + both FPS constants: `FrameClock::poll(now,
│                       office_idle) -> (paint, deadline)`. `about_to_wait` runs on EVERY event-loop iteration,
│                       so the redraw REQUEST — not just the `ControlFlow::WaitUntil` beside it — has to be gated
│                       on the deadline: an unconditional `request_redraw()` there leaves winit a pending redraw
│                       whenever it reaches its wait, so `WaitUntil` never sleeps and BOTH cadences collapse to
│                       max-rate. That was the shipped behavior until it was MEASURED: 100.6% of one core with an
│                       empty office and 100.5% with agents, vs 0.5% / 13.2% once the request is gated).
│                       **mod.rs + window.rs are codecov-IGNORED** (winit `EventLoop`/`ApplicationHandler` +
│                       tokio glue, the floating twin of driver.rs — need a real display); the floating crate's
│                       TESTED surface is offscreen.rs (render seam) + geometry.rs (rect math) + input.rs
│                       (audio keys) + cadence.rs (the throttle). Visual check:
│                       `examples/floating_snapshot.rs` (the floating twin of the `snapshot` example).
└── tui/                ratatui App + TuiRenderer (inherent `render` flush) — the half-block flush + widgets +
                        event loop, a thin painter over the pixtuoid-scene crate (the engine is its own crate now) — see src/tui/CLAUDE.md

sprites/                character/environment packs (NOT under pixtuoid-hook; the DEFAULT pack moved OUT to
│                       crates/pixtuoid-scene/sprites/default/ — scene include_str!s it via its own build.rs):
├── robot/              proof-of-concept TV-head robot pack (loadable via --pack-dir)
└── skeleton/           template pack for custom sprite creation (embedded via init_pack; extracted via init-pack)
```

