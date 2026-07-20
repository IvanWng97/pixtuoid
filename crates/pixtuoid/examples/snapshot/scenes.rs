use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use pixtuoid_core::source::jsonl::JsonlWatcher;
use pixtuoid_core::source::AgentEvent;
use pixtuoid_core::state::{ActivityState, ToolKind};
use pixtuoid_core::{AgentId, AgentSlot, GlobalDeskIndex, Reducer, SceneState, Transport};
use tokio::sync::{mpsc, RwLock};

/// BFS from `layout.door_threshold` and count visited vs total walkable
/// pixels. If the two differ, the mask has multiple connected components
/// — that's the structural cause of A*'s "no path found" fallback, which
pub(crate) async fn capture_live_scene(
    projects_root: &str,
    listen_secs: u64,
) -> Result<SceneState> {
    println!(
        "listening for real CC events under {} for {}s...",
        projects_root, listen_secs
    );
    let scene: Arc<RwLock<SceneState>> = Arc::new(RwLock::new(SceneState::uniform(12)));
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(1024);
    let tx_hook = tx.clone();
    let root = PathBuf::from(projects_root);
    let mut watcher = JsonlWatcher::new(
        root.clone(),
        pixtuoid_core::source::claude_code::SOURCE_NAME.to_string(),
        pixtuoid_core::source::claude_code::decode_cc_line,
        pixtuoid_core::source::claude_code::cc_session_ended,
    )
    .with_label_deriver(pixtuoid_core::source::claude_code::cc_derive_label)
    // The SAME UUID keying the real CC source wires (claude_code/native.rs):
    // without it the watcher registers under the path-hash id while every
    // line-decoded event (activity, ModelInfo) keys on the UUID — nothing
    // coalesces and --live agents sit Idle/model-less forever (latent since
    // #203, exposed by the burn-tier replay).
    .with_id_deriver(pixtuoid_core::source::claude_code::cc_id_from_path);
    // The SAME liveness probe as the real source — the same harness-vs-app
    // parity gap as the id-deriver above (a JsonlWatcher builder call the
    // real CC source makes and this harness omitted): without the probe an
    // in-flight session first-sights at EOF via the #204 gate (no history
    // replay), so session-cumulative state — tokens_used above all — reads
    // ZERO from attach and a --live capture renders no paper tower for a
    // session that burned millions (caught by the #632 dogfood).
    if let Some(sessions_dir) = pixtuoid_core::source::cc_registry_dir(&root) {
        watcher = watcher.with_liveness_probe(std::sync::Arc::new(move || {
            pixtuoid_core::source::claude_code::live_cc_session_ids(&sessions_dir)
        }));
    }
    let watcher_handle = tokio::spawn(async move { watcher.run(tx).await });
    // ALSO listen on the real hook socket: a live CC session's hooks carry
    // activity + the burn-tier effort level (hooks.md `effort:{level}`;
    // ultracode reports as xhigh) that the transcript alone can't provide in
    // real time. Same latent-gap class as the id-deriver above — "--live"
    // without hooks was silently activity-poor. SocketBusy (a running
    // pixtuoid owns the socket) degrades to transcript-only, like the app.
    let hook_handle = {
        use pixtuoid_core::source::hook::HookRouter;
        use pixtuoid_core::source::DynSource;
        let socket = pixtuoid_core::source::claude_code::ClaudeCodeSource::default_socket_path();
        let router: Box<dyn DynSource> = Box::new(HookRouter::new(socket));
        let tx = tx_hook;
        tokio::spawn(async move {
            if let Err(e) = router.run(tx).await {
                eprintln!("hook listener unavailable ({e}); transcript-only capture");
            }
        })
    };

    let mut reducer = Reducer::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(listen_secs);
    let mut event_count: u64 = 0;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some((transport, ev))) => {
                let now = SystemTime::now();
                let mut s = scene.write().await;
                reducer.apply(&mut s, ev, now, transport);
                event_count += 1;
            }
            _ => break,
        }
    }
    let snapshot = scene.read().await.clone();
    println!(
        "captured {} events; final scene has {} agents",
        event_count,
        snapshot.agents.len()
    );
    for (id, slot) in &snapshot.agents {
        println!(
            "  {} ({}) at desk {}: {:?} model={:?} effort={:?}",
            slot.label,
            id,
            slot.desk_index.0,
            slot.state,
            slot.model,
            slot.effort.as_ref().map(|e| e.value.clone())
        );
    }
    watcher_handle.abort();
    hook_handle.abort();
    Ok(snapshot)
}

pub(crate) fn sample_scene(now: SystemTime, max_desks: usize, n_agents: usize) -> SceneState {
    let mut s = SceneState::uniform(max_desks);
    fill_sample_agents(&mut s, now, 0..n_agents);
    s
}

/// Inject an OpenClaw gateway presence for the beautify visual loop — drives
/// the wandering lobster mascot. `state` ∈ {idle, busy, down}; off the gen-media
/// path so baselines hold.
pub(crate) fn inject_openclaw_presence(
    s: &mut SceneState,
    state: &str,
    now: SystemTime,
) -> Result<()> {
    use pixtuoid_core::state::{DaemonLiveness, DaemonPresence};
    // Busy carries in-flight RUN keys (two, for a lively demo stream) — Busy is
    // DERIVED from the run set (#460), so "busy" = UP + runs, not a stored state.
    let (liveness, active_sessions, runs) = match state {
        "idle" => (DaemonLiveness::UP, 1, Vec::new()),
        "busy" => (
            DaemonLiveness::UP,
            1,
            vec!["run-a".to_string(), "run-b".to_string()],
        ),
        // #317: the gateway is up but its model backend is failing every run —
        // a sickly-red, sluggishly-wandering lobster (no in-flight runs: the last
        // one FAILED out of the set).
        "degraded" => (DaemonLiveness::Up { degraded: true }, 1, Vec::new()),
        "down" => (DaemonLiveness::Down, 0, Vec::new()),
        other => {
            anyhow::bail!("unknown --openclaw {other:?}; valid: idle | busy | degraded | down")
        }
    };
    // `entered_at` ~20s in the past → past the enter animation, so a static
    // snapshot captures the steady wander (not the walk-in). For `down`,
    // `last_seen = now` so the frame catches the start of the walk-out.
    let entered_at = now
        .checked_sub(std::time::Duration::from_secs(20))
        .unwrap_or(now);
    s.daemons_mut().insert(
        pixtuoid_core::source::openclaw::SOURCE_NAME.to_string(),
        DaemonPresence {
            liveness,
            active_sessions,
            last_seen: now,
            entered_at,
            in_flight_run_keys: runs.into_iter().collect(),
            current_pid: Some(4242),
        },
    );
    Ok(())
}

/// Insert the standard sample-scene archetypes at desk indices `desks`
/// (`i % 12` cycles the archetype list). Split out of `sample_scene` so
/// `meeting_scene` can fill desks N.. around its staged agents.
/// One exemplar per token-meter tier, each one step past its threshold. The
/// values cross a crate boundary (the tier consts are scene-internal), so the
/// debug_assert in `fill_sample_agents` pins them to the REAL ladder — a tier
/// recalibration fails the next snapshot run instead of silently shrinking
/// the gallery's spread.
const TIER_EXEMPLARS: [u64; 4] = [0, 400_000, 3_000_000, 20_000_000];

fn fill_sample_agents(s: &mut SceneState, now: SystemTime, desks: std::ops::Range<usize>) {
    debug_assert!(
        TIER_EXEMPLARS
            .iter()
            .map(|&t| pixtuoid_scene::token_meter::token_tier(t))
            .eq(0u8..=3),
        "gallery exemplars must span all four tiers"
    );
    use std::time::Duration as D;
    let agents: [(&str, ActivityState, D); 12] = [
        (
            "working",
            ActivityState::Active {
                tool_use_id: Some("tu_a".into()),
                detail: Some("Write: src/foo.rs".into()),
                kind: ToolKind::Edit,
            },
            D::from_millis(0),
        ),
        (
            "waiting",
            ActivityState::Waiting {
                reason: "permission?".into(),
            },
            D::from_secs(10),
        ),
        ("thinking", ActivityState::Idle, D::from_secs(5)), // 5s ago — within thinking window
        ("idle-a", ActivityState::Idle, D::from_secs(300)), // 5 min — wander/sleep cycle
        ("idle-b", ActivityState::Idle, D::from_secs(301)),
        ("idle-c", ActivityState::Idle, D::from_secs(303)),
        (
            "couch-act",
            ActivityState::Active {
                tool_use_id: Some("tu_c".into()),
                detail: Some("Read: README.md".into()),
                kind: ToolKind::Read,
            },
            D::from_millis(140),
        ),
        (
            "couch-bk",
            ActivityState::Waiting {
                reason: "review".into(),
            },
            D::from_millis(0),
        ),
        (
            "floor-act",
            ActivityState::Active {
                tool_use_id: Some("tu_d".into()),
                detail: Some("Bash: cargo test".into()),
                kind: ToolKind::Bash,
            },
            D::from_millis(140),
        ),
        ("floor-idle", ActivityState::Idle, D::from_millis(2_000)),
        (
            "floor-act2",
            ActivityState::Active {
                tool_use_id: Some("tu_e".into()),
                detail: Some("Grep: TODO".into()),
                kind: ToolKind::Search,
            },
            D::from_millis(280),
        ),
        ("floor-idle2", ActivityState::Idle, D::from_millis(3_000)),
    ];
    const DEMO_REPOS: [&str; 3] = ["/demo/api", "/demo/web", "/demo/infra"];
    for i in desks {
        let (key, state, age) = &agents[i % agents.len()];
        // Keys must be unique across the full desk range: bare key for the first
        // pass over the archetypes, suffixed once they cycle so each desk slot gets
        // its own AgentId and BTreeMap entry.
        let unique_key = if i < agents.len() {
            key.to_string()
        } else {
            format!("{key}-{i}")
        };
        let cwd_str = DEMO_REPOS[i % DEMO_REPOS.len()];
        let id = AgentId::from_transcript_path(&format!("/demo/{unique_key}.jsonl"));
        s.agents.insert(
            id,
            AgentSlot {
                agent_id: id,
                source: std::sync::Arc::from("claude-code"),
                session_id: std::sync::Arc::from(format!("demo-{unique_key}").as_str()),
                cwd: std::sync::Arc::from(PathBuf::from(cwd_str).as_path()),
                label: unique_key.as_str().into(),
                state: state.clone(),
                state_started_at: now - *age,
                created_at: now - *age,
                last_event_at: now - *age,
                exiting_at: None,
                pending_idle_at: None,

                desk_index: GlobalDeskIndex(i),
                // floor_of maps the global desk_index to the correct floor based on
                // per-floor capacities; hardcoding 0 would leave overflow agents invisible.
                floor_idx: s.floor_of(GlobalDeskIndex(i)),
                tool_call_count: 0,
                active_ms: 0,
                unknown_cwd: false,
                parent_id: None,
                pid: None,
                model: None,
                effort: None,
                // Token-meter gallery spread (#632): cycle the desks across
                // all four tiers so the paper towers show in every capture —
                // exemplar values one step past each threshold, pinned below
                // so a tier recalibration can't silently shrink the spread.
                tokens_used: TIER_EXEMPLARS[i % 4],
                last_usage: None,
            },
        );
    }
}

/// One (agent, cycle) whose deterministic wander destination is a meeting-room
/// slot — a candidate for `--meeting` staging.
#[derive(Debug, Clone)]
struct MeetingCandidate {
    path: String,
    id: AgentId,
    cycle_n: u64,
    wp_idx: usize,
    room_id: usize,
    is_sofa: bool,
    /// The room's bottom-most meeting seat renders the sitter in BACK view,
    /// mostly occluded behind the table — a staged group should avoid it so
    /// all sprites read on camera (review finding; the seat itself is fine
    /// for organic wander).
    is_south_seat: bool,
    dwell_ms: u64,
}

/// Max per-group spread of the staged agents' desk dwells (ms) — they should
/// rise from their desks near-together so arrivals overlap well inside the
/// 20–40s meeting dwell.
const MEETING_DWELL_SPREAD_MS: u64 = 3_000;

/// Encoded clip starts this long before the earliest staged rise.
const MEETING_WARMUP_LEAD_MS: u64 = 1_500;

/// Build a scene where `n` agents (desks 0..n) are staged to converge on ONE
/// meeting room: each agent's `state_started_at` is back-dated by
/// `cycle_n * est_wander_cycle_ms + ε` so motion's bootstrap fast-forward
/// selects a cycle whose deterministic `waypoint_index_for_cycle` lands on a
/// DISTINCT slot of the same room (so they don't fight for one seat). Motion
/// deliberately restarts the phase clock on first observation (anti-teleport),
/// so every staged agent still sits out its full `seated_dwell_ms` before
/// rising — the returned warmup (min dwell − 1.5s) pre-rolls the capture to
/// just before the first rise. Desks n..n_agents get the sample archetypes.
pub(crate) fn meeting_scene(
    now: SystemTime,
    n: usize,
    cols: u16,
    rows: u16,
    floor_seed: u64,
    max_desks: usize,
    n_agents: usize,
) -> Result<(SceneState, u64)> {
    use pixtuoid_scene::layout::{SceneLayout, WaypointKind};
    use pixtuoid_scene::pose::{
        est_wander_cycle_ms, is_aimless_cycle, seated_dwell_ms, takes_trip,
        waypoint_index_for_cycle,
    };

    // Match the renderer's layout EXACTLY (terminal minus 1-row footer,
    // half-block doubling) — same convention as anim_scene. `None` = the same
    // desk fill draw_scene passes, or the waypoint indices shift and the
    // staging silently misses.
    let (buf_w, buf_h) = (cols, rows.saturating_sub(1).saturating_mul(2));
    let l = SceneLayout::compute_with_seed(buf_w, buf_h, None, floor_seed)
        .ok_or_else(|| anyhow::anyhow!("--meeting: scene too small to compute a layout"))?;
    let nw = l.waypoints.len();

    // Candidate sweep: deterministic synthetic ids; for each, the LOWEST cycle
    // (≥1 — cycle 0's 400ms back-date sits inside ENTRY_ANIMATION_MS, so the
    // door entry-walk override would hijack the staging; the thinking window
    // can never fire here since last_event_at == created_at) whose trip
    // deterministically lands on a meeting slot. The sweep trusts motion's
    // approach_point fallback for reachability: a boxed-in seat degrades to an
    // aimless amble, caught by the visual check at `just gen` time, not here.
    // Per room, the bottom-most (max-y) meeting seat seats its sitter in BACK
    // view behind the table — flag it so the picker can avoid staging there.
    let south_y_of_room = |room: usize| -> Option<u16> {
        l.waypoints
            .iter()
            .filter(|w| {
                w.room_id == Some(room)
                    && matches!(
                        w.kind,
                        WaypointKind::MeetingSofa | WaypointKind::MeetingChair
                    )
            })
            .map(|w| w.pos.y)
            .max()
    };

    let mut cands: Vec<MeetingCandidate> = Vec::new();
    for i in 0..20_000u64 {
        let path = format!("/meeting/agent_{i}.jsonl");
        let id = AgentId::from_transcript_path(&path);
        for cycle_n in 1..=5u64 {
            if !takes_trip(id, cycle_n) || is_aimless_cycle(id, cycle_n) {
                continue;
            }
            let wp_idx = waypoint_index_for_cycle(id, cycle_n, nw);
            let wp = l.waypoints[wp_idx];
            let is_sofa = match wp.kind {
                WaypointKind::MeetingSofa => true,
                WaypointKind::MeetingChair => false,
                _ => continue,
            };
            let room_id = wp.room_id.unwrap_or(0);
            cands.push(MeetingCandidate {
                path,
                id,
                cycle_n,
                wp_idx,
                room_id,
                is_sofa,
                is_south_seat: south_y_of_room(room_id) == Some(wp.pos.y),
                dwell_ms: seated_dwell_ms(id),
            });
            break;
        }
    }
    cands.sort_by_key(|c| (c.dwell_ms, c.id.raw()));

    // Lowest-dwell window of n candidates with: same room, distinct slots,
    // dwell spread ≤ 3s. Prefer windows seating ≥2 on sofas (reads "meeting"
    // far better than a stand-up cluster); fall back without that bias.
    let pick = |need_sofas: usize, avoid_south: bool| -> Option<Vec<MeetingCandidate>> {
        for (i, base) in cands.iter().enumerate() {
            if avoid_south && base.is_south_seat {
                continue;
            }
            let mut sel = vec![base.clone()];
            for c in cands[i + 1..].iter() {
                if c.dwell_ms - base.dwell_ms > MEETING_DWELL_SPREAD_MS {
                    break;
                }
                if (avoid_south && c.is_south_seat)
                    || c.room_id != base.room_id
                    || sel.iter().any(|s| s.wp_idx == c.wp_idx)
                {
                    continue;
                }
                sel.push(c.clone());
                if sel.len() == n {
                    break;
                }
            }
            if sel.len() == n && sel.iter().filter(|c| c.is_sofa).count() >= need_sofas {
                return Some(sel);
            }
        }
        None
    };
    let staged = pick(2.min(n), true)
        .or_else(|| pick(2.min(n), false))
        .or_else(|| pick(0, false))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "--meeting {n}: no candidate group found ({} meeting-bound candidates at \
             {buf_w}x{buf_h} seed {floor_seed})",
                cands.len()
            )
        })?;

    let min_dwell = staged.iter().map(|c| c.dwell_ms).min().unwrap_or(0);
    let warmup_ms = min_dwell.saturating_sub(MEETING_WARMUP_LEAD_MS);

    let mut s = SceneState::uniform(max_desks);
    for (i, c) in staged.iter().enumerate() {
        let wp = l.waypoints[c.wp_idx];
        eprintln!(
            "MEETING agent {} desk {} id={} cycle={} → waypoint[{}] {:?} room {} at ({}, {}) \
             desk_dwell={}ms rise@{}ms",
            (b'a' + i as u8) as char,
            i,
            c.path,
            c.cycle_n,
            c.wp_idx,
            wp.kind,
            c.room_id,
            wp.pos.x,
            wp.pos.y,
            c.dwell_ms,
            c.dwell_ms.saturating_sub(warmup_ms),
        );
        // Back-date past `cycle_n` whole estimated cycles (+ a hair so the
        // integer division can't land on the boundary). created_at matches so
        // the entry-walk override can't fire.
        let back_date = Duration::from_millis(c.cycle_n * est_wander_cycle_ms(c.id) + 400);
        let label = format!("meet-{}", (b'a' + i as u8) as char);
        s.agents.insert(
            c.id,
            AgentSlot {
                agent_id: c.id,
                source: std::sync::Arc::from("claude-code"),
                session_id: std::sync::Arc::from(format!("meeting-{i}").as_str()),
                cwd: std::sync::Arc::from(PathBuf::from("/meeting").as_path()),
                label: label.as_str().into(),
                state: ActivityState::Idle,
                state_started_at: now - back_date,
                created_at: now - back_date,
                last_event_at: now - back_date,
                exiting_at: None,
                pending_idle_at: None,
                desk_index: GlobalDeskIndex(i),
                floor_idx: s.floor_of(GlobalDeskIndex(i)),
                tool_call_count: 0,
                active_ms: 0,
                unknown_cwd: false,
                parent_id: None,
                pid: None,
                model: None,
                effort: None,
                tokens_used: 0,
                last_usage: None,
            },
        );
    }
    // Fill the rest of the office with the normal archetypes so the floor
    // doesn't look dead around the meeting.
    fill_sample_agents(&mut s, now, n..n_agents);
    eprintln!("MEETING staged {n} agents, warmup={warmup_ms}ms (min desk_dwell {min_dwell}ms − {MEETING_WARMUP_LEAD_MS}ms)");
    Ok((s, warmup_ms))
}

/// Build a representative 5-agent scene for the `--dashboard` demo: a CC parent
/// with 2 subagents, a Codex root, and a Reasonix root — distinct badges and
/// varied activity states.
pub(crate) fn dashboard_scene(now: SystemTime) -> SceneState {
    use pixtuoid_core::source::{claude_code, codex, reasonix};
    use pixtuoid_core::state::{ActivityState, ToolKind};
    use std::time::Duration as D;

    // Named to satisfy clippy::type_complexity (a 6-field tuple trips the lint):
    // (label, transcript path, state, parent_id, desk_index, source SOURCE_NAME).
    type DashAgentSpec = (
        &'static str,
        &'static str,
        ActivityState,
        Option<AgentId>,
        usize,
        &'static str,
    );

    let mut s = SceneState::uniform(12);

    let cc_root_id = AgentId::from_transcript_path("/demo/dash_cc_root.jsonl");

    let agents: &[DashAgentSpec] = &[
        (
            "cc·pixtuoid",
            "/demo/dash_cc_root.jsonl",
            ActivityState::Active {
                tool_use_id: Some("tu0".into()),
                detail: Some("Edit: reducer.rs".into()),
                kind: ToolKind::Edit,
            },
            None,
            0,
            claude_code::SOURCE_NAME,
        ),
        (
            "code-explorer",
            "/demo/dash_cc_sub1.jsonl",
            ActivityState::Active {
                tool_use_id: Some("tu1".into()),
                detail: Some("Grep: TODO".into()),
                kind: ToolKind::Search,
            },
            Some(cc_root_id),
            1,
            claude_code::SOURCE_NAME,
        ),
        (
            "code-reviewer",
            "/demo/dash_cc_sub2.jsonl",
            ActivityState::Idle,
            Some(cc_root_id),
            2,
            claude_code::SOURCE_NAME,
        ),
        (
            "cx·sidecar",
            "/demo/dash_cx_root.jsonl",
            ActivityState::Idle,
            None,
            3,
            codex::SOURCE_NAME,
        ),
        (
            "rx·helper",
            "/demo/dash_rx_root.jsonl",
            ActivityState::Waiting {
                reason: "permission?".into(),
            },
            None,
            4,
            reasonix::SOURCE_NAME,
        ),
    ];

    for (label, path, state, parent_id, desk_index, source) in agents {
        let id = AgentId::from_transcript_path(path);
        s.agents.insert(
            id,
            AgentSlot {
                agent_id: id,
                source: std::sync::Arc::from(*source),
                session_id: std::sync::Arc::from(
                    format!("demo-dash-{}", label.replace('·', "-")).as_str(),
                ),
                cwd: std::sync::Arc::from(PathBuf::from("/demo").as_path()),
                label: (*label).into(),
                state: state.clone(),
                state_started_at: now,
                created_at: now - D::from_secs(*desk_index as u64),
                last_event_at: now,
                exiting_at: None,
                pending_idle_at: None,
                desk_index: GlobalDeskIndex(*desk_index),
                floor_idx: s.floor_of(GlobalDeskIndex(*desk_index)),
                tool_call_count: 0,
                active_ms: 0,
                unknown_cwd: false,
                parent_id: *parent_id,
                pid: None,
                model: None,
                effort: None,
                tokens_used: 0,
                last_usage: None,
            },
        );
    }
    s
}

/// Build a ONE-agent scene whose wander targets `target` furniture, back-dated so
/// the walk-OUT starts at frame 0 — for `--anim` visual verification of the
/// approach→settle (no pop, no teleport). Prints the furniture's buffer position
/// so the caller can crop the GIF to it. `target` ∈ {couch, sofa, chair, pantry,
/// printer, vending, island, snackshelf, desk}; "desk" captures the
/// always-present return-to-desk leg.
/// The ONE name→waypoint vocabulary shared by `--anim` and `--crop-furniture`
/// ("desk" stays each caller's special case; "meeting" aliases the sofa). The
/// #567 review completed one flag's list and missed the sibling — a shared
/// resolver plus the exhaustive census below stops the two from forking again.
pub(crate) fn waypoint_target(name: &str) -> Option<pixtuoid_scene::layout::WaypointKind> {
    use pixtuoid_scene::layout::WaypointKind as K;
    match name {
        "couch" => Some(K::Couch),
        "meeting" | "sofa" => Some(K::MeetingSofa),
        "chair" => Some(K::MeetingChair),
        "pantry" => Some(K::Pantry),
        "printer" => Some(K::Printer),
        "vending" => Some(K::VendingMachine),
        "island" => Some(K::Island),
        "snackshelf" => Some(K::SnackShelf),
        _ => None,
    }
}

pub(crate) fn anim_scene(
    now: SystemTime,
    target: &str,
    cols: u16,
    rows: u16,
    floor_seed: u64,
    facing: Option<&str>,
) -> (SceneState, u64) {
    use pixtuoid_scene::layout::{Facing, SceneLayout, CLASSIC_OFFICE_DESKS};
    use pixtuoid_scene::pose::{
        is_aimless_cycle, seated_dwell_ms, takes_trip, waypoint_index_for_cycle,
    };

    // Match the renderer EXACTLY: it draws into scene_rect = terminal minus the
    // 1-row footer, then buf_h = scene_rect.height*2 (half-block). A 2px mismatch
    // shifts the waypoint set and the agent targets the wrong furniture.
    let (buf_w, buf_h) = (cols, rows.saturating_sub(1).saturating_mul(2));
    let l = SceneLayout::compute_with_seed(buf_w, buf_h, None, floor_seed)
        .expect("anim layout computes");
    let n = l.waypoints.len();

    // "desk" (and any unknown name) resolves None: always visited via the
    // return-to-desk leg, not a waypoint.
    let target_kind = waypoint_target(target);
    let want_facing = match facing {
        Some("north") => Some(Facing::North),
        Some("south") => Some(Facing::South),
        Some("east") => Some(Facing::East),
        Some("west") => Some(Facing::West),
        _ => None,
    };
    let target_idxs: Vec<usize> = l
        .waypoints
        .iter()
        .enumerate()
        .filter(|(_, w)| Some(w.kind) == target_kind)
        .filter(|(_, w)| want_facing.is_none_or(|f| w.facing == f))
        .map(|(i, _)| i)
        .collect();

    if target == "desk" {
        if let Some(d) = l.home_desks.first() {
            eprintln!("ANIM target=desk buf_pos≈({}, {}) [home desk 0]", d.x, d.y);
        }
    } else if let Some(&i) = target_idxs.first() {
        let p = l.waypoints[i].pos;
        eprintln!(
            "ANIM target={target} buf_pos=({}, {}) [{} matching waypoints, {n} total]",
            p.x,
            p.y,
            target_idxs.len()
        );
    } else {
        eprintln!(
            "ANIM target={target}: no matching waypoint at {buf_w}x{buf_h} seed {floor_seed}"
        );
    }

    // Brute-force an agent whose cycle-0 trip lands on the target (any tripping,
    // non-aimless agent for "desk").
    let path = (0u64..40_000)
        .map(|i| format!("/anim/{target}_{i}.jsonl"))
        .find(|p| {
            let id = AgentId::from_transcript_path(p);
            takes_trip(id, 0)
                && !is_aimless_cycle(id, 0)
                && (target == "desk"
                    || (n > 0 && target_idxs.contains(&waypoint_index_for_cycle(id, 0, n))))
        })
        .unwrap_or_else(|| format!("/anim/{target}_fallback.jsonl"));

    let id = AgentId::from_transcript_path(&path);
    // Print the agent's ACTUAL cycle-0 target — NOT the first matching waypoint
    // above (which is misleading when several seats match: the agent may sit on
    // a different one, so cropping to the printed pos shows an empty seat). This
    // is the buffer position to crop to for verification.
    if target != "desk" && n > 0 {
        let wi = waypoint_index_for_cycle(id, 0, n);
        let wp = l.waypoints[wi];
        eprintln!(
            "ANIM agent ACTUAL target = waypoint[{wi}] {:?} facing {:?} at buf_pos=({}, {})",
            wp.kind, wp.facing, wp.pos.x, wp.pos.y
        );
    }
    // Fresh agent at `now` (clean Seated start — the TUI re-anchors fresh agents
    // there regardless of created_at). The GIF PRE-ROLLS `skip_ms` past the
    // seated dwell so capture begins right as it walks out (see save_as_gif).
    let skip_ms = seated_dwell_ms(id).saturating_sub(1_000);
    eprintln!(
        "ANIM agent seated_dwell={}ms → pre-roll skip={skip_ms}ms",
        seated_dwell_ms(id)
    );

    let mut s = SceneState::uniform(CLASSIC_OFFICE_DESKS);
    s.agents.insert(
        id,
        AgentSlot {
            agent_id: id,
            source: std::sync::Arc::from("claude-code"),
            session_id: std::sync::Arc::from("anim"),
            cwd: std::sync::Arc::from(PathBuf::from("/anim").as_path()),
            label: target.into(),
            state: ActivityState::Idle,
            state_started_at: now,
            created_at: now,
            last_event_at: now,
            exiting_at: None,
            pending_idle_at: None,
            desk_index: GlobalDeskIndex(0),
            floor_idx: 0,
            tool_call_count: 0,
            active_ms: 0,
            unknown_cwd: false,
            parent_id: None,
            pid: None,
            model: None,
            effort: None,
            tokens_used: 0,
            last_usage: None,
        },
    );
    (s, skip_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Re-derive the motion bootstrap from the staged slots and pin every
    /// invariant the convergence depends on: motion's first-frame fast-forward
    /// (`elapsed_idle / est_wander_cycle_ms`) must select a trip cycle whose
    /// deterministic destination is a meeting slot, all staged agents in ONE
    /// room on DISTINCT slots, desk dwells within the spread, and the warmup
    /// pre-roll just under the earliest rise.
    #[test]
    fn meeting_scene_stages_a_convergent_group() {
        use pixtuoid_scene::layout::{SceneLayout, WaypointKind};
        use pixtuoid_scene::pose::{
            est_wander_cycle_ms, is_aimless_cycle, seated_dwell_ms, takes_trip,
            waypoint_index_for_cycle,
        };

        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let (cols, rows, max_desks) = (208u16, 88u16, 12);
        let (scene, warmup_ms) = meeting_scene(now, 3, cols, rows, 0, max_desks, 12).unwrap();
        assert_eq!(scene.agents.len(), 12, "staged 3 + 9 archetype fillers");

        let layout =
            SceneLayout::compute_with_seed(cols, (rows - 1) * 2, Some(max_desks), 0).unwrap();
        let staged: Vec<_> = scene
            .agents
            .values()
            .filter(|s| s.label.starts_with("meet-"))
            .collect();
        assert_eq!(staged.len(), 3);

        let mut wp_idxs = Vec::new();
        let mut rooms = Vec::new();
        let mut dwells = Vec::new();
        for slot in &staged {
            assert!(slot.desk_index.0 < 3, "staged agents take desks 0..n");
            let id = slot.agent_id;
            let elapsed_ms = now
                .duration_since(slot.state_started_at)
                .unwrap()
                .as_millis() as u64;
            // Mirror motion's bootstrap fast-forward exactly.
            let cycle_n = elapsed_ms / est_wander_cycle_ms(id);
            assert!(cycle_n >= 1, "cycle 0 back-dates under the thinking window");
            assert!(takes_trip(id, cycle_n), "staged cycle must be a trip");
            assert!(!is_aimless_cycle(id, cycle_n), "trip must be directed");
            let wp_idx = waypoint_index_for_cycle(id, cycle_n, layout.waypoints.len());
            let wp = layout.waypoints[wp_idx];
            assert!(
                matches!(
                    wp.kind,
                    WaypointKind::MeetingSofa | WaypointKind::MeetingChair
                ),
                "destination must be a meeting slot, got {:?}",
                wp.kind
            );
            wp_idxs.push(wp_idx);
            rooms.push(wp.room_id.expect("meeting slots carry a room_id"));
            dwells.push(seated_dwell_ms(id));
        }
        wp_idxs.sort_unstable();
        wp_idxs.dedup();
        assert_eq!(wp_idxs.len(), 3, "slots must be distinct (no seat fights)");
        assert!(
            rooms.iter().all(|r| *r == rooms[0]),
            "one room → one chitchat venue"
        );
        let (min_d, max_d) = (*dwells.iter().min().unwrap(), *dwells.iter().max().unwrap());
        assert!(
            max_d - min_d <= MEETING_DWELL_SPREAD_MS,
            "dwell spread {}ms exceeds {}ms",
            max_d - min_d,
            MEETING_DWELL_SPREAD_MS
        );
        assert_eq!(warmup_ms, min_d.saturating_sub(MEETING_WARMUP_LEAD_MS));
    }
}

#[cfg(test)]
mod vocabulary_tests {
    use super::*;

    /// Exhaustive-by-construction inverse of `waypoint_target`: a NEW
    /// `WaypointKind` fails this match (clippy --all-targets builds it) until
    /// it either joins the vocabulary or is deliberately excluded here
    /// (PhoneBooth/StandingDesk: stand-in-place spots with no distinct
    /// approach to film or crop to).
    fn canonical_target_name(kind: pixtuoid_scene::layout::WaypointKind) -> Option<&'static str> {
        use pixtuoid_scene::layout::WaypointKind as K;
        match kind {
            K::Couch => Some("couch"),
            K::MeetingSofa => Some("sofa"),
            K::MeetingChair => Some("chair"),
            K::Pantry => Some("pantry"),
            K::Printer => Some("printer"),
            K::VendingMachine => Some("vending"),
            K::Island => Some("island"),
            K::SnackShelf => Some("snackshelf"),
            K::PhoneBooth | K::StandingDesk => None,
        }
    }

    #[test]
    fn anim_and_crop_share_one_waypoint_vocabulary() {
        // Round-trip pin: every canonical name resolves to its kind and back.
        // The exhaustive match inside canonical_target_name is the real tooth —
        // a NEW WaypointKind fails compilation until it joins the vocabulary or
        // is deliberately excluded there.
        use pixtuoid_scene::layout::WaypointKind as K;
        for kind in K::ALL.iter().copied() {
            if let Some(name) = canonical_target_name(kind) {
                assert_eq!(waypoint_target(name), Some(kind), "{name} round-trips");
            }
        }
        assert_eq!(
            waypoint_target("meeting"),
            Some(K::MeetingSofa),
            "meeting aliases the sofa"
        );
        assert_eq!(
            waypoint_target("desk"),
            None,
            "desk stays the callers' special case"
        );
    }
}
