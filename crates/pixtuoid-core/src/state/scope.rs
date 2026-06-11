//! The agent **scope** layer (Layer B) — the parent↔subagent tree and the
//! lifecycle rules that propagate along it.
//!
//! The reducer runs two stacked state machines: the per-agent FSM (Layer A —
//! `Idle / Active / Waiting` plus the exit + debounce lifecycle, in
//! [`super::reducer`]) and this **scope** layer over `AgentSlot.parent_id`. The
//! scope encodes one invariant — *a subagent's lifetime is contained in its
//! parent's* (structured concurrency / an OTP-style supervision tree) — and
//! expresses it as a few directional operations the reducer delegates to.
//!
//! Housing them here gives the containment invariant a single home: a new
//! lifecycle concern becomes a function in this module rather than yet another
//! bespoke `parent_id` walk bolted onto the reducer (which is exactly how this
//! logic accreted before — cascade, then liveness, then readiness, then
//! completion, each a separate reactive scan).
//!
//! - **exit flows DOWN** — [`cascade_exit`]: a node leaving takes its whole
//!   subtree. Used by `SessionEnd`, the stale-sweep, and subagent-completion.
//! - **liveness flows UP** — [`refresh_lineage`]: a working descendant keeps its
//!   ancestors alive, so a blocked-but-delegating parent isn't stale-swept.
//! - **readiness, queried UP** — [`has_waiting_ancestor`]: a node blocked under a
//!   `Waiting` ancestor is "not ready", not dead (liveness vs readiness, k8s-style).

use std::collections::{BTreeMap, HashSet};
use std::time::SystemTime;

use crate::state::{ActivityState, AgentSlot, SceneState};
use crate::AgentId;

/// Mark every not-yet-exiting descendant of `root` exiting, BFS over `parent_id`
/// links (exit flows DOWN). `root` is only the BFS seed and is never re-stamped
/// *by this function* — whether the caller marks `root` itself is caller-specific:
/// the `SessionEnd` arm and `sweep_stale` stamp it first (the whole subtree
/// leaves together), while subagent-completion does NOT (the parent keeps
/// running; only its subtree leaves). Idempotent: slots already exiting are
/// filtered out, so a leaf or a partly-exiting subtree is a safe no-op.
///
/// FOOTGUN: whether `root` itself exits is encoded IMPLICITLY by whether the
/// caller set `root.exiting_at` *before* calling — there is no assertion here to
/// catch a caller that forgets. A future caller that means "the whole tree
/// leaves" but neglects to stamp `root` first would silently leave the root
/// running while exiting its entire subtree. All three current callers are
/// correct (SessionEnd + sweep_stale stamp root first; subagent-completion
/// deliberately does not); if a fourth is added, make the per-call-site intent
/// explicit (e.g. a typed `StampRoot::{Yes,No}` param) rather than relying on
/// this convention.
pub(crate) fn cascade_exit(scene: &mut SceneState, root: AgentId, now: SystemTime) {
    let mut visited: HashSet<AgentId> = HashSet::new();
    visited.insert(root);
    let mut frontier = vec![root];
    while let Some(parent) = frontier.pop() {
        let children: Vec<AgentId> = scene
            .agents
            .values()
            .filter(|s| s.parent_id == Some(parent) && s.exiting_at.is_none())
            .map(|s| s.agent_id)
            .collect();
        for cid in children {
            if visited.insert(cid) {
                if let Some(slot) = scene.agents.get_mut(&cid) {
                    slot.exiting_at = Some(now);
                }
                frontier.push(cid);
            }
        }
    }
}

/// Refresh `last_event_at` for `id` and every ancestor (liveness flows UP), so a
/// parent (and grandparent) isn't stale-swept while a descendant is still
/// emitting events — even if the parent's own hooks dropped or a subagent's hook
/// was misattributed to it. The mirror of [`cascade_exit`]. Cycle-guarded;
/// `last_event_at` only gates the stale-sweep, so this never alters an ancestor's
/// visible state/pose. The `None => break` arm tolerates a DANGLING `parent_id`
/// (a JSONL-first orphan whose parent slot was never created, or a parent already
/// swept from `scene.agents` — `sweep_exited` removes a parent without nulling its
/// children's `parent_id`, by design): the walk stops at the missing link, a safe
/// no-op rather than a crash. Intentional, not a bug.
pub(crate) fn refresh_lineage(scene: &mut SceneState, id: AgentId, now: SystemTime) {
    let mut visited: HashSet<AgentId> = HashSet::new();
    let mut cur = Some(id);
    while let Some(aid) = cur {
        if !visited.insert(aid) {
            break;
        }
        match scene.agents.get_mut(&aid) {
            Some(slot) => {
                slot.last_event_at = now;
                cur = slot.parent_id;
            }
            None => break,
        }
    }
}

/// True if any ancestor of `id` (walking `parent_id`, the node itself excluded)
/// satisfies `pred`. The ONE cycle-guarded ancestor walk behind the readiness
/// queries — [`has_waiting_ancestor`] and `sweep_stale`'s vouched-delegating-
/// ancestor exemption express their predicates through it so the walk (cycle
/// guard, dangling-parent tolerance) can't fork. Takes `&BTreeMap` rather than
/// `&SceneState` so it can be called inside `sweep_stale`'s pass-1 closure
/// while `&scene.agents` is already borrowed immutably — `&SceneState` would
/// conflict with that live borrow. The chain is shallow in practice; the
/// `None => break` arm tolerates a dangling `parent_id` (same contract as
/// [`refresh_lineage`]).
pub(crate) fn has_ancestor_where(
    agents: &BTreeMap<AgentId, AgentSlot>,
    id: AgentId,
    pred: impl Fn(&AgentSlot) -> bool,
) -> bool {
    // Seeded with the start node: in a parent cycle the walk returns to `id`
    // and would otherwise run `pred` on it before the cycle guard breaks —
    // violating "the node itself excluded" (a Waiting cycle member counting
    // as its own Waiting ancestor self-exempts from sweep_stale forever).
    let mut visited: HashSet<AgentId> = HashSet::from([id]);
    let mut cur = agents.get(&id).and_then(|s| s.parent_id);
    while let Some(pid) = cur {
        if !visited.insert(pid) {
            break;
        }
        match agents.get(&pid) {
            Some(p) if pred(p) => return true,
            Some(p) => cur = p.parent_id,
            None => break,
        }
    }
    false
}

/// True if any ancestor of `id` is in `Waiting` state. A subagent's permission
/// `Notification` is attributed to the PARENT (the hook `transcript_path` is
/// the parent's), so the parent goes `Waiting` while the blocked subagent stays
/// `Active`. Such a subagent is paused on a human gate the ancestor holds —
/// "not ready", not dead — so `sweep_stale` exempts it from the aggressive
/// Active timer (liveness vs readiness).
pub(crate) fn has_waiting_ancestor(agents: &BTreeMap<AgentId, AgentSlot>, id: AgentId) -> bool {
    has_ancestor_where(agents, id, |p| {
        matches!(p.state, ActivityState::Waiting { .. })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    fn slot(id: AgentId, parent_id: Option<AgentId>, state: ActivityState) -> AgentSlot {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        AgentSlot {
            agent_id: id,
            source: Arc::from("cc"),
            session_id: Arc::from("s"),
            cwd: Arc::from(std::path::Path::new("/repo")),
            label: Arc::from("cc·repo"),
            state,
            state_started_at: now,
            last_event_at: now,
            created_at: now,
            exiting_at: None,
            pending_idle_at: None,
            desk_index: crate::state::GlobalDeskIndex(0),
            floor_idx: 0,
            tool_call_count: 0,
            active_ms: 0,
            unknown_cwd: false,
            parent_id,
        }
    }

    fn waiting() -> ActivityState {
        ActivityState::Waiting {
            reason: Arc::from("perm"),
        }
    }

    // --- Dangling-parent guard: a child whose `parent_id` points to a slot that
    // does not exist in `scene.agents` (the `None => break` arms). -------------

    #[test]
    fn refresh_lineage_tolerates_dangling_parent_id() {
        let child = AgentId::from_transcript_path("/p/child.jsonl");
        let missing = AgentId::from_transcript_path("/p/never-created.jsonl");
        let mut scene = SceneState::uniform(4);
        scene
            .agents
            .insert(child, slot(child, Some(missing), ActivityState::Idle));

        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000);
        // Must not panic walking into the missing parent; stamps only the child.
        refresh_lineage(&mut scene, child, now);

        assert_eq!(scene.agents.get(&child).unwrap().last_event_at, now);
        assert!(
            !scene.agents.contains_key(&missing),
            "the dangling parent is never materialized by the walk"
        );
    }

    #[test]
    fn has_waiting_ancestor_false_when_parent_id_dangling() {
        let child = AgentId::from_transcript_path("/p/child.jsonl");
        let missing = AgentId::from_transcript_path("/p/never-created.jsonl");
        let mut scene = SceneState::uniform(4);
        scene
            .agents
            .insert(child, slot(child, Some(missing), ActivityState::Idle));

        assert!(
            !has_waiting_ancestor(&scene.agents, child),
            "a dangling parent_id is not a Waiting ancestor — the walk breaks safely"
        );
    }

    // --- Cycle guard: a `parent_id` cycle A->B->A (two SessionStarts each naming
    // the other) must terminate via `visited` (the `!visited.insert(_) { break }`
    // arms), not hang. ----------------------------------------------------------

    fn cycle_scene(
        a_state: ActivityState,
        b_state: ActivityState,
    ) -> (SceneState, AgentId, AgentId) {
        let a = AgentId::from_transcript_path("/p/a.jsonl");
        let b = AgentId::from_transcript_path("/p/b.jsonl");
        let mut scene = SceneState::uniform(4);
        scene.agents.insert(a, slot(a, Some(b), a_state));
        scene.agents.insert(b, slot(b, Some(a), b_state));
        (scene, a, b)
    }

    #[test]
    fn refresh_lineage_terminates_on_parent_id_cycle() {
        let (mut scene, a, b) = cycle_scene(ActivityState::Idle, ActivityState::Idle);
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000);

        // Walks a -> b -> a; the revisit of `a` hits `!visited.insert` -> break.
        // Reaching this assertion at all proves no infinite loop.
        refresh_lineage(&mut scene, a, now);

        assert_eq!(scene.agents.get(&a).unwrap().last_event_at, now);
        assert_eq!(
            scene.agents.get(&b).unwrap().last_event_at,
            now,
            "the cycle's other node is stamped exactly once before the break"
        );
    }

    #[test]
    fn has_waiting_ancestor_breaks_on_cycle_with_no_waiting_node() {
        // Neither node Waiting: the walk a -> b -> (a revisited) hits the cycle
        // break and returns false instead of looping forever.
        let (scene, a, _b) = cycle_scene(ActivityState::Idle, ActivityState::Idle);
        assert!(
            !has_waiting_ancestor(&scene.agents, a),
            "a cycle with no Waiting node must terminate and return false"
        );
    }

    #[test]
    fn has_waiting_ancestor_true_via_cyclic_ancestor() {
        // B is Waiting and is A's parent: the very first hop short-circuits true,
        // before the cycle break — confirms the cycle setup doesn't mask a real
        // Waiting ancestor.
        let (scene, a, _b) = cycle_scene(ActivityState::Idle, waiting());
        assert!(has_waiting_ancestor(&scene.agents, a));
    }

    #[test]
    fn has_waiting_ancestor_excludes_self_reached_via_cycle() {
        // The queried node is the cycle's ONLY Waiting member: the walk
        // b -> a -> (b again) must break BEFORE the predicate runs on the
        // start node — "the node itself excluded" — or a Waiting cycle member
        // counts as its own Waiting ancestor and self-exempts from
        // `sweep_stale` forever (an immortal slot).
        let (scene, _a, b) = cycle_scene(ActivityState::Idle, waiting());
        assert!(
            !has_waiting_ancestor(&scene.agents, b),
            "a node must never count as its own Waiting ancestor through a parent cycle"
        );
    }

    #[test]
    fn has_waiting_ancestor_excludes_self_parented_node() {
        // The degenerate one-node cycle: a slot naming ITSELF as parent must
        // not satisfy the predicate through its own state.
        let x = AgentId::from_transcript_path("/p/self.jsonl");
        let mut scene = SceneState::uniform(4);
        scene.agents.insert(x, slot(x, Some(x), waiting()));
        assert!(
            !has_waiting_ancestor(&scene.agents, x),
            "a self-parented Waiting node is not its own ancestor"
        );
    }
}
