//! The NAME-KEYED delegation family: sources whose subagent dispatch is a tool
//! literally called `task`. Grouped by capability rather than by CLI, because
//! the contract under test is one rule shared by all of them — and because each
//! of these runs is two sprites (the child is its own session), which the
//! conformance harness's one-AgentId rule cannot hold.
//!
//! The rule is the point: these sources claim the Task detail by tool NAME and
//! deliberately NOT by the presence of a `subagent_type` key, so a
//! model-authored argument on an ordinary tool cannot spoof a delegation, seed
//! `active_tasks`, and cascade a real child out on drain.

use std::path::Path;

use pixtuoid_core::source::copilot::decode_copilot_line;
use pixtuoid_core::source::decoder::decode_hook_payload;
use pixtuoid_core::source::{AgentEvent, ToolDetail};

fn lines(cli: &str, name: &str) -> Vec<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/sources/delegation/fixtures")
        .join(cli)
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

fn opencode_events() -> Vec<AgentEvent> {
    lines("opencode", "hook-payloads.jsonl")
        .iter()
        .flat_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).expect("valid hook json");
            decode_hook_payload(v).expect("captured opencode payload must decode")
        })
        .collect()
}

fn copilot_events() -> Vec<AgentEvent> {
    // copilot keys the session on the transcript's PARENT dir, so the logical
    // path has to carry the session id the capture ran under.
    let logical = "d28d6fbd-b0cb-421a-abaa-1240ee4dae97/events.jsonl";
    lines("copilot", "events.jsonl")
        .iter()
        .flat_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).expect("valid transcript json");
            decode_copilot_line(logical, "copilot", v).expect("captured copilot line must decode")
        })
        .collect()
}

fn tasks(evs: &[AgentEvent]) -> usize {
    evs.iter()
        .filter(|e| {
            matches!(
                e,
                AgentEvent::ActivityStart {
                    detail: Some(ToolDetail::Task),
                    ..
                }
            )
        })
        .count()
}

#[test]
fn every_name_keyed_source_mints_exactly_one_task_from_its_capture() {
    for (cli, evs) in [
        ("opencode", opencode_events()),
        ("copilot", copilot_events()),
    ] {
        assert_eq!(
            tasks(&evs),
            1,
            "{cli}: the one `task` call in the capture must be the one Task detail"
        );
    }
}

#[test]
fn the_delegation_run_puts_more_than_one_agent_on_the_floor() {
    for (cli, evs) in [
        ("opencode", opencode_events()),
        ("copilot", copilot_events()),
    ] {
        let ids: std::collections::BTreeSet<_> = evs.iter().map(AgentEvent::agent_id).collect();
        assert!(
            ids.len() >= 2,
            "{cli}: a delegation is a parent AND a child — this is why the capture \
             cannot live under the conformance one-AgentId rule, got {}",
            ids.len()
        );
    }
}

#[test]
fn an_ordinary_tool_never_carries_the_task_detail() {
    // The anti-spoof half of the rule, asserted against the same real runs: the
    // captures contain bash/read/glob calls, and exactly none of them may claim
    // Task just because a delegation happened in the same session.
    for (cli, evs) in [
        ("opencode", opencode_events()),
        ("copilot", copilot_events()),
    ] {
        let starts = evs
            .iter()
            .filter(|e| matches!(e, AgentEvent::ActivityStart { .. }))
            .count();
        assert!(
            starts > tasks(&evs),
            "{cli}: the capture must also hold ordinary tools, else the rule is untested"
        );
    }
}
