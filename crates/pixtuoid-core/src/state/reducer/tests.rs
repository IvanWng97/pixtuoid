use super::source_label_prefix;
use crate::source::registry;

/// Every registered source needs a 2-char prefix. The unregistered-source
/// fallback silently degrades a missing/short prefix to the long source
/// name (e.g. "opencode·proj" instead of "oc·proj"), which then collides
/// visually with another source sharing a cwd. End-to-end through the
/// REAL `source_label_prefix` (registry lookup included) — stronger than
/// the registry-local shape check, which can't see a name↔row mismatch.
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

/// The back-fill's clobber gate, now recorded at MINT time as
/// [`LabelProvenance`]: only a derivation fallback (ordinal ghost /
/// bare-prefix) may be upgraded; a cwd-basename- or Rename-derived label
/// is real information and never is. Pins the Rename arm's mint-time
/// classification (the one remaining string judgment — bare prefix vs
/// real name) and the upgradability of each variant.
#[test]
fn rename_classification_and_upgradability_cover_each_provenance() {
    use super::classify_rename;
    use crate::state::{LabelProvenance, SlotLabel};
    for (label, source, expect) in [
        // Exactly the registry prefix = the LabelDeriver's empty-cwd
        // fallback — still upgradable by a later cwd-bearing back-fill.
        ("cx", "codex", LabelProvenance::PrefixFallback),
        // Everything else arriving via Rename is a real display name.
        ("cc·repo", "claude-code", LabelProvenance::Renamed),
        ("code-explorer", "claude-code", LabelProvenance::Renamed),
        // No Rename ever mints an ordinal — even an ordinal-LOOKING name
        // is treated as real (only `register_slot` mints OrdinalGhost).
        ("cc#3", "claude-code", LabelProvenance::Renamed),
        // Degenerate: empty is not the prefix.
        ("", "claude-code", LabelProvenance::Renamed),
    ] {
        assert_eq!(
            classify_rename(label, source).provenance(),
            expect,
            "{label:?} under source {source:?} must classify as {expect:?}"
        );
    }
    // The clobber gate per variant: the two derivation fallbacks may be
    // upgraded, the two real-information provenances never.
    assert!(SlotLabel::ordinal_ghost("cc#3").is_upgradable());
    assert!(SlotLabel::ordinal_ghost("#1").is_upgradable());
    assert!(SlotLabel::prefix_fallback("cx").is_upgradable());
    assert!(!SlotLabel::cwd_derived("cc·repo").is_upgradable());
    assert!(!SlotLabel::renamed("code-explorer").is_upgradable());
}

// The `< → <=` (correlation.rs) and `> → >=` (sweep_stale/sweep_exited)
// boundary mutants formerly documented here as accepted equivalents are
// now PINNED: `apply`/`tick`/`gc` all take an injected `now`, so the
// exact boundary is a hand-built SystemTime pair (deterministic, no wall
// clock) — see correlation.rs's test mod and the two
// `*_at_exactly_the_*` tests in tests/reducer/liveness.rs.
//
// One accepted-equivalent residual remains: the SessionStart arm's
// resurrect gate (`slot.exiting_at.is_some() && slot.parent_id.is_none()
// && parent_id.is_none()`) survives an `&&`→`||` flip on the LAST
// conjunct because the two parent sides cannot disagree by the time the
// gate runs — the ledger ADOPTION rewrites a parentless event's
// `parent_id` to the remembered parent, the #244-w2 gate drops a
// parented start on a recently-ended child, and the orphan-enrichment
// just above copies a surviving event link onto `slot.parent_id` — so
// the third conjunct is defense-in-depth (kept deliberately: it is the
// documented "gated on BOTH sides" belt), not independently observable.

/// Pin the deliberate stale-timeout DURATIONS. Every timing test correctly
/// derives its offsets FROM these constants (hardcoded ms make leg tests
/// vacuous), so mutating `10 * 60` also mutates each test's own
/// expectation — leaving the literal value unguarded. A direct pin is the
/// only thing that catches `*`→`/` collapsing a window to 0s (everything
/// reaped on the next tick) or a typo'd minute count. The values ARE the
/// product decision (see the doc comments on each const); change this test
/// deliberately when a window changes, never to make it pass.
#[test]
fn stale_timeout_constants_have_their_intended_durations() {
    use super::{
        PROOF_OF_LIFE_TTL, STALE_ACTIVE_TIMEOUT, STALE_IDLE_TIMEOUT, STALE_SHORT_IDLE_TIMEOUT,
        STALE_UNKNOWN_CWD_TIMEOUT, STALE_WAITING_TIMEOUT,
    };
    use std::time::Duration;
    assert_eq!(STALE_ACTIVE_TIMEOUT, Duration::from_secs(600)); // 10 min
    assert_eq!(STALE_IDLE_TIMEOUT, Duration::from_secs(1800)); // 30 min
    assert_eq!(STALE_WAITING_TIMEOUT, Duration::from_secs(3600)); // 60 min
    assert_eq!(STALE_UNKNOWN_CWD_TIMEOUT, Duration::from_secs(180)); // 3 min
    assert_eq!(STALE_SHORT_IDLE_TIMEOUT, Duration::from_secs(300)); // 5 min
    assert_eq!(PROOF_OF_LIFE_TTL, Duration::from_secs(150)); // 2.5× the 60s poll
}

// The Delegating stale carve-out is caps-driven; pin the POLICY half with
// a synthetic caps value so caps combinations beyond the registered rows
// stay covered — that's what the lookup/policy split exists for. (The
// registered path — reasonix is the row that sets
// `delegations_are_hook_silent` — is pinned end-to-end by
// `reasonix_delegating_slot_survives_the_active_timeout` in
// tests/reducer/liveness.rs.)
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

    // Detail-gate negative: caps on + an ORDINARY tool active must stay on
    // the Active timer — the cap widens the window for delegations only.
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

// The delegation carve-out must key on the TYPED tool kind, not the
// human-facing display string: a GENERIC tool whose display merely spells
// "Delegating" is NOT a delegation and must reap on the normal Active
// timer. (Before `ToolKind`, the policy string-compared the display and
// this slot wrongly got the 60-min Waiting-class window.)
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

// White-box: `gated_before_waiting` is reclaimed in TWO places — `tick`'s
// retain and `sweep_exited`'s explicit remove (the apply path, where tick's
// retain never runs). All existing reducer tests go through `tick`; this
// pins the apply-path eviction so a future refactor can't silently drop it
// and leak a swept Waiting slot's gated tool_use_id.
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
    // Active mid-tool, then a permission Waiting → gate records the tool id.
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

    // End it; advance past the grace window; apply an UNRELATED event so
    // sweep_exited runs on the APPLY path (not tick).
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

// White-box: the resurrect-in-place branch must evict the previous life's
// entries from all three correlation maps while KEEPING the proof-of-life
// vouch (the resurrecting slot's process is alive — that's what a vouch
// asserts). The public pins (tests/reducer/) cover the active_tasks and
// pending_b1_cascades harms behaviorally; a stale `gated_before_waiting`
// entry has no public observable today (every path into Waiting rewrites
// the gate first), so its eviction — and the vouch's survival — are
// pinned directly here.
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
    // Gate: an ordinary tool mid-flight when a permission Waiting fires.
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
    // Tasks: a dispatch that fully drains arms the b1 cascade and leaves
    // an (empty) active_tasks entry behind.
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

    // End + resurrect inside the walkout window (and before the armed
    // cascade's grace elapses).
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

// White-box: the child ledger's BOUNDING contract (#244). An entry is
// created with `ended_at: None` when a parent link is applied; a child
// removed WITHOUT an as_child end (here: the parent's cascade) must get
// ended_at stamped by `sweep_exited` — that both arms the #244-w2 gate
// for those exits and starts the gc clock — and gc must prune it after
// CHILD_END_LEDGER_TTL. Roots never enter the ledger. The public
// behavioral pins live in tests/reducer/child_ledger.rs; the stamping/pruning
// internals have no other observable.
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

    // The parent's clean exit cascades the child out; neither end was
    // `as_child`, so only sweep_exited can stamp the clock.
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
    assert!(
        !r.corr.child_ledger.contains_key(&child),
        "gc must prune an ended entry past CHILD_END_LEDGER_TTL"
    );
}
