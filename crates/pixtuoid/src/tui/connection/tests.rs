use super::*;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use pixtuoid_core::source::manager::SourceDeath;
use pixtuoid_core::state::{ActivityState, AgentSlot, GlobalDeskIndex};
use pixtuoid_core::AgentId;

fn claude_target() -> &'static Target {
    crate::install::target::by_name("claude").expect("claude target registered")
}

/// A slot with the fields `live_for` reads; the rest are inert.
fn mk_slot(id: AgentId, source: &str, last_event_at: SystemTime) -> AgentSlot {
    AgentSlot {
        agent_id: id,
        source: Arc::from(source),
        session_id: Arc::from("s"),
        cwd: Arc::from(Path::new("/repo")),
        label: Arc::from("x"),
        state: ActivityState::Idle,
        state_started_at: SystemTime::UNIX_EPOCH,
        created_at: SystemTime::UNIX_EPOCH,
        last_event_at,
        exiting_at: None,
        pending_idle_at: None,
        desk_index: GlobalDeskIndex(0),
        floor_idx: 0,
        tool_call_count: 0,
        active_ms: 0,
        unknown_cwd: false,
        parent_id: None,
    }
}

#[test]
fn build_rows_from_classifies_present_absent_installed_and_jsonl() {
    let cc = claude_target();
    let with = |present, installed| {
        Some(HookFacts {
            present,
            installed,
            config_path: Some(PathBuf::from("/c")),
        })
    };
    let rows = build_rows_from(vec![
        RowInput {
            source_id: "claude",
            label_prefix: "cc",
            target: Some(cc),
            facts: with(true, true),
        },
        RowInput {
            source_id: "claude",
            label_prefix: "cc",
            target: Some(cc),
            facts: with(true, false),
        },
        RowInput {
            source_id: "claude",
            label_prefix: "cc",
            target: Some(cc),
            facts: with(false, false),
        },
        RowInput {
            source_id: "antigravity",
            label_prefix: "ag",
            target: None,
            facts: None,
        },
    ]);
    assert_eq!(rows[0].hooks, HookState::On);
    assert_eq!(rows[1].hooks, HookState::Off);
    assert_eq!(rows[2].hooks, HookState::NoCli);
    assert_eq!(rows[3].hooks, HookState::JsonlNoHooks);
    assert_eq!(rows[0].display_name, "Claude Code");
    assert_eq!(rows[3].display_name, "Antigravity");
    assert!(rows[3].target.is_none());
    assert!(rows[3].config_path.is_none());
}

#[test]
fn live_for_counts_groups_and_ages() {
    let base = SystemTime::UNIX_EPOCH;
    let now = base + Duration::from_secs(100);
    let mut scene = SceneState::uniform(8);
    let a = AgentId::from_transcript_path("/p/a.jsonl");
    let b = AgentId::from_transcript_path("/p/b.jsonl");
    let c = AgentId::from_transcript_path("/p/c.jsonl");
    // two claude-code agents (t=90 and t=95 → max → age 5s), one codex (t=80).
    scene
        .agents
        .insert(a, mk_slot(a, "claude-code", base + Duration::from_secs(90)));
    scene
        .agents
        .insert(b, mk_slot(b, "claude-code", base + Duration::from_secs(95)));
    scene
        .agents
        .insert(c, mk_slot(c, "codex", base + Duration::from_secs(80)));

    let none: &[SourceDeath] = &[];
    let cc = live_for(now, "claude-code", &scene, none);
    assert_eq!(cc.agents, 2);
    assert_eq!(cc.last_event_age, Some(Duration::from_secs(5)));
    assert!(!cc.dead);

    // An empty source → idle (0 agents, no age) — both sides of the count.
    let empty = live_for(now, "reasonix", &scene, none);
    assert_eq!(empty.agents, 0);
    assert_eq!(empty.last_event_age, None);

    // `dead` from a matching SourceDeath (keyed on the same name string).
    let health = [SourceDeath::new("codex", "boom")];
    let cx = live_for(now, "codex", &scene, &health);
    assert_eq!(cx.agents, 1);
    assert!(cx.dead);
}

#[test]
fn move_selection_clamps_at_both_ends() {
    let cc = claude_target();
    let rows = build_rows_from(vec![
        RowInput {
            source_id: "claude",
            label_prefix: "cc",
            target: Some(cc),
            facts: Some(HookFacts {
                present: true,
                installed: true,
                config_path: None,
            }),
        },
        RowInput {
            source_id: "antigravity",
            label_prefix: "ag",
            target: None,
            facts: None,
        },
    ]);
    assert_eq!(move_selection(&rows, 0, -1), 0); // clamp at the low end
    assert_eq!(move_selection(&rows, 0, 1), 1);
    assert_eq!(move_selection(&rows, 1, 1), 1); // clamp at the high end
    assert_eq!(move_selection(&[], 0, 1), 0); // empty → 0
}

#[test]
fn build_rows_covers_every_registry_source_with_aligned_live_view() {
    // The real registry-backed builder produces one row per source, and
    // live_view returns a parallel vec of the same length (the painter relies on
    // the index alignment).
    let rows = build_rows();
    assert!(rows.len() >= 5, "expected the 5 install targets + sources");
    assert!(
        rows.iter()
            .any(|r| r.source_id == "antigravity" && r.hooks == HookState::JsonlNoHooks),
        "antigravity must appear as a JSONL/no-hooks row"
    );
    let scene = SceneState::uniform(8);
    let live = live_view(SystemTime::UNIX_EPOCH, &rows, &scene, &[]);
    assert_eq!(live.len(), rows.len());
}

// Regression guard for the claude-code-vs-claude join (review CRITICAL): the
// REAL build_rows() must produce an actionable (target-bearing) row for EVERY
// registry source that has an install target — Antigravity is the only
// JSONL/no-target source. A namespace drift between `SourceDescriptor.name` and
// `Target.core_source` would make a flagship row non-actionable, and this fails.
#[test]
fn build_rows_makes_every_source_with_a_target_actionable() {
    use pixtuoid_core::source::registry::REGISTRY;
    let rows = build_rows();
    for d in REGISTRY {
        let row = rows
            .iter()
            .find(|r| r.source_id == d.name)
            .unwrap_or_else(|| panic!("no Connection row for registered source {:?}", d.name));
        let has_target = crate::install::target::by_source(d.name).is_some();
        if has_target {
            assert!(
                row.target.is_some() && row.hooks != HookState::JsonlNoHooks,
                "source {:?} has an install target but its Connection row is non-actionable \
                 (target={:?}, hooks={:?}) — the registry/target join drifted",
                d.name,
                row.target.map(|t| t.name),
                row.hooks,
            );
        } else {
            assert!(
                row.target.is_none() && row.hooks == HookState::JsonlNoHooks,
                "source {:?} has no install target but rendered as actionable",
                d.name,
            );
        }
    }
    // The flagship specifically: claude-code → a "Claude Code" actionable row.
    let claude = rows.iter().find(|r| r.source_id == "claude-code").unwrap();
    assert!(
        claude.target.is_some(),
        "Claude Code row must be actionable"
    );
    assert_eq!(claude.display_name, "Claude Code");
}
