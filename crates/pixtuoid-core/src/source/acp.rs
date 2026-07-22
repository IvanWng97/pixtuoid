//! Shared **ACP** (Agent Client Protocol, `agent-client-protocol`) decode — the
//! ONE place the cross-vendor ACP wire vocabulary lives, so any pixtuoid source
//! whose transcript speaks ACP reuses it instead of re-hosting the tag set.
//!
//! ACP is a versioned, published, multi-vendor wire STANDARD (a Rust crate + a
//! JSON Schema + git tags + a changelog), so this is "a shared serde model for a
//! protocol", NOT per-source format knowledge — the documented cross-source
//! exception to architecture invariant #3 (see `pixtuoid-core/CLAUDE.md`), of the
//! same shape as the shared CC-hook arms in `decoder.rs`. What stays PER-SOURCE
//! (invariant #3, injected by the caller): the tool-detail vocabulary + any
//! Task-detection (dispatch judgment), and a source's OWN extension namespace
//! (grok's `_x.ai/session/update` — the `_`-prefix is ACP's reserved
//! implementation-specific marker — is NOT ACP and stays bespoke in grok.rs).
//!
//! Scope today: the **v1** `SessionUpdate` notification under the standard
//! `session/update` method — the tag vocabulary ([`KNOWN_ACP_TAGS`]), the two
//! activity-bearing standard tags, and the flood-safe unknown-tag breadcrumb.
//! grok is the sole caller now; [`decode_session_update`] is written so a future
//! ACP-transcript source plugs in with its own id/cwd/tool-detail. ACP **v2** has
//! a materially different (partly non-overlapping) tag set — it is a future
//! ADDITIVE version arm, deliberately NOT built until a source emits it.

use serde_json::{Map, Value};

use crate::source::{AgentEvent, ToolDetail};
use crate::AgentId;

/// The COMPLETE **latest ACP v1** `sessionUpdate` tag vocabulary — the 13 tags of
/// `schema/v1/schema.unstable.json` (verified live vs the canonical
/// `agentclientprotocol` repo @ v1.6.0; grok pins `features = ["unstable"]`, so the
/// unstable surface is its real vocabulary). grok's currently-resolved
/// `agent-client-protocol-schema` v0.11.4 emits 11 of these; the two plan-op tags
/// (`plan_update`/`plan_removed`) are latest-v1-unstable additions grok will emit on
/// its next ACP bump — kept KNOWN so they never flood. An unrecognized tag under the
/// ACP `session/update` method is a NEW ACP capability worth a one-shot breadcrumb;
/// the three per-token `*_message_chunk` tags MUST stay in this set or the tag tier
/// floods (`drift::unknown_event` has NO dedup). Kept honest by `read_acp_tags` in
/// `check_upstream_drift.py` vs the live v1 schema: a new upstream tag is a review
/// ping BEFORE it can flood.
pub(crate) const KNOWN_ACP_TAGS: &[&str] = &[
    "user_message_chunk", // per-token flood tag — decodes to nothing (no paired end)
    "agent_message_chunk", // per-token flood tag
    "agent_thought_chunk", // per-token flood tag
    "tool_call",          // handled → ActivityStart
    "tool_call_update",   // handled → ActivityEnd on terminal status
    "plan",
    "plan_update",  // latest-v1-unstable (post-v0.11.4); grok emits on next bump
    "plan_removed", // latest-v1-unstable (post-v0.11.4)
    "available_commands_update",
    "current_mode_update",
    "config_option_update",
    "session_info_update",
    "usage_update",
];

/// Decode one ACP `session/update` notification's `update` object into activity
/// events. `agent_id` is the acting agent (the caller derives it — path-keyed for
/// grok). `tool_detail` builds the per-source tool label from the ACP-standard
/// `title` + `rawInput` — the tool vocabulary + any Task-detection is the caller's
/// per-source dispatch judgment (invariant #3), NOT ACP's.
///
/// - `tool_call` → `ActivityStart` (a FRESH call OMITS `status` — Pending is the
///   serde skip-default, so absence still starts).
/// - `tool_call_update` with a TERMINAL `status` (`completed`/`failed`) →
///   `ActivityEnd`; `in_progress` / status-less (a content/locations delta) is not
///   a completion.
/// - a KNOWN-but-not-activity-bearing tag (the message/thought chunks stream per
///   token, `plan`/`usage_update`/… have no sprite lifecycle) → nothing, SILENTLY.
/// - an UNKNOWN tag = a new ACP capability → one flood-safe breadcrumb
///   (`session/update:{tag}`, composed so the drift log distinguishes it from a
///   method-tier crumb), then nothing.
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
        // Known ACP tags we don't map to a sprite lifecycle stay SILENT — the
        // high-cardinality axis (chunks stream per token; breadcrumbing would flood).
        t if KNOWN_ACP_TAGS.contains(&t) => vec![],
        // A new ACP capability under the standard method — the LOW-cardinality
        // structural signal (a bounded, versioned enum) → breadcrumb once.
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
    // A minimal per-source tool-detail builder (the real callers inject their own
    // vocabulary + Task-detection; the shared decode only needs the type).
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
        // A FRESH tool_call OMITS status → ActivityStart keyed on toolCallId.
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
        // Only terminal status ends; in_progress / status-less do not.
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
        // A status-less tool_call_update (a content delta) is not a completion.
        assert!(
            decode(json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1"})).is_empty()
        );
    }

    /// The tag tier (#766): a novel `sessionUpdate` tag breadcrumbs (a new ACP
    /// capability), while every KNOWN tag — INCLUDING the per-token `*_chunk`
    /// flood tags — stays SILENT. This is the flood the tag tier must never open.
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

        // silent-real: every KNOWN tag (the flood chunks + the ignored capabilities)
        // must stay SILENT — a KNOWN_ACP_TAGS omission would flood on the chunks.
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

    /// Teeth: the three per-token `*_message_chunk` tags MUST be in the vocabulary
    /// — a future edit dropping one re-opens the per-token flood. Falsifiable by
    /// construction (remove a chunk from KNOWN_ACP_TAGS → this reddens).
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
