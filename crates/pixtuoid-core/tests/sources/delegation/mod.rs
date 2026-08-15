//! The NAME-KEYED delegation family: sources whose subagent dispatch is a tool
//! literally called `task`. They claim the Task detail by tool NAME and
//! deliberately NOT by the presence of a `subagent_type` key, so a
//! model-authored argument on an ordinary tool cannot spoof a delegation, seed
//! `active_tasks`, and cascade a real child out on drain.

use std::path::Path;

use pixtuoid_core::source::copilot::decode_copilot_line;
use pixtuoid_core::source::decoder::decode_hook_payload;
use pixtuoid_core::source::omp::decode_omp_line;
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

fn omp_events() -> Vec<AgentEvent> {
    // omp writes the PARENT as `<ts>_<id>.jsonl` and each child inside a
    // same-named DIR, so the id comes from this file's own stem.
    let logical = "2026-08-15T18-35-53-366Z_01a006b5-8096-7000-9fed-3dc3604e8efb.jsonl";
    lines("omp", "parent.jsonl")
        .iter()
        .flat_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).expect("valid transcript json");
            decode_omp_line(logical, "omp", v).expect("captured omp line must decode")
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
        ("omp", omp_events()),
    ] {
        assert_eq!(
            tasks(&evs),
            1,
            "{cli}: the one `task` call in the capture must be the one Task detail"
        );
    }
}

#[test]
fn the_child_is_in_band_for_some_of_them_and_a_separate_file_for_others() {
    // Not cosmetic: where the child is ANNOUNCED in the parent's own stream, the
    // capture is two sprites and cannot sit under the conformance one-AgentId
    // rule. omp announces nothing — its child is a sibling transcript the
    // watcher discovers — so its parent capture is a single agent.
    for (cli, evs, in_band) in [
        ("opencode", opencode_events(), true),
        ("copilot", copilot_events(), true),
        ("omp", omp_events(), false),
    ] {
        let ids: std::collections::BTreeSet<_> = evs.iter().map(AgentEvent::agent_id).collect();
        assert_eq!(
            ids.len() >= 2,
            in_band,
            "{cli}: in-band child announcement is {in_band}, got {} agent(s)",
            ids.len()
        );
    }
}

/// The call id sitting beside a `task` tool name in the SAME raw object.
fn task_call_ids(raw: &[String]) -> std::collections::BTreeSet<String> {
    fn walk(v: &serde_json::Value, out: &mut std::collections::BTreeSet<String>) {
        match v {
            serde_json::Value::Object(o) => {
                let names = ["tool", "toolName", "name"];
                let ids = ["toolCallId", "callID", "call_id", "id"];
                if names
                    .iter()
                    .any(|k| o.get(*k).and_then(|n| n.as_str()) == Some("task"))
                {
                    if let Some(id) = ids.iter().find_map(|k| o.get(*k).and_then(|i| i.as_str())) {
                        out.insert(id.to_string());
                    }
                }
                o.values().for_each(|c| walk(c, out));
            }
            serde_json::Value::Array(a) => a.iter().for_each(|c| walk(c, out)),
            _ => {}
        }
    }
    let mut out = std::collections::BTreeSet::new();
    for l in raw {
        walk(
            &serde_json::from_str(l).expect("valid capture json"),
            &mut out,
        );
    }
    out
}

#[test]
fn the_task_detail_lands_on_the_call_actually_named_task() {
    // It has to BIND: counting one Task and one ordinary tool passes just as
    // well when the decoder tags the bash call and leaves the dispatch bare.
    for (cli, evs, raw) in [
        (
            "opencode",
            opencode_events(),
            lines("opencode", "hook-payloads.jsonl"),
        ),
        (
            "copilot",
            copilot_events(),
            lines("copilot", "events.jsonl"),
        ),
        ("omp", omp_events(), lines("omp", "parent.jsonl")),
    ] {
        let dispatch = task_call_ids(&raw);
        assert!(
            !dispatch.is_empty(),
            "{cli}: the capture must name a `task` call, else this rule is untested"
        );
        let starts: Vec<(&Option<String>, bool)> = evs
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ActivityStart {
                    tool_use_id,
                    detail,
                    ..
                } => Some((tool_use_id, matches!(detail, Some(ToolDetail::Task)))),
                _ => None,
            })
            .collect();
        assert!(
            starts.len() > tasks(&evs),
            "{cli}: the capture must also hold ordinary tools, else the rule is untested"
        );
        for (id, is_task) in &starts {
            let named_task = id.as_deref().is_some_and(|i| dispatch.contains(i));
            assert_eq!(
                *is_task, named_task,
                "{cli}: the Task detail must sit on the `task` call ({dispatch:?}) and \
                 on no other — start {id:?} claims Task={is_task}"
            );
        }
    }
}
