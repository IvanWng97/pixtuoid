//! `AgentSlot` fixtures the crate's tests share.

use std::time::SystemTime;

use pixtuoid_core::AgentSlot;

pub(crate) fn make_slot(
    id: pixtuoid_core::AgentId,
    state: pixtuoid_core::state::ActivityState,
) -> AgentSlot {
    let now = SystemTime::UNIX_EPOCH;
    AgentSlot {
        agent_id: id,
        source: std::sync::Arc::from("claude-code"),
        session_id: std::sync::Arc::from("s"),
        cwd: std::sync::Arc::from(std::path::Path::new("/x")),
        label: "x".into(),
        state,
        state_started_at: now,
        created_at: now,
        last_event_at: now,
        exiting_at: None,
        pending_idle_at: None,

        desk_index: pixtuoid_core::state::GlobalDeskIndex(0),
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
    }
}

pub(crate) fn make_slot_cwd(id_path: &str, cwd: &str, unknown_cwd: bool) -> AgentSlot {
    let id = pixtuoid_core::AgentId::from_transcript_path(id_path);
    let mut s = make_slot(id, pixtuoid_core::state::ActivityState::Idle);
    s.cwd = std::sync::Arc::from(std::path::Path::new(cwd));
    s.unknown_cwd = unknown_cwd;
    s
}
