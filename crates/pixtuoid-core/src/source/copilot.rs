//! GitHub Copilot CLI source. Watches the agentic `copilot` (`@github/copilot`)
//! session transcript (`<copilot_home>/session-state/<sessionId>/events.jsonl`)
//! via `JsonlWatcher`. Transcript-ONLY (Antigravity/Codex-class): the whole
//! lifecycle is persisted to `events.jsonl` — `session.start`,
//! `tool.execution_start/complete`, `permission.requested/completed`,
//! `subagent.started/completed`, `session.task_complete`, `session.shutdown` —
//! so there is no hook install target (the Sources panel shows `cp·` as a
//! no-target flag-flip row, like Antigravity). Only streaming events
//! (`session.idle`, `*_delta`, `*_progress`, …) carry `ephemeral` and never hit
//! disk; the decoder simply ignores everything it doesn't map.
//!
//! Grounded in the canonical schema (npm `@github/copilot` tarball
//! `schemas/session-events.schema.json`) + real on-disk `events.jsonl` files:
//! two committed (the `tool-run` fixture + the tamirdresher `subagent.started/
//! completed` lines) plus a live `copilot` 1.0.62 capture (#294) that upgraded
//! `permission.requested/completed` (approve + both deny kinds) and
//! `subagent.failed` from schema-faithful to byte-real (see
//! `docs/superpowers/specs/2026-06-14-copilot-cli-source-design.md`). The
//! captured event lines are verbatim; the lone neutralized field is the
//! `permission` conformance fixture's `session.start` `cwd` (a machine temp
//! path → `/home/user/project`) so the insta golden stays portable.
//!
//! Sharp edges (real-byte-confirmed):
//! - **Session id = the PARENT-DIR UUID** of `events.jsonl` (the filename stem is
//!   the constant `events`, NOT the id) — `copilot_id_from_path`.
//! - **Sub-agents INTERLEAVE in the root file**, distinguished by the top-level
//!   envelope `agentId` (== the spawning `task` tool's `data.toolCallId`); there
//!   is no per-agent file split. A line with `agentId` set belongs to that child.
//! - `subagent.completed` is **minimal** on disk (`toolCallId`/`agentName`/
//!   `agentDisplayName` only) — never require model/token/duration fields.
//! - The `ephemeral` envelope flag is inconsistent across CLI versions — never
//!   rely on it; map by `type` and ignore the rest.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use crate::source::decoder::{ellipsize, generic_tool_display, MAX_DECODED_FIELD_CHARS};
use crate::source::{AgentEvent, ToolDetail};
use crate::AgentId;

// The runtime half (`CopilotSource` + its watcher wiring, incl. the
// first-sight session-ended checker) — ONE gate for the whole `native` layer
// of this source; the re-export keeps the pre-split
// `source::copilot::CopilotSource` path.
#[cfg(feature = "native")]
mod native;
#[cfg(feature = "native")]
pub use native::CopilotSource;

pub const SOURCE_NAME: &str = "copilot";

/// `$COPILOT_HOME` if set, else `~/.copilot`. An empty OR whitespace-only
/// `$COPILOT_HOME` counts as unset via the shared `platform::nonempty` trim
/// authority (was `var_os` + empty-only, which let a `"  "` value resolve the
/// sessions base to a relative `"  /…"`).
pub fn copilot_home() -> PathBuf {
    match crate::platform::nonempty(std::env::var("COPILOT_HOME").ok()) {
        Some(v) => PathBuf::from(v),
        None => PathBuf::from(crate::platform::user_home()).join(".copilot"),
    }
}

/// The session id = the **parent directory name** of `events.jsonl`
/// (`…/session-state/<sessionId>/events.jsonl`). The filename stem is the
/// constant `events`, so — unlike CC/Codex — the id is the containing dir.
/// A parentless path (never produced by the directory-walking watcher) yields
/// an empty key, not the colliding constant stem `events`.
pub fn copilot_id_from_path(path: &Path) -> String {
    path.parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

pub fn derive_copilot_label(_path: &Path, source: &str, cwd: &Path) -> String {
    crate::source::decoder::derive_prefixed_label(source, cwd)
}

fn str_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str())
}

/// The subagent child key for a `subagent.*` envelope: the top-level `agentId`
/// (== the spawning `task` tool's `data.toolCallId`), falling back to
/// `data.toolCallId`. One spelling shared by the started/completed/failed arms.
fn copilot_child_key<'a>(v: &'a Value, data: Option<&'a Value>) -> Option<&'a str> {
    str_at(v, "agentId")
        .filter(|s| !s.is_empty())
        .or_else(|| data.and_then(|d| str_at(d, "toolCallId")))
}

/// First-sight cwd extractor (the walker's head scan, dispatched via the
/// registry row — invariant #3): Copilot's `session.start` nests the cwd at
/// `data.context.cwd`. Without it, a copilot transcript gated at first sight
/// then tail-revived would register an empty-cwd root (→ the short unknown-cwd
/// reap) and the root's label would degrade from `cp·<dir>` to `cp#N`.
pub(crate) fn extract_copilot_cwd(v: &Value) -> Option<PathBuf> {
    v.get("data")?
        .get("context")?
        .get("cwd")?
        .as_str()
        .map(PathBuf::from)
}

/// Decode one `events.jsonl` line into zero or more `AgentEvent`s. Unknown,
/// ephemeral, or malformed shapes return `vec![]` (the watcher logs + continues;
/// this never panics — real files carry embedded-newline / U+2028 corruption,
/// upstream copilot-cli #2649/#2012).
pub fn decode_copilot_line(
    transcript_path: &str,
    source: &str,
    v: Value,
) -> Result<Vec<AgentEvent>> {
    let root = AgentId::from_parts(source, &copilot_id_from_path(Path::new(transcript_path)));
    let Some(obj) = v.as_object() else {
        return Ok(vec![]);
    };
    let kind = obj.get("type").and_then(|s| s.as_str()).unwrap_or("");
    let data = obj.get("data");

    // A line tagged with a top-level `agentId` belongs to that sub-agent;
    // otherwise it is the root agent. (Sub-agents interleave in the root file.)
    let acting = match str_at(&v, "agentId") {
        Some(aid) if !aid.is_empty() => AgentId::from_parts(source, aid),
        _ => root,
    };

    let out = match kind {
        "session.start" => {
            let session_id = data
                .and_then(|d| str_at(d, "sessionId"))
                .unwrap_or_else(|| {
                    crate::source::drift::missing_field(source, "session.start", "sessionId");
                    ""
                });
            let cwd = data
                .and_then(|d| d.get("context"))
                .and_then(|c| str_at(c, "cwd"))
                .unwrap_or("");
            vec![AgentEvent::SessionStart {
                agent_id: root,
                source: source.to_string(),
                session_id: session_id.to_string(),
                cwd: PathBuf::from(cwd),
                parent_id: None,
            }]
        }
        "tool.execution_start" => {
            let Some(d) = data else {
                crate::source::drift::missing_field(source, "tool.execution_start", "data");
                return Ok(vec![]);
            };
            let Some(tool_call_id) = str_at(d, "toolCallId") else {
                crate::source::drift::missing_field(source, "tool.execution_start", "toolCallId");
                return Ok(vec![]);
            };
            let tool_name = str_at(d, "toolName").unwrap_or_else(|| {
                crate::source::drift::missing_field(source, "tool.execution_start", "toolName");
                ""
            });
            let detail = copilot_tool_detail(tool_name, d.get("arguments"));
            vec![AgentEvent::ActivityStart {
                agent_id: acting,
                tool_use_id: Some(tool_call_id.to_string()),
                detail: Some(detail),
            }]
        }
        "tool.execution_complete" => {
            let Some(d) = data else {
                crate::source::drift::missing_field(source, "tool.execution_complete", "data");
                return Ok(vec![]);
            };
            let Some(tool_call_id) = str_at(d, "toolCallId") else {
                crate::source::drift::missing_field(
                    source,
                    "tool.execution_complete",
                    "toolCallId",
                );
                return Ok(vec![]);
            };
            let mut out = vec![AgentEvent::ActivityEnd {
                agent_id: acting,
                tool_use_id: Some(tool_call_id.to_string()),
            }];
            // Burn-tier observation: copilot stamps the model PER TOOL
            // (data.model — it can differ mid-session, e.g. a haiku-class
            // side tool under a sonnet session; capture-verified 1.0.62).
            // Attributed to the ACTING agent so a subagent's tool doesn't
            // repaint the root.
            if let Some(model) = str_at(d, "model").filter(|m| !m.is_empty()) {
                out.push(AgentEvent::ModelInfo {
                    agent_id: acting,
                    model: Some(ellipsize(model, MAX_DECODED_FIELD_CHARS)),
                    effort: None,
                });
            }
            out
        }
        "permission.requested" => {
            // permissionRequest.kind (write/shell/read/…) names the gate; fall
            // back to a generic reason. (Byte-real: the on-disk permission
            // envelope is capture-verified against copilot 1.0.62, #294.)
            let reason = data
                .and_then(|d| d.get("permissionRequest"))
                .and_then(|p| str_at(p, "kind"))
                // Cap at the decode boundary like every other content-derived
                // Waiting reason (opencode/reasonix) — `kind` is read raw off
                // events.jsonl and persists in the slot + headless summary.
                .map(|k| ellipsize(&format!("permission: {k}"), MAX_DECODED_FIELD_CHARS))
                .unwrap_or_else(|| "permission".to_string());
            vec![AgentEvent::Waiting {
                agent_id: acting,
                reason,
            }]
        }
        // Approval resolved. On APPROVED the gated tool's own `tool.execution_start`
        // follows immediately and clears the Waiting gate — so emit nothing (a
        // detail-less ActivityStart here would only inflate tool_call_count). On
        // a DENIAL/cancel no tool runs, so emit the clearing ActivityStart
        // ourselves to un-wait the slot.
        "permission.completed" => {
            let approved = data
                .and_then(|d| d.get("result"))
                .and_then(|r| str_at(r, "kind"))
                .is_some_and(|k| k.starts_with("approved"));
            if approved {
                vec![]
            } else {
                vec![AgentEvent::ActivityStart {
                    agent_id: acting,
                    tool_use_id: None,
                    detail: None,
                }]
            }
        }
        "subagent.started" => {
            // The child id is the envelope `agentId` (== data.toolCallId). Register
            // it as a child of the root session, then name it from the display name.
            let Some(child_key) = copilot_child_key(&v, data) else {
                return Ok(vec![]);
            };
            let child = AgentId::from_parts(source, child_key);
            let mut evs = vec![AgentEvent::SessionStart {
                agent_id: child,
                source: source.to_string(),
                session_id: child_key.to_string(),
                cwd: PathBuf::new(), // sub-agents carry no cwd; label comes from Rename
                parent_id: Some(root),
            }];
            if let Some(name) = data
                .and_then(|d| str_at(d, "agentDisplayName"))
                .filter(|s| !s.is_empty())
            {
                // Capped at decode (CONTRIBUTING pitfall 3 / R0612-06): agentDisplayName
                // is transcript content that persists in slot state for the session +
                // egresses on the headless summary — the sibling cap #364/R0620-364-04
                // applied to the permission path. `.filter(non-empty)` mirrors CC: an
                // empty name must not emit a blanking `Rename { label: "" }`.
                evs.push(AgentEvent::Rename {
                    agent_id: child,
                    label: ellipsize(name, MAX_DECODED_FIELD_CHARS),
                });
            }
            evs
        }
        "subagent.completed" | "subagent.failed" => {
            let Some(child_key) = copilot_child_key(&v, data) else {
                return Ok(vec![]);
            };
            vec![AgentEvent::SessionEnd {
                agent_id: AgentId::from_parts(source, child_key),
                as_child: true,
            }]
        }
        // A finished task/turn → settle the root agent toward idle.
        "session.task_complete" => vec![AgentEvent::ActivityEnd {
            agent_id: root,
            tool_use_id: None,
        }],
        // Token-meter usage (#645): copilot's ONLY usage wire is this
        // shutdown summary — one final delta as the session ends.
        // `tokenDetails.input` already EXCLUDES cache reads (the arithmetic
        // is pinned by `real_session_shutdown_usage_summary_lands_one_final_
        // delta`), so fresh = input + cache_write (inferred snake_case key
        // from the sibling `cache_read`; unconfirmed — no fixture carries a
        // nonzero bucket, and a differently-spelled key only UNDERcounts) +
        // output. No walk-out flash: the painter suppresses the tower/sheet
        // for any EXITING desk (the occupant filter's `exiting_at.is_none()`)
        // — NOT the event order; the counter lands on the slot either way,
        // since cascade_exit keeps the slot for the GC window. SessionEnd
        // FIRST is defense-in-depth on top: the one theoretically observable
        // intermediate state is then the already-exiting slot, which paints
        // nothing. Payoff: an honest dossier Σ on the walk-out hover.
        "session.shutdown" => {
            let mut evs = vec![AgentEvent::SessionEnd {
                agent_id: root,
                as_child: false,
            }];
            if let Some(details) = data
                .and_then(|d| d.get("tokenDetails"))
                .and_then(|t| t.as_object())
            {
                let bucket = |k: &str| {
                    details
                        .get(k)
                        .and_then(|b| b.get("tokenCount"))
                        .and_then(|n| n.as_u64())
                        .unwrap_or(0)
                };
                let fresh = bucket("input")
                    .saturating_add(bucket("cache_write"))
                    .saturating_add(bucket("output"));
                if fresh > 0 {
                    evs.push(AgentEvent::Usage {
                        agent_id: root,
                        fresh_tokens: fresh,
                    });
                }
            }
            evs
        }
        // Everything else (ephemeral streaming, assistant.*, hook.*, user.message,
        // session.* metadata) is not a sprite-visible lifecycle change → ignore.
        _ => vec![],
    };
    Ok(out)
}

/// Copilot's tool-detail dispatch. The sub-agent dispatch is the `task` tool
/// (`arguments.agent_type`), detected by NAME here — Copilot does NOT use CC's
/// `subagent_type` field. Routing non-`task` tools through the shared
/// [`crate::source::decoder::make_tool_detail`] (which flags a Task on the mere
/// PRESENCE of a `subagent_type` input key) would let a model-authored /
/// hallucinated `subagent_type` arg spoof an ordinary tool into a delegation —
/// prematurely draining `active_tasks` and evicting real children. Name-only
/// detection closes that (the vector Reasonix guards with `rx_tool_detail`),
/// and mapping Copilot's own arg vocabulary restores the `: target` display its
/// tools otherwise lose (`describe_tool_target` knows only CC's keys). The
/// child sprite still comes from the explicit `subagent.started` event.
fn copilot_tool_detail(tool: &str, args: Option<&Value>) -> ToolDetail {
    if tool == "task" {
        return ToolDetail::Task;
    }
    // Copilot's builtin tools key their target under these names (bash→command,
    // view/read/write→path, grep→pattern); the shared last-mile assembly caps +
    // formats the `: …` suffix so the policy can't drift per source.
    const KEYS: &[&str] = &["command", "path", "filePath", "pattern", "query"];
    let target = args.and_then(|a| crate::source::decoder::first_present_str(a, KEYS));
    generic_tool_display(tool, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- burn-tier observations (ModelInfo) ----

    #[test]
    fn execution_complete_surfaces_the_per_tool_model() {
        // Byte-real shape (from a real capture): data.model rides every
        // tool.execution_complete.
        let line = r#"{"type":"tool.execution_complete","data":{"toolCallId":"t1","model":"claude-haiku-4.5","success":true},"id":"e1","timestamp":"ts","parentId":null}"#;
        let evs = decode_copilot_line(
            "/p/11111111-2222-3333-4444-555555555555/events.jsonl",
            "copilot",
            serde_json::from_str(line).unwrap(),
        )
        .unwrap();
        assert!(
            evs.iter().any(|e| matches!(e, AgentEvent::ModelInfo { model: Some(m), effort: None, .. } if m == "claude-haiku-4.5")),
            "per-tool model must surface, got {evs:?}"
        );
        // Model-less completion: just the ActivityEnd, no phantom ModelInfo.
        let line = r#"{"type":"tool.execution_complete","data":{"toolCallId":"t2","success":true},"id":"e2","timestamp":"ts","parentId":null}"#;
        let evs = decode_copilot_line(
            "/p/11111111-2222-3333-4444-555555555555/events.jsonl",
            "copilot",
            serde_json::from_str(line).unwrap(),
        )
        .unwrap();
        assert!(
            !evs.iter()
                .any(|e| matches!(e, AgentEvent::ModelInfo { .. })),
            "got {evs:?}"
        );
    }
    use serde_json::json;

    // Real on-disk session-state path → id is the PARENT DIR uuid, not "events".
    const PATH: &str = "/p/session-state/65f8cef9-7dd8-46fa-9f6a-78cc95f68ab3/events.jsonl";

    fn root() -> AgentId {
        AgentId::from_parts(SOURCE_NAME, "65f8cef9-7dd8-46fa-9f6a-78cc95f68ab3")
    }
    fn decode(line: &str) -> Vec<AgentEvent> {
        decode_copilot_line(PATH, SOURCE_NAME, serde_json::from_str(line).unwrap()).unwrap()
    }

    #[test]
    fn id_from_path_uses_the_parent_dir_not_the_stem() {
        assert_eq!(
            copilot_id_from_path(Path::new(PATH)),
            "65f8cef9-7dd8-46fa-9f6a-78cc95f68ab3"
        );
    }

    #[test]
    fn label_is_cp_dot_basename_with_bare_prefix_fallback() {
        assert_eq!(
            derive_copilot_label(Path::new(PATH), SOURCE_NAME, Path::new("/home/u/proj")),
            "cp·proj"
        );
        assert_eq!(
            derive_copilot_label(Path::new(PATH), SOURCE_NAME, Path::new("")),
            "cp",
            "an empty cwd falls back to the bare prefix"
        );
    }

    /// An EMPTY envelope `agentId` is the root speaking, not a phantom
    /// ""-keyed sub-agent — the `!aid.is_empty()` guard on the attribution
    /// match. A guard→true flip would split every empty-tagged line off the
    /// root sprite.
    #[test]
    fn empty_agent_id_attributes_to_the_root() {
        let line = r#"{"type":"tool.execution_start","agentId":"","data":{"toolCallId":"call_1","toolName":"bash","arguments":{"command":"ls"}}}"#;
        match &decode(line)[..] {
            [AgentEvent::ActivityStart { agent_id, .. }] => {
                assert_eq!(*agent_id, root(), "empty agentId must key to the root");
            }
            other => panic!("expected one ActivityStart, got {other:?}"),
        }
    }

    /// A `tool.execution_start` whose `data` lacks the REQUIRED `toolCallId`
    /// (the ActivityStart/End pairing key) is dropped — but must leave a
    /// `missing_field` drift breadcrumb like its sibling gates (`session.start`
    /// `sessionId`, `toolName`), so an upstream field rename surfaces in
    /// `pixtuoid doctor` instead of a copilot sprite that connects but silently
    /// never animates tools.
    #[test]
    fn missing_tool_call_id_drops_with_a_drift_breadcrumb() {
        let out = crate::test_capture::capture_logs(|| {
            let line = r#"{"type":"tool.execution_start","data":{"toolName":"bash"}}"#;
            assert!(
                decode(line).is_empty(),
                "an unkeyable tool start yields no event"
            );
        });
        for needle in [
            crate::source::drift::TARGET,
            "missing_field",
            "tool.execution_start",
            "toolCallId",
            SOURCE_NAME,
        ] {
            assert!(
                out.contains(needle),
                "missing {needle:?} in captured log:\n{out}"
            );
        }
    }

    /// `subagent.completed` keys the child off the ENVELOPE `agentId` when
    /// present (the `data.toolCallId` fallback is for older lines without
    /// one) — a `delete !` on the non-empty filter would reject every real
    /// agentId and drop the end entirely when no fallback exists.
    #[test]
    fn subagent_completed_keys_off_the_envelope_agent_id() {
        let line = r#"{"type":"subagent.completed","agentId":"call_7","data":{}}"#;
        match &decode(line)[..] {
            [AgentEvent::SessionEnd { agent_id, as_child }] => {
                assert_eq!(*agent_id, AgentId::from_parts(SOURCE_NAME, "call_7"));
                assert!(as_child);
            }
            other => panic!("expected one child SessionEnd, got {other:?}"),
        }
    }

    // ── byte-real lines (verbatim from the committed shreya661 / tamirdresher files) ──

    #[test]
    fn real_session_start_registers_root_with_cwd_and_session_id() {
        let line = r#"{"type":"session.start","data":{"sessionId":"65f8cef9-7dd8-46fa-9f6a-78cc95f68ab3","version":1,"producer":"copilot-agent","copilotVersion":"unknown","startTime":"2026-05-22T05:59:45.408Z","selectedModel":"claude-haiku-4.5","context":{"cwd":"d:\\contentforge-fullstack (1)"},"alreadyInUse":false},"id":"0bc5f1ba-1abe-49c9-a303-d843bd0c3fa8","timestamp":"2026-05-22T05:59:45.488Z","parentId":null}"#;
        match &decode(line)[..] {
            [AgentEvent::SessionStart {
                agent_id,
                source,
                session_id,
                cwd,
                parent_id,
            }] => {
                assert_eq!(*agent_id, root());
                assert_eq!(source, "copilot");
                assert_eq!(session_id, "65f8cef9-7dd8-46fa-9f6a-78cc95f68ab3");
                assert_eq!(cwd, Path::new(r"d:\contentforge-fullstack (1)"));
                assert_eq!(*parent_id, None);
            }
            other => panic!("expected one SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn real_tool_round_is_active_then_idle_keyed_on_tool_call_id() {
        let start = r#"{"type":"tool.execution_start","data":{"toolCallId":"tooluse_9CoqZL2lZlJUsz7TjJsSUk","toolName":"report_intent","arguments":{"intent":"Exploring project setup"}},"id":"595a6493-1763-4c80-b75a-936d4f263a11","timestamp":"2026-05-22T06:00:14.298Z","parentId":"2902a578-0304-4abc-8402-afefefff9e70"}"#;
        match &decode(start)[..] {
            [AgentEvent::ActivityStart {
                agent_id,
                tool_use_id,
                detail: Some(_),
            }] => {
                assert_eq!(*agent_id, root());
                assert_eq!(
                    tool_use_id.as_deref(),
                    Some("tooluse_9CoqZL2lZlJUsz7TjJsSUk")
                );
            }
            other => panic!("expected ActivityStart, got {other:?}"),
        }
        let complete = r#"{"type":"tool.execution_complete","data":{"toolCallId":"tooluse_9CoqZL2lZlJUsz7TjJsSUk","model":"claude-haiku-4.5","interactionId":"65f25156-0095-4746-ac3e-fa52340df72b","success":true,"result":{"content":"Intent logged","detailedContent":"Exploring project setup"},"toolTelemetry":{}},"id":"cd7e82e8","timestamp":"2026-05-22T06:00:14.323Z","parentId":"d97de833"}"#;
        match &decode(complete)[..] {
            [AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id,
            }, ..] => {
                assert_eq!(*agent_id, root());
                assert_eq!(
                    tool_use_id.as_deref(),
                    Some("tooluse_9CoqZL2lZlJUsz7TjJsSUk")
                );
            }
            other => panic!("expected ActivityEnd first, got {other:?}"),
        }
    }

    #[test]
    fn real_task_tool_is_delegating() {
        // The `task` dispatch (real tamirdresher line, trimmed args) → Delegating.
        let line = r#"{"type":"tool.execution_start","data":{"toolCallId":"call_SGMJ1yjMtpgFUbZct2fEo2Hk","toolName":"task","arguments":{"description":"Incident command response","agent_type":"sisko","name":"sisko-incident-command","mode":"sync"},"turnId":"0"},"id":"a","timestamp":"t","parentId":null}"#;
        match &decode(line)[..] {
            [AgentEvent::ActivityStart {
                detail: Some(d), ..
            }] => assert!(d.is_task(), "task tool must be Delegating, got {d:?}"),
            other => panic!("expected Delegating ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn spoofed_subagent_type_arg_does_not_make_a_task() {
        // Copilot's `arguments` are fully model-authored; a hallucinated/injected
        // `subagent_type` key on an ORDINARY tool must NOT flip it to a Task
        // dispatch (only the `task` tool name delegates). Regression for the
        // shared-detector spoof vector (make_tool_detail keys on the mere
        // presence of `subagent_type`, incl. null) — Reasonix's twin guard.
        let line = r#"{"type":"tool.execution_start","data":{"toolCallId":"c1","toolName":"view","arguments":{"path":"x.rs","subagent_type":null}},"id":"a","timestamp":"t","parentId":null}"#;
        match &decode(line)[..] {
            [AgentEvent::ActivityStart {
                detail: Some(d), ..
            }] => assert!(
                !d.is_task(),
                "a spoofed subagent_type arg must stay Generic, got {d:?}"
            ),
            other => panic!("expected Generic ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn ordinary_tool_shows_its_own_arg_target() {
        // Copilot's own arg vocabulary (bash→command) renders as a `: target`
        // suffix — the display it lost while routing through the CC-keyed
        // describe_tool_target (which only knows CC's Write/Bash/Grep keys).
        let line = r#"{"type":"tool.execution_start","data":{"toolCallId":"c2","toolName":"bash","arguments":{"command":"cargo test"}},"id":"a","timestamp":"t","parentId":null}"#;
        match &decode(line)[..] {
            [AgentEvent::ActivityStart {
                detail: Some(ToolDetail::Generic { display }),
                ..
            }] => assert!(
                display.contains("cargo test"),
                "bash tool should show its command target, got {display:?}"
            ),
            other => panic!("expected Generic ActivityStart with target, got {other:?}"),
        }
    }

    #[test]
    fn real_subagent_started_registers_child_parented_to_root_then_renamed() {
        let line = r#"{"type":"subagent.started","data":{"toolCallId":"call_SGMJ1yjMtpgFUbZct2fEo2Hk","agentName":"sisko","agentDisplayName":"Sisko - Incident Commander / SRE Lead","agentDescription":"Sisko"},"id":"d171d290","timestamp":"2026-05-26T14:14:22.773Z","parentId":"83d641f1","agentId":"call_SGMJ1yjMtpgFUbZct2fEo2Hk"}"#;
        let child = AgentId::from_parts(SOURCE_NAME, "call_SGMJ1yjMtpgFUbZct2fEo2Hk");
        match &decode(line)[..] {
            [AgentEvent::SessionStart {
                agent_id,
                parent_id,
                ..
            }, AgentEvent::Rename { agent_id: r, label }] => {
                assert_eq!(*agent_id, child);
                assert_eq!(*parent_id, Some(root()));
                assert_eq!(*r, child);
                assert_eq!(label, "Sisko - Incident Commander / SRE Lead");
            }
            other => panic!("expected SessionStart+Rename, got {other:?}"),
        }
    }

    #[test]
    fn subagent_display_name_is_capped_and_empty_is_dropped() {
        // `agentDisplayName` is untrusted transcript content that flows into the
        // child's Rename label, persists in AgentSlot for the session, and egresses
        // on the headless summary line — so it MUST be `ellipsize`-capped
        // (CONTRIBUTING pitfall 3 / R0612-06) and an EMPTY value must not emit a
        // blanking `Rename { label: "" }`, the same guards CC applies
        // (claude_code.rs). The N-1-of-N sibling fix #364 (R0620-364-04) capped
        // copilot's permission reason but left this subagent arm uncapped.
        let over = "x".repeat(MAX_DECODED_FIELD_CHARS + 50);
        let line = serde_json::json!({
            "type": "subagent.started",
            "data": {"toolCallId": "call_X", "agentDisplayName": over},
            "parentId": "p",
            "agentId": "call_X"
        })
        .to_string();
        match &decode(&line)[..] {
            [AgentEvent::SessionStart { .. }, AgentEvent::Rename { label, .. }] => {
                assert!(
                    label.chars().count() <= MAX_DECODED_FIELD_CHARS + 1,
                    "label not capped: {} chars",
                    label.chars().count()
                );
                assert!(
                    label.ends_with('…'),
                    "expected an ellipsis on the capped label"
                );
            }
            other => panic!("expected SessionStart + capped Rename, got {other:?}"),
        }
        // An EMPTY agentDisplayName must NOT emit a Rename (it would blank the good
        // ordinal label with no recovery until the next Rename).
        let empty = serde_json::json!({
            "type": "subagent.started",
            "data": {"toolCallId": "call_Y", "agentDisplayName": ""},
            "parentId": "p",
            "agentId": "call_Y"
        })
        .to_string();
        match &decode(&empty)[..] {
            [AgentEvent::SessionStart { .. }] => {}
            other => panic!("expected SessionStart only (no blanking Rename), got {other:?}"),
        }
    }

    #[test]
    fn real_subagent_completed_ends_child_as_child() {
        let line = r#"{"type":"subagent.completed","data":{"toolCallId":"call_kuB1BVYZyE3ih6ClBvbyKtZk","agentName":"rom","agentDisplayName":"Rom - Database Reliability Engineer"},"id":"e7ab205e","timestamp":"2026-05-26T14:14:43.099Z","parentId":"f85ba2bd","agentId":"call_kuB1BVYZyE3ih6ClBvbyKtZk"}"#;
        match &decode(line)[..] {
            [AgentEvent::SessionEnd { agent_id, as_child }] => {
                assert_eq!(
                    *agent_id,
                    AgentId::from_parts(SOURCE_NAME, "call_kuB1BVYZyE3ih6ClBvbyKtZk")
                );
                assert!(*as_child);
            }
            other => panic!("expected child SessionEnd, got {other:?}"),
        }
    }

    #[test]
    fn real_subagent_failed_ends_child_as_child() {
        // BYTE-REAL (#294): a general-purpose subagent that errored out on copilot
        // 1.0.62 (`error:"No response generated"` — captured by aborting a running
        // task subagent). Same envelope + keying as subagent.completed (top-level
        // agentId == data.toolCallId), so the shared `completed | failed` arm ends
        // the CHILD as_child, ignoring the failure-only fields (error/durationMs).
        let line = r#"{"type":"subagent.failed","data":{"toolCallId":"toolu_bdrk_014wc1joyQCq3f6RBzGcxVRb","agentName":"general-purpose","agentDisplayName":"General Purpose Agent","model":"claude-haiku-4.5","totalToolCalls":0,"durationMs":2183,"error":"No response generated"},"id":"225a0bef-8b18-4d4d-a643-4cedd7f2e603","timestamp":"2026-06-14T21:30:31.494Z","parentId":"5a8b7e5e-9d2c-43b7-82b2-1d8f98f820de","agentId":"toolu_bdrk_014wc1joyQCq3f6RBzGcxVRb"}"#;
        match &decode(line)[..] {
            [AgentEvent::SessionEnd { agent_id, as_child }] => {
                assert_eq!(
                    *agent_id,
                    AgentId::from_parts(SOURCE_NAME, "toolu_bdrk_014wc1joyQCq3f6RBzGcxVRb")
                );
                assert!(*as_child, "a subagent failure is a child end");
            }
            other => panic!("expected child SessionEnd, got {other:?}"),
        }
    }

    #[test]
    fn child_tool_line_attributes_to_the_child_via_envelope_agent_id() {
        // Schema-derived (no public capture logs child tool events with agentId;
        // pins the defensive interleave-demux per the design §8.2).
        let line = json!({
            "type": "tool.execution_start",
            "data": {"toolCallId": "tooluse_child1", "toolName": "view", "arguments": {}},
            "id": "x", "timestamp": "t", "parentId": null,
            "agentId": "call_SGMJ1yjMtpgFUbZct2fEo2Hk"
        })
        .to_string();
        match &decode(&line)[..] {
            [AgentEvent::ActivityStart { agent_id, .. }] => assert_eq!(
                *agent_id,
                AgentId::from_parts(SOURCE_NAME, "call_SGMJ1yjMtpgFUbZct2fEo2Hk"),
                "a line with envelope agentId must attribute to the CHILD, not root"
            ),
            other => panic!("expected child ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn permission_requested_reason_is_capped_at_the_decode_boundary() {
        // `kind` is read raw off events.jsonl and stored in a persistent slot
        // field + the headless summary; like every other content-derived Waiting
        // reason it must be ellipsize-capped at the decode boundary (CONTRIBUTING
        // pitfall 3 / R0612-06), not left unbounded.
        use crate::source::decoder::MAX_DECODED_FIELD_CHARS;
        let kind = "x".repeat(MAX_DECODED_FIELD_CHARS * 4);
        let line = format!(
            r#"{{"type":"permission.requested","data":{{"permissionRequest":{{"kind":"{kind}"}}}},"id":"a","timestamp":"t","parentId":null}}"#
        );
        match &decode(&line)[..] {
            [AgentEvent::Waiting { reason, .. }] => {
                assert_eq!(reason.chars().count(), MAX_DECODED_FIELD_CHARS + 1);
            }
            other => panic!("expected Waiting, got {other:?}"),
        }
    }

    #[test]
    fn permission_requested_waits_and_completed_clears() {
        // BYTE-REAL (#294): captured from a live `copilot` 1.0.62 session — an
        // interactive shell-permission prompt approved once and denied once,
        // plus the headless `denied-no-approval-rule…` auto-deny. Pins BOTH
        // sides of the gate against the real on-disk envelope, not a mock.
        let req = r#"{"type":"permission.requested","data":{"requestId":"8c508e21-0a6c-4a06-8824-3930476499ea","permissionRequest":{"kind":"shell","toolCallId":"call_K8WLZkwufHsI9bTvkZmMKec2","fullCommandText":"cat /etc/hostname","intention":"Print /etc/hostname contents","commands":[{"identifier":"cat","readOnly":true}],"possiblePaths":["/etc/hostname"],"possibleUrls":[],"hasWriteFileRedirection":false,"canOfferSessionApproval":true},"promptRequest":{"kind":"path","accessKind":"shell","paths":["/etc/hostname"],"toolCallId":"call_K8WLZkwufHsI9bTvkZmMKec2"}},"id":"1f975691-a108-4d6f-924b-d48263d46274","timestamp":"2026-06-14T21:35:55.637Z","parentId":"e0a534c6-d548-4def-b0bd-316c83efe5fd"}"#;
        match &decode(req)[..] {
            [AgentEvent::Waiting { agent_id, reason }] => {
                assert_eq!(*agent_id, root());
                assert!(reason.contains("shell"), "reason names the gate: {reason}");
            }
            other => panic!("expected Waiting, got {other:?}"),
        }
        // APPROVED → emit nothing (the approved tool's own start clears Waiting;
        // a phantom ActivityStart would inflate tool_call_count). Real `approved`.
        let approved = r#"{"type":"permission.completed","data":{"requestId":"8c508e21-0a6c-4a06-8824-3930476499ea","toolCallId":"call_K8WLZkwufHsI9bTvkZmMKec2","result":{"kind":"approved"}},"id":"8123a44a-3471-4262-9191-b3cddaf5224d","timestamp":"2026-06-14T21:35:58.218Z","parentId":"1f975691-a108-4d6f-924b-d48263d46274"}"#;
        assert!(
            decode(approved).is_empty(),
            "approved → no event (tool start clears the gate)"
        );

        // Every NON-approved result.kind clears the slot via a detail-less
        // ActivityStart (no tool follows). Two REAL deny variants: the
        // interactive user reject, and the non-interactive no-rule auto-deny —
        // both must hit the same clear path (decoder keys on !starts_with
        // "approved", so a new deny kind can't silently strand a Waiting slot).
        for denied in [
            r#"{"type":"permission.completed","data":{"requestId":"954afe31-559a-4afc-9eb6-13e30cf48aea","toolCallId":"call_nf1RvU9GxssNg2g7WtPgHqQ4","result":{"kind":"denied-interactively-by-user"}},"id":"60dae716-c76c-45e2-84e1-c3248ce3790c","timestamp":"2026-06-14T21:38:43.086Z","parentId":"5240af45-3ad2-4bf7-bc37-83c329c9c2ea"}"#,
            r#"{"type":"permission.completed","data":{"requestId":"eab9bd2c-ca42-4ab6-8567-1c11906500a6","toolCallId":"toolu_bdrk_015JoceQkzNKnLkeCj5NaLzT","result":{"kind":"denied-no-approval-rule-and-could-not-request-from-user"}},"id":"2cc6bffe-6443-4d8b-9765-dbfeda13c4de","timestamp":"2026-06-14T21:27:17.209Z","parentId":"c113f81d-6f12-4080-bd13-8613526543dc"}"#,
        ] {
            assert!(
                matches!(&decode(denied)[..], [AgentEvent::ActivityStart { .. }]),
                "a non-approved result must clear Waiting: {denied}"
            );
        }
    }

    #[test]
    fn real_session_shutdown_ends_the_root() {
        // A tokenDetails-less shutdown (older wire / crashy teardown) still
        // ends the session — no Usage, no panic.
        let line = r#"{"type":"session.shutdown","data":{"shutdownType":"routine","totalPremiumRequests":1},"id":"220c4131","timestamp":"2026-05-22T06:17:01.077Z","parentId":"cd21bd01"}"#;
        match &decode(line)[..] {
            [AgentEvent::SessionEnd { agent_id, as_child }] => {
                assert_eq!(*agent_id, root());
                assert!(!*as_child, "a root shutdown is NOT a child end");
            }
            other => panic!("expected root SessionEnd, got {other:?}"),
        }
    }

    #[test]
    fn shutdown_usage_sums_a_cache_write_bucket_into_fresh() {
        // The `cache_write` bucket is an INFERRED snake_case key — no fixture
        // confirms copilot's real spelling (see the arm comment). This pins the
        // decoder CONTRACT: IF a tokenDetails.cache_write bucket arrives, it counts
        // toward fresh (input + cache_write + output). The real-shutdown sibling
        // below carries NO cache_write bucket, so the `.saturating_add(bucket(
        // "cache_write"))` branch is otherwise never exercised with a nonzero value.
        let line = r#"{"type":"session.shutdown","data":{"shutdownType":"routine","tokenDetails":{"input":{"tokenCount":11175},"cache_write":{"tokenCount":500},"cache_read":{"tokenCount":1664},"output":{"tokenCount":212}},"currentModel":"gpt-5-mini"},"id":"56992353","timestamp":"2026-06-14T21:38:47.162Z","parentId":"3079df1f"}"#;
        match &decode(line)[..] {
            [AgentEvent::SessionEnd { .. }, AgentEvent::Usage {
                agent_id,
                fresh_tokens,
            }] => {
                assert_eq!(*agent_id, root());
                assert_eq!(
                    *fresh_tokens, 11_887,
                    "fresh = input 11175 + cache_write 500 + output 212 (cache_read excluded)"
                );
            }
            other => panic!("expected [SessionEnd, Usage], got {other:?}"),
        }
    }

    #[test]
    fn real_session_shutdown_usage_summary_lands_one_final_delta() {
        // #645: the fixture's real shutdown shape. tokenDetails.input already
        // EXCLUDES cache reads (usage.inputTokens 12839 − cacheReadTokens 1664
        // = 11175 — THE authoritative arithmetic pin), so fresh = 11175 + 0
        // (no cache_write bucket) + 212, never 13_051 (summing cache_read in
        // would break this). SessionEnd first — defense-in-depth, see the arm.
        let line = r#"{"type":"session.shutdown","data":{"shutdownType":"routine","tokenDetails":{"input":{"tokenCount":11175},"cache_read":{"tokenCount":1664},"output":{"tokenCount":212}},"currentModel":"gpt-5-mini"},"id":"56992353","timestamp":"2026-06-14T21:38:47.162Z","parentId":"3079df1f"}"#;
        match &decode(line)[..] {
            [AgentEvent::SessionEnd { agent_id, as_child }, AgentEvent::Usage {
                agent_id: u_id,
                fresh_tokens,
            }] => {
                assert_eq!(*u_id, root());
                assert_eq!(
                    *fresh_tokens, 11_387,
                    "fresh = input 11175 + output 212, cache_read excluded"
                );
                assert_eq!(*agent_id, root());
                assert!(!*as_child);
            }
            other => panic!("expected [SessionEnd, Usage], got {other:?}"),
        }
    }

    #[test]
    fn ephemeral_unknown_and_malformed_lines_are_ignored_not_panicked() {
        // session.idle is ephemeral (never on disk, but be defensive); an unknown
        // type, a missing-data tool line, and a non-object are all no-ops.
        assert!(decode(
            r#"{"type":"session.idle","data":{},"id":"i","timestamp":"t","parentId":null}"#
        )
        .is_empty());
        assert!(decode(r#"{"type":"assistant.message_delta","data":{},"id":"d","timestamp":"t","parentId":null}"#).is_empty());
        assert!(decode(
            r#"{"type":"tool.execution_start","id":"n","timestamp":"t","parentId":null}"#
        )
        .is_empty());
        assert!(
            decode_copilot_line(PATH, SOURCE_NAME, json!("not an object"))
                .unwrap()
                .is_empty()
        );
        assert!(decode_copilot_line(PATH, SOURCE_NAME, json!(["array"]))
            .unwrap()
            .is_empty());
    }

    // ── drift fallbacks + missing-id early returns (the un-happy-path arms) ──

    #[test]
    fn session_start_without_session_id_registers_root_with_empty_id() {
        // `data` is present but carries NO `sessionId` → the missing-field drift
        // fallback yields an empty session_id (NOT the path-derived id), and the
        // agent is still the path-derived root. Falsifiable: a wrong fallback
        // (e.g. reusing the root uuid) would change session_id.
        let line = r#"{"type":"session.start","data":{"version":1},"id":"x","timestamp":"t","parentId":null}"#;
        match &decode(line)[..] {
            [AgentEvent::SessionStart {
                agent_id,
                source,
                session_id,
                cwd,
                parent_id,
            }] => {
                assert_eq!(*agent_id, root());
                assert_eq!(source, "copilot");
                assert_eq!(session_id, "", "missing sessionId → empty fallback");
                assert_eq!(cwd, Path::new(""), "no context.cwd → empty path");
                assert_eq!(*parent_id, None);
            }
            other => panic!("expected one SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn tool_execution_start_with_data_but_no_tool_call_id_is_ignored() {
        // `data` present (so the line passes the no-data early return at 135) but
        // WITHOUT `toolCallId` → the call-id early return at 137 drops the line.
        // Distinct from the already-covered missing-`data` case.
        let line = r#"{"type":"tool.execution_start","data":{"toolName":"view"},"id":"x","timestamp":"t","parentId":null}"#;
        assert!(
            decode(line).is_empty(),
            "no toolCallId → no ActivityStart (can't key the tool)"
        );
    }

    #[test]
    fn tool_execution_complete_with_data_but_no_tool_call_id_is_ignored() {
        let line = r#"{"type":"tool.execution_complete","data":{"success":true},"id":"x","timestamp":"t","parentId":null}"#;
        assert!(
            decode(line).is_empty(),
            "no toolCallId → no ActivityEnd (can't key the tool)"
        );
    }

    #[test]
    fn tool_execution_start_without_tool_name_still_emits_activity_start_keyed_on_call_id() {
        // `toolCallId` present but `toolName` ABSENT → the missing-field drift
        // fallback gives an empty name; "" is NOT "task", so make_tool_detail("")
        // runs and the ActivityStart is still emitted keyed on the call id (NOT a
        // Task detail). Falsifiable: an early-return on missing toolName would
        // yield an empty Vec; treating "" as task would flip is_task().
        let line = r#"{"type":"tool.execution_start","data":{"toolCallId":"tc1","arguments":{}},"id":"x","timestamp":"t","parentId":null}"#;
        match &decode(line)[..] {
            [AgentEvent::ActivityStart {
                agent_id,
                tool_use_id,
                detail: Some(d),
            }] => {
                assert_eq!(*agent_id, root());
                assert_eq!(tool_use_id.as_deref(), Some("tc1"));
                assert!(!d.is_task(), "an empty tool name is NOT the task dispatch");
            }
            other => panic!("expected one ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn session_task_complete_ends_root_activity_with_no_tool_id() {
        // A finished task/turn settles the root toward idle: an ActivityEnd for
        // the ROOT with NO tool_use_id (un-keyed, settles whatever was active).
        let line = r#"{"type":"session.task_complete","data":{},"id":"x","timestamp":"t","parentId":null}"#;
        match &decode(line)[..] {
            [AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id,
            }] => {
                assert_eq!(*agent_id, root());
                assert!(tool_use_id.is_none(), "the root settle carries no tool id");
            }
            other => panic!("expected one root ActivityEnd, got {other:?}"),
        }
    }

    #[test]
    fn subagent_started_without_any_child_key_is_ignored() {
        // Neither a non-empty envelope `agentId` NOR a `data.toolCallId` → the
        // child is un-keyable, so the line is dropped (NOT registered against
        // root). Falsifiable: a fallback to root would emit a SessionStart.
        let line = r#"{"type":"subagent.started","data":{"agentDisplayName":"X"},"id":"x","timestamp":"t","parentId":null}"#;
        assert!(
            decode(line).is_empty(),
            "un-keyable child → no SessionStart/Rename"
        );
        // An empty-string envelope agentId is filtered the same way (the
        // `.filter(|s| !s.is_empty())` guard), so it too drops with no toolCallId.
        let empty_aid = r#"{"type":"subagent.started","data":{"agentDisplayName":"X"},"id":"x","timestamp":"t","parentId":null,"agentId":""}"#;
        assert!(
            decode(empty_aid).is_empty(),
            "an empty agentId is not a usable key"
        );
    }

    #[test]
    fn subagent_completed_without_any_child_key_is_ignored() {
        // Shared `completed | failed` arm: no envelope `agentId`, `data` lacks
        // `toolCallId` → un-keyable → no child SessionEnd.
        let completed = r#"{"type":"subagent.completed","data":{"agentName":"rom"},"id":"x","timestamp":"t","parentId":null}"#;
        assert!(
            decode(completed).is_empty(),
            "un-keyable completed child → no SessionEnd"
        );
        let failed = r#"{"type":"subagent.failed","data":{"agentName":"rom"},"id":"x","timestamp":"t","parentId":null}"#;
        assert!(
            decode(failed).is_empty(),
            "the failed arm shares the same keying gate"
        );
    }

    #[test]
    fn copilot_home_honors_non_empty_env_override() {
        // COPILOT_HOME set non-empty → the `Some(v)` arm uses it verbatim; unset
        // → the `~/.copilot` fallback. Env-mutating → take the process-global
        // guard (the env-mutating-test convention) and restore the prior value.
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var_os("COPILOT_HOME");

        std::env::set_var("COPILOT_HOME", "/custom/cp");
        assert_eq!(
            copilot_home(),
            PathBuf::from("/custom/cp"),
            "a non-empty COPILOT_HOME is used verbatim"
        );

        // Set-but-empty OR whitespace-only is treated as unset (the shared
        // `platform::nonempty` trim authority), so it falls through to the
        // `<home>/.copilot` default — never a relative "  /.copilot".
        std::env::set_var("COPILOT_HOME", "");
        assert!(
            copilot_home().ends_with(".copilot"),
            "empty COPILOT_HOME → ~/.copilot fallback"
        );

        std::env::set_var("COPILOT_HOME", "   ");
        assert!(
            copilot_home().ends_with(".copilot"),
            "whitespace-only COPILOT_HOME → ~/.copilot fallback"
        );

        std::env::remove_var("COPILOT_HOME");
        assert!(
            copilot_home().ends_with(".copilot"),
            "unset COPILOT_HOME → ~/.copilot fallback"
        );

        match saved {
            Some(v) => std::env::set_var("COPILOT_HOME", v),
            None => std::env::remove_var("COPILOT_HOME"),
        }
    }

    // The session-ended checker tests live with the runtime half in
    // `native.rs`.
}
