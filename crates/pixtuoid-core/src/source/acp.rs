//! Shared **ACP** (Agent Client Protocol) decode — a published multi-vendor wire
//! STANDARD, so hosting its vocabulary once is the documented exception to
//! architecture invariant #3 (the workspace `AGENTS.md`). What stays PER-SOURCE
//! and must be injected by the caller: the tool-detail vocabulary + any
//! Task-detection, and a source's OWN `_`-prefixed extension namespace (grok's
//! `_x.ai/session/update` is NOT ACP and stays bespoke in grok.rs).
//!
//! Scope is ACP **v1**'s `session/update` notification. **v2**'s tag set is
//! materially different and is a future ADDITIVE arm, not built until a source
//! emits it.

use serde_json::{Map, Value};

use crate::AgentId;
#[cfg(any(test, feature = "upstream"))]
use crate::source::upstream::{Doc, Family, Finding, Read, Release, Repo, Siblings, Watch, When};
use crate::source::{AgentEvent, ToolDetail};

const TOOL_CALL: &str = "tool_call";
const TOOL_CALL_UPDATE: &str = "tool_call_update";

const STATUS_COMPLETED: &str = "completed";
const STATUS_FAILED: &str = "failed";

/// The `ToolCallStatus` values that END a tool call. Pinned to the arms by
/// `tool_call_starts_and_terminal_update_ends_keyed_by_tool_call_id`.
#[cfg(any(test, feature = "upstream"))]
pub(crate) const DECODED_TERMINAL_STATUSES: &[&str] = &[STATUS_COMPLETED, STATUS_FAILED];

/// The `sessionUpdate` tags this decoder turns into events. Pinned to the arms
/// below by `the_decoded_tag_set_is_exactly_what_the_arms_match`.
///
/// Off shipped builds: only the drift watch and tests read it. The ARMS are what
/// production dispatches on, and a second copy of the vocabulary must not be
/// something the shipped crate can read and drift against.
#[cfg(any(test, feature = "upstream"))]
pub(crate) const DECODED_TAGS: &[&str] = &[TOOL_CALL, TOOL_CALL_UPDATE];

/// The notification [`decode_session_update`] decodes; a source gates its
/// call on this method.
pub(crate) const SESSION_UPDATE_METHOD: &str = "session/update";

#[cfg(any(test, feature = "upstream"))]
const SCHEMA_TAG: &str = "v0.11.4";

#[cfg(any(test, feature = "upstream"))]
const REPO: Repo = Repo {
    name: "agentclientprotocol/agent-client-protocol",
    release: Release::Pinned {
        tag: SCHEMA_TAG,
        why: "grok-build runs the `agent-client-protocol-schema` its `Cargo.lock` pins",
    },
};

/// Both schemas, each proven by the declaration a watch reads: grok pins
/// `features = ["unstable"]`, so its ACP surface is the UNION of stable and
/// unstable.
#[cfg(any(test, feature = "upstream"))]
const fn schemas(anchor: &'static str, owns: &'static str) -> [Doc; 2] {
    [
        REPO.doc("schema/schema.json", anchor, owns),
        REPO.doc("schema/schema.unstable.json", anchor, owns),
    ]
}

#[cfg(any(test, feature = "upstream"))]
pub(crate) const UPSTREAM: &[Watch] = &[
    Watch {
        id: "acp.method",
        ours: || vec![SESSION_UPDATE_METHOD],
        docs: &schemas(r#""x-method"\s*:"#, "the schema's method declarations"),
        read: Read::Each {
            pattern: r#""x-method"\s*:\s*"{name}""#,
        },
        when: When::Vanishes,
        finding: Finding::Breaking(
            "a source gates the ACP decode on this method, so every ACP-standard \
             activity event is lost with no breadcrumb",
        ),
        siblings: None,
    },
    Watch {
        id: "acp.session_update_tags",
        ours: || DECODED_TAGS.to_vec(),
        docs: &schemas(r#""SessionUpdate"\s*:"#, "the `SessionUpdate` union"),
        read: Read::JsonUnion {
            def: "SessionUpdate",
            prop: Some("sessionUpdate"),
        },
        when: When::Vanishes,
        finding: Finding::Breaking(
            "every ACP-speaking source loses that half of its activity decode",
        ),
        siblings: Some(Siblings {
            family: Family::Prefix(TOOL_CALL),
            ignored: &[],
        }),
    },
    Watch {
        id: "acp.terminal_statuses",
        ours: || DECODED_TERMINAL_STATUSES.to_vec(),
        docs: &schemas(r#""ToolCallStatus"\s*:"#, "the `ToolCallStatus` union"),
        read: Read::JsonUnion {
            def: "ToolCallStatus",
            prop: None,
        },
        when: When::Vanishes,
        finding: Finding::Breaking(
            "the `tool_call_update` arm falls through to `_ => vec![]`, so every \
             ACP tool call stays Active forever",
        ),
        siblings: None,
    },
    Watch {
        id: "acp.grok_pin",
        ours: || vec![SCHEMA_TAG.trim_start_matches('v')],
        docs: &[crate::source::grok::REPO.doc(
            "Cargo.lock",
            r#"(?m)^name = "agent-client-protocol-schema""#,
            "grok's `agent-client-protocol-schema` lock entry",
        )],
        read: Read::Matches {
            pattern: r#"(?m)^name = "agent-client-protocol-schema"\s+version = "([^"]+)""#,
        },
        when: When::Vanishes,
        finding: Finding::Review(
            "the ACP watches read a schema grok no longer runs: repin `SCHEMA_TAG` to the one \
             it locks",
        ),
        siblings: None,
    },
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
    update: &Map<String, Value>,
    tool_detail: impl Fn(&str, Option<&Value>) -> ToolDetail,
) -> Vec<AgentEvent> {
    let str_field = |key: &str| update.get(key).and_then(|s| s.as_str());
    let tool_call_id = || str_field("toolCallId").map(String::from);

    match str_field("sessionUpdate").unwrap_or("") {
        TOOL_CALL => vec![AgentEvent::ActivityStart {
            agent_id,
            tool_use_id: tool_call_id(),
            detail: Some(tool_detail(
                str_field("title").unwrap_or("?"),
                update.get("rawInput"),
            )),
        }],
        TOOL_CALL_UPDATE => match str_field("status") {
            Some(STATUS_COMPLETED) | Some(STATUS_FAILED) => vec![AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id: tool_call_id(),
            }],
            _ => vec![],
        },
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
        decode_session_update(agent(), update.as_object().unwrap(), detail)
    }

    #[test]
    fn tool_call_starts_and_terminal_update_ends_keyed_by_tool_call_id() {
        match decode(json!({"sessionUpdate": "tool_call", "toolCallId": "c1", "title": "grep"}))
            .as_slice()
        {
            [
                AgentEvent::ActivityStart {
                    tool_use_id: Some(id),
                    detail: Some(ToolDetail::Generic { display }),
                    ..
                },
            ] => {
                assert_eq!(id, "c1");
                assert_eq!(display, "grep");
            }
            other => panic!("expected one ActivityStart, got {other:?}"),
        }
        for status in DECODED_TERMINAL_STATUSES {
            match decode(
                json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": status}),
            )
            .as_slice()
            {
                [
                    AgentEvent::ActivityEnd {
                        tool_use_id: Some(id),
                        ..
                    },
                ] => assert_eq!(id, "c1"),
                other => panic!("expected one ActivityEnd for {status}, got {other:?}"),
            }
        }
        // The negative half binds the export's WIDTH: an added member would have
        // to break one of these, so the `acp.terminal_statuses` watch cannot
        // quietly grow.
        for status in ["in_progress", "pending"] {
            assert!(
                !DECODED_TERMINAL_STATUSES.contains(&status),
                "{status} must not be in the exported terminal set"
            );
            assert!(
                decode(json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": status})).is_empty(),
                "{status} must not end the activity"
            );
        }
        assert!(
            decode(json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1"})).is_empty()
        );
    }

    /// The exported set IS the arms: every member drives the decoder to a real
    /// event and a non-member decodes to nothing. Without this the const is a
    /// second copy of the vocabulary, free to drift off the arms it describes —
    /// and the drift watcher reads the const, not the arms.
    #[test]
    fn the_decoded_tag_set_is_exactly_what_the_arms_match() {
        assert_eq!(DECODED_TAGS, ["tool_call", "tool_call_update"]);
        for tag in DECODED_TAGS {
            let update = json!({
                "sessionUpdate": tag,
                "toolCallId": "c1",
                "title": "grep",
                "status": "completed",
            });
            assert!(!decode(update).is_empty(), "{tag} must reach a real arm");
        }
        assert!(decode(json!({"sessionUpdate": "tool_call_pending"})).is_empty());
    }

    /// Blank, per-token and brand-new tags alike decode to nothing IN SILENCE:
    /// a tag we never read cannot cost us an event, and breadcrumbing one meant
    /// hand-maintaining the whole v1 vocabulary to hold back a per-token flood.
    #[test]
    fn no_session_update_tag_breadcrumbs_however_new() {
        for update in [
            json!({}),
            json!({"sessionUpdate": ""}),
            json!({"sessionUpdate": "agent_message_chunk"}),
            json!({"sessionUpdate": "usage_update"}),
            json!({"sessionUpdate": "teleport_update"}),
        ] {
            let logs = crate::test_capture::capture_logs(|| {
                assert!(
                    decode(update.clone()).is_empty(),
                    "{update} decodes to no events"
                );
            });
            assert!(
                !logs.contains(crate::source::drift::TARGET),
                "{update} reached the drift log:\n{logs}"
            );
        }
    }
}
