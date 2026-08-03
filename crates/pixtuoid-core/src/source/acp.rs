//! Shared **ACP** (Agent Client Protocol) decode — a published multi-vendor wire
//! STANDARD, so hosting its vocabulary once is the documented exception to
//! architecture invariant #3 (see `pixtuoid-core/CLAUDE.md`). What stays PER-SOURCE
//! and must be injected by the caller: the tool-detail vocabulary + any
//! Task-detection, and a source's OWN `_`-prefixed extension namespace (grok's
//! `_x.ai/session/update` is NOT ACP and stays bespoke in grok.rs).
//!
//! Scope is ACP **v1**'s `session/update` notification. **v2**'s tag set is
//! materially different and is a future ADDITIVE arm, not built until a source
//! emits it.

use serde_json::{Map, Value};

use crate::source::{AgentEvent, ToolDetail};
use crate::AgentId;

/// The COMPLETE latest ACP v1 `sessionUpdate` tag vocabulary
/// (`schema/v1/schema.unstable.json` — grok pins `features = ["unstable"]`, so the
/// unstable surface is its real vocabulary; some tags land only on grok's next ACP
/// bump and are KNOWN early so they never flood). The three per-token
/// `*_message_chunk` tags MUST stay in this set or the unknown-tag breadcrumb floods
/// (`drift::unknown_event` has NO dedup). `read_acp_tags` in
/// `check_upstream_drift.py` pings review when upstream adds a tag, BEFORE it can
/// flood.
pub(crate) const KNOWN_ACP_TAGS: &[&str] = &[
    "user_message_chunk",
    "agent_message_chunk",
    "agent_thought_chunk",
    "tool_call",
    "tool_call_update",
    "plan",
    "plan_update",
    "plan_removed",
    "available_commands_update",
    "current_mode_update",
    "config_option_update",
    "session_info_update",
    "usage_update",
];

/// Decode one ACP `session/update` notification's `update` object into activity
/// events; the caller injects `agent_id` and its own `tool_detail` (invariant #3).
///
/// A FRESH `tool_call` OMITS `status` (Pending is the serde skip-default), so
/// absence still starts. Only a TERMINAL `tool_call_update` status
/// (`completed`/`failed`) ends — `in_progress` and status-less content deltas
/// are not completions.
pub(crate) fn decode_session_update(
    agent_id: AgentId,
    source: &str,
    update: &Map<String, Value>,
    tool_detail: impl Fn(&str, Option<&Value>) -> ToolDetail,
) -> Vec<AgentEvent> {
    let str_field = |key: &str| update.get(key).and_then(|s| s.as_str());
    let tool_call_id = || str_field("toolCallId").map(String::from);

    match str_field("sessionUpdate").unwrap_or("") {
        "tool_call" => vec![AgentEvent::ActivityStart {
            agent_id,
            tool_use_id: tool_call_id(),
            detail: Some(tool_detail(
                str_field("title").unwrap_or("?"),
                update.get("rawInput"),
            )),
        }],
        "tool_call_update" => match str_field("status") {
            Some("completed") | Some("failed") => vec![AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id: tool_call_id(),
            }],
            _ => vec![],
        },
        t if KNOWN_ACP_TAGS.contains(&t) => vec![],
        t if !t.is_empty() => {
            crate::source::drift::unknown_event(source, &format!("session/update:{t}"));
            vec![]
        }
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SRC: &str = "grok";
    fn agent() -> AgentId {
        AgentId::from_parts(SRC, "sess")
    }
    fn detail(title: &str, _raw: Option<&Value>) -> ToolDetail {
        ToolDetail::Generic {
            display: title.to_string(),
        }
    }
    fn decode(update: Value) -> Vec<AgentEvent> {
        decode_session_update(agent(), SRC, update.as_object().unwrap(), detail)
    }

    #[test]
    fn tool_call_starts_and_terminal_update_ends_keyed_by_tool_call_id() {
        match decode(json!({"sessionUpdate": "tool_call", "toolCallId": "c1", "title": "grep"}))
            .as_slice()
        {
            [AgentEvent::ActivityStart {
                tool_use_id: Some(id),
                detail: Some(ToolDetail::Generic { display }),
                ..
            }] => {
                assert_eq!(id, "c1");
                assert_eq!(display, "grep");
            }
            other => panic!("expected one ActivityStart, got {other:?}"),
        }
        for status in ["completed", "failed"] {
            match decode(
                json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": status}),
            )
            .as_slice()
            {
                [AgentEvent::ActivityEnd {
                    tool_use_id: Some(id),
                    ..
                }] => assert_eq!(id, "c1"),
                other => panic!("expected one ActivityEnd for {status}, got {other:?}"),
            }
        }
        for status in ["in_progress", "pending"] {
            assert!(
                decode(json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": status})).is_empty(),
                "{status} must not end the activity"
            );
        }
        assert!(
            decode(json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1"})).is_empty()
        );
    }

    #[test]
    fn unknown_tag_breadcrumbs_but_known_tags_stay_silent() {
        let logs = crate::test_capture::capture_logs(|| {
            assert!(
                decode(json!({"sessionUpdate": "teleport_update"})).is_empty(),
                "an unknown tag decodes to no events"
            );
        });
        assert!(
            logs.contains("unknown_event") && logs.contains("session/update:teleport_update"),
            "a new ACP tag must breadcrumb the composed name, got:\n{logs}"
        );

        for tag in KNOWN_ACP_TAGS {
            let quiet = crate::test_capture::capture_logs(|| {
                decode(json!({ "sessionUpdate": tag }));
            });
            assert!(
                !quiet.contains("unknown_event"),
                "known ACP tag {tag:?} must NOT breadcrumb (it would flood), got:\n{quiet}"
            );
        }
    }

    #[test]
    fn the_per_token_flood_chunks_are_in_the_vocabulary() {
        for chunk in [
            "user_message_chunk",
            "agent_message_chunk",
            "agent_thought_chunk",
        ] {
            assert!(
                KNOWN_ACP_TAGS.contains(&chunk),
                "{chunk} must stay in KNOWN_ACP_TAGS or the tag tier floods per token"
            );
        }
    }
}
