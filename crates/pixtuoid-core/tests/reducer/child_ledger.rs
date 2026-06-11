use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use pixtuoid_core::source::{AgentEvent, Transport};
use pixtuoid_core::state::reducer::{
    Reducer, CHILD_END_LEDGER_TTL, HOOK_SESSION_END_TOMBSTONE_TTL,
};
use pixtuoid_core::state::SceneState;
use pixtuoid_core::AgentId;

use crate::start;

#[test]
fn hook_session_end_tombstone_blocks_reordered_trailing_event_synthesis() {
    // Hook connections are per-connection spawned tasks, so a session's
    // SessionEnd and a trailing Stop/ActivityEnd can be DELIVERED reordered.
    // For an INVISIBLE (never-registered) session ending at /exit, the
    // reordered ActivityEnd used to hit the proof-of-life synthesis and mint
    // a blank Idle ghost — and with the session over, no SessionEnd will
    // ever come again: the ghost lived out the full 30-min idle sweep.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("claude-code", "exited-invisible");
    let other = AgentId::from_parts("claude-code", "still-alive");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: id,
            as_child: false,
        },
        t0,
        Transport::Hook,
    );
    // The straggler lands shortly after — within the tombstone TTL.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: None,
        },
        t0 + Duration::from_millis(50),
        Transport::Hook,
    );
    assert!(
        !scene.agents.contains_key(&id),
        "a reordered trailing event must not resurrect a tombstoned session"
    );

    // Control: a DIFFERENT id is untouched by the tombstone — hook proof of
    // life still synthesizes for it.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: other,
            tool_use_id: None,
        },
        t0 + Duration::from_millis(50),
        Transport::Hook,
    );
    assert!(
        scene.agents.contains_key(&other),
        "the tombstone must be per-id, not a global synthesis gate"
    );
}

#[test]
fn hook_event_after_tombstone_ttl_synthesizes_again() {
    // The tombstone is a short reorder guard, not a permanent ban: a hook
    // event well past the TTL is genuine NEW proof of life (a fresh process
    // turn on the same session id) and must register.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("claude-code", "revived-later");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: id,
            as_child: false,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        t0 + HOOK_SESSION_END_TOMBSTONE_TTL + Duration::from_secs(1),
        Transport::Hook,
    );
    assert!(
        scene.agents.contains_key(&id),
        "past the TTL a hook event is fresh proof of life and must synthesize"
    );
}

#[test]
fn jsonl_child_session_start_within_tombstone_is_gated_too() {
    // #242, transport scoping: the tombstone is evidence the child ALREADY
    // ENDED — transport-agnostic. A CC subagent transcript first-sighted by
    // the watcher AFTER the hook SubagentStop ended the never-registered
    // child has the same phantom shape as the reordered hook Start: the
    // transcript carries no end marker, so the JSONL-registered slot would
    // also linger to the stale sweeps. (A historical replay never
    // SessionStarts — the watcher's first-sight gate — so no legitimate
    // JSONL flow reaches this gate.)
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let parent = AgentId::from_parts("claude-code", "parent-sess");
    let child = AgentId::from_parts("claude-code", "agent-late-file");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    start(&mut r, &mut scene, parent);
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: child,
            as_child: true,
        },
        t0,
        Transport::Hook,
    );
    // The watcher's first-sight emission for the child's transcript lands
    // within the TTL.
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "agent-late-file".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(200),
        Transport::Jsonl,
    );
    assert!(
        !scene.agents.contains_key(&child),
        "a JSONL child SessionStart racing its own hook Stop must not register"
    );
}

#[test]
fn non_child_session_end_tombstone_alone_gates_a_parented_start() {
    // #242 independence pin: an unknown-id hook SessionEnd with
    // `as_child: false` mints the 5s tombstone but writes NO child-ledger
    // `ended_at` (only the SubagentStop decoders stamp as_child), so the 90s
    // ledger gate cannot fire here — only the original #242 tombstone gate
    // can block the parented Start inside the TTL. Deleting that gate would
    // pass every ledger-armed test above; this one keeps it load-bearing.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let parent = AgentId::from_parts("claude-code", "parent-sess");
    let child = AgentId::from_parts("claude-code", "agent-nonchild-end");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    start(&mut r, &mut scene, parent);
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: child,
            as_child: false,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "agent-nonchild-end".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(200),
        Transport::Hook,
    );
    assert!(
        !scene.agents.contains_key(&child),
        "an as_child: false end arms ONLY the 5s #242 tombstone — that gate \
         alone must block the parented Start inside the TTL"
    );
}

#[test]
fn child_session_start_past_tombstone_ttl_registers() {
    // The gates are tombstones, not blacklists: child ids are per-spawn
    // unique, so a Start past the windows is the late-discovery case (e.g. a
    // notify outage deferring the transcript first-sight to the 60s poll)
    // and must register — the TTLs bound the guards, the sweeps own the rest.
    // The end here is a SubagentStop (as_child) for an UNKNOWN id, so BOTH
    // guards arm: the 5s #242 hook tombstone and the 90s child ledger
    // (#244); the registration must clear the LONGER one.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let parent = AgentId::from_parts("claude-code", "parent-sess");
    let child = AgentId::from_parts("claude-code", "agent-recycled");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    start(&mut r, &mut scene, parent);
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: child,
            as_child: true,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "agent-recycled".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + CHILD_END_LEDGER_TTL + Duration::from_secs(1),
        Transport::Hook,
    );
    assert!(
        scene.agents.contains_key(&child),
        "past the ledger TTL a child SessionStart is a fresh registration"
    );
}

#[test]
fn tombstoned_parentless_session_start_still_registers() {
    // Reasonix's documented SessionEnd→SessionStart resurrect rides the SAME
    // cwd-keyed id: an INVISIBLE (never-registered) session's `/new` rotation
    // fires SessionEnd (→ tombstone, unknown id) then SessionStart
    // back-to-back. The #242 gate is scoped to CHILD registrations
    // (`parent_id: Some`) precisely so this PARENTLESS start keeps
    // registering — Reasonix has no other re-creation signal.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("reasonix", "/Users/dev/proj");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: id,
            as_child: false,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "reasonix".into(),
            session_id: "/Users/dev/proj".into(),
            cwd: PathBuf::from("/Users/dev/proj"),
            parent_id: None,
        },
        t0 + Duration::from_millis(20),
        Transport::Hook,
    );
    assert!(
        scene.agents.contains_key(&id),
        "a parentless SessionStart must register straight through a fresh \
         tombstone (the Reasonix resurrect)"
    );
}

// ---- Child ledger (#244 / #246) -------------------------------------------
//
// The #242 hook tombstone above covers only the 5s reorder window for
// UNKNOWN-id ends. The reducer-private child ledger covers the residual
// windows: it remembers each child's APPLIED parent and stamps `ended_at`
// from `as_child` SessionEnds (SubagentStop decodes) and from slot removal,
// so (w2) a KNOWN child's late parented re-registration is gated for
// CHILD_END_LEDGER_TTL, and a PARENTLESS start that DOES occur — a
// post-un-claim revival (#246's adoption seam) or a tombstoned child's
// flat-rollout first-sight (#244-w1) — re-links to the remembered parent
// instead of registering as an orphan. (For the IN-FLIGHT multi-turn Codex
// child, upstream provides NO SessionStart carrier at turn N+1 — the
// child-end un-claim side-channel manufactures one: the hook tee +
// `ChildEndUnclaims` release the rollout's `seen` claim so the next append
// first-sights; pinned end-to-end in tests/watcher/unclaim.rs
// `in_flight_multi_turn_codex_child_revives_and_relinks_via_unclaim`.)

/// Drive the captured-shape hook payload through the REAL decoder and apply
/// every decoded event — the same end-to-end path the listener uses, so these
/// scenarios exercise the `as_child` stamping, not hand-rolled events.
fn apply_hook_payload(
    r: &mut Reducer,
    scene: &mut SceneState,
    payload: serde_json::Value,
    now: SystemTime,
) {
    for ev in pixtuoid_core::source::decoder::decode_hook_payload(payload).expect("decodes") {
        r.apply(scene, ev, now, Transport::Hook);
    }
}

#[test]
fn late_parented_restart_of_an_ended_child_is_gated_by_the_child_ledger() {
    // #244-w2: Start→Stop on a KNOWN slot mints NO #242 tombstone (the Stop
    // had a slot to mark exiting), so after the 4.5s GC a late transcript
    // first-sight (notify outage → the 60s poll backstop) used to re-register
    // the dead child as a phantom — no future SessionEnd would ever remove
    // it. The ledger's `ended_at` (stamped by the as_child Stop) must gate it
    // for CHILD_END_LEDGER_TTL, and registration resumes past the TTL.
    use pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW;
    use serde_json::json;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let parent = AgentId::from_parts("claude-code", "01000000-0000-7000-8000-0000000000cc");
    let child = AgentId::from_parts("claude-code", "agent-a0000000000000001");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    apply_hook_payload(
        &mut r,
        &mut scene,
        json!({
            "hook_event_name": "SessionStart",
            "session_id": "01000000-0000-7000-8000-0000000000cc",
            "_pixtuoid_source": "claude-code",
            "cwd": "/repo",
        }),
        t0,
    );
    apply_hook_payload(
        &mut r,
        &mut scene,
        json!({
            "hook_event_name": "SubagentStart",
            "session_id": "01000000-0000-7000-8000-0000000000cc",
            "agent_id": "a0000000000000001",
            "cwd": "/repo",
            "_pixtuoid_source": "claude-code",
        }),
        t0 + Duration::from_secs(1),
    );
    assert!(scene.agents.contains_key(&child), "child registered");
    let stop = t0 + Duration::from_secs(2);
    apply_hook_payload(
        &mut r,
        &mut scene,
        json!({
            "hook_event_name": "SubagentStop",
            "session_id": "01000000-0000-7000-8000-0000000000cc",
            "agent_id": "a0000000000000001",
            "_pixtuoid_source": "claude-code",
        }),
        stop,
    );
    // GC the exiting child — well past EXIT_GRACE_WINDOW.
    r.tick(
        &mut scene,
        stop + EXIT_GRACE_WINDOW + Duration::from_secs(1),
    );
    assert!(!scene.agents.contains_key(&child), "child GC'd");

    // The late parented first-sight (CC subagent transcripts carry the
    // parent in their path) lands at +30s: past the 5s #242 tombstone — only
    // the ledger can catch it.
    let late_start = |r: &mut Reducer, scene: &mut SceneState, at: SystemTime| {
        r.apply(
            scene,
            AgentEvent::SessionStart {
                agent_id: child,
                source: "claude-code".into(),
                session_id: "agent-a0000000000000001".into(),
                cwd: PathBuf::from("/repo"),
                parent_id: Some(parent),
            },
            at,
            Transport::Jsonl,
        );
    };
    late_start(&mut r, &mut scene, stop + Duration::from_secs(30));
    assert!(
        !scene.agents.contains_key(&child),
        "a late parented restart of an ENDED child inside the ledger TTL \
         must not re-register a phantom (#244-w2)"
    );

    // The guard is TTL-bounded, not a blacklist.
    late_start(
        &mut r,
        &mut scene,
        stop + CHILD_END_LEDGER_TTL + Duration::from_secs(1),
    );
    assert!(
        scene.agents.contains_key(&child),
        "past CHILD_END_LEDGER_TTL the registration resumes"
    );
}

#[test]
fn parentless_revival_start_of_an_ended_codex_child_relinks_via_ledger() {
    // #246's re-link mechanism, pinned at the reducer seam: when a
    // parentless SessionStart on a known-ended child id arrives — a
    // post-un-claim revival (negative vouch / instant exit / decoded
    // terminator / the #246 child-end un-claim releases the rollout from
    // `seen`, so its next line re-emits SessionStart) or a flat first-sight
    // — the ledger must restore the remembered parent so the revived child
    // re-joins the scope tree instead of registering as an orphan, on EITHER
    // transport. The IN-FLIGHT multi-turn child rides exactly this arm:
    // upstream provides NO SessionStart carrier at turn N+1 (codex-rs fires
    // SubagentStop at EVERY turn end but SubagentStart only at thread
    // STARTUP; hook_runtime.rs verified 2026-06-11), so the un-claim
    // side-channel manufactures the carrier — the watcher+reducer e2e lives
    // in tests/watcher/unclaim.rs
    // `in_flight_multi_turn_codex_child_revives_and_relinks_via_unclaim`.
    use pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW;
    use serde_json::json;
    for transport in [Transport::Jsonl, Transport::Hook] {
        let mut scene = SceneState::uniform(4);
        let mut r = Reducer::new();
        let parent = AgentId::from_parts("codex", "parent-sess");
        let child_uuid = "02000000-0000-7000-8000-0000000000cd";
        let child = AgentId::from_parts("codex", child_uuid);
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

        apply_hook_payload(
            &mut r,
            &mut scene,
            json!({
                "hook_event_name": "UserPromptSubmit",
                "session_id": "parent-sess",
                "_pixtuoid_source": "codex",
                "cwd": "/repo",
            }),
            t0,
        );
        apply_hook_payload(
            &mut r,
            &mut scene,
            json!({
                "hook_event_name": "SubagentStart",
                "session_id": "parent-sess",
                "agent_id": child_uuid,
                "cwd": "/repo",
                "_pixtuoid_source": "codex",
            }),
            t0 + Duration::from_secs(1),
        );
        assert_eq!(
            scene.agents.get(&child).map(|s| s.parent_id),
            Some(Some(parent)),
            "first life: child registered with the parent link ({transport:?})"
        );
        // The child's first life ends; the slot exits and GCs.
        let stop = t0 + Duration::from_secs(2);
        apply_hook_payload(
            &mut r,
            &mut scene,
            json!({
                "hook_event_name": "SubagentStop",
                "session_id": "parent-sess",
                "agent_id": child_uuid,
                "_pixtuoid_source": "codex",
            }),
            stop,
        );
        r.tick(
            &mut scene,
            stop + EXIT_GRACE_WINDOW + Duration::from_secs(1),
        );
        assert!(
            !scene.agents.contains_key(&child),
            "child GC'd after its first life"
        );

        // The revival start arrives as a PARENTLESS SessionStart on the same
        // id (a post-un-claim re-emit / flat first-sight shape).
        r.apply(
            &mut scene,
            AgentEvent::SessionStart {
                agent_id: child,
                source: "codex".into(),
                session_id: child_uuid.into(),
                cwd: PathBuf::from("/repo"),
                parent_id: None,
            },
            stop + Duration::from_secs(20),
            transport,
        );
        assert_eq!(
            scene.agents.get(&child).map(|s| s.parent_id),
            Some(Some(parent)),
            "the parentless revival start must re-link to the ledger's \
             remembered parent, not register as an orphan ({transport:?})"
        );
    }
}

#[test]
fn parentless_session_start_enriching_a_parentless_child_slot_adopts_ledger_parent() {
    // The ENRICHMENT-path twin of the registration-path adoption above — the
    // self-heal of the hook-straggler residual: a dead child's hook
    // straggler landing in the (5s, 90s] window re-registers it PARENTLESS
    // (the Identity arm / blank hook synthesis consult only the 5s #242
    // tombstone, never the ledger), so the slot EXISTS parentless when the
    // later parentless SessionStart arrives. That start lands in the
    // duplicate-SessionStart arm, whose enrichment must adopt the ledger's
    // remembered parent — not leave the orphan.
    use pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let parent = AgentId::from_parts("codex", "parent-sess");
    let child_uuid = "05000000-0000-7000-8000-0000000000d0";
    let child = AgentId::from_parts("codex", child_uuid);
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let session_start = |agent_id, sid: &str, parent_id| AgentEvent::SessionStart {
        agent_id,
        source: "codex".into(),
        session_id: sid.into(),
        cwd: PathBuf::from("/repo"),
        parent_id,
    };

    // The child's first life: registered parented (ledger remembers the
    // link), ended as_child, GC'd.
    r.apply(
        &mut scene,
        session_start(parent, "parent-sess", None),
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        session_start(child, child_uuid, Some(parent)),
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: child,
            as_child: true,
        },
        t0 + Duration::from_secs(2),
        Transport::Hook,
    );
    let gone = t0 + Duration::from_secs(2) + EXIT_GRACE_WINDOW + Duration::from_secs(1);
    r.tick(&mut scene, gone);
    assert!(!scene.agents.contains_key(&child), "child GC'd");

    // The hook straggler at +10s (inside the 90s ledger window, no #242
    // tombstone — the end had a slot) re-registers the child PARENTLESS via
    // hook synthesis.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: child,
            tool_use_id: Some("t-straggler".into()),
            detail: None,
        },
        gone + Duration::from_secs(10),
        Transport::Hook,
    );
    assert_eq!(
        scene.agents.get(&child).map(|s| s.parent_id),
        Some(None),
        "precondition: the straggler re-registered the child parentless"
    );

    // The later parentless SessionStart hits the duplicate-SessionStart arm:
    // enrichment must adopt the ledger parent (the self-heal).
    r.apply(
        &mut scene,
        session_start(child, child_uuid, None),
        gone + Duration::from_secs(11),
        Transport::Jsonl,
    );
    assert_eq!(
        scene.agents.get(&child).map(|s| s.parent_id),
        Some(Some(parent)),
        "the enrichment path must adopt the ledger's remembered parent for a \
         parentless child slot"
    );
}

#[test]
fn tombstoned_codex_child_flat_first_sight_relinks_within_ledger_ttl() {
    // #244-w1: a straggler SubagentStop AFTER the child's slot was GC'd
    // lands on an unknown id and mints the #242 tombstone — which can't
    // catch the child's PARENTLESS flat-rollout first-sight (parentless
    // starts are tombstone-exempt for the Reasonix resurrect). The ledger
    // turns that former orphan-phantom into a parent-LINKED registration:
    // it then rides the parent's cascade / the 5-min Codex short-idle reap
    // instead of ghosting as a flat root.
    use pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW;
    use serde_json::json;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let parent = AgentId::from_parts("codex", "parent-sess");
    let child_uuid = "03000000-0000-7000-8000-0000000000ce";
    let child = AgentId::from_parts("codex", child_uuid);
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    apply_hook_payload(
        &mut r,
        &mut scene,
        json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "parent-sess",
            "_pixtuoid_source": "codex",
            "cwd": "/repo",
        }),
        t0,
    );
    let subagent_stop = json!({
        "hook_event_name": "SubagentStop",
        "session_id": "parent-sess",
        "agent_id": child_uuid,
        "_pixtuoid_source": "codex",
    });
    apply_hook_payload(
        &mut r,
        &mut scene,
        json!({
            "hook_event_name": "SubagentStart",
            "session_id": "parent-sess",
            "agent_id": child_uuid,
            "cwd": "/repo",
            "_pixtuoid_source": "codex",
        }),
        t0 + Duration::from_secs(1),
    );
    let stop = t0 + Duration::from_secs(2);
    apply_hook_payload(&mut r, &mut scene, subagent_stop.clone(), stop);
    r.tick(
        &mut scene,
        stop + EXIT_GRACE_WINDOW + Duration::from_secs(1),
    );
    assert!(!scene.agents.contains_key(&child), "child GC'd");

    // The straggler Stop (codex fires one per child turn end) hits the now
    // UNKNOWN id → #242 tombstone minted.
    let straggler = stop + EXIT_GRACE_WINDOW + Duration::from_secs(2);
    apply_hook_payload(&mut r, &mut scene, subagent_stop, straggler);

    // The flat rollout's first-sight lands INSIDE the 5s hook tombstone —
    // parentless, so the #242 gate must not block it, and the ledger must
    // supply the parent.
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "codex".into(),
            session_id: child_uuid.into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        straggler + Duration::from_millis(500),
        Transport::Jsonl,
    );
    assert_eq!(
        scene.agents.get(&child).map(|s| s.parent_id),
        Some(Some(parent)),
        "the tombstoned child's parentless flat first-sight must register \
         parent-LINKED via the ledger (#244-w1), not as an orphan phantom"
    );
}

#[test]
fn adopted_ledger_parent_still_runs_the_cycle_filter() {
    // The adoption seam must not bypass #240's cycle refusal: a ledger entry
    // whose remembered parent has SINCE become a descendant of the reviving
    // child (constructible only through a dangling-parent enrichment naming
    // the dead child — i.e. a poisoned/degenerate lineage) must degrade to
    // PARENTLESS, exactly like a wire-carried cyclic link. Guards the
    // implementation against adopt-without-filter.
    use pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let p = AgentId::from_parts("codex", "p-root");
    let x = AgentId::from_parts("codex", "x-child");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let session_start = |agent_id, sid: &str, parent_id| AgentEvent::SessionStart {
        agent_id,
        source: "codex".into(),
        session_id: sid.into(),
        cwd: PathBuf::from("/repo"),
        parent_id,
    };

    // X registers as P's child → ledger remembers X→P. Then X ends as_child
    // and GCs.
    r.apply(
        &mut scene,
        session_start(p, "p-root", None),
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        session_start(x, "x-child", Some(p)),
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: x,
            as_child: true,
        },
        t0 + Duration::from_secs(2),
        Transport::Hook,
    );
    r.tick(
        &mut scene,
        t0 + Duration::from_secs(2) + EXIT_GRACE_WINDOW + Duration::from_secs(1),
    );
    assert!(!scene.agents.contains_key(&x), "X GC'd");

    // Poison the lineage: P is enriched with the DEAD X as its parent (X has
    // no slot, so the cycle walk can't see X→P any more — the link applies
    // as a tolerated dangle).
    r.apply(
        &mut scene,
        session_start(p, "p-root", Some(x)),
        t0 + Duration::from_secs(10),
        Transport::Hook,
    );
    assert_eq!(
        scene.agents.get(&p).map(|s| s.parent_id),
        Some(Some(x)),
        "precondition: P now dangles on the dead X"
    );

    // X revives parentless inside the ledger TTL → adopting P would close
    // the cycle X→P→X. The filter must refuse and register X parentless.
    r.apply(
        &mut scene,
        session_start(x, "x-child", None),
        t0 + Duration::from_secs(11),
        Transport::Jsonl,
    );
    let slot = scene.agents.get(&x).expect("X re-registers");
    assert_eq!(
        slot.parent_id, None,
        "an adopted ledger parent that would close a cycle must degrade to \
         parentless (the #240 filter runs on adopted links too)"
    );
}

#[test]
fn reasonix_resurrect_is_unaffected_by_a_ledger_entry_for_another_id() {
    // Reasonix safety pin: its cwd-keyed sessions are parentless and never
    // end as_child, so they never enter the ledger — a fresh ledger entry
    // for a DIFFERENT id (a just-ended Codex child) must not perturb the
    // documented SessionEnd→SessionStart resurrect in any way.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let codex_parent = AgentId::from_parts("codex", "parent-sess");
    let codex_child = AgentId::from_parts("codex", "04000000-0000-7000-8000-0000000000cf");
    let rx = AgentId::from_parts("reasonix", "/Users/dev/proj");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    start(&mut r, &mut scene, codex_parent);
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: codex_child,
            source: "codex".into(),
            session_id: "04000000-0000-7000-8000-0000000000cf".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(codex_parent),
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: codex_child,
            as_child: true,
        },
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );

    // The Reasonix `/new` rotation, inside every ledger/tombstone window.
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: rx,
            as_child: false,
        },
        t0 + Duration::from_secs(2),
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: rx,
            source: "reasonix".into(),
            session_id: "/Users/dev/proj".into(),
            cwd: PathBuf::from("/Users/dev/proj"),
            parent_id: None,
        },
        t0 + Duration::from_secs(2) + Duration::from_millis(20),
        Transport::Hook,
    );
    let slot = scene
        .agents
        .get(&rx)
        .expect("the Reasonix resurrect registers");
    assert_eq!(
        slot.parent_id, None,
        "a ledger entry for a DIFFERENT id must never re-parent a Reasonix \
         session (its ids never enter the ledger by construction)"
    );
}
