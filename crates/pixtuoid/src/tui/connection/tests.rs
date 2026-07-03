use super::*;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use pixtuoid_core::source::manager::SourceDeath;
use pixtuoid_core::state::{ActivityState, AgentSlot, GlobalDeskIndex};
use pixtuoid_core::AgentId;

// `Target` moved with the status model to `crate::sources`; import it directly
// (these row tests exercise the re-exported `build_rows`/`build_rows_from`).
use crate::install::target::Target;

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
        label: "x".into(),
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
fn build_rows_from_state_follows_connected_set_with_nocli_override() {
    let cc = claude_target();
    let present = |connected| RowInput {
        source_id: "claude",
        label_prefix: "cc",
        target: Some(cc),
        facts: Some(RowFacts {
            present: true,
            config_path: Some(PathBuf::from("/c")),
        }),
        connected,
        health: None,
    };
    let absent = |connected| RowInput {
        source_id: "claude",
        label_prefix: "cc",
        target: Some(cc),
        facts: Some(RowFacts {
            present: false,
            config_path: Some(PathBuf::from("/c")),
        }),
        connected,
        health: None,
    };
    let no_target = |connected| RowInput {
        source_id: "antigravity",
        label_prefix: "ag",
        target: None,
        facts: None,
        connected,
        health: None,
    };
    let rows = build_rows_from(vec![
        present(true),    // present + connected → Connected
        present(false),   // present + unbound → Disconnected
        absent(true),     // CLI absent → NoCli even if a stale flag says connected
        no_target(true),  // no-target source binds via flag only → Connected
        no_target(false), // → Disconnected
    ]);
    assert_eq!(rows[0].state, ConnState::Connected);
    assert_eq!(rows[1].state, ConnState::Disconnected);
    assert_eq!(rows[2].state, ConnState::NoCli);
    assert_eq!(rows[3].state, ConnState::Connected);
    assert_eq!(rows[4].state, ConnState::Disconnected);
    assert_eq!(rows[0].display_name, "Claude Code");
    assert_eq!(rows[0].config_path, Some(PathBuf::from("/c")));
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
            facts: Some(RowFacts {
                present: true,
                config_path: None,
            }),
            connected: true,
            health: None,
        },
        RowInput {
            source_id: "antigravity",
            label_prefix: "ag",
            target: None,
            facts: None,
            connected: false,
            health: None,
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
    let rows = build_rows(&HashSet::new(), "");
    assert!(rows.len() >= 5, "expected the 5 install targets + sources");
    assert!(
        rows.iter()
            .any(|r| r.source_id == "antigravity" && r.target.is_none()),
        "antigravity must appear as a no-target (JSONL) row"
    );
    let scene = SceneState::uniform(8);
    let live = live_view(SystemTime::UNIX_EPOCH, &rows, &scene, &[]);
    assert_eq!(live.len(), rows.len());
}

#[test]
fn build_rows_honors_the_connected_set() {
    // A source named in the connected-set renders Connected (unless its CLI is
    // absent → NoCli); one omitted renders Disconnected/NoCli — never Connected.
    let mut set = HashSet::new();
    set.insert("antigravity".to_string()); // no-target → flag alone drives it
    let rows = build_rows(&set, "");
    let ag = rows.iter().find(|r| r.source_id == "antigravity").unwrap();
    assert_eq!(ag.state, ConnState::Connected);
    // A source NOT in the set is never Connected.
    for r in &rows {
        if r.source_id != "antigravity" {
            assert_ne!(
                r.state,
                ConnState::Connected,
                "source {:?} not in the set but rendered Connected",
                r.source_id
            );
        }
    }
}

// Regression guard for the claude-code-vs-claude join (review CRITICAL): the
// REAL build_rows() must produce an actionable (target-bearing) row for EVERY
// registry source that has an install target — Antigravity is the only
// JSONL/no-target source. A namespace drift between `SourceDescriptor.name` and
// `Target.core_source` would make a flagship row non-actionable, and this fails.
#[test]
fn build_rows_makes_every_source_with_a_target_actionable() {
    use pixtuoid_core::source::registry::REGISTRY;
    let rows = build_rows(&HashSet::new(), "");
    for d in REGISTRY {
        let row = rows
            .iter()
            .find(|r| r.source_id == d.name)
            .unwrap_or_else(|| panic!("no Connection row for registered source {:?}", d.name));
        let has_target = crate::install::target::by_source(d.name).is_some();
        // `target.is_some()` IS actionability — the connection state (Connected/
        // Disconnected/NoCli) depends on FS presence, which is environment-specific.
        assert_eq!(
            row.target.is_some(),
            has_target,
            "source {:?} target join drifted (row target={:?}, registry has_target={has_target})",
            d.name,
            row.target.map(|t| t.name),
        );
    }
    // The flagship specifically: claude-code → a "Claude Code" actionable row.
    let claude = rows.iter().find(|r| r.source_id == "claude-code").unwrap();
    assert!(
        claude.target.is_some(),
        "Claude Code row must be actionable"
    );
    assert_eq!(claude.display_name, "Claude Code");
}

#[test]
fn every_no_target_row_has_an_explicit_display_name_not_the_raw_id() {
    // A NO-TARGET source's display name comes from `display_name_for`, which must
    // have an explicit arm — its `other => other` fallthrough leaks the lowercase
    // registry id into the panel name column + prompts (the PR #292 `copilot` nit).
    // Target-bearing rows are exempt: they use `Target.display_name`, which may be
    // a deliberate lowercase brand (e.g. "opencode"). Mechanized so the next
    // no-target source fails loudly.
    for row in build_rows(&HashSet::new(), "") {
        if row.target.is_none() {
            assert_ne!(
                row.display_name, row.source_id,
                "no-target source {:?} shows its raw id — add a title-cased arm to display_name_for",
                row.source_id
            );
        }
    }
}

#[test]
fn format_connect_result_renders_connected_plus_backup_and_path_notes() {
    use crate::install::{InstallOutcome, InstallReport};
    let base = |outcome, backup, path_warning| InstallReport {
        outcome,
        config_path: PathBuf::from("/c"),
        backup,
        path_warning,
    };
    // Both outcomes read as "connected" (the flag flip is the real action).
    let plain = format_connect_result(&base(InstallOutcome::Installed, None, false), "Claude Code");
    assert_eq!(plain, "\u{2713} Claude Code connected");
    assert_eq!(
        format_connect_result(
            &base(InstallOutcome::AlreadyUpToDate, None, false),
            "Claude Code"
        ),
        "\u{2713} Claude Code connected"
    );
    // Backup + PATH notes append.
    let noted = format_connect_result(
        &base(
            InstallOutcome::Installed,
            Some(PathBuf::from("/c.bak")),
            true,
        ),
        "Claude Code",
    );
    assert!(noted.contains("connected"), "{noted}");
    assert!(noted.contains("backup saved"), "{noted}");
    assert!(noted.contains("PATH"), "{noted}");
}

#[test]
fn format_disconnect_result_renders_disconnected_plus_backup_note() {
    use crate::install::{UninstallOutcome, UninstallReport};
    let removed = UninstallReport {
        outcome: UninstallOutcome::Removed,
        config_path: PathBuf::from("/c"),
        removed_backup: Some(PathBuf::from("/c.bak")),
    };
    let s = format_disconnect_result(&removed, "Claude Code");
    assert!(s.contains("disconnected"), "{s}");
    assert!(s.contains("backup cleared"), "{s}");

    // NothingToRemove still reads as disconnected, no backup line.
    let nothing = UninstallReport {
        outcome: UninstallOutcome::NothingToRemove,
        config_path: PathBuf::from("/c"),
        removed_backup: None,
    };
    let s2 = format_disconnect_result(&nothing, "Codex");
    assert!(s2.contains("disconnected"), "{s2}");
    assert!(!s2.contains("backup"), "{s2}");
}

#[test]
fn no_action_hint_distinguishes_nocli_from_actionable() {
    let cc = claude_target();
    let rows = build_rows_from(vec![
        RowInput {
            source_id: "claude",
            label_prefix: "cc",
            target: Some(cc),
            facts: Some(RowFacts {
                present: false, // → NoCli
                config_path: None,
            }),
            connected: false,
            health: None,
        },
        RowInput {
            source_id: "claude",
            label_prefix: "cc",
            target: Some(cc),
            facts: Some(RowFacts {
                present: true, // → Disconnected (actionable)
                config_path: None,
            }),
            connected: false,
            health: None,
        },
    ]);
    assert_eq!(rows[0].state, ConnState::NoCli);
    assert!(
        no_action_hint(&rows[0]).contains("not detected"),
        "NoCli hint: {}",
        no_action_hint(&rows[0])
    );
    // The actionable fallback arm (not normally surfaced — the painter routes
    // Disconnected to the action line — but the toggle effect calls it).
    assert!(
        no_action_hint(&rows[1]).contains("nothing to do"),
        "fallback hint: {}",
        no_action_hint(&rows[1])
    );
}
