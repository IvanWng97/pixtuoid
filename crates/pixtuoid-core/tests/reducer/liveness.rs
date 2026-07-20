use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use pixtuoid_core::source::{AgentEvent, Transport};
use pixtuoid_core::state::reducer::Reducer;
use pixtuoid_core::state::SceneState;
use pixtuoid_core::AgentId;

use crate::{act_end, act_start, delegating_pair, proof_of_life, sess_end, waiting};

// --- stale-agent sweep ---------------------------------------------------

#[test]
fn stale_idle_agent_is_marked_exiting_after_timeout() {
    use pixtuoid_core::state::reducer::STALE_IDLE_TIMEOUT;
    let mut scene = SceneState::uniform(4);
    let mut reducer = Reducer::new();
    let id = AgentId::from_transcript_path("/p/stale.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    reducer.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "s".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    assert!(scene.agents.get(&id).unwrap().exiting_at.is_none());

    // Tick just before the threshold — should NOT mark exiting.
    reducer.tick(&mut scene, t0 + STALE_IDLE_TIMEOUT - Duration::from_secs(1));
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_none(),
        "should not mark exiting before timeout"
    );

    // Tick past the threshold — should mark exiting.
    reducer.tick(&mut scene, t0 + STALE_IDLE_TIMEOUT + Duration::from_secs(1));
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_some(),
        "should mark exiting after timeout"
    );
}

/// The stale sweep is STRICT (`age > threshold`): a slot whose silence
/// equals the threshold to the instant is NOT yet stale. `apply`/`tick` take
/// an injected `now`, so the exact boundary is a hand-built SystemTime pair —
/// deterministic, no wall clock. Pins the `>`→`>=` boundary mutant in
/// `sweep_stale` a full cargo-mutants run reported surviving.
#[test]
fn stale_sweep_spares_a_slot_at_exactly_the_threshold() {
    use pixtuoid_core::state::reducer::STALE_IDLE_TIMEOUT;
    let mut scene = SceneState::uniform(4);
    let mut reducer = Reducer::new();
    let id = AgentId::from_transcript_path("/p/boundary.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    reducer.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "s".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    reducer.tick(&mut scene, t0 + STALE_IDLE_TIMEOUT);
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_none(),
        "age == threshold is not yet stale (strict >)"
    );
    reducer.tick(
        &mut scene,
        t0 + STALE_IDLE_TIMEOUT + Duration::from_millis(1),
    );
    assert!(scene.agents.get(&id).unwrap().exiting_at.is_some());
}

/// `sweep_exited`'s GC is strict too: a slot whose walkout age equals
/// `EXIT_GRACE_WINDOW` exactly is still on stage. Same injected-now boundary
/// discipline as above; pins the `>`→`>=` mutant in `sweep_exited`.
#[test]
fn exit_gc_spares_a_slot_at_exactly_the_grace_window() {
    use pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW;
    let mut scene = SceneState::uniform(4);
    let mut reducer = Reducer::new();
    let id = AgentId::from_transcript_path("/p/grace.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    reducer.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "s".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    sess_end(&mut reducer, &mut scene, id, false, t0, Transport::Hook);
    assert!(scene.agents.get(&id).unwrap().exiting_at.is_some());
    reducer.tick(&mut scene, t0 + EXIT_GRACE_WINDOW);
    assert!(
        scene.agents.contains_key(&id),
        "walkout age == grace window is not yet GC-able (strict >)"
    );
    reducer.tick(
        &mut scene,
        t0 + EXIT_GRACE_WINDOW + Duration::from_millis(1),
    );
    assert!(!scene.agents.contains_key(&id));
}

#[test]
fn stale_active_agent_uses_shorter_timeout_than_idle() {
    use pixtuoid_core::state::reducer::{STALE_ACTIVE_TIMEOUT, STALE_IDLE_TIMEOUT};
    assert!(
        STALE_ACTIVE_TIMEOUT < STALE_IDLE_TIMEOUT,
        "active timeout should be shorter than idle"
    );

    let mut scene = SceneState::uniform(4);
    let mut reducer = Reducer::new();
    let id = AgentId::from_transcript_path("/p/active.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    reducer.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "s".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    act_start(
        &mut reducer,
        &mut scene,
        id,
        Some("t"),
        None,
        t0,
        Transport::Hook,
    );

    // Active timeout is 10 min — should mark exiting after that.
    reducer.tick(
        &mut scene,
        t0 + STALE_ACTIVE_TIMEOUT + Duration::from_secs(1),
    );
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_some(),
        "active agent should be reaped after STALE_ACTIVE_TIMEOUT"
    );
}

#[test]
fn codex_idle_agent_reaps_faster_than_claude_idle() {
    use pixtuoid_core::state::reducer::{STALE_IDLE_TIMEOUT, STALE_SHORT_IDLE_TIMEOUT};
    // Codex exposes no exit signal a short reaper could RELY on (its #710
    // SessionEnd hook is teardown-only best-effort; no PID, no durable rollout
    // marker), so an abruptly-closed Codex session is still reaped only by the
    // stale-sweep — hence a much shorter idle window than CC, which keeps the
    // long lunch-break-safe timeout.
    assert!(
        STALE_SHORT_IDLE_TIMEOUT < STALE_IDLE_TIMEOUT,
        "codex idle timeout must be shorter than the generic idle timeout"
    );

    let mut scene = SceneState::uniform(4);
    let mut reducer = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    // One Codex agent and one Claude-Code agent, both idle since t0. The source
    // is carried by the SessionStart event (the AgentId is just the slot key).
    let cx = AgentId::from_transcript_path("/p/codex-sess.jsonl");
    let cc = AgentId::from_transcript_path("/p/cc-sess.jsonl");
    for (id, source) in [(cx, "codex"), (cc, "claude-code")] {
        reducer.apply(
            &mut scene,
            AgentEvent::SessionStart {
                agent_id: id,
                source: source.into(),
                session_id: "s".into(),
                cwd: PathBuf::from("/repo"),
                parent_id: None,
            },
            t0,
            Transport::Hook,
        );
    }

    // Just past the Codex idle window (but far under CC's 30 min): the Codex
    // sprite is reaped; the CC one is spared.
    reducer.tick(
        &mut scene,
        t0 + STALE_SHORT_IDLE_TIMEOUT + Duration::from_secs(1),
    );
    assert!(
        scene.agents.get(&cx).unwrap().exiting_at.is_some(),
        "codex idle agent should reap after STALE_SHORT_IDLE_TIMEOUT"
    );
    assert!(
        scene.agents.get(&cc).unwrap().exiting_at.is_none(),
        "claude-code idle agent must NOT reap on the codex-fast window"
    );
}

// --- probe-vouched sweep exemption (#220) ----------------------------------
//
// The liveness probe (CC sessions registry / Codex open-rollout fd) is ground
// truth that the owning PROCESS is alive; the watcher re-emits ProofOfLife per
// probe refresh. A vouched slot must not be swept on event silence alone —
// the motivating case is a permission-parked CC session that renders Active
// after attach-replay (its hook-only Waiting is unreconstructable from JSONL)
// and emits nothing while the human decides.

#[test]
fn proof_of_life_exempts_active_slot_from_stale_sweep() {
    use pixtuoid_core::state::reducer::{PROOF_OF_LIFE_TTL, STALE_ACTIVE_TIMEOUT};
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/pol-active.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "s".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    act_start(&mut r, &mut scene, id, Some("t"), None, t0, Transport::Hook);

    // The watcher re-vouches every ~60s, so by the time the slot crosses the
    // Active threshold a fresh ProofOfLife has landed well inside the TTL.
    let vouch_at = t0 + STALE_ACTIVE_TIMEOUT;
    proof_of_life(&mut r, &mut scene, id, vouch_at, Transport::Jsonl);

    // Past the Active threshold (measured from last_event_at = t0) but inside
    // the vouch TTL: without the exemption this sweep reaps the slot (pinned
    // by stale_active_agent_uses_shorter_timeout_than_idle).
    let sweep_at = vouch_at + Duration::from_secs(1);
    assert!(sweep_at.duration_since(vouch_at).unwrap() < PROOF_OF_LIFE_TTL);
    r.tick(&mut scene, sweep_at);
    let slot = scene.agents.get(&id).expect("vouched slot must survive");
    assert!(
        slot.exiting_at.is_none(),
        "a probe-vouched slot must be exempt from the stale sweep"
    );
}

#[test]
fn proof_of_life_lapse_restores_normal_sweep() {
    use pixtuoid_core::state::reducer::{PROOF_OF_LIFE_TTL, STALE_ACTIVE_TIMEOUT};
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/pol-lapse.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "s".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    act_start(&mut r, &mut scene, id, Some("t"), None, t0, Transport::Hook);

    // Last vouch lands mid-window (the process then exits: emissions stop).
    let vouch_at = t0 + STALE_ACTIVE_TIMEOUT - Duration::from_secs(100);
    proof_of_life(&mut r, &mut scene, id, vouch_at, Transport::Jsonl);

    // Inside the TTL the slot is exempt — also pins that ProofOfLife did NOT
    // refresh last_event_at (the slot is past the Active threshold here).
    let exempt_at = t0 + STALE_ACTIVE_TIMEOUT + Duration::from_secs(1);
    r.tick(&mut scene, exempt_at);
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_none(),
        "still inside the vouch TTL — exempt"
    );

    // Once the vouch lapses, the normal sweep resumes (age is measured from
    // last_event_at = t0, long past the Active threshold by now).
    let lapsed_at = vouch_at + PROOF_OF_LIFE_TTL + Duration::from_secs(1);
    r.tick(&mut scene, lapsed_at);
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_some(),
        "a lapsed vouch must fall back to the normal stale sweep"
    );
}

#[test]
fn proof_of_life_for_unknown_id_is_a_no_op() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/pol-unknown.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    proof_of_life(&mut r, &mut scene, id, t0, Transport::Jsonl);
    assert!(
        scene.agents.is_empty(),
        "ProofOfLife must never create a slot — only hook tool/permission events synthesize"
    );
}

#[test]
fn proof_of_life_does_not_touch_activity_state() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/pol-state.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "s".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    act_start(
        &mut r,
        &mut scene,
        id,
        Some("t1"),
        Some("Edit: foo.rs"),
        t0,
        Transport::Hook,
    );
    // Arm the idle debounce — ProofOfLife must not cancel or re-arm it.
    act_end(&mut r, &mut scene, id, Some("t1"), t0, Transport::Hook);
    let before = scene.agents.get(&id).unwrap().clone();

    proof_of_life(
        &mut r,
        &mut scene,
        id,
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    let after = scene.agents.get(&id).unwrap();
    assert_eq!(
        after.state, before.state,
        "ProofOfLife must not change activity state"
    );
    assert_eq!(
        after.last_event_at, before.last_event_at,
        "ProofOfLife must not refresh last_event_at — it is not a real event"
    );
    assert_eq!(
        after.pending_idle_at, before.pending_idle_at,
        "ProofOfLife must not disturb the armed Active→Idle debounce"
    );
}

#[test]
fn proof_of_life_does_not_block_session_end() {
    use pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/pol-end.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "s".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    proof_of_life(&mut r, &mut scene, id, t0, Transport::Jsonl);
    // A real exit still removes promptly: SessionEnd marks exiting despite the
    // fresh vouch, and the grace GC reclaims the slot on schedule.
    sess_end(
        &mut r,
        &mut scene,
        id,
        false,
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_some(),
        "SessionEnd must mark a vouched slot exiting immediately"
    );
    r.tick(
        &mut scene,
        t0 + Duration::from_secs(1) + EXIT_GRACE_WINDOW + Duration::from_secs(1),
    );
    assert!(
        !scene.agents.contains_key(&id),
        "the vouch must not delay the exit GC"
    );
}

#[test]
fn codex_vouched_idle_slot_outlives_short_idle_reap() {
    use pixtuoid_core::state::reducer::{PROOF_OF_LIFE_TTL, STALE_SHORT_IDLE_TIMEOUT};
    // The new Codex semantic (#220): while the FD probe vouches for a rollout
    // (the codex process lives, holding it open), the 5-min short-idle reap is
    // exempt — it now effectively measures from the moment the process exits
    // and the vouch lapses. Without the vouch, the short reap is unchanged.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let vouched = AgentId::from_transcript_path("/p/codex-vouched.jsonl");
    let ghost = AgentId::from_transcript_path("/p/codex-ghost.jsonl");
    for id in [vouched, ghost] {
        r.apply(
            &mut scene,
            AgentEvent::SessionStart {
                agent_id: id,
                source: "codex".into(),
                session_id: "s".into(),
                cwd: PathBuf::from("/repo"),
                parent_id: None,
            },
            t0,
            Transport::Hook,
        );
    }
    let vouch_at = t0 + STALE_SHORT_IDLE_TIMEOUT - Duration::from_secs(100);
    proof_of_life(&mut r, &mut scene, vouched, vouch_at, Transport::Jsonl);

    let sweep_at = t0 + STALE_SHORT_IDLE_TIMEOUT + Duration::from_secs(1);
    assert!(sweep_at.duration_since(vouch_at).unwrap() < PROOF_OF_LIFE_TTL);
    r.tick(&mut scene, sweep_at);
    assert!(
        scene.agents.get(&vouched).unwrap().exiting_at.is_none(),
        "an fd-vouched codex slot must outlive the short-idle reap"
    );
    assert!(
        scene.agents.get(&ghost).unwrap().exiting_at.is_some(),
        "an unvouched codex slot keeps the 5-min short-idle reap"
    );
}

#[test]
fn proof_of_life_on_delegating_parent_shields_its_active_subtree() {
    use pixtuoid_core::state::reducer::{PROOF_OF_LIFE_TTL, STALE_ACTIVE_TIMEOUT};
    // The probe never vouches subagent ids (their transcript stems are
    // `agent-<id>`, not session UUIDs), and a permission-parked parent renders
    // Active after attach-replay (not Waiting — `has_waiting_ancestor` can't
    // fire). So a vouched, actively-delegating ANCESTOR must shield its
    // delegated subtree from the stale sweep: sweeping the live-but-blocked
    // child is unrecoverable (its JSONL events become unknown-id no-ops; its
    // hooks attribute to the parent).
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let (parent, child) = delegating_pair(&mut r, &mut scene, "pol-shield", t0);
    // A grandchild proves the walk is multi-level, not parent-only.
    let grandchild = AgentId::from_parts(
        "claude-code",
        "/p/pol-shield/subagents/agent-1/subagents/agent-2.jsonl",
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: grandchild,
            source: "claude-code".into(),
            session_id: "g".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(child),
        },
        t0 + Duration::from_millis(150),
        Transport::Jsonl,
    );
    // Parent dispatches a Task → active_tasks[parent] non-empty (delegating).
    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        Some("Agent"),
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    // Child + grandchild go Active via their own JSONL, then fall silent
    // (blocked behind the parent's permission prompt).
    act_start(
        &mut r,
        &mut scene,
        child,
        Some("c1"),
        Some("Read: /x"),
        t0 + Duration::from_secs(2),
        Transport::Jsonl,
    );
    act_start(
        &mut r,
        &mut scene,
        grandchild,
        Some("g1"),
        Some("Read: /y"),
        t0 + Duration::from_secs(3),
        Transport::Jsonl,
    );

    // The probe re-vouches the PARENT only, well past the subtree's Active
    // threshold (the watcher re-emits every ~60s, so the vouch is fresh).
    let vouch_at = t0 + STALE_ACTIVE_TIMEOUT + Duration::from_secs(60);
    proof_of_life(&mut r, &mut scene, parent, vouch_at, Transport::Jsonl);

    let sweep_at = vouch_at + Duration::from_secs(1);
    assert!(sweep_at.duration_since(vouch_at).unwrap() < PROOF_OF_LIFE_TTL);
    r.tick(&mut scene, sweep_at);
    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_none(),
        "the vouched parent survives via its own-id exemption"
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_none(),
        "a vouched delegating parent must shield its silent Active child"
    );
    assert!(
        scene.agents.get(&grandchild).unwrap().exiting_at.is_none(),
        "the shield must walk the whole ancestor chain, not one level"
    );
}

#[test]
fn vouch_lapse_restores_subtree_sweep() {
    use pixtuoid_core::state::reducer::{PROOF_OF_LIFE_TTL, STALE_ACTIVE_TIMEOUT};
    // When the process exits, emissions stop and the lapse must restore the
    // normal sweep for the whole subtree — the shield is strictly
    // process-liveness-scoped, never a permanent exemption.
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let (parent, child) = delegating_pair(&mut r, &mut scene, "pol-lapse-tree", t0);
    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        Some("Agent"),
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    act_start(
        &mut r,
        &mut scene,
        child,
        Some("c1"),
        Some("Read: /x"),
        t0 + Duration::from_secs(2),
        Transport::Jsonl,
    );

    // Last vouch lands mid-window; the process then exits — emissions stop.
    let vouch_at = t0 + STALE_ACTIVE_TIMEOUT - Duration::from_secs(100);
    proof_of_life(&mut r, &mut scene, parent, vouch_at, Transport::Jsonl);

    let lapsed_at = vouch_at + PROOF_OF_LIFE_TTL + Duration::from_secs(1);
    r.tick(&mut scene, lapsed_at);
    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_some(),
        "a lapsed vouch must restore the parent's normal stale sweep"
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "the child must be swept too once the ancestor vouch lapses"
    );
}

#[test]
fn vouched_idle_parent_without_tasks_does_not_shield_idle_child() {
    use pixtuoid_core::state::reducer::{PROOF_OF_LIFE_TTL, STALE_IDLE_TIMEOUT};
    // The backstop pin: the ancestor shield is gated on the ancestor ACTIVELY
    // delegating (non-empty active_tasks). A vouched parent with no Task in
    // flight must not shield a lingering completed/idle child — that's the
    // documented 30-min idle backstop for the b1 chained-dispatch residual.
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let (parent, child) = delegating_pair(&mut r, &mut scene, "pol-backstop", t0);
    // NO Task dispatch: active_tasks[parent] stays empty; both slots sit Idle.

    let vouch_at = t0 + STALE_IDLE_TIMEOUT + Duration::from_secs(60);
    proof_of_life(&mut r, &mut scene, parent, vouch_at, Transport::Jsonl);

    let sweep_at = vouch_at + Duration::from_secs(1);
    assert!(sweep_at.duration_since(vouch_at).unwrap() < PROOF_OF_LIFE_TTL);
    r.tick(&mut scene, sweep_at);
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "a vouched but non-delegating parent must NOT shield its idle child — the 30-min backstop holds"
    );
    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_none(),
        "the vouched parent itself keeps the own-id exemption"
    );
}

#[test]
fn fresh_event_resets_stale_timer() {
    use pixtuoid_core::state::reducer::STALE_IDLE_TIMEOUT;
    let mut scene = SceneState::uniform(4);
    let mut reducer = Reducer::new();
    let id = AgentId::from_transcript_path("/p/fresh.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    reducer.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "s".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );

    // At 29 min (just before 30 min idle threshold), send a new event.
    let almost = t0 + STALE_IDLE_TIMEOUT - Duration::from_secs(60);
    waiting(
        &mut reducer,
        &mut scene,
        id,
        "perm",
        almost,
        Transport::Hook,
    );

    // Now tick at original t0 + 31 min — should NOT reap because
    // last_event_at was reset to `almost` (29 min mark).
    reducer.tick(
        &mut scene,
        t0 + STALE_IDLE_TIMEOUT + Duration::from_secs(60),
    );
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_none(),
        "fresh event should have reset the stale timer"
    );
}

#[test]
fn unknown_cwd_agent_reaps_faster() {
    use pixtuoid_core::state::reducer::STALE_UNKNOWN_CWD_TIMEOUT;
    let mut scene = SceneState::uniform(4);
    let mut reducer = Reducer::new();
    let id = AgentId::from_transcript_path("/p/ghost.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    // SessionStart with empty cwd → label falls back to "cc#N".
    reducer.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "s".into(),
            cwd: PathBuf::new(),
            parent_id: None,
        },
        t0,
        Transport::Jsonl,
    );
    let slot = scene.agents.get(&id).unwrap();
    assert!(slot.unknown_cwd, "empty cwd should set unknown_cwd");
    let label = slot.label.clone();
    assert!(
        label.contains('#'),
        "empty cwd should produce source#N label, got {label}"
    );

    // 3 min + 1s → should be reaped (STALE_UNKNOWN_CWD_TIMEOUT = 3 min).
    reducer.tick(
        &mut scene,
        t0 + STALE_UNKNOWN_CWD_TIMEOUT + Duration::from_secs(1),
    );
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_some(),
        "unknown-cwd agent should reap after STALE_UNKNOWN_CWD_TIMEOUT"
    );
}

#[test]
fn parented_empty_cwd_subagent_is_not_ghost_reaped() {
    // A sub-agent (e.g. Copilot's subagent.started) registers with NO cwd but a
    // parent link — it is a real, process-proven child, not a startup-seed
    // ghost, so it must NOT ride the 3-min unknown-cwd reap (else a multi-minute
    // sub-agent vanishes while alive). Regression for the PR #292 lifecycle find.
    use pixtuoid_core::state::reducer::STALE_UNKNOWN_CWD_TIMEOUT;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let parent = AgentId::from_parts("copilot", "root-sess");
    let child = AgentId::from_parts("copilot", "call_child1");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "copilot".into(),
            session_id: "root-sess".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Jsonl,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "copilot".into(),
            session_id: "call_child1".into(),
            cwd: PathBuf::new(), // sub-agents carry no cwd
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(10),
        Transport::Jsonl,
    );
    assert!(
        !scene.agents.get(&child).unwrap().unknown_cwd,
        "a parented (subagent) slot must NOT be flagged unknown_cwd"
    );
    // It survives well past the 3-min ghost window while the parent delegates.
    r.tick(
        &mut scene,
        t0 + STALE_UNKNOWN_CWD_TIMEOUT + Duration::from_secs(30),
    );
    assert!(
        scene
            .agents
            .get(&child)
            .is_some_and(|s| s.exiting_at.is_none()),
        "a parented empty-cwd subagent must not be reaped on the 3-min ghost timer"
    );
}

#[test]
fn session_end_cascades_to_children() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/parent.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/parent/subagents/agent-1.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "parent".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "child".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    assert!(scene.agents.get(&child).unwrap().exiting_at.is_none());

    sess_end(
        &mut r,
        &mut scene,
        parent,
        false,
        t0 + Duration::from_secs(10),
        Transport::Hook,
    );
    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_some(),
        "parent should be exiting"
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "child should cascade to exiting when parent ends"
    );
}

#[test]
fn session_end_cascades_to_grandchildren() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let grandparent = AgentId::from_transcript_path("/p/gp.jsonl");
    let parent = AgentId::from_parts("claude-code", "/p/gp/subagents/agent-p.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/gp/subagents/agent-c.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: grandparent,
            source: "claude-code".into(),
            session_id: "gp".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "p".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(grandparent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "c".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(200),
        Transport::Jsonl,
    );

    sess_end(
        &mut r,
        &mut scene,
        grandparent,
        false,
        t0 + Duration::from_secs(10),
        Transport::Hook,
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "grandchild should cascade to exiting via BFS"
    );
}

// --- parent-child cascade --------------------------------------------------

#[test]
fn session_end_cascade_marks_all_descendants_exiting() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/cascade-parent.jsonl");
    let child_a = AgentId::from_parts("claude-code", "/p/cascade-parent/subagents/agent-a.jsonl");
    let child_b = AgentId::from_parts("claude-code", "/p/cascade-parent/subagents/agent-b.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "p".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child_a,
            source: "claude-code".into(),
            session_id: "ca".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child_b,
            source: "claude-code".into(),
            session_id: "cb".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(200),
        Transport::Jsonl,
    );

    assert!(scene.agents.get(&child_a).unwrap().exiting_at.is_none());
    assert!(scene.agents.get(&child_b).unwrap().exiting_at.is_none());

    sess_end(
        &mut r,
        &mut scene,
        parent,
        false,
        t0 + Duration::from_secs(5),
        Transport::Hook,
    );

    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_some(),
        "parent must be marked exiting"
    );
    assert!(
        scene.agents.get(&child_a).unwrap().exiting_at.is_some(),
        "child_a must cascade to exiting when parent ends"
    );
    assert!(
        scene.agents.get(&child_b).unwrap().exiting_at.is_some(),
        "child_b must cascade to exiting when parent ends"
    );
}

// --- sweep_stale -----------------------------------------------------------

#[test]
fn sweep_stale_marks_old_agent_exiting_on_tick() {
    use pixtuoid_core::state::reducer::STALE_IDLE_TIMEOUT;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/stale-sweep.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "sw".into(),
            cwd: PathBuf::from("/old-project"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    assert!(scene.agents.get(&id).unwrap().exiting_at.is_none());

    // Tick well past the idle stale timeout with no intervening events.
    r.tick(
        &mut scene,
        t0 + STALE_IDLE_TIMEOUT + Duration::from_secs(60),
    );
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_some(),
        "tick past STALE_IDLE_TIMEOUT should mark agent exiting"
    );
}

#[test]
fn stale_sweep_cascades_to_children() {
    use pixtuoid_core::state::reducer::STALE_IDLE_TIMEOUT;
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/stale-cascade.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/stale-cascade/subagents/agent-1.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "parent".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "child".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    // Heartbeat the child so it is NOT independently stale at the tick below.
    // Only the parent (no events since t0) crosses STALE_IDLE_TIMEOUT, so the
    // child's exit can only come from the cascade.
    r.apply(
        &mut scene,
        AgentEvent::Rename {
            agent_id: child,
            label: "cc·sub".into(),
        },
        t0 + Duration::from_secs(25 * 60),
        Transport::Jsonl,
    );

    r.tick(&mut scene, t0 + STALE_IDLE_TIMEOUT + Duration::from_secs(1));

    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_some(),
        "stale parent should be marked exiting"
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "child should cascade-exit with a stale-swept parent (it is not independently stale)"
    );
}

// When BOTH parent and child are independently stale, both enter sweep_stale's
// pass-1 `stale` vec. The parent's pass-2 cascade marks the child exiting; the
// child's own pass-2 iteration then hits the `exiting_at.is_some() -> continue`
// write-once guard (reducer.rs) instead of re-stamping / re-logging it. The
// existing cascade tests heartbeat the descendant so it is NEVER in `stale`, so
// they don't exercise this branch — this test drops the heartbeat.
#[test]
fn stale_sweep_already_cascaded_child_is_skipped_in_pass_two() {
    use pixtuoid_core::state::reducer::STALE_IDLE_TIMEOUT;
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/double-stale.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/double-stale/subagents/agent-1.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "parent".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "child".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );

    // No heartbeat for either: both cross STALE_IDLE_TIMEOUT, so both enter the
    // pass-1 `stale` vec. The id is set once, on whichever pass-2 iteration runs
    // first; the other iteration must hit the write-once skip.
    let now = t0 + STALE_IDLE_TIMEOUT + Duration::from_secs(1);
    r.tick(&mut scene, now);

    let parent_exit = scene.agents.get(&parent).unwrap().exiting_at;
    let child_exit = scene.agents.get(&child).unwrap().exiting_at;
    assert!(parent_exit.is_some(), "stale parent marked exiting");
    assert!(
        child_exit.is_some(),
        "independently-stale child also marked exiting (write-once, no double-stamp)"
    );
    // Both stamped at the same sweep `now`: the pass-2 skip preserved the first
    // write rather than overwriting it on the second iteration.
    assert_eq!(parent_exit, Some(now));
    assert_eq!(child_exit, Some(now));
}

#[test]
fn stale_sweep_cascades_to_grandchildren() {
    use pixtuoid_core::state::reducer::STALE_IDLE_TIMEOUT;
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let grandparent = AgentId::from_transcript_path("/p/stale-gp.jsonl");
    let parent = AgentId::from_parts("claude-code", "/p/stale-gp/subagents/agent-p.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/stale-gp/subagents/agent-c.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: grandparent,
            source: "claude-code".into(),
            session_id: "gp".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "p".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(grandparent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "c".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(200),
        Transport::Jsonl,
    );
    // Heartbeat the middle + leaf so only the grandparent is independently stale.
    for (id, label) in [(parent, "cc·p"), (child, "cc·c")] {
        r.apply(
            &mut scene,
            AgentEvent::Rename {
                agent_id: id,
                label: label.into(),
            },
            t0 + Duration::from_secs(25 * 60),
            Transport::Jsonl,
        );
    }

    r.tick(&mut scene, t0 + STALE_IDLE_TIMEOUT + Duration::from_secs(1));

    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "grandchild should cascade-exit via BFS through the stale grandparent"
    );
}

#[test]
fn stale_sweep_cascade_skips_unrelated_fresh_agents() {
    use pixtuoid_core::state::reducer::STALE_IDLE_TIMEOUT;
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/stale-host.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/stale-host/subagents/agent-1.jsonl");
    let unrelated = AgentId::from_transcript_path("/p/other-session.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "parent".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "child".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: unrelated,
            source: "claude-code".into(),
            session_id: "other".into(),
            cwd: PathBuf::from("/other-repo"),
            parent_id: None,
        },
        t0 + Duration::from_millis(150),
        Transport::Hook,
    );
    // Heartbeat the child AND the unrelated agent so neither is independently
    // stale: only the parent crosses the threshold.
    for (id, label) in [(child, "cc·sub"), (unrelated, "cc·other")] {
        r.apply(
            &mut scene,
            AgentEvent::Rename {
                agent_id: id,
                label: label.into(),
            },
            t0 + Duration::from_secs(25 * 60),
            Transport::Jsonl,
        );
    }

    r.tick(&mut scene, t0 + STALE_IDLE_TIMEOUT + Duration::from_secs(1));

    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "the stale parent's child must cascade-exit"
    );
    assert!(
        scene.agents.get(&unrelated).unwrap().exiting_at.is_none(),
        "a fresh, unrelated agent must NOT be cascaded out"
    );
}

#[test]
fn long_delegation_keeps_parent_and_live_subagent_alive() {
    // A parent delegating a single Task longer than STALE_ACTIVE_TIMEOUT
    // gets no events of its OWN — the subagent's hook events are misattributed
    // to the parent's AgentId and suppressed. Those suppressed events are still
    // proof the subtree is alive, so they must refresh the parent's
    // last_event_at; otherwise sweep_stale reaps the live parent and the
    // cascade drags its still-working subagent out with it.
    use pixtuoid_core::state::reducer::STALE_ACTIVE_TIMEOUT;
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/deleg.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/deleg/subagents/agent-1.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "p".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "c".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );

    // Parent delegates one long Task → Active{Delegating}. The Task-start arm
    // does NOT bump last_event_at, so the parent's liveness is frozen at t0.
    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        Some("Agent"),
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );

    // The subagent works for ~9 min; each tool call is a hook event CC
    // misattributes to the parent's AgentId, so the reducer suppresses it.
    for (mins, tuid) in [(5u64, "sub-R1"), (9u64, "sub-R2")] {
        act_start(
            &mut r,
            &mut scene,
            parent,
            Some(tuid),
            Some("Read: /x"),
            t0 + Duration::from_secs(mins * 60),
            Transport::Hook,
        );
    }

    // Tick just past the parent's Active stale threshold measured from t0, but
    // well within it measured from the last suppressed child event (t0+9min).
    r.tick(
        &mut scene,
        t0 + STALE_ACTIVE_TIMEOUT + Duration::from_secs(1),
    );

    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_none(),
        "a delegating parent must stay alive while its subagent emits events"
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_none(),
        "the live subagent must NOT be cascaded out by a falsely-stale parent"
    );
}

#[test]
fn stale_sweep_spares_subagent_blocked_under_a_waiting_parent() {
    // A subagent's permission prompt is attributed to the PARENT (hook
    // transcript_path → parent), so the parent goes Waiting (60-min) while the
    // subagent stays Active (its last tool, 10-min) and emits nothing while
    // blocked. The subagent is alive — waiting on a human gate the parent holds
    // — so the stale-sweep must NOT reap it on the aggressive Active timer.
    // Liveness vs readiness: a node under a Waiting ancestor is "not ready",
    // not "dead".
    use pixtuoid_core::state::reducer::STALE_ACTIVE_TIMEOUT;
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/perm-parent.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/perm-parent/subagents/agent-1.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "p".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "c".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    // Subagent runs a tool → Active (10-min stale timeout).
    act_start(
        &mut r,
        &mut scene,
        child,
        Some("c-tool"),
        Some("WebFetch: /x"),
        t0 + Duration::from_secs(1),
        Transport::Jsonl,
    );
    // That tool needs permission → CC's Notification hook lands on the PARENT.
    waiting(
        &mut r,
        &mut scene,
        parent,
        "permission?",
        t0 + Duration::from_secs(2),
        Transport::Hook,
    );

    // User ignores the prompt for >10 min. No further events.
    r.tick(
        &mut scene,
        t0 + STALE_ACTIVE_TIMEOUT + Duration::from_secs(60),
    );

    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_none(),
        "Waiting parent (60-min threshold) must survive a 10-min wait"
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_none(),
        "a subagent blocked under a Waiting parent must NOT be reaped on the Active timer"
    );
}

#[test]
fn stale_sweep_spares_grandchild_under_a_waiting_ancestor() {
    // The readiness exemption walks the whole parent_id chain: a stale
    // grandchild whose grandparent is Waiting is still "blocked", not dead.
    use pixtuoid_core::state::reducer::STALE_ACTIVE_TIMEOUT;
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let gp = AgentId::from_transcript_path("/p/perm-gp.jsonl");
    let parent = AgentId::from_parts("claude-code", "/p/perm-gp/subagents/agent-p.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/perm-gp/subagents/agent-c.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: gp,
            source: "claude-code".into(),
            session_id: "gp".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "p".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(gp),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "c".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(200),
        Transport::Jsonl,
    );
    // Middle + leaf are Active (10-min); grandparent holds the permission gate.
    for id in [parent, child] {
        act_start(
            &mut r,
            &mut scene,
            id,
            Some("t"),
            Some("WebFetch: /x"),
            t0 + Duration::from_secs(1),
            Transport::Jsonl,
        );
    }
    waiting(
        &mut r,
        &mut scene,
        gp,
        "permission?",
        t0 + Duration::from_secs(2),
        Transport::Hook,
    );

    r.tick(
        &mut scene,
        t0 + STALE_ACTIVE_TIMEOUT + Duration::from_secs(60),
    );

    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_none(),
        "a grandchild under a Waiting ancestor must NOT be reaped on the Active timer"
    );
    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_none(),
        "the middle agent under a Waiting ancestor must NOT be reaped either"
    );
}

#[test]
fn active_subagent_keeps_parent_alive_via_jsonl_events() {
    // Liveness flows up the tree via the subagent's OWN JSONL events — not only
    // suppressed hook events (hooks are best-effort and can drop). A subagent
    // actively emitting JSONL keeps its delegating parent from being
    // stale-swept, so the cascade can't evict the live subagent.
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/deleg2.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/deleg2/subagents/agent-1.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "p".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "c".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    // Parent delegates → Active{Delegating} (10-min threshold); its OWN last
    // event is now frozen at t0+1s.
    act_start(
        &mut r,
        &mut scene,
        parent,
        Some("task-T"),
        Some("Agent"),
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    // Subagent works for >10 min, emitting ONLY JSONL events (no hooks reach the
    // parent). Each keeps the parent's lineage alive.
    for mins in [4u64, 8, 12] {
        act_start(
            &mut r,
            &mut scene,
            child,
            Some("c"),
            Some("Read: /x"),
            t0 + Duration::from_secs(mins * 60),
            Transport::Jsonl,
        );
    }
    // Tick shortly after the last child event — but ~12 min past the parent's
    // OWN last event (the Task start at t0+1s).
    r.tick(
        &mut scene,
        t0 + Duration::from_secs(12 * 60) + Duration::from_secs(30),
    );

    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_none(),
        "a delegating parent must stay alive while its subagent emits JSONL events"
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_none(),
        "the live subagent must not be cascaded out by a falsely-stale parent"
    );
}

// A Delegating Reasonix slot is hook-silent by construction (its in-process
// subagents fire no hooks), so a >10-min research/review delegation must not
// be stale-swept mid-turn — it gets the Waiting-class 60-min window.
#[test]
fn reasonix_delegating_slot_survives_the_active_timeout() {
    use pixtuoid_core::source::ToolDetail;
    use pixtuoid_core::state::reducer::{STALE_ACTIVE_TIMEOUT, STALE_WAITING_TIMEOUT};
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("reasonix", "/Users/dev/proj");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "reasonix".into(),
            session_id: "/Users/dev/proj".into(),
            cwd: "/Users/dev/proj".into(),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    // PreToolUse(task) — no tool id (Reasonix hooks carry none).
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: None,
            detail: Some(ToolDetail::Task),
        },
        t0,
        Transport::Hook,
    );

    // Survives well past the generic Active timeout…
    r.tick(
        &mut scene,
        t0 + STALE_ACTIVE_TIMEOUT + Duration::from_secs(60),
    );
    assert!(
        scene
            .agents
            .get(&id)
            .is_some_and(|s| s.exiting_at.is_none()),
        "a hook-silent Delegating rx slot must not be swept on the 10-min Active timer"
    );
    // …but is still reaped on the Waiting-class window (no immortal ghosts).
    r.tick(
        &mut scene,
        t0 + STALE_WAITING_TIMEOUT + Duration::from_secs(60),
    );
    assert!(
        scene.agents.get(&id).is_none_or(|s| s.exiting_at.is_some()),
        "the carve-out must not make the slot immortal"
    );
}

// A cycle-ATTEMPTING input (two crafted/buggy SessionStarts each naming the
// other) with a Waiting member must still be reaped by the stale sweep.
// History: pre-#234 the Waiting node counted as its OWN Waiting ancestor and
// the pair held two desks forever; since #238 the second registration's
// cycle-closing parent is REFUSED at the link seam, so the 2-cycle never
// forms in the scene — this test now pins the no-immortal-pair observable
// end-to-end (registration-path refusal + reap), while the crafted-state
// cycle WALKS stay pinned by the scope.rs unit tests (`cycle_scene`).
#[test]
fn waiting_parent_cycle_is_still_reaped_by_the_stale_sweep() {
    use pixtuoid_core::state::reducer::STALE_WAITING_TIMEOUT;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let a = AgentId::from_transcript_path("/p/cycle-a.jsonl");
    let b = AgentId::from_transcript_path("/p/cycle-b.jsonl");
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: a,
            source: "claude-code".into(),
            session_id: "cyc-a".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(b),
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: b,
            source: "claude-code".into(),
            session_id: "cyc-b".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(a),
        },
        t0,
        Transport::Hook,
    );
    // One member parks on a permission prompt → Waiting (its resolution
    // never arrives — the slots are malformed input, not a real session).
    waiting(&mut r, &mut scene, b, "permission", t0, Transport::Hook);

    // Even the most generous threshold elapsing must reap the whole cycle:
    // the Waiting member is collected on STALE_WAITING_TIMEOUT (it is NOT
    // its own ancestor) and the cascade takes its cycle partner.
    r.tick(
        &mut scene,
        t0 + STALE_WAITING_TIMEOUT + Duration::from_secs(60),
    );
    for id in [a, b] {
        assert!(
            scene.agents.get(&id).is_none_or(|s| s.exiting_at.is_some()),
            "a Waiting parent-cycle member must not self-exempt from the stale sweep"
        );
    }
}

// The #234 residual (#238): a 2-cycle whose members are BOTH Waiting would
// mutually exempt — each has the OTHER as a genuine Waiting ancestor, so
// `has_waiting_ancestor` skips both every sweep tick (an immortal pair). The
// fix is upstream of the sweep: the SessionStart arm REFUSES a parent link
// whose ancestor chain reaches the child (warn + degrade to parentless), so
// the cycle never exists and the sweep needs no cycle awareness.
#[test]
fn mutual_waiting_parent_cycle_is_refused_at_the_link_seam_and_reaped() {
    use pixtuoid_core::state::reducer::STALE_WAITING_TIMEOUT;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let a = AgentId::from_transcript_path("/p/mutual-a.jsonl");
    let b = AgentId::from_transcript_path("/p/mutual-b.jsonl");
    // B registers parentless, then A registers parented to B (a legitimate
    // link — B's chain is empty, no cycle).
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: b,
            source: "claude-code".into(),
            session_id: "mut-b".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: a,
            source: "claude-code".into(),
            session_id: "mut-a".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(b),
        },
        t0,
        Transport::Hook,
    );
    // B's duplicate SessionStart proposes parent A — the orphan-enrichment
    // seam. A's chain reaches B (A → B), so the link would close the cycle:
    // it must be refused (warn + continue, never panic), leaving B parentless.
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: b,
            source: "claude-code".into(),
            session_id: "mut-b".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(a),
        },
        t0,
        Transport::Hook,
    );
    assert_eq!(
        scene.agents.get(&b).and_then(|s| s.parent_id),
        None,
        "a cycle-closing enrichment must degrade to parentless"
    );
    // Both members park Waiting — pre-fix this pair was immortal.
    for id in [a, b] {
        waiting(&mut r, &mut scene, id, "permission", t0, Transport::Hook);
    }

    r.tick(
        &mut scene,
        t0 + STALE_WAITING_TIMEOUT + Duration::from_secs(60),
    );
    for id in [a, b] {
        assert!(
            scene.agents.get(&id).is_none_or(|s| s.exiting_at.is_some()),
            "a mutual-Waiting pair must not exempt each other from the stale sweep"
        );
    }
}
