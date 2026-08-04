use super::source_label_prefix;
use crate::source::registry;

#[test]
fn every_registered_source_has_two_char_label_prefix() {
    for src in registry::registered_source_names() {
        let prefix = source_label_prefix(src);
        assert_eq!(
            prefix.chars().count(),
            2,
            "source {src:?} has no 2-char label prefix (got {prefix:?}) — fix its SourceDescriptor row in source/registry.rs"
        );
    }
}

/// Only a derivation fallback (ordinal ghost / bare prefix) may be upgraded by
/// a later back-fill; a cwd- or Rename-derived label is real information.
#[test]
fn rename_classification_and_upgradability_cover_each_provenance() {
    use super::classify_rename;
    use crate::state::{LabelProvenance, SlotLabel};
    for (label, source, expect) in [
        ("cx", "codex", LabelProvenance::PrefixFallback),
        ("cc·repo", "claude-code", LabelProvenance::Renamed),
        ("code-explorer", "claude-code", LabelProvenance::Renamed),
        ("cc#3", "claude-code", LabelProvenance::Renamed),
        ("", "claude-code", LabelProvenance::Renamed),
    ] {
        assert_eq!(
            classify_rename(label, source).provenance(),
            expect,
            "{label:?} under source {source:?} must classify as {expect:?}"
        );
    }
    assert!(SlotLabel::ordinal_ghost("cc#3").is_upgradable());
    assert!(SlotLabel::ordinal_ghost("#1").is_upgradable());
    assert!(SlotLabel::prefix_fallback("cx").is_upgradable());
    assert!(!SlotLabel::cwd_derived("cc·repo").is_upgradable());
    assert!(!SlotLabel::renamed("code-explorer").is_upgradable());
}

/// Every timing test derives its offsets FROM these constants, so mutating a
/// window also mutates each test's own expectation — only a direct pin catches
/// a collapsed or typo'd duration. Change it deliberately, never to pass.
#[test]
fn stale_timeout_constants_have_their_intended_durations() {
    use super::{
        PROOF_OF_LIFE_TTL, STALE_ACTIVE_TIMEOUT, STALE_IDLE_TIMEOUT, STALE_SHORT_IDLE_TIMEOUT,
        STALE_UNKNOWN_CWD_TIMEOUT, STALE_WAITING_TIMEOUT,
    };
    use std::time::Duration;
    assert_eq!(STALE_ACTIVE_TIMEOUT, Duration::from_secs(600));
    assert_eq!(STALE_IDLE_TIMEOUT, Duration::from_secs(1800));
    assert_eq!(STALE_WAITING_TIMEOUT, Duration::from_secs(3600));
    assert_eq!(STALE_UNKNOWN_CWD_TIMEOUT, Duration::from_secs(180));
    assert_eq!(STALE_SHORT_IDLE_TIMEOUT, Duration::from_secs(300));
    assert_eq!(PROOF_OF_LIFE_TTL, Duration::from_secs(150)); // 2.5× the 60s poll
}

// Synthetic caps on an unregistered source, so the POLICY half stays covered
// for caps combinations the registered rows don't happen to spell.
#[test]
fn delegating_slot_with_hook_silent_caps_gets_waiting_window() {
    use super::{stale_threshold_with_caps, STALE_ACTIVE_TIMEOUT, STALE_WAITING_TIMEOUT};
    use crate::source::registry::SourceCaps;
    use crate::source::{AgentEvent, ToolDetail, Transport};
    use crate::{AgentId, Reducer, SceneState};
    use std::time::SystemTime;
    let caps = SourceCaps {
        has_exit_signal: true,
        resurrects_on_prompt: true,
        delegations_are_hook_silent: true,
    };
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("hook-silent-cli", "/p");
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "hook-silent-cli".into(),
            session_id: "/p".into(),
            cwd: "/p".into(),
            parent_id: None,
        },
        SystemTime::UNIX_EPOCH,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: None,
            detail: Some(ToolDetail::Task),
        },
        SystemTime::UNIX_EPOCH,
        Transport::Hook,
    );
    let slot = scene.agents.get(&id).unwrap();
    assert_eq!(
        stale_threshold_with_caps(slot, Some(caps)),
        STALE_WAITING_TIMEOUT,
        "hook-silent Delegating slot must get the Waiting-class window"
    );
    assert_eq!(
        stale_threshold_with_caps(slot, None),
        STALE_ACTIVE_TIMEOUT,
        "without the cap, Delegating reaps on the normal Active timer"
    );

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: None,
            detail: Some(ToolDetail::Generic {
                display: "bash: ls".into(),
            }),
        },
        SystemTime::UNIX_EPOCH,
        Transport::Hook,
    );
    let slot = scene.agents.get(&id).unwrap();
    assert_eq!(
        stale_threshold_with_caps(slot, Some(caps)),
        STALE_ACTIVE_TIMEOUT,
        "caps-on but non-Task detail must keep the Active timer"
    );
}

#[test]
fn generic_tool_displaying_delegating_keeps_the_active_window() {
    use super::{stale_threshold_with_caps, STALE_ACTIVE_TIMEOUT};
    use crate::source::registry::SourceCaps;
    use crate::source::{AgentEvent, ToolDetail, Transport};
    use crate::{AgentId, Reducer, SceneState};
    use std::time::SystemTime;
    let caps = SourceCaps {
        has_exit_signal: true,
        resurrects_on_prompt: true,
        delegations_are_hook_silent: true,
    };
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_parts("hook-silent-cli", "/p");
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "hook-silent-cli".into(),
            session_id: "/p".into(),
            cwd: "/p".into(),
            parent_id: None,
        },
        SystemTime::UNIX_EPOCH,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: None,
            detail: Some(ToolDetail::Generic {
                display: "Delegating".into(),
            }),
        },
        SystemTime::UNIX_EPOCH,
        Transport::Hook,
    );
    let slot = scene.agents.get(&id).unwrap();
    assert_eq!(
        stale_threshold_with_caps(slot, Some(caps)),
        STALE_ACTIVE_TIMEOUT,
        "a Generic tool spelling 'Delegating' must not ride the delegation carve-out"
    );
}

#[test]
fn gated_before_waiting_evicted_on_apply_path_sweep() {
    use crate::source::{AgentEvent, ToolDetail, Transport};
    use crate::state::SceneState;
    use crate::AgentId;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    let mut r = super::Reducer::new();
    let mut scene = SceneState::uniform(4);
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    let t0 = SystemTime::now();
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
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: Some("toolT".into()),
            detail: Some(ToolDetail::from("Bash")),
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: id,
            reason: "perm".into(),
        },
        t0,
        Transport::Hook,
    );
    assert!(
        r.corr.gated_before_waiting.contains_key(&id),
        "gate recorded while Waiting mid-tool"
    );

    // The UNRELATED event below is what runs sweep_exited on the APPLY path.
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: id,
            as_child: false,
        },
        t0,
        Transport::Hook,
    );
    let later = t0 + super::EXIT_GRACE_WINDOW + Duration::from_secs(1);
    let other = AgentId::from_transcript_path("/p/other.jsonl");
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: other,
            source: "claude-code".into(),
            session_id: "s2".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        later,
        Transport::Hook,
    );

    assert!(
        !scene.agents.contains_key(&id),
        "exited slot swept on the apply path"
    );
    assert!(
        !r.corr.gated_before_waiting.contains_key(&id),
        "apply-path sweep_exited must evict the gated entry (not only tick's retain)"
    );
}

#[test]
fn resurrect_in_place_evicts_correlation_maps_but_keeps_proof_of_life() {
    use crate::source::{AgentEvent, ToolDetail, Transport};
    use crate::state::SceneState;
    use crate::AgentId;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    let mut r = super::Reducer::new();
    let mut scene = SceneState::uniform(4);
    let id = AgentId::from_transcript_path("/p/res-maps.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let session_start = |sid: &str| AgentEvent::SessionStart {
        agent_id: id,
        source: "claude-code".into(),
        session_id: sid.into(),
        cwd: PathBuf::from("/repo"),
        parent_id: None,
    };
    r.apply(&mut scene, session_start("s"), t0, Transport::Hook);
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: Some("t-gate".into()),
            detail: Some(ToolDetail::from("Bash")),
        },
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: id,
            reason: "perm".into(),
        },
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    // A dispatch that fully drains arms the b1 cascade and leaves an (empty)
    // active_tasks entry behind.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            tool_use_id: Some("task-1".into()),
            detail: Some(ToolDetail::from("Agent")),
        },
        t0 + Duration::from_secs(2),
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("task-1".into()),
        },
        t0 + Duration::from_secs(3),
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::ProofOfLife { agent_id: id },
        t0 + Duration::from_secs(3),
        Transport::Hook,
    );
    assert!(
        r.corr.active_tasks.contains_key(&id),
        "ledger entry populated"
    );
    assert!(
        r.corr.gated_before_waiting.contains_key(&id),
        "gate populated"
    );
    assert!(r.pending_b1_cascades.contains_key(&id), "cascade armed");
    assert!(
        r.corr.recent_proof_of_life.contains_key(&id),
        "vouch recorded"
    );

    // The resurrect must land inside the walkout window AND before the armed
    // cascade's grace elapses.
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: id,
            as_child: false,
        },
        t0 + Duration::from_secs(4),
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        session_start("s2"),
        t0 + Duration::from_millis(4_500),
        Transport::Jsonl,
    );

    assert!(
        scene
            .agents
            .get(&id)
            .is_some_and(|s| s.exiting_at.is_none()),
        "resurrected"
    );
    assert!(
        !r.corr.active_tasks.contains_key(&id),
        "resurrect must evict the dead life's active_tasks entry"
    );
    assert!(
        !r.corr.gated_before_waiting.contains_key(&id),
        "resurrect must evict the dead life's gated_before_waiting entry"
    );
    assert!(
        !r.pending_b1_cascades.contains_key(&id),
        "resurrect must disarm the dead life's pending b1 cascade"
    );
    assert!(
        r.corr.recent_proof_of_life.contains_key(&id),
        "the vouch must SURVIVE resurrection — the process is alive"
    );
}

/// Past the GATE's `CHILD_END_LEDGER_TTL` the entry is deliberately RETAINED:
/// a parentless revival re-links through its `parent_id` across an unbounded
/// turn gap, so the link rides the longer relink budget. (Pruning at the gate's
/// TTL is what made a multi-turn child idle >90s come back an orphan.)
#[test]
fn child_ledger_is_stamped_on_sweep_and_pruned_by_gc() {
    use crate::source::{AgentEvent, Transport};
    use crate::state::SceneState;
    use crate::AgentId;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    let mut r = super::Reducer::new();
    let mut scene = SceneState::uniform(4);
    let parent = AgentId::from_parts("codex", "ledger-parent");
    let child = AgentId::from_parts("codex", "ledger-child");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let session_start = |agent_id, sid: &str, parent_id| AgentEvent::SessionStart {
        agent_id,
        source: "codex".into(),
        session_id: sid.into(),
        cwd: PathBuf::from("/repo"),
        parent_id,
    };
    r.apply(
        &mut scene,
        session_start(parent, "ledger-parent", None),
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        session_start(child, "ledger-child", Some(parent)),
        t0,
        Transport::Hook,
    );
    assert!(
        !r.corr.child_ledger.contains_key(&parent),
        "a root registration must not enter the child ledger"
    );
    let entry = r
        .corr
        .child_ledger
        .get(&child)
        .expect("child link recorded");
    assert_eq!(entry.parent_id, Some(parent));
    assert!(entry.ended_at.is_none(), "alive — no gc clock yet");

    // Neither end below is `as_child`, so only sweep_exited can stamp the clock.
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: parent,
            as_child: false,
        },
        t0 + Duration::from_secs(1),
        Transport::Hook,
    );
    let swept = t0 + Duration::from_secs(1) + super::EXIT_GRACE_WINDOW + Duration::from_secs(1);
    r.tick(&mut scene, swept);
    assert!(!scene.agents.contains_key(&child), "child swept");
    assert!(
        r.corr
            .child_ledger
            .get(&child)
            .is_some_and(|e| e.ended_at.is_some()),
        "sweep_exited must stamp ended_at for a child whose end wasn't as_child"
    );

    r.tick(
        &mut scene,
        swept + super::CHILD_END_LEDGER_TTL + Duration::from_secs(1),
    );
    assert_eq!(
        r.corr.child_ledger.get(&child).and_then(|e| e.parent_id),
        Some(parent),
        "the parent link must outlive the end GATE it does not share a purpose with"
    );

    r.tick(
        &mut scene,
        swept + super::CHILD_END_RELINK_TTL + Duration::from_secs(1),
    );
    assert!(
        !r.corr.child_ledger.contains_key(&child),
        "gc must prune an ended entry past CHILD_END_RELINK_TTL"
    );
}

/// A synthetic ~1 event/s stream through the REAL reducer: a per-event leak
/// (a missed prune site) grows a map ~linearly and blows the bound, while a
/// healthy stream stays an order of magnitude under.
#[test]
fn correlation_maps_stay_bounded_across_a_long_stream() {
    use crate::source::{AgentEvent, ToolDetail, Transport};
    use crate::state::SceneState;
    use crate::AgentId;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    /// ~3× the widest steady-state working set (CHILD_END_RELINK_TTL = 300s at
    /// ~1 event/s ⇒ ~300 entries) and FAR below ITERS. A bound that merely
    /// clears the steady state proves nothing about a slow leak.
    const MAX_CORR_ENTRIES: usize = 1024;
    const ITERS: u64 = 3_000;
    const MAP_NAMES: [&str; 7] = [
        "recent_hook_tool_uses",
        "recent_hook_session_ends",
        "active_tasks",
        "recent_task_drains",
        "child_ledger",
        "recent_proof_of_life",
        "gated_before_waiting",
    ];

    let mut r = super::Reducer::new();
    // 32 desks comfortably exceeds the un-swept working set: a slot lives until
    // `sweep_exited` removes it ~EXIT_GRACE_WINDOW after its end, so only a
    // handful are concurrent at ~1s/iter.
    let mut scene = SceneState::uniform(32);
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    let mut peak = [0usize; 7];

    for i in 0..ITERS {
        let now = t0 + Duration::from_secs(i);
        let a = AgentId::from_parts("claude-code", &format!("s{i}"));

        r.apply(
            &mut scene,
            AgentEvent::SessionStart {
                agent_id: a,
                source: "claude-code".into(),
                session_id: format!("s{i}"),
                cwd: PathBuf::from("/repo"),
                parent_id: None,
            },
            now,
            Transport::Hook,
        );
        r.apply(
            &mut scene,
            AgentEvent::ActivityStart {
                agent_id: a,
                tool_use_id: Some("task1".into()),
                detail: Some(ToolDetail::Task),
            },
            now,
            Transport::Hook,
        );
        r.apply(
            &mut scene,
            AgentEvent::ActivityEnd {
                agent_id: a,
                tool_use_id: Some("task1".into()),
            },
            now,
            Transport::Hook,
        );
        r.apply(
            &mut scene,
            AgentEvent::ActivityStart {
                agent_id: a,
                tool_use_id: Some("gate1".into()),
                detail: Some(ToolDetail::Generic {
                    display: "Bash".into(),
                }),
            },
            now,
            Transport::Hook,
        );
        r.apply(
            &mut scene,
            AgentEvent::Waiting {
                agent_id: a,
                reason: "perm".into(),
            },
            now,
            Transport::Hook,
        );
        r.apply(
            &mut scene,
            AgentEvent::ProofOfLife { agent_id: a },
            now,
            Transport::Hook,
        );
        r.apply(
            &mut scene,
            AgentEvent::SessionEnd {
                agent_id: a,
                as_child: true,
            },
            now,
            Transport::Hook,
        );
        // An UNREGISTERED id (fresh each iter) is what mints a tombstone.
        r.apply(
            &mut scene,
            AgentEvent::SessionEnd {
                agent_id: AgentId::from_parts("claude-code", &format!("u{i}")),
                as_child: false,
            },
            now,
            Transport::Hook,
        );
        // An ORPHAN Task (no SessionStart, so no slot) leaves an active_tasks
        // entry reaped ONLY by tick's slot-removal retain.
        r.apply(
            &mut scene,
            AgentEvent::ActivityStart {
                agent_id: AgentId::from_parts("claude-code", &format!("o{i}")),
                tool_use_id: Some("orphan".into()),
                detail: Some(ToolDetail::Task),
            },
            now,
            Transport::Hook,
        );

        // The interleaved tick is the ONLY reaper of the non-TTL orphan retains.
        r.tick(&mut scene, now);

        let lens = [
            r.corr.recent_hook_tool_uses.len(),
            r.corr.recent_hook_session_ends.len(),
            r.corr.active_tasks.len(),
            r.corr.recent_task_drains.len(),
            r.corr.child_ledger.len(),
            r.corr.recent_proof_of_life.len(),
            r.corr.gated_before_waiting.len(),
        ];
        for (p, &l) in peak.iter_mut().zip(lens.iter()) {
            *p = (*p).max(l);
        }
        for (l, name) in lens.iter().zip(MAP_NAMES) {
            assert!(
                *l <= MAX_CORR_ENTRIES,
                "Correlation map `{name}` grew to {l} at iter {i} (> {MAX_CORR_ENTRIES}) — a prune site is missing"
            );
        }
    }

    for (p, name) in peak.iter().zip(MAP_NAMES) {
        assert!(
            *p > 0,
            "Correlation map `{name}` was never populated — its bound check was vacuous"
        );
    }
}
