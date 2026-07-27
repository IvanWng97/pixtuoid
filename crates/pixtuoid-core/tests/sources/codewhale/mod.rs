//! Regression for the CodeWhale subagent hook lifecycle.
//!
//! CodeWhale is HOOK-ONLY. Its `subagent_spawn`/`subagent_complete` observer
//! hooks are forwarded RAW on stdin (no env-mode `--event`), and
//! `decode_cw_subagent` keys the CHILD on its own `agent_id` — NOT the workspace
//! cwd, else it would coalesce with the same-workspace parent — parent-linking to
//! the cwd-keyed parent sprite (the mixed-keying pattern CC/Codex also use). This
//! drives a real spawn→complete flow through the reducer to pin, end-to-end (the
//! conformance harness's one-AgentId rule can't hold a two-sprite scenario): the
//! parent (cwd-keyed) and child (agent_id-keyed) are DISTINCT sprites, the child
//! links to the parent, and `subagent_complete` ends the child (`as_child`) while
//! the parent keeps running. Payload shapes follow CodeWhale's documented
//! observer-hook wire (Hmbown/CodeWhale `crates/tui/src/hooks/config.rs` `HookEvent` +
//! `docs/CONFIGURATION.md`).

use std::path::Path;
use std::time::SystemTime;

use pixtuoid_core::source::decoder::decode_hook_payload;
use pixtuoid_core::source::{AgentEvent, Transport};
use pixtuoid_core::state::reducer::Reducer;
use pixtuoid_core::state::SceneState;
use pixtuoid_core::AgentId;

const WORKSPACE: &str = "/Users/dev/cwproj";
const CHILD: &str = "agent_12345678";

/// Decode the hook payloads in file order (a payload can decode to multiple
/// events — Identity ahead of a tool/permission event, #221).
fn hook_events() -> Vec<AgentEvent> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/sources/codewhale/fixtures/hook-payloads.jsonl");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        .lines()
        .filter(|l| !l.trim().is_empty())
        .flat_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).expect("valid hook json");
            decode_hook_payload(v).expect("CodeWhale hook payload must decode")
        })
        .collect()
}

#[test]
fn codewhale_subagent_spawn_links_child_and_complete_ends_it() {
    let parent = AgentId::from_parts("codewhale", WORKSPACE);
    let child = AgentId::from_parts("codewhale", CHILD);
    assert_ne!(
        parent, child,
        "mixed keying (cwd parent / agent_id child) must not collapse the two"
    );

    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let now = SystemTime::now();
    for ev in hook_events() {
        r.apply(&mut scene, ev, now, Transport::Hook);
    }

    // session_start created the cwd-keyed parent; subagent_spawn created the
    // agent_id-keyed child linked to it.
    let child_slot = scene
        .agents
        .get(&child)
        .expect("subagent_spawn must create the child sprite");
    assert_eq!(
        child_slot.parent_id,
        Some(parent),
        "the child links to the workspace-keyed parent"
    );
    // subagent_complete ends the CHILD; the parent keeps running (its own
    // session_end is a separate event, not part of this flow).
    assert!(
        child_slot.exiting_at.is_some(),
        "subagent_complete must mark the child exiting"
    );
    let parent_slot = scene.agents.get(&parent).expect("parent still present");
    assert!(
        parent_slot.exiting_at.is_none(),
        "the parent must keep running after the subagent completes"
    );
}

#[test]
fn codewhale_subagent_complete_before_spawn_is_a_safe_noop() {
    // Observer hooks are best-effort and unordered: a subagent_complete can win
    // the race against the child's spawn. SessionEnd for a not-yet-existing child
    // must be harmless — no panic, no phantom slot, no spurious parent cascade.
    let parent = AgentId::from_parts("codewhale", WORKSPACE);
    let child = AgentId::from_parts("codewhale", CHILD);
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let now = SystemTime::now();

    // Parent exists; the child's complete arrives with no child slot.
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "codewhale".into(),
            session_id: WORKSPACE.into(),
            cwd: std::path::PathBuf::from(WORKSPACE),
            parent_id: None,
        },
        now,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: child,
            as_child: true,
        },
        now,
        Transport::Hook,
    );
    assert!(
        !scene.agents.contains_key(&child),
        "a subagent_complete for an absent child must not mint a phantom slot"
    );
    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_none(),
        "an orphan subagent_complete must not cascade the unrelated parent"
    );
}
