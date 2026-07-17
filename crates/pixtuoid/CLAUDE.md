# pixtuoid (binary) — agent guide

The **TUI binary**: `ratatui` + `crossterm` + `winit` + `tokio` + `clap`. Wires
sources → reducer → renderer, owns the CLI subcommands, hook installation, config
persistence, and multi-floor orchestration. The backend-agnostic render +
simulation **engine** (layout, pose/motion/pathfinding, the pixel pass, the theme
model, pets, chitchat) is its OWN dependency crate `pixtuoid-scene` (it used to be
an in-binary module) — see [`../pixtuoid-scene/CLAUDE.md`](../pixtuoid-scene/CLAUDE.md);
the DAG is `pixtuoid-core ← pixtuoid-scene ← {pixtuoid, pixtuoid-web}`. This
binary's two thin painters **over the `pixtuoid-scene` crate** are the terminal
renderer `src/tui/` ([`src/tui/CLAUDE.md`](src/tui/CLAUDE.md)) and the
`floating/` desktop window (neither depends on the other); the wasm `<canvas>`
painter is the SIBLING crate `pixtuoid-web`. Cross-cutting rules: workspace
[`CLAUDE.md`](../../CLAUDE.md); headless-lib detail:
[`../pixtuoid-core/CLAUDE.md`](../pixtuoid-core/CLAUDE.md).

## Layout

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
│                       (doctor dispatch + sources_cli + RunConfig.log_path read it)
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
│                       {id, outcome, message?} — a bare token + optional failure detail, see the sharp edge below). connect/disconnect
│                       are the PERSISTED half (save the [sources] flag + install/uninstall hooks + rollback) — the
│                       in-TUI panel (tui::connect_source/disconnect_source) delegates here and adds the one live-gate
│                       line (connected.set) a separate CLI process can't; reconcile_to = the declarative `sources set`
│                       (connected set = exactly the args). `apply_choices(cfg, &[(id,bool)])` = the onboarding apply
│                       (connect checked / disconnect unchecked), SCOPED to the ids passed so an unlisted source's
│                       flag is never written (the reason it's NOT the declarative reconcile_to); shares
│                       `apply_one` with reconcile_to. OWNS the source-status MODEL relocated from tui::connection
│                       (ConnState/ConnectionRow/build_rows*, re-exported back so the panel/harness are unchanged)
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
│                       drifted_sources/footer_warning (also pure, tested) feed the LIVE footer nudge —
│                       run_tui throttle-scans the same log (≤15s) → ⚠ decode drift footer (see tui guide).
│                       **THE unified source-HEALTH module** (#309 health-consolidation): `SourceDiagnostics`
│                       { install: Option<SchemaVerifyResult>, drift } + `diagnose(src, log)` (install
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
│                       stack, zero TCC); windows.rs Toolhelp32 + EnumWindows/SetForegroundWindow
│                       (foreground-lock denial = silent no-op); linux.rs /proc walk + ONE channel per env:
│                       sway/hyprland IPC by env marker (focusable asks the compositor tree for pid ownership,
│                       so the walk surfaces the terminal, not the agent) else EWMH _NET_ACTIVE_WINDOW via
│                       x11rb — i3 rides EWMH, NOT swaymsg (GNOME Wayland fails closed). ONE failure rule: every
│                       miss = tracing::debug + silent no-op — no fallback tiers, no info UI (user-directed).
│                       App-level only in v1 (no tab/pane precision — backlog). On Windows the SHIM sends no
│                       pid (transient cmd.exe parent — see pixtuoid-hook), but a plugin-stamped pid
│                       (opencode's process.pid) still flows: the `_pid` peek doesn't need the exit-watch.
├── config.rs           AppConfig persistence (~/.config/pixtuoid/config.toml), XDG-aware
├── runtime/            mod.rs (RunConfig, boot-capacity math, headless summarize — all unit-tested;
│                       ConnectedSources = the live `Arc<Mutex<HashSet<String>>>` connected-set,
│                       seeded from config::resolve_connected, mutated by the Sources panel toggle,
│                       read by the reducer task — recovers via into_inner on lock poison),
│                       driver.rs (tokio task wiring: source ── (Transport, AgentEvent) ──► reducer ──►
│                       renderer, compute_boot_capacities terminal-size query, Ctrl-C loop —
│                       untestable async glue, codecov-ignored, #103; exception: headless_loop
│                       takes its ctrl_c future as an injected seam, so its signal arms — incl.
│                       the registration-failure disarm — are unit-tested. The CONNECTION GATE lives in
│                       reducer_task: every incoming event is dropped if its source (resolved by the pure
│                       `event_source` — unit-tested — off SessionStart/Identity, else the slot) is not in
│                       the connected-set; every sweep tick RECONCILES the scene toward the set via
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
│                       (daemon_sources()) so N daemons need no driver edit)
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
│                       the five TRACK-OWNED loop buffers (rain registers separately at spawn — weather is
│                       track-independent) HANDED OFF at registration/swap and dropped — RodioSink copies each
│                       into its own SamplesBuffer, so retaining them would double the bed RAM. Synthesis
│                       at spawn on a fixed seed, MEASURED ~2s release / >10s debug on M-series: frames
│                       try_sent in that window drop harmlessly (levels re-send every render frame) and MUTE
│                       rides an AtomicBool on the handle — NOT the droppable frame channel — so an m/p press
│                       mid-window can never be lost; run_loop = the device-agnostic thread body; rain at spawn,
│                       track beds on the first frame),
│                       dsp.rs (radix-2 FFT + brickwall bands + spectral-envelope noise shaping [circularly
│                       seamless bed loops] + warp_resample [tape wow/flutter] + splitmix64 NoiseStream),
│                       score.rs (the FROZEN lofi compositions — day (72 BPM) + night (68 BPM, at-seconds
│                       humanized events); each ratified realization as const tables; a regen via the spec's
│                       export scripts is a NEW take → fresh LISTEN gate),
│                       synth.rs (the Phase 0 OWNER-RATIFIED recipes 1:1 — elevator ding + cooler glug were later owner-CUT (dogfood round), the spec keeps their recipes —
│                       change docs/superpowers/specs/2026-07-16-ambient-sound-phase0/ first, re-audition,
│                       then mirror; spectral-sanity tests pin the fingerprints AGAINST THE FLOAT CHAIN,
│                       never the audition wavs — write_wav's stereo interleave + soft clip once poisoned
│                       the reference numbers; plus the Phase 2 musical stems: lofi_post tape chain +
│                       stem_pad/sparkle/keys/drums, ALL-PROCEDURAL by owner decision — no committed
│                       assets, no decoder dep; the four musical loops share one sample count and start
│                       together = phase-locked), mixer.rs (pure gain ramps
│                       ~2s crossfade + typing-burst/raindrop schedulers — level-driven, no backlog replay),
│                       sink.rs (AudioSink seam: NullSink for CI/no-device, RodioSink = rodio 0.22 Player
│                       glue, codecov-excluded winit-class). Audio NEVER blocks render: bounded channel,
│                       drop-on-backpressure. TUI feeds one AudioFrame per rendered frame (renderer-side
│                       cue tracker + DrawCtx.occupied_waypoints out-param); m toggles mute. Audio is
│                       FLOOR-SCOPED (owner call): stems + door/appliance cues come from the floor
│                       being VIEWED (per_floor_counts + floor_idx-filtered ids; tracker re-primes on
│                       floor switch); rain stays global (weather, not agent activity). No elevator
│                       ding (owner-cut). Floating has FULL cue parity (#633 close-out): stems + door +
│                       appliance one-shots, scoped to its rendered floor — the occupancy feed is
│                       `FloorSession::occupied_waypoints()` (the scene seam surfaces the sim's set via
│                       `FloorFrame`) + the render-returned layout's waypoint kinds.
│                       [audio] config: ONE switch `muted` default TRUE (owner-cut the redundant enabled
│                       knob; `m` = the whole opt-in, persisted via save_audio_muted) + volume clamped [0,1];
│                       the system LAZY-SPAWNS on the first unmute (muted = zero cost: no device/thread/
│                       buffers) — run_tui swaps the fresh handle into the renderer; floating boot-spawns
│                       iff !muted AND has the SAME m/+/- runtime keys. The mute/volume TRANSITION is
│                       ONE authority — `audio::apply_audio_action(&mut AudioUi, action, paused, spawn)`
│                       (audio/mod.rs, unit-tested) — that BOTH painters run: the TUI's ToggleAudioMute/
│                       AdjustVolume arms (via run_audio_action, marshalling the loop locals) and floating's
│                       key handler. Only the KEY→action decode is painter-specific: crossterm dispatch in
│                       tui/mod.rs, winit in floating/input.rs (the pure key-map, `m`/`+`=/`-`_, lowercase
│                       m only; winit's repeat flag swallows a held m — the TUI's crossterm path lacks it).
│                       window.rs stays thin winit glue; lazy spawn + persistence identical; feedback = a
│                       transient bottom-right `♩ N%`/`♩ off` AA overlay (offscreen::volume_flash_text +
│                       paint_volume_flash_into_surface) — the window has no footer. The KeyboardInput arm
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
│                       MOOD TRACKS (#644): TrackId rides AudioFrame (scene's select_track over the
│                       lighting's OWN sun window + precipitation); run_loop's switch machine holds the
│                       five TRACK_STEMS at 0, synthesizes the new track's TrackBeds under the silence
│                       (~2s), swap_loop's them (RodioSink drops+recreates the Player at gain 0), ramps
│                       back — LATCHED per cycle (boundary flapping can't thrash synths); rain is
│                       weather, never swapped. Track beds register on the FIRST frame (it names the
│                       right mood — booting Day at night would synth a track just to fade it away).
│                       NIGHT = the Lofi Girl-anchored v4 take (LOFI-BIBLE.md; BPM 68, sub-bass floor
│                       in the pad, at-seconds frozen humanization, duck-baked texture at the night
│                       loop length = phase-locked); bus glue is deliberately NOT runtime (rodio has
│                       no insert) — the listen gate renders the honest no-glue approximation.
├── fonts/              MonaspaceNeon-SemiBold.otf + OFL-Monaspace.txt (the ONE bundled face; vendored VERBATIM
│                       from githubnext/monaspace v1.400 static — unmodified, so the OFL Reserved-Font-Name
│                       clause is never triggered)
├── install/            multi-target (Claude + Codex + Reasonix + CodeWhale + opencode + Cursor + Hermes + OpenClaw + grok) hook install via the `Target` registry:
│                       mod.rs (install_target/uninstall_target = structured core → InstallReport/UninstallReport,
│                         driven SOLELY by the in-TUI Sources panel's connect/disconnect (no CLI orchestration —
│                         plan_targets/interactive_pick/run_install/run_uninstall + inquire were deleted with the
│                         install-hooks CLI); has_hooks(t) is `pub(crate)` — its callers are doctor (diagnose's verify
│                         gate + run's per-source hooks_installed report row) and the onboarding-skip freeze
│                         (`sources::skip_freeze`, which probes it to keep a pre-0.12 upgrader's hooks); 0.12.0 dropped
│                         resolve_connected's install-state migrate inference),
│                       target.rs (Target trait + TARGETS = [CLAUDE, CODEX, REASONIX, CODEWHALE, OPENCODE, CURSOR, HERMES, OPENCLAW, GROK];
│                         each Target carries a `verify_schema` fn-ptr — the #309 install-soundness check, per-source
│                         format-local like merge_install/uninstall),
│                       verify.rs (the READ-ONLY #309 install-schema verifier: SchemaParse/SchemaVerifyResult/ShimRef +
│                         shared read helpers shell_shim_ref (4 shell targets) / flat_json_verify (reasonix+cursor) /
│                         assemble; install::verify_target(t, config) = the I/O wrapper that reads the config +
│                         calls verify_schema + stats the shim + (for `extra_artifacts` targets like OpenClaw)
│                         stats each wholly-owned plugin file for existence — a missing one is a HARD break, the
│                         silent-dead class the config check is blind to (#332; paths are hook-path-independent so a
│                         placeholder arg yields the install locations without resolving the binary). ONLY call when has_hooks(t) — the load-bearing gate
│                         (an uninstalled config verifies "broken"; a disconnect removes hooks → has_hooks=false →
│                         never called → never a false broken)),
│                       merge.rs (the install-WRITE shared helpers, split OUT of verify.rs so the read/write
│                         halves live apart: parse_json_or_empty/parse_toml_or_empty (empty ⇒ {}), hook_path_str
│                         (the ONE non-UTF-8-path rejector), bake_hook_path (opencode/openclaw plugin templater),
│                         and flat_json_merge_install/uninstall — the sentinel-keyed per-event merge Reasonix/Cursor/
│                         Claude ride (the entry SHAPE rides in the caller's make_entry closure, so Claude's nested
│                         entry fits the same core)),
│                       claude.rs / codex.rs / reasonix.rs / codewhale.rs / opencode.rs (+ bundled opencode_plugin.ts) /
│                         cursor.rs / hermes.rs (hook-only, GLOBAL ~/.hermes/config.yaml) / openclaw.rs (+ bundled openclaw_plugin.js) (per-target hook_command + config path;
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
│                         JSON-escaped) pipes lifecycle/tool/permission EventV2 to the shim on stdin; merge_install
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
│                         that detects "nothing changed".)
├── floating/           `pixtuoid floating` — the frameless, always-on-top DESKTOP WINDOW (winit + softbuffer,
│                       binary-only; pixtuoid-core stays window-free, invariant #1). ALL floating-only source
│                       lives here: mod.rs (run: reuses the SAME pipeline as the TUI — build_source_set [the
│                       ONE source-construction site] + reducer_task, both relaxed to pub(crate) — spawned on
│                       a bg runtime, NEVER block_on [winit owns the main thread]; an EventLoopProxy bridges
│                       scene changes → redraw), offscreen.rs (OfficeRenderer — owns one
│                       pixtuoid_scene::floor::FloorSession, the scene-owned painter session over the shared
│                       render_floor seam (#423; eviction is structural — render() runs it); moved here from tui/ as it's floating-only; the testable unit;
│                       also OfficeRenderer::{labels + paint_labels_into_surface, board + paint_wall_board_into_surface}
│                       — agent name badges from the shared pixtuoid_scene::overlay model AND the neon wall board
│                       from pixtuoid_scene::board, both rendered as anti-aliased Monaspace Neon via crate::aa_text
│                       (NOT the old 8px pixtuoid_scene::font — that pixelated), blitted at NATIVE surface res
│                       POST-upscale with a near-black drop-shadow so the crisp caption reads over the chunky office),
│                       window.rs (FloatingApp ApplicationHandler: renders the office at a DOWNSCALED buffer
│                       [~window/SCALE, OFFICE_TARGET_H≈180] then nearest-neighbor UPSCALES into the surface —
│                       a 1:1 blit renders 8×12 sprites unreadably tiny; ~30fps tick WHILE agents OR a live gateway
│                       daemon (the OpenClaw lobster — a time-driven wandering mascot in scene.daemons, Idle/Busy/
│                       Degraded) are present, else a ~1fps IDLE_AMBIENT tick (keeps the clock/weather/pet alive
│                       without burning CPU on an empty office — was a full 0fps freeze); restored [floating] position is validated against
│                       the live monitors (off-every-screen → OS-default placement, not unrecoverable off-screen);
│                       left-press drag / corner resize; m/+/- audio keys (the pure half in input.rs — see
│                       the audio/ entry); persists [floating] geometry (+ any pending volume) on close;
│                       floor_caps synced to the rendered layout's home-desk count so no agent is stranded
│                       off-screen; macOS Accessory + shadow, #[cfg(windows)] skip-taskbar; opacity = honest v1
│                       no-op, winit has none + softbuffer is opaque → wgpu/native deferred),
│                       geometry.rs (the pure window/monitor rect math extracted OUT of window.rs so it's
│                       unit-testable: window_visible_on_monitors = the off-screen-recovery AABB overlap +
│                       empty-monitor-list guard; near_resize_corner = the drag-vs-resize hit-test),
│                       input.rs (the PURE winit key → audio::AudioAction map; the mute/volume TRANSITION
│                       itself is shared with the TUI in audio::apply_audio_action — see the audio/ entry).
│                       **mod.rs + window.rs are codecov-IGNORED** (winit `EventLoop`/`ApplicationHandler` +
│                       tokio glue, the floating twin of driver.rs — need a real display); the floating crate's
│                       TESTED surface is offscreen.rs (render seam) + geometry.rs (rect math) + input.rs
│                       (audio keys). Visual check:
│                       `examples/floating_snapshot.rs` (the floating twin of the `snapshot` example).
└── tui/                ratatui App + TuiRenderer (inherent `render` flush; core Renderer trait retired #483) — the half-block flush + widgets +
                        event loop, a thin painter over the pixtuoid-scene crate (the engine is its own crate now) — see src/tui/CLAUDE.md

sprites/                character/environment packs (NOT under pixtuoid-hook; the DEFAULT pack moved OUT to
│                       crates/pixtuoid-scene/sprites/default/ — scene include_str!s it via its own build.rs):
├── robot/              proof-of-concept TV-head robot pack (loadable via --pack-dir)
└── skeleton/           template pack for custom sprite creation (embedded via init_pack; extracted via init-pack)
```

## Known sharp edges (don't be surprised by these)

- **Terminal cell aspect drives sprite design.** The half-block ▀ technique assumes ~1:2 cell aspect. Sprites larger than ~16×16 px break on terminals with taller cells (Ghostty default, large Fira Code). The bundled **character** sprites max at **8×12 px** (e.g. `standing`/`walking_*`), safely under the ~16×16 threshold; static environment art (door 16×14, pantry 32×10) is wider but isn't an animated half-block agent. A PNG-loader experiment hit this wall and was deleted in favor of hand-drawn `.sprite` art.
- **`--max-desks` has no hard default.** It's `Option<usize>` (hidden flag / `max-desks` config key) — when absent, per-floor capacity is auto-computed from terminal size at boot. `FALLBACK_DESKS = 16` (`runtime/mod.rs`) is used only in headless mode or when the terminal-size query errors. The auto path clamps each floor to its real layout capacity; if you add an explicit-cap boot path, clamp it the same way (don't seed the floor-capacity atomics above the layout's real capacity — `fetch_max` only grows, so an over-seed leaves agents assigned to non-existent desks until the terminal grows). **`max-desks` applies to `run` (TUI) only, NOT `floating`** — the desktop window seeds capacity purely from its window-pixel size (`floating::offscreen::boot_capacities_for_window` at boot + `window::sync_floor_caps` per redraw, both deriving buffer dims from the shared `offscreen::window_buffer_geometry` so the seed can't drift from the redraw; the TUI's footer-subtracting, `office_scale`-ignorant `runtime::boot_capacities_for` is deliberately NOT reused here — it over-seeds) and never reads `RunConfig.desk_cap`, so a `max-desks` config value is silently ignored there (a `max-desks = 0` still emits its `resolve_max_desks` warning during `build_run_config`, so only the legitimate positive-cap case is silent). The monotone `fetch_max` growth rule is TUI-only — `floating::window::sync_floor_caps` deliberately uses `store`: the window's pixel size is exact and authoritative per redraw, so a shrink LOWERS capacity (excess agents go invisible-but-alive); don't "harmonize" it to `fetch_max`.
- **Re-install is a SEMANTIC no-op, and backups APPEND their suffix.** `MergeOutcome.changed = merged != doc` (`install/claude.rs`) compares the *parsed/merged* config, NOT bytes — so a second connect (or a disconnect of an absent hook) detects "nothing changed", skips the write, and preserves the user's hand-formatting + skips backup churn. And `backup_once` names backups via `sibling()` = `format!("{}.{}", path, "pixtuoid.bak")` which **appends** — deliberately NOT `with_extension`, which would truncate `config.local.toml` → `config.local.pixtuoid.bak` (losing `.toml`). So a multi-dot config keeps its full name (`config.local.toml.pixtuoid.bak`). Both pinned by tests in `install/io.rs`.
- **Two surfaces bind a source, ONE core.** `crate::sources::{connect,disconnect}` (persist the `[sources]` flag + install/uninstall hooks + rollback) is the single seam; it has TWO presenters: (1) the **interactive** in-TUI Sources panel (`s` → `tui::connect_source`/`disconnect_source`, which delegate to the core and add the one live-gate line `connected.set` so a running office walks characters in/out NOW), and (2) the **scriptable** CLI (`pixtuoid connect/disconnect/sources [set]`, Raycast/automation/onboarding — NO live set; a running instance reflects it on next launch). This is NOT a re-litigation of #284 (which deleted the install-hooks CLI's interactive *orchestration*); it's a second presenter over the structured core (R0618-01). The CLI is persist-only by design — the live `ConnectedSources` is in-process, so a separate CLI process can't touch a running office. `connect` ERRs + rolls the flag back on install failure; `disconnect` reserves `Err` for the persist-abort and folds a hook-removal failure into the `Ok` outcome (`DisconnectOutcome::HookRemovalFailed` — the gate still closes, the flag is false; BOTH presenters surface it: the panel as "disconnected, but hook removal failed", the CLI as `disconnected (hook removal failed: …)` / `sources set` as a `hooks not removed: …` token — never a silent clean "disconnected"). `ConnectOutcome`/`DisconnectOutcome` carry the `Install/UninstallReport` so the panel renders rich notes while the CLI maps to a wire token. **The CLI honors the explicit id without the panel's `NoCli` guard** — `connect`/`sources set` install for any registered id even if that CLI isn't installed yet (pre-provisioning for automation; `detect()` returns only PRESENT CLIs so onboarding offers only installed ones), whereas the interactive panel refuses an absent CLI. The status MODEL (`ConnState`/`ConnectionRow`/`build_rows`) lives in `sources` (re-exported by `tui::connection`), so `sources::status` (the `SourceStatus` `--json` Raycast contract) doesn't depend on the painter.
- **`OutcomeRow` is `{id, outcome, message?}` — a bare machine token + a SEPARATE optional detail field.** The `--json` batch envelope (`connect`/`disconnect`/`sources set`) split the old folded `failed: <msg>` string into `outcome: "failed"` + `message: "<msg>"` (present exactly on failure, OMITTED — not null — on success) while the in-repo Raycast extension was still the ONLY consumer: it ships atomically with the binary and is NOT yet published to the Raycast store, so the breaking wire change was free — post-publication it would be expensive forever. The drift gate is the schema→codegen chain: `OutcomeRow` (schemars) → the committed `integrations/raycast/contract/outcome-row.schema.json` golden (`outcome_row_schema_matches_the_committed_contract`, regen via `just gen-contract`) → the generated Raycast TS type (`gen:contract`) → `tsc`; the exact bytes are pinned by `outcome_row_json_shape_is_the_raycast_contract` + the envelope test in `sources_cli.rs`, and the TOKEN set (`connected`/`disconnected`/`no_op`/`failed`) by `change_outcome_wire_tokens_are_stable`. `OutcomeRow::new` is the ONE outcome→row authority (both emitting surfaces route through it). **Once the extension publishes to the store this freedom ends**: installed copies parse the wire independently of the binary's version, so any further wire change needs a version handshake / deliberate coupled migration — not a flag-day edit like this one.
- **Code-artifact targets: install writes ⊆ verify checks (#387).** A `Target` ships CODE (not just a config block) two ways — opencode's TS plugin IS its `config_path` (so `verify_schema` over the config content covers it), and OpenClaw's JS plugin is a separate `extra_artifacts` DIR (so `install::verify_target` STATs each declared artifact for existence — a missing one is a HARD `doctor` break, #332, the silent-dead class the config-level check is blind to). `install_target`'s ENTIRE code-write surface is exactly those two paths, and BOTH are verify-covered. **The invariant for a future 3rd code-artifact target:** any NEW code-shipping path you add to `install_target` MUST gain a matching check in `verify_target`, or `doctor` reports the source HEALTHY while the runtime can't load it. Pinned generically by `verify_target_hard_flags_a_missing_code_artifact_for_every_extra_artifacts_target` (it loops EVERY `extra_artifacts` target, so a future one is auto-guarded — don't special-case OpenClaw back into it).

## Where to look

- "How do hooks get installed?" → `install::claude::merge_install` for the JSON merge logic (CC registers SEVEN events: SessionStart / PreToolUse / PostToolUse / Notification / **SubagentStart / SubagentStop** (#241 — instant subagent register + the only end signal Workflow-fleet subagents get) / SessionEnd; a re-run over an older install ADDS newly registered events idempotently, and uninstall strips them all via the `_pixtuoid` sentinel), `install::io::write_config_atomic` for the safe filesystem write. Multi-target install via the `install::target::Target` registry (`TARGETS = [CLAUDE, CODEX, REASONIX, CODEWHALE, OPENCODE, CURSOR, HERMES, OPENCLAW, GROK]`; grok is the second wholly-owned-file target after opencode — it DROPS `{grok_home}/hooks/pixtuoid.json` (grok scans every `*.json` there, always trusted, no shared-file merge), registers 13 keys incl. BOTH `SubagentStop` AND `SubagentEnd` (upstream's finish site fires the alias, the docs name the former), attributes via the handler `env` map (`PIXTUOID_SOURCE=grok` — the command stays an argument-less absolute path that grok direct-execs on every platform), pins `timeout: 2` per entry (grok awaits hooks INLINE and sequentially), and uninstalls to a sentinel-free `{"hooks":{}}` stub, opencode-style); the user picks a CLI to bind/unbind one row at a time in the in-TUI Sources panel (`s`) — there is no CLI-side target-selection orchestration (the old `plan_targets`/`interactive_pick`/confirm policy was deleted with the `install-hooks` CLI). **Unix:** Claude's `hook_command` emits bare `pixtuoid-hook` (shell-form entry, PATH-resolved at runtime); resolution is soft (warn-only) because the shell resolves it — EXCEPT when the user passed an explicit `--hook-path` (or the `PIXTUOID_HOOK` env override — the flag outranks it, empty/whitespace = unset via `io::nonempty_env`, drive-relative `C:foo.exe` bails, and it EMBEDS exactly like the flag), which always wins: the absolute path is embedded (single-quoted) for Claude too, matching Codex/Reasonix, and the PATH warning is skipped. Default config paths are fallible (`fn() -> Result<PathBuf>`): the home-anchored targets (Claude fallback arm, Reasonix) hard-error via `io::home_relative_checked` when no home dir resolves ("pass --config") instead of writing a CWD-relative file nothing reads; Codex routes through `codex_home()` (always absolute). **Windows:** `hook_command` returns the ABSOLUTE resolved `.exe` path; the hook entry gains `"args": []` (exec form — Git Bash/PowerShell make shell-form unportable); resolution is a HARD error because exec form can't fall back to PATH. Codex embeds the absolute path on all platforms; its hook `command` runs under a shell (`/bin/sh -lc` on Unix, `cmd.exe /C` on Windows — verified in codex-rs `command_runner.rs`, which runs the plain `command` field on every OS, so no `commandWindows` override is written for a locally-generated config). **Unix:** env-prefix form `PIXTUOID_SOURCE=codex '<path>'`. **Windows:** BARE exec form `<path> --source codex` (codex's own documented `command_windows` style). cmd.exe `/C` can't express the env-prefix (it would exec a program literally named `PIXTUOID_SOURCE=codex`), so the source rides as the shim's generic `--source` flag (the flag wins over `PIXTUOID_SOURCE`; either way the shim stamps `_pixtuoid_source`). We must NOT quote the path: codex spawns the hook via `Command::new(cmd.exe).arg("/C").arg(command)`, and `Command::arg`'s Windows escaping turns an embedded `"` into `\"`, which cmd.exe mangles → the hook silently never fires. codex injects no per-hook env we could use instead. A pixtuoid-hook.exe under a path with a SPACE/cmd-metacharacter can't be invoked unquoted, so `hook_cmd::windows::windows_bare_hook_command` substitutes the path's DOS 8.3 SHORT name (`GetShortPathNameW` — space/metachar-free) and only rejects if 8.3 generation is disabled on the volume (#195). The faithful cmd.exe round-trip is pinned by `pixtuoid-hook/tests/shim_pipe.rs::codex_cmd_c_invocation_of_hook_command_stamps_source` (windows-test). **Reasonix** mirrors Codex exactly (it also shells hooks via `cmd.exe /c` on Windows — `hook.go:414`): Unix env-prefix `PIXTUOID_SOURCE=reasonix`, Windows bare `<path> --source reasonix`. Both Codex and Reasonix route their Windows arm through **`hook_cmd::windows::windows_bare_hook_command(path, source)`** — the ONE place the bare-form + the cmd-unsafe-path handling (8.3 short-name substitution via `GetShortPathNameW`, else reject; chars `space tab ; , = " & | < > ( ) ^ %` — the first five are cmd.exe first-token DELIMITERS, and `; , =` are legal NTFS filename chars, #195) lives, so a security guard can't drift between targets. (Claude is the odd one out — it uses Windows *exec form* with an `args` array, not a shell string.) Reasonix's settings shape is its own FLAT per-event array (`{match?, command, timeout(ms), description}` — NOT Claude's nested `{matcher, hooks:[…]}`), the file is the GLOBAL `<reasonix-home>/settings.json` — `~/.reasonix/settings.json` on macOS/Linux but **`%APPDATA%\reasonix\settings.json` on Windows** (Reasonix's `ReasonixHomeDir` is platform-ASYMMETRIC: Go `os.UserConfigDir()/reasonix` on Windows, `~/.reasonix` elsewhere, `REASONIX_HOME` override; `reasonix::reasonix_home` mirrors it — writing the generic `~/.reasonix` on Windows would land hooks where Reasonix never reads, the %APPDATA% axis of the same Windows bug class) — project scope only loads after `/hooks trust` (a project-scope install would silently never fire), and `match` is OMITTED = every tool (upstream special-cases `"*"` to every-tool too; any other value is an ANCHORED regex where a malformed pattern never fires — omission is the simplest always-fires form); detection uses `reasonix::detect_installed` (a `presence_probe` on the Target) because Reasonix never creates the settings.json we write — it probes the v2 config dir (`os.UserConfigDir()/reasonix`) and `~/.reasonix` instead. The hook entry detection/uninstall keys on the `_pixtuoid` sentinel, not the command shape, so both forms are idempotent. **CodeWhale** writes `~/.codewhale/config.toml` (or legacy `~/.deepseek/config.toml` when that is the file it reads — mirrors CodeWhale's own `default_config_path` so we don't shadow the user's provider/key config), and the `~` is resolved by `pixtuoid_core::platform::home_first_dir` — **HOME-FIRST then USERPROFILE on Windows**, the OPPOSITE of pixtuoid's generic `USERPROFILE`-first `io::home_relative_checked`, because CodeWhale's own `effective_home_dir` is HOME-first; without this a Windows user who exports `HOME` (Git Bash/MSYS2/Cygwin) gets hooks written to `%USERPROFILE%\.codewhale\` while CodeWhale reads `%HOME%\.codewhale\` → installed-but-no-sprite. `default_config_path` mirrors CodeWhale's FULL precedence: `CODEWHALE_CONFIG_PATH`→`DEEPSEEK_CONFIG_PATH` (file overrides, absolute-only) → the modern app dir = `CODEWHALE_HOME` VERBATIM (it IS the `.codewhale`-equivalent dir, not a home base) else `<home>/.codewhale`, with the legacy `.deepseek` anchored to the OS home regardless of `CODEWHALE_HOME` (never shadow a real `.deepseek` config). **OpenClaw shares the same `home_first_dir`**: its `infra/home-dir.ts` resolves `$HOME ?? $USERPROFILE ?? os.homedir()` (HOME-first), so `install/openclaw.rs` mirrors `resolveStateDir`+`resolveConfigPath`: `openclaw_state_dir` = `OPENCLAW_STATE_DIR` → `OPENCLAW_HOME` → `home_first_dir()`, then prefer existing `.openclaw` else legacy `.clawdbot`; `default_config_path` = `OPENCLAW_CONFIG_PATH` → existing `openclaw.json` else legacy `clawdbot.json` in that state dir (was generic USERPROFILE-first `~/.openclaw/openclaw.json` — the same Windows bug + env/legacy gaps). All three OpenClaw overrides are `~`-EXPANDED against that home via `io::expand_tilde` (OpenClaw's `resolveUserPath`/`resolveRawHomeDir` apply `^~(?=$|[/\\])`, #342) — and `detect_installed` applies the SAME expansion to the same env vars so install and detect can't diverge (a `~`-prefixed override that installs into `<home>/…` is also PROBED there, never at a literal `~/…` that exists nowhere → no "installed but the Sources panel won't offer it"). `CodeWhale`/`Reasonix` instead only TRIM their overrides (no `~`-expand — `cleanEnvDir`/`val.trim()`), so they pass `io::expand_tilde(.., None)`; don't "simplify" by blanket-expanding all targets — that would land a verbatim-taking CLI's hooks at the wrong path. Every OTHER target's CLI uses a USERPROFILE-first stdlib home, so they correctly stay on `io::home_relative` (which calls the generic `pixtuoid_core::platform::user_home_opt` directly — the former in-`io` home wrapper was deleted, so callers reach `user_home_opt` straight from `pixtuoid_core::platform`). `pixtuoid doctor` emits a Windows `home_split_advisory` when `HOME`≠`USERPROFILE` — the host condition under which any residual resolver mismatch would bite. The config is a `[hooks]` table with `enabled = true` plus a `hooks` ARRAY of `{event, command, _pixtuoid}` entries. Unlike Codex/Reasonix (one command for all events), CodeWhale sets NO event env var, so `hook_command` returns the BASE form (`PIXTUOID_SOURCE=codewhale '<path>'` / Windows bare `<path> --source codewhale`) and `merge_install` appends ` --event <name>` PER ENTRY; the shim's env-mode reads that flag + `DEEPSEEK_*` env (it must NOT read stdin — CodeWhale leaves env-only hooks' stdin = the TUI terminal). `enabled = true` is set on install ONLY when the key is ABSENT (CodeWhale gates all hooks on it, so a fresh install must enable them); an explicit `enabled = false` is the user's own global "all hooks off" switch and is left UNTOUCHED — we can't faithfully restore it on disconnect (no per-source install state since 0.12.0), so flipping it would permanently re-enable the user's OWN hooks. Ours then don't fire, but the verify/`doctor` `[hooks].enabled = false — none fire` note surfaces exactly that (not a silent no-sprite). Uninstall drops the `[hooks]` table only when nothing but our `enabled = true` remains — a surviving `enabled = false` (the user's) keeps its table. Detection probes `~/.codewhale`/`~/.deepseek` (presence_probe), and the `_pixtuoid` sentinel drives idempotency (the per-event command's last token is the event, so a command-basename fallback wouldn't apply). **opencode** is the odd one out — NOT a config block but a CODE artifact: opencode has no config-level shell hook (and SQLite-only sessions), so pixtuoid ships a TS plugin. opencode auto-discovers `<config>/plugins/*.{ts,js}` (plural — the canonical docs' dir; the anomalyco fork globs `{plugin,plugins}` so both work there, but plural is what canonical scans), so the target DROPS `<opencode-config>/plugins/pixtuoid.ts` (`OPENCODE_CONFIG_DIR` > `$XDG_CONFIG_HOME/opencode` > `~/.config/opencode`) — NO edit to the user's `opencode.jsonc`. The bundled `opencode_plugin.ts` (shim absolute path baked in JSON-escaped) pipes the lifecycle/tool/permission EventV2 stream to the shim on stdin (`--source opencode`). The plugin file is wholly pixtuoid's, so `merge_install` renders the whole file (changed = content diff, so a same-path re-install is a no-op) and reuses the ConfigLock/backup/atomic-write machinery; `merge_uninstall` writes a sentinel-free no-op stub (`export {}`) rather than deleting (the write-only orchestrator can't delete — an ACCEPTED residual; the stub is a harmless empty module), keying on the `@pixtuoid-opencode-plugin` sentinel for its changed-detection. Auto-detect (`presence_probe`/`detect_installed`) probes the opencode CLI's OWN dirs (`<config>` + `~/.local/share/opencode`), NOT our plugin file — keying on our own artifact would chicken-and-egg (opencode could never be auto-detected until after we'd installed into it), the same reason Reasonix/CodeWhale probe their CLI dirs. `hook_command` returns the absolute path (Err on non-UTF-8); `binary_strategy = EmbedAbsolute` (opencode runs the plugin under Bun, no PATH reliance — the per-target binary-resolution strategy is the `BinaryStrategy { BareNameOnPath | EmbedAbsolute }` enum on `Target`, replacing the old `needs_path_warning`/`needs_resolved_binary` bool pair). **Structured core, one DIRECT presenter of the install reports:** `install::install_target`/`uninstall_target` are the pure ConfigLock round (read→merge→backup→write, invariant #4 intact) returning an `InstallReport`/`UninstallReport`; the in-TUI **Sources panel** (`s` — see `src/tui/CLAUDE.md`) is the sole presenter that renders those reports directly (one-line result, hook path resolved via `hook_path=None`/`config=None`). There is no install-hooks CLI presenter anymore — `run_install`/`run_uninstall` were deleted with the `install-hooks`/`uninstall-hooks` subcommands. (No contradiction with the two-presenter sharp edge above: the scriptable `connect`/`disconnect` CLI is a second presenter of `crate::sources` OUTCOMES, one layer up — it surfaces install results only folded into those outcome rows, never the raw reports.) `has_hooks` is `pub(crate)` — its old cross-crate consumer (main.rs feeding `config::resolve_connected`'s migrate-default) went away when 0.12.0 dropped that inference; its callers are now `doctor` (both `diagnose`'s verify gate and `run`'s per-source `hooks_installed` report row) and `sources::skip_freeze` (the onboarding-skip freeze probes it so a pre-0.12 upgrader's hooks survive a skip).
- "How does the default character pack get into the binary?" → `pixtuoid_scene::embedded_pack` does the `include_str!` at compile time, now from the SCENE crate's own `crates/pixtuoid-scene/sprites/default/` (watched by `pixtuoid-scene`'s `build.rs` for rerun-if-changed); `sprite::format::load_pack_from_strings` parses it. (The binary's `init_pack` separately embeds `sprites/skeleton/` for `init-pack`.)
- "How do custom sprite packs work?" → `pixtuoid init-pack ./dir` extracts the skeleton template from `sprites/skeleton/` (embedded via `include_str!`, see `init_pack.rs`). `pixtuoid validate-pack ./dir` loads the pack and checks against `REQUIRED_CHARACTER_ANIMATIONS` / `OPTIONAL_*` registries in `sprite::format`. `--pack-dir` CLI flag or `pack-dir` config key loads a custom pack at runtime. Custom packs only need character sprites — furniture/environment animations are merged from the embedded default via `Pack::merge_from()` (only `OPTIONAL_FURNITURE_ANIMATIONS`, never character poses). The robot pack at `sprites/robot/` is a TV-head character set (10×12 sprites).
- "How does the crash log work?" → `crash.rs::install_crash_hook` (a bin-crate module, installed first thing by `main()`) sets a panic hook that restores the terminal, writes a timestamped backtrace to `~/.cache/pixtuoid/crash.log`.
- "Where do runtime errors / config warnings surface?" (#157, #87) → THREE sinks, by failure class. (1) **Always-on file log**: TUI mode installs a warn-floor file subscriber unconditionally (`logging.rs::init`; the alternate screen owns the terminal, so the file is the only runtime channel) — every `tracing::warn!/error!` (source deaths, decode failures, config warns) lands in `~/.cache/pixtuoid/log` (or `$XDG_STATE_HOME`/`$PIXTUOID_LOG`), one-deep rotated at 5MB. (2) **Pre-altscreen stderr**: `config.rs` resolvers (`load`/`resolve_theme`/`resolve_pets`) stay layer-clean and COLLECT human-readable warnings into a `&mut Vec<String>`; `main.rs` eprintlns them before `setup_terminal` (visible in scrollback after exit — the crash hook's channel; skipped in headless where the stderr tracing subscriber already shows them). (3) **In-TUI source-health footer**: a fatal `Source::run` exit is published as `SourceDeath` on a `watch` side channel (`SourceManager::spawn_with_health` — deliberately NOT an `AgentEvent`; the one event channel carries agent activity, invariant #2), threaded `driver.rs → run_tui → TuiRenderer::set_source_warning`, and `footer.rs::source_warning_message` renders a persistent footer warning that replaces the stats tier (the stats go stale when a transport dies) and survives every width + floor transitions.
- "How does config persistence work?" → `config.rs` defines `AppConfig` (theme + optional max-desks cap + pack-dir + `last_seen_version` (the version-popup stamp) + `[sources]` connected flags + `[floating]` window geometry (size/position/opacity — opacity is parsed + clamped to [0.2, 1.0] but an honest v1 no-op) + `[[pets]]`), `config_path()` (XDG-aware: `$XDG_CONFIG_HOME/pixtuoid/config.toml` or `~/.config/pixtuoid/config.toml`; an empty, whitespace, or RELATIVE `XDG_CONFIG_HOME` counts as unset (the basedir spec defines EMPTY as unset AND requires the value be an ABSOLUTE path) — the `io::nonempty_abs_env` filter (#610), also used for `XDG_STATE_HOME` in `crash.rs`/`logging.rs`; `PIXTUOID_HOOK` stays on the plain `io::nonempty_env` since it may legitimately be relative), `load()` (never crashes — logs warning on malformed TOML), `save()`/`save_version()` (route through `install/io.rs`'s ConfigLock — one lock across the whole read→mutate→write, fsync + atomic rename + perms preservation; they edit the raw document via `toml_edit`, so unknown keys and the user's comments/formatting survive; a config that EXISTS but doesn't parse is NEVER rewritten — the save errs and both callers warn-and-continue, and the first overwrite of an existing file takes a one-time `config.toml.pixtuoid.bak`), `resolve_theme()` (CLI > config > default; returns `Result<&'static Theme>` — the ONE place themes are validated: a `--theme` CLI typo is a HARD error listing valid names, a config typo is a soft warn+fallback so a stale config never bricks startup; `runtime::RunConfig` carries the already-resolved `&'static Theme`, so an unknown theme can't reach the runtime by construction). Theme saved on `[t]` picker Enter confirm in `tui/mod.rs`. `max-desks` is an optional cap — when set, auto-compute clamps each floor's capacity to `min(layout_capacity, cap)`. When absent, fully auto-computed from terminal size. `pack-dir` supports `~` expansion via `resolve_pack_dir`. **Per-source connection is `[sources]`** — a `BTreeMap<String, bool>` (`source_id → connected`), persisted by `save_source_connected` (via the same `toml_edit` ConfigLock path; `skip_serializing_if = is_empty` so a fresh config omits the table). **`resolve_connected(&AppConfig) -> HashSet<String>`** (pure, FS-free) is the boot seed for `runtime::ConnectedSources`: a source is connected iff its `[sources]` flag is an explicit `true` — an absent flag (or table) is plainly disconnected. (The v0.4–0.7 "absent flag MIGRATES from the install state" inference — connected-iff-hooks-installed via `install::has_hooks`, no-target sources defaulting connected — was DROPPED in 0.12.0: those configs are too old to keep supporting. Consequence, encoded in `setup.rs` tests: such an upgrader's config reads as a first run, so onboarding replays and IS their re-connect flow.) The Sources panel (`s`) is the only writer; the reducer task is the reader (gate + reconcile). **Pets are `[[pets]]` array-of-tables** — each `PetEntry { kind: String, name: Option<String> }`; `kind` is a raw String (NOT a serde `PetKind`) ON PURPOSE so a typo is warn-skipped in `resolve_pets`, not fatal (a typed enum would fail the whole `toml::from_str` → `load`'s malformed arm wipes EVERY setting). **`resolve_pets(&AppConfig) -> Vec<Pet>`** maps the stanzas → `Vec<Pet>` (`Pet { kind, name }`): absent `pets` → all kinds with default names; `pets = []` → none; unknown `kind` → warn+skip (non-fatal); `name` trimmed, empty/absent → `PetKind::default_name()`. No runtime kind→name map — the name rides on each `Pet`, so the renderer reads `pet.name` directly. No `enabled-pets`/`[pet-names]` keys (removed; backward compat is a non-goal). **`pets` MUST stay the LAST field in `AppConfig`** by convention — an array-of-tables serializes cleanest after all scalar keys (where `pet_names` used to sit); don't rely on toml's key/table interleaving.
- "How do multi-floor offices work?" → `pixtuoid_scene::floor` defines `FloorCtx` (per-floor render state: router/cache/overlay/history/**light** [LightingState]/motion) + `FloorTransition` (slide animation) + `build_floor_scene()` (agent projection — the engine half, see [`../pixtuoid-scene/CLAUDE.md`](../pixtuoid-scene/CLAUDE.md)). `tui_renderer/mod.rs` owns `Vec<PerFloor>` + one `PerOffice` (the session halves) and switches between them. Floor membership is stored on `AgentSlot.floor_idx` (set once by the reducer at desk assignment, immutable thereafter). Each floor's capacity is **boot-seeded from the actual terminal size** via `compute_boot_capacities()` in `runtime/driver.rs` (queries `crossterm::terminal::size()` at startup, falls back to `FALLBACK_DESKS=16` in headless mode or on error). Per-frame, `tui/mod.rs` derives each floor's capacity via `pixtuoid_scene::floor::floor_seed` + `floor_capacity` (uncapped by default since #421; min-clamped by the optional `desk_cap`) and writes the result via per-floor `AtomicUsize::fetch_max` (monotone growth — capacity never shrinks, preventing cumulative-offset shifts that would remap floor 1+ agents to wrong desks). The reducer syncs all `MAX_FLOORS` capacities into `scene.floor_capacities: [usize; MAX_FLOORS]` each tick. `next_free_desk` in `state/mod.rs` scans `0..total_capacity()`. On terminal shrink, agents beyond the layout's capacity become invisible but stay alive; they reappear when the terminal grows back. PageUp/PageDown/↑↓/jk in `tui/mod.rs`. Agents past a floor's capacity overflow to additional floors (up to `MAX_FLOORS=10`; only 5 layout variants exist, so floors 6-10 cycle through the same 5 looks).
