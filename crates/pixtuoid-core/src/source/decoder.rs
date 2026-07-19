//! Shared decoder utilities used by per-source decoders (CC, Codex,
//! Antigravity, Reasonix). Hook payload decoding lives here because the hook
//! socket is shared; Reasonix's non-CC-shaped envelope is dispatched out to
//! its own module before the CC/Codex field requirements apply.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use serde_json::Value;

use crate::id::normalize_path_key;
use crate::source::{AgentEvent, ToolDetail};
use crate::AgentId;

/// The JSONL line-decoder fn pointer: `(transcript_path, source, raw_line) ->
/// events`. Defined HERE (the always-compiled decoder module), NOT in the
/// `native`-gated `jsonl` module, so the registry's `SourceDescriptor` can name
/// it in a `--no-default-features` (wasm) build; `jsonl` re-exports it.
pub type LineDecoder = fn(&str, &str, Value) -> Result<Vec<AgentEvent>>;

/// The first-sight cwd-extractor fn pointer: ONE parsed transcript line → the
/// working dir it carries, if any. Uniform signature like [`LineDecoder`], and
/// defined here for the same wasm-build reason. Each transcript-bearing
/// source's registry row names its own extractor (per-source transcript-format
/// knowledge lives in the source's module — invariant #3); the JSONL walker's
/// head scan dispatches by the source being scanned, so one source's shape is
/// never tried against another source's transcript (a foreign-shaped line —
/// e.g. a codex-style `payload.cwd` inside a CC transcript — must not label
/// the session with a foreign, identity-bearing cwd).
pub type CwdExtractor = fn(&Value) -> Option<PathBuf>;

/// The shared/default [`CwdExtractor`]: a TOP-LEVEL `cwd` string. CC writes it
/// on every transcript line (and Antigravity's row points here too — its steps
/// carry no cwd field, so the shape simply never matches and the label falls
/// back); also the fallback for sources with no registry row (test harnesses).
pub(crate) fn extract_top_level_cwd(v: &Value) -> Option<PathBuf> {
    v.get("cwd").and_then(Value::as_str).map(PathBuf::from)
}

/// The directory a CC subagent transcript sits under: `<parent>/subagents/
/// agent-*.jsonl`. Matched as a whole path COMPONENT (never a substring) so a
/// project dir merely *containing* the word (e.g. `subagents-paper`) is not
/// mistaken for one, and so Windows backslash-separated paths match too. Single
/// source of truth for both `is_subagent_path` and the watcher's
/// `detect_parent_id` so they cannot diverge.
pub(crate) const SUBAGENTS_DIR: &str = "subagents";

/// Whether a transcript path is a CC subagent transcript (vs a top-level
/// session). Codex subagents are FLAT (no such segment) — they're linked via the
/// `SubagentStart` hook instead, so this predicate is CC-layout-specific. Lives
/// here (not the `native`-gated `jsonl` module) because CC's pure line decoder
/// consults it.
pub(crate) fn is_subagent_path(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == SUBAGENTS_DIR)
}

/// `"{prefix}·{basename}"` from a working directory, or `None` when `cwd` is
/// empty / the filesystem root / has no final component. The cwd-basename label
/// rule, shared by the per-source derivers (cc / cx / ag) so it lives once; each
/// source supplies its 2-char prefix and its own fallback for the `None` case
/// (CC falls back to its project dir, codex/antigravity to a bare prefix).
pub(crate) fn cwd_basename_label(prefix: &str, cwd: &Path) -> Option<String> {
    // No explicit empty/root pre-check: `file_name()` is None for "", "/",
    // and any path without a final normal component, so the `?` below IS the
    // gate (a separate `cwd == ""` guard was redundant dead code — pinned by
    // the empty/root cases in this file's tests).
    let base = cwd.file_name().and_then(|n| n.to_str())?;
    // The cwd is transcript/hook CONTENT (extract_cwd / read_head_cwd /
    // payload cwd), and a slashless crafted value makes the whole string the
    // basename — capped here so all three derivers (cc/cx/ag) are bounded at
    // one chokepoint (pitfall 3); the label persists in slot state.
    Some(format!(
        "{prefix}·{}",
        ellipsize(base, MAX_DECODED_FIELD_CHARS)
    ))
}

/// The registered 2-char display prefix for `source` (invariant #3: the prefix
/// is a cross-source registry FACT, not transcript-format knowledge), or the
/// raw source name when it has no row — the same fallback the reducer's
/// `source_label_prefix` honors. The single source-layer authority so no
/// deriver hardcodes a prefix that could drift from the registry.
pub(crate) fn label_prefix_for(source: &str) -> &str {
    crate::source::registry::descriptor_for(source)
        .map(|d| d.label_prefix)
        .unwrap_or(source)
}

/// `"{prefix}·{basename}"` from a working directory, prefix looked up from the
/// registry by `source` — the shared body of the codex / copilot / antigravity
/// `LabelDeriver`s (which differ only in the source name they carry). Falls
/// back to the bare prefix when `cwd` has no basename. CC keeps its own deriver
/// for the subagent + project-dir fallbacks, but reads its prefix from the same
/// [`label_prefix_for`] authority.
pub(crate) fn derive_prefixed_label(source: &str, cwd: &Path) -> String {
    let prefix = label_prefix_for(source);
    cwd_basename_label(prefix, cwd).unwrap_or_else(|| prefix.to_string())
}

/// The first key in `keys` (priority order) whose value on `obj` is a string.
/// The "first present tool-arg / reason key from a per-source vocabulary" scan,
/// reimplemented at every per-source tool/permission decoder — centralized so
/// only the VOCABULARY (each caller's own `KEYS`) stays per-source, not the
/// scan itself. `None` for a non-object `obj` or when no key matches.
pub(crate) fn first_present_str<'a>(obj: &'a Value, keys: &[&str]) -> Option<&'a str> {
    let m = obj.as_object()?;
    keys.iter().find_map(|k| m.get(*k).and_then(|v| v.as_str()))
}

/// Parse every COMPLETE line of a tail-scan window as JSON, yielding the
/// parsed `Value`s and silently dropping empty, torn (the leading partial line
/// of a byte window), and non-JSON lines. The ONE tail-parse scaffold the
/// source-specific `*_session_ended` checkers share — each passes only its own
/// STRUCTURAL end-marker predicate (the per-source vocabulary), never a
/// substring scan (user-controllable content — a tool result QUOTING the marker
/// — must not drive lifecycle, the CC sharp edge). The `first_present_str`
/// centralization for the tail scan: the scaffold is shared, the vocabulary
/// stays per-source (invariant #3).
pub(crate) fn parsed_tail_lines(tail: &[u8]) -> impl Iterator<Item = Value> + '_ {
    tail.split(|b| *b == b'\n').filter_map(|line| {
        let s = std::str::from_utf8(line).ok()?;
        serde_json::from_str::<Value>(s).ok()
    })
}

/// Decode one hook payload into the event sequence the reducer applies.
///
/// Tool/permission arms (PreToolUse / PostToolUse / Notification /
/// PermissionRequest) return TWO events: an [`AgentEvent::Identity`] carrying
/// the payload's source/session_id/cwd, then the activity event (#221) — so
/// the reducer's proof-of-life registration for an unknown id lands with REAL
/// identity instead of a blank `#N` slot. Identity is deliberately NOT
/// attached to: `SessionStart`/`UserPromptSubmit` (the SessionStart event
/// already carries full identity), `Stop`/`SessionEnd` (an end for an unknown
/// agent proves nothing worth registering — the reducer's end-events-don't-
/// synthesize boundary stays meaningful), and the custom Subagent arms
/// (already enriched with parent links).
pub fn decode_hook_payload(v: Value) -> Result<Vec<AgentEvent>> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("hook payload must be an object"))?;
    // CLI attribution comes ONLY from the shim-owned `_pixtuoid_source` (the
    // shim stamps it from `PIXTUOID_SOURCE`). We must NOT read the public
    // `source` field: CC's SessionStart payload uses `source` for the start
    // *reason* (startup/resume/clear/compact), which would namespace the agent
    // under "startup" and split it from the claude-code-keyed tool/JSONL/
    // SessionEnd events (an un-reapable ghost). Absent the private key (bare
    // `pixtuoid-hook` with no env, i.e. CC), default to claude-code.
    let source = obj
        .get("_pixtuoid_source")
        .and_then(|s| s.as_str())
        .unwrap_or(crate::source::claude_code::SOURCE_NAME);
    let desc = crate::source::registry::descriptor_for(source);

    // A DAEMON source produces ZERO AgentEvents — its payloads ride the sibling
    // presence channel (the `HookRouter` demux routes them via the daemon's
    // `presence_decoder`). Short-circuit so a daemon envelope never reaches the
    // shared agent arms below (which would bail on the missing
    // `hook_event_name`). Registry-driven: a 2nd daemon needs no edit here.
    if desc.is_some_and(|d| d.is_daemon()) {
        return Ok(vec![]);
    }

    // CROSS-FIRE guard: grok scans `~/.claude/settings.json` AND
    // `~/.cursor/hooks.json` BY DEFAULT (Claude/Cursor compat, xai-grok-hooks
    // discovery) and executes the shim commands pixtuoid installed THERE with
    // its OWN envelope — which then arrives tagged `claude-code`/`cursor`
    // while a grok-tagged duplicate arrives via our native `~/.grok/hooks`
    // file. The `hookEventName` KEY (camelCase name, vs CC/cursor's
    // `hook_event_name`) is grok's envelope fingerprint; a mis-tagged copy is
    // a known duplicate, not drift — drop it QUIETLY (trace, not warn: it
    // recurs on every tool call of every grok session once CC hooks are
    // installed) BEFORE the per-source custom decoders, which would otherwise
    // Err per event (CC falls through to the shared arms' missing-
    // `hook_event_name` bail; cursor's claims-all errors on the same absent
    // field).
    if source != crate::source::grok::SOURCE_NAME && obj.contains_key("hookEventName") {
        tracing::trace!(source, "dropping grok cross-fired hook envelope");
        return Ok(vec![]);
    }

    // A source's own hook arms run FIRST — before the shared field
    // requirements below — so an alien envelope (Reasonix: camelCase, `event`
    // discriminator, no `session_id` at all) or a subject-changing event
    // (CC's and Codex's SubagentStart/Stop, whose AgentId is the CHILD's)
    // decodes in the source's module, not here. An `Extend` decoder that
    // declines (`Ok(None)`) falls through to the shared CC-shaped arms; a
    // `ClaimsAll` decoder handles EVERY event and CANNOT fall through (no
    // `Option` to return) — the contract is the type, see `HookCustom`.
    use crate::source::registry::HookCustom;
    match desc.and_then(|d| d.hook()).and_then(|h| h.custom) {
        Some(HookCustom::ClaimsAll(decode)) => return decode(&v),
        Some(HookCustom::Extend(decode)) => {
            if let Some(evs) = decode(&v)? {
                return Ok(evs);
            }
        }
        None => {}
    }

    let event = obj
        .get("hook_event_name")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("missing hook_event_name"))?;

    // `.filter(non-empty)`: an empty session_id passes `as_str` but, for Codex
    // (which keys the AgentId on session_id), would mint a phantom agent that
    // never coalesces with any rollout — reject it as malformed (same idiom as
    // the SubagentStart agent_id guard).
    let session_id = obj
        .get("session_id")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("missing/empty session_id"))?
        .to_string();
    // The per-session key strategy is registry data (`HookDecoding::id_key`),
    // not a name match: CC and Codex key on `session_id` (the session UUID);
    // Antigravity — and the unknown-source default — keys on `transcript_path`,
    // falling back to `session_id`. Codex MUST use `session_id` since its
    // `transcript_path` is `string | null` (keying on the path would split hook
    // and JSONL into two sprites); CC keys on it because that UUID equals its
    // transcript filename stem (`cc_id_from_path`), so a subagent->parent link
    // survives a git-worktree cwd-split.
    use crate::source::registry::IdKey;
    // Normalized transcript_path: fold `\`→`/` + lowercase on Windows so the
    // hook key and the JSONL watcher key (which walks real Path strings) hash to
    // the same AgentId. The session_id fallback is a UUID — NOT normalized
    // (UUIDs are already canonical and case-normalized UUIDs could collide on
    // case-only variants, which no real UUID generator produces anyway). The
    // `.filter(!is_empty)` guard is preserved: an empty transcript_path must
    // still fall back to session_id.
    let normalized_transcript_path: String;
    let id_key = match desc
        .and_then(|d| d.hook())
        .map_or(IdKey::TranscriptPathThenSessionId, |h| h.id_key)
    {
        IdKey::SessionId => session_id.as_str(),
        IdKey::TranscriptPathThenSessionId => {
            match obj
                .get("transcript_path")
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty())
            {
                Some(tp) => {
                    normalized_transcript_path = normalize_path_key(tp);
                    &normalized_transcript_path
                }
                None => session_id.as_str(),
            }
        }
    };
    let agent_id = AgentId::from_parts(source, id_key);

    // The identity context the tool/permission arms attach ahead of their
    // activity event (#221). `cwd` is on the wire for CC tool hooks (verified
    // on PreToolUse fixtures) but absent on e.g. Codex PermissionRequest/CC
    // PostToolUse — absent or empty maps to `None` so the reducer's cwd-less
    // registration path (ordinal label, reap-exempt) applies.
    let identity = || AgentEvent::Identity {
        agent_id,
        source: source.to_string(),
        session_id: session_id.clone(),
        cwd: obj
            .get("cwd")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from),
        pid: None,
    };

    // Burn-tier effort observation (CC): tool-context hook payloads carry an
    // `effort: {level}` object (documented in hooks.md — low/medium/high/
    // xhigh/max; ULTRACODE mode "is not a distinct level and reports as
    // xhigh", also exported as $CLAUDE_EFFORT).
    // Codex hook payloads carry no such field — absent = emit nothing. This
    // is the primary CC effort channel (per tool event, verbatim vocabulary);
    // the transcript's periodic ultra attachment markers are the JSONL twin.
    let effort_info = || {
        obj.get("effort")
            .and_then(|e| e.get("level"))
            .and_then(|l| l.as_str())
            .filter(|l| !l.is_empty())
            .map(|level| AgentEvent::ModelInfo {
                agent_id,
                model: None,
                effort: Some(ellipsize(level, MAX_DECODED_FIELD_CHARS)),
            })
    };

    match event {
        "SessionStart" => {
            let cwd = obj.get("cwd").and_then(|s| s.as_str()).unwrap_or("").into();
            let source = source.to_string();
            let mut evs = vec![AgentEvent::SessionStart {
                agent_id,
                source: source.clone(),
                session_id,
                cwd,
                parent_id: None,
            }];
            // "Only SessionStart hooks can receive a `model` field, and it is
            // not guaranteed to be present" (hooks.md) — take it when offered.
            if let Some(model) = obj
                .get("model")
                .and_then(|m| m.as_str())
                .filter(|m| !m.is_empty())
            {
                evs.push(AgentEvent::ModelInfo {
                    agent_id,
                    model: Some(ellipsize(model, MAX_DECODED_FIELD_CHARS)),
                    effort: None,
                });
            }
            Ok(evs)
        }
        "PreToolUse" => {
            let tool_name = obj
                .get("tool_name")
                .and_then(|s| s.as_str())
                .unwrap_or_else(|| {
                    super::drift::missing_field(source, "PreToolUse", "tool_name");
                    "?"
                });
            let tool_use_id = obj
                .get("tool_use_id")
                .and_then(|s| s.as_str())
                .map(String::from);
            let mut evs = vec![
                identity(),
                AgentEvent::ActivityStart {
                    agent_id,
                    tool_use_id,
                    detail: Some(make_tool_detail(source, tool_name, obj.get("tool_input"))),
                },
            ];
            evs.extend(effort_info());
            Ok(evs)
        }
        "PostToolUse" => {
            let tool_use_id = obj
                .get("tool_use_id")
                .and_then(|s| s.as_str())
                .map(String::from);
            let mut evs = vec![
                identity(),
                AgentEvent::ActivityEnd {
                    agent_id,
                    tool_use_id,
                },
            ];
            evs.extend(effort_info());
            Ok(evs)
        }
        "Notification" => {
            let msg = obj
                .get("message")
                .and_then(|s| s.as_str())
                .unwrap_or("waiting");
            Ok(vec![
                identity(),
                AgentEvent::Waiting {
                    agent_id,
                    reason: ellipsize(msg, MAX_DECODED_FIELD_CHARS),
                },
            ])
        }
        // Codex's permission prompt is a "waiting on the human" signal — maps to
        // the same Waiting state as Claude's Notification.
        "PermissionRequest" => Ok(vec![
            identity(),
            AgentEvent::Waiting {
                agent_id,
                reason: "permission".into(),
            },
        ]),
        // Codex agent-creation signal. Codex DOES fire SessionStart (carries
        // session_id + cwd) and Pre/PostToolUse — but the tool hooks fire only
        // for shell/apply_patch/MCP; ~25 other handlers (web_search, read_file,
        // grep, …) fire nothing (openai/codex#20204), and hook firing is
        // version-unstable: a `matcher="*"` group is silently dropped (hence the
        // matcher-less install) and some builds emit no hooks at all
        // (openai/codex#21639). So we DON'T trust the SessionStart hook alone —
        // UserPromptSubmit ALSO emits SessionStart (idempotent in the reducer,
        // ignored if the agent already exists), and the JSONL rollout stays the
        // system of record for tool activity regardless. The fresh `last_event_at`
        // makes the cx· agent show seated-thinking, so it reads as "working" right
        // after a prompt.
        "UserPromptSubmit" => {
            let cwd = obj.get("cwd").and_then(|s| s.as_str()).unwrap_or("").into();
            Ok(vec![AgentEvent::SessionStart {
                agent_id,
                source: source.to_string(),
                session_id,
                cwd,
                parent_id: None,
            }])
        }
        // Turn end — Codex fires no SessionEnd, so keep the slot; just settle to
        // idle (harmless no-op if the agent is already idle). NO Identity: a
        // turn end for an unknown agent proves nothing worth registering, so it
        // must keep riding the reducer's blank-synthesis fallback.
        "Stop" => Ok(vec![AgentEvent::ActivityEnd {
            agent_id,
            tool_use_id: None,
        }]),
        "SessionEnd" => Ok(vec![AgentEvent::SessionEnd {
            agent_id,
            as_child: false,
        }]),
        // SubagentStart/SubagentStop live in the source modules'
        // `claude_code::decode_cc_hook_custom` / `codex::decode_codex_hook_custom`
        // (dispatched above via the registry) — they change the event's
        // SUBJECT to the child AgentId, which these shared session-keyed arms
        // cannot express. A source whose row has no custom decoder bails here.
        other => {
            // Drift breadcrumb: a hook event we don't handle (and no custom
            // decoder claimed) — upstream added or renamed one. Surfaced before
            // the bail so the self-diagnosis layer can see it.
            super::drift::unknown_event(source, other);
            bail!("unsupported hook_event_name: {other}")
        }
    }
}

pub(crate) fn make_tool_detail(source: &str, tool_name: &str, input: Option<&Value>) -> ToolDetail {
    // Detect the subagent-dispatch tool SEMANTICALLY, by the PRESENCE of a
    // `subagent_type` input field — this is THE mechanism. The dispatch tool
    // was renamed `Task` → `Agent` (CC v2.1.63, undocumented) and upstream can
    // rename it again, but the field is stable. Key on presence (not value): a
    // renamed tool emitting `subagent_type: null` is still caught AND surfaces
    // the drift breadcrumb — the one drift we most need to see. The known name
    // (`Agent`, current CC) is the fallback for the rare input-less call; the
    // legacy `Task` name arm was DROPPED in 0.12.0 (pre-v2.1.63 CC is too old
    // to keep supporting — its dispatches carry `subagent_type` and stay
    // caught semantically; only a name-only, input-less `Task` call loses
    // coverage, and one WITH `subagent_type` now correctly fires the
    // `unknown_dispatch` breadcrumb: an unrecognized dispatch name IS
    // drift-worthy). The reducer keys subagent-leak suppression
    // (`active_tasks`) and b1 Task-drain completion on `is_task()`, so a
    // missed dispatch silently disables both for real subagents.
    let has_subagent_type = input.and_then(|v| v.get("subagent_type")).is_some();
    // DELIBERATELY NOT a known name: `Workflow` (CC's fleet dispatcher). Its
    // children fire no per-agent `Agent` tool_use, so mapping Workflow → Task
    // would park ONE months-long entry in the parent's `active_tasks` for the
    // whole workflow — and the vouched-Delegating subtree shield
    // (`sweep_stale`'s ancestor-vouch ∧ active-delegation gate) would then
    // sweep-EXEMPT every FINISHED fleet subagent until the workflow ends:
    // worse desk starvation than the gap it would "fix". Fleet lifecycle is
    // owned by the SubagentStart/Stop hooks instead
    // (`claude_code::decode_cc_hook_custom`, #241).
    let known_name = tool_name == "Agent";
    if has_subagent_type || known_name {
        // Drift breadcrumb: a dispatch under a name we don't recognise means
        // upstream renamed the tool again. Semantic detection keeps us working;
        // this surfaces the new name so the known set / docs can be updated.
        if has_subagent_type && !known_name {
            super::drift::unknown_dispatch(source, tool_name);
        }
        ToolDetail::Task
    } else {
        // `target` (the file/cmd descriptor) is only meaningful on the Generic
        // branch, so derive it here lazily — no wasted alloc on the dispatch
        // path, and callers can't pass a `target` computed from a different
        // `input` than the one used for detection. CC's per-key dispatch lives
        // in `describe_tool_target`; the format-agnostic last-mile assembly is
        // shared in `generic_tool_display` so the per-source generic fallbacks
        // can't drift.
        generic_tool_display(tool_name, describe_tool_target(tool_name, input))
    }
}

/// The format-agnostic Generic-tool fallback display, shared by every source's
/// `*_tool_detail` so the cap policy can't drift between them. `tool` is wire
/// content (capped at [`MAX_DECODED_FIELD_CHARS`]); `target` is the per-source
/// file/cmd descriptor (capped at [`MAX_TOOL_TARGET_CHARS`] and rendered as a
/// `: …` suffix). The per-source DISPATCH (which tool maps to which specialized
/// `ToolDetail`, and which input keys carry the target) stays in each source's
/// own fn — only this last-mile string assembly is shared.
pub(crate) fn generic_tool_display(tool: &str, target: Option<&str>) -> ToolDetail {
    let suffix = target
        .map(|t| format!(": {}", ellipsize(t, MAX_TOOL_TARGET_CHARS)))
        .unwrap_or_default();
    ToolDetail::Generic {
        display: format!("{}{suffix}", ellipsize(tool, MAX_DECODED_FIELD_CHARS)),
    }
}

/// The non-CC "scan a key list, then assemble" last mile: find the first
/// present target key in `args`, then hand it to [`generic_tool_display`] for
/// the cap + `: …` formatting. Bundling the scan and the cap in ONE call means a
/// source's Generic fallback cannot scan a target and format it raw, bypassing
/// [`MAX_TOOL_TARGET_CHARS`] — the residual chokepoint-bypass bug class a
/// copy-pasted 9th source would otherwise reintroduce. The per-source `keys`
/// vocabulary is passed IN as data (invariant #3), so this stays
/// format-agnostic; CC keeps its own per-tool key dispatch
/// (`describe_tool_target`), which is a tool→key map, not a key list.
pub(crate) fn generic_keyed_detail(tool: &str, args: Option<&Value>, keys: &[&str]) -> ToolDetail {
    generic_tool_display(tool, args.and_then(|a| first_present_str(a, keys)))
}

/// CC's per-tool target key dispatch: the raw `file/cmd` descriptor for the
/// Generic display, or `None` for a tool with no keyed target. The cap +
/// `: …` formatting is applied by [`generic_tool_display`], so this returns
/// the raw borrowed string (per-source knowledge stays here, assembly is
/// shared).
pub(crate) fn describe_tool_target<'a>(tool: &str, input: Option<&'a Value>) -> Option<&'a str> {
    let key = match tool {
        "Write" | "Edit" | "MultiEdit" | "Read" => "file_path",
        "Bash" => "command",
        "Grep" | "Glob" => "pattern",
        _ => return None,
    };
    input?.get(key).and_then(|v| v.as_str())
}

/// Tighter cap for the tool-target descriptor (the `: file/cmd` suffix on a
/// Generic tool display) — a glanceable fragment, not a full field.
pub(crate) const MAX_TOOL_TARGET_CHARS: usize = 40;

/// Cap for content-derived strings that become slot state (Waiting reason,
/// Rename label) — generous against every legitimate value on those fields
/// (subagent names, "Claude needs your permission to use Bash"), tight
/// against a crafted ~1 MiB hook/transcript line: every TUI display site is
/// individually bounded (tooltip char cap + rect clip, ratatui cell
/// clipping), but the headless summary line is not, and
/// the uncapped value would sit in `AgentSlot` for the session's lifetime
/// either way.
pub(crate) const MAX_DECODED_FIELD_CHARS: usize = 80;

/// Char-safe truncation for untrusted display strings at the decode boundary
/// — where the content ENTERS (CONTRIBUTING pitfall 3), on char boundaries,
/// never bytes (pitfall 1). Shared by the tool-target cap above and the
/// Waiting-reason / Rename-label caps (CC + Reasonix) so the sites can't
/// drift apart. Budget: the `…` is EXCLUDED, so a clipped result is
/// `max_chars + 1` chars — unlike `widgets::truncate`, which counts it (N).
pub(crate) fn ellipsize(s: &str, max_chars: usize) -> String {
    let mut out: String = s.chars().take(max_chars).collect();
    if s.chars().count() > max_chars {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsed_tail_lines_yields_only_complete_parseable_lines() {
        // The shared session_ended tail-parse scaffold — pins the byte-level
        // parity the #9 refactor rests on (grok moved from from_utf8_lossy +
        // str::lines to split(b'\n') + from_utf8). A tail byte-window can begin
        // mid-line (torn leading partial), carry CRLF terminators, and hold
        // empty/torn segments; only the COMPLETE, JSON-parseable lines yield.
        let tail = b"3,\"torn\":tru\n{\"type\":\"a\"}\n{\"type\":\"b\"}\r\n\n{\"type\":\"c\"";
        let kinds: Vec<String> = parsed_tail_lines(tail)
            .filter_map(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_owned))
            .collect();
        // "a" (plain) + "b" (CRLF → trailing \r is JSON whitespace) parse; the
        // torn leading partial, the empty segment, and the torn trailing partial
        // are all dropped — exactly what a session-end marker sweep needs so
        // content never false-ends and a window edge never mis-parses.
        assert_eq!(kinds, vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(
            parsed_tail_lines(b"").count(),
            0,
            "empty tail yields nothing"
        );
        assert_eq!(
            parsed_tail_lines(b"not json at all\n").count(),
            0,
            "non-JSON lines are skipped, never panic",
        );
    }

    // ---- burn-tier observations riding the shared hook arms ----

    #[test]
    fn tool_hooks_surface_the_effort_level() {
        // hooks.md: tool-context payloads carry `effort:{level}`; ultracode
        // reports as "xhigh" (exported via $CLAUDE_EFFORT).
        for event in ["PreToolUse", "PostToolUse"] {
            let v = serde_json::json!({
                "hook_event_name": event,
                "session_id": "ses-e",
                "transcript_path": "/p/ses-e.jsonl",
                "cwd": "/repo",
                "tool_name": "Bash",
                "tool_use_id": "t1",
                "effort": {"level": "xhigh"}
            });
            let evs = decode_hook_payload(v).unwrap();
            assert!(
                evs.iter().any(|e| matches!(e, AgentEvent::ModelInfo { model: None, effort: Some(f), .. } if f == "xhigh")),
                "{event} must surface effort, got {evs:?}"
            );
        }
        // Effort-less payloads (codex hooks, older CC) emit no ModelInfo.
        let v = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "session_id": "ses-e",
            "transcript_path": "/p/ses-e.jsonl",
            "tool_name": "Bash"
        });
        let evs = decode_hook_payload(v).unwrap();
        assert!(
            !evs.iter()
                .any(|e| matches!(e, AgentEvent::ModelInfo { .. })),
            "got {evs:?}"
        );
    }

    #[test]
    fn session_start_hook_surfaces_the_model_when_offered() {
        // "Only SessionStart hooks can receive a model field, and it is not
        // guaranteed" (hooks.md) — present → observation, absent → nothing.
        let v = serde_json::json!({
            "hook_event_name": "SessionStart",
            "session_id": "ses-m",
            "transcript_path": "/p/ses-m.jsonl",
            "cwd": "/repo",
            "model": "claude-fable-5"
        });
        let evs = decode_hook_payload(v).unwrap();
        assert!(
            evs.iter().any(|e| matches!(e, AgentEvent::ModelInfo { model: Some(m), effort: None, .. } if m == "claude-fable-5")),
            "got {evs:?}"
        );
        let v = serde_json::json!({
            "hook_event_name": "SessionStart",
            "session_id": "ses-m",
            "transcript_path": "/p/ses-m.jsonl",
            "cwd": "/repo"
        });
        let evs = decode_hook_payload(v).unwrap();
        assert!(
            !evs.iter()
                .any(|e| matches!(e, AgentEvent::ModelInfo { .. })),
            "got {evs:?}"
        );
    }
    use serde_json::json;

    // Real CC sessions are full of task-management tools whose names START WITH
    // "Task" — TaskCreate/TaskUpdate/TaskList/TaskStop/TaskOutput (1757
    // occurrences across a local 822 MB / 2379-session corpus) — but NONE carry
    // a `subagent_type`, so they are ordinary tools, NOT the subagent dispatch.
    // make_tool_detail must key the dispatch on the EXACT name (`Agent`)
    // or the `subagent_type` field, never a `starts_with("Task")` prefix — a
    // prefix match would mis-class every TaskUpdate as a delegation and wrongly
    // trip `active_tasks` subagent-leak suppression. The existing negative test
    // uses `Read` (doesn't start with "Task"), so it cannot catch a prefix
    // regression — this one pins the exact collision boundary.
    #[test]
    fn task_prefixed_tools_without_subagent_type_are_not_the_dispatch() {
        for name in [
            "TaskCreate",
            "TaskUpdate",
            "TaskList",
            "TaskStop",
            "TaskOutput",
        ] {
            assert!(
                !make_tool_detail("test", name, Some(&json!({"id": "t-1"}))).is_task(),
                "{name} (no subagent_type) must be a Generic tool, not the subagent dispatch"
            );
        }
        // The current dispatch name + the semantic signal resolve to Task; the
        // legacy pre-v2.1.63 `Task` NAME arm was dropped in 0.12.0 — a bare,
        // input-less `Task` is now an ordinary Generic tool (an old CC's real
        // dispatch still carries `subagent_type`, so it rides the semantic
        // detection instead).
        assert!(!make_tool_detail("test", "Task", None).is_task());
        assert!(make_tool_detail("test", "Agent", None).is_task());
        assert!(
            make_tool_detail("test", "Task", Some(&json!({"subagent_type": "x"}))).is_task(),
            "a legacy-named dispatch is still caught by the subagent_type field"
        );
        assert!(
            make_tool_detail(
                "test",
                "WhateverUpstreamRenamesItTo",
                Some(&json!({"subagent_type": "x"}))
            )
            .is_task(),
            "a renamed dispatch is still caught by the subagent_type field"
        );
    }

    /// A payload expected to decode to EXACTLY one event (lifecycle arms —
    /// the Identity-attaching tool/permission arms assert their pair shape
    /// explicitly instead).
    fn decode_single(v: Value) -> AgentEvent {
        let mut evs = decode_hook_payload(v).expect("decodes");
        assert_eq!(evs.len(), 1, "expected exactly one event, got {evs:?}");
        evs.pop().expect("one event")
    }

    #[test]
    fn codex_session_start_without_transcript_path_uses_session_id() {
        // Codex sends transcript_path as string|null; decode must still work,
        // namespacing the AgentId under the explicit "codex" source.
        let ev = decode_single(json!({
            "hook_event_name": "SessionStart",
            "session_id": "codex-sess-1",
            "_pixtuoid_source": "codex",
            "cwd": "/Users/me/work/myrepo"
        }));
        match ev {
            AgentEvent::SessionStart {
                agent_id,
                source,
                cwd,
                ..
            } => {
                assert_eq!(source, "codex");
                assert_eq!(agent_id, AgentId::from_parts("codex", "codex-sess-1"));
                assert_eq!(cwd, std::path::PathBuf::from("/Users/me/work/myrepo"));
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    /// The `TranscriptPathThenSessionId` key strategy (Antigravity + the
    /// unknown-source default): a NON-EMPTY `transcript_path` is the key —
    /// the same normalized path the JSONL watcher keys on, so hook and
    /// transcript coalesce into one sprite — and only an empty/absent path
    /// falls back to `session_id`. A `delete !` on the non-empty filter
    /// inverts the fallback and splits every AG session in two.
    #[test]
    fn transcript_path_key_strategy_prefers_a_non_empty_path() {
        let ev = decode_single(json!({
            "hook_event_name": "SessionStart",
            "session_id": "ag-sess-1",
            "_pixtuoid_source": "antigravity",
            "transcript_path": "/tmp/ag/brain/x.json",
            "cwd": "/repo"
        }));
        match ev {
            AgentEvent::SessionStart { agent_id, .. } => assert_eq!(
                agent_id,
                AgentId::from_parts(
                    "antigravity",
                    &crate::id::normalize_path_key("/tmp/ag/brain/x.json")
                ),
                "a non-empty transcript_path is the key, not session_id"
            ),
            other => panic!("expected SessionStart, got {other:?}"),
        }
        let ev = decode_single(json!({
            "hook_event_name": "SessionStart",
            "session_id": "ag-sess-1",
            "_pixtuoid_source": "antigravity",
            "transcript_path": "",
            "cwd": "/repo"
        }));
        match ev {
            AgentEvent::SessionStart { agent_id, .. } => assert_eq!(
                agent_id,
                AgentId::from_parts("antigravity", "ag-sess-1"),
                "an EMPTY transcript_path falls back to session_id"
            ),
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn codex_permission_request_maps_to_identity_plus_waiting() {
        // A cwd-less PermissionRequest (the captured Codex shape) still gets
        // an Identity — source/session_id alone fix the blank-slot bug class;
        // cwd: None routes the reducer to the ordinal-but-reap-exempt path.
        let evs = decode_hook_payload(json!({
            "hook_event_name": "PermissionRequest",
            "session_id": "s",
            "_pixtuoid_source": "codex"
        }))
        .expect("decodes");
        assert_eq!(evs.len(), 2, "Identity + Waiting, got {evs:?}");
        match &evs[0] {
            AgentEvent::Identity {
                source,
                session_id,
                cwd,
                ..
            } => {
                assert_eq!(source, "codex");
                assert_eq!(session_id, "s");
                assert_eq!(*cwd, None, "no cwd on the wire → None");
            }
            other => panic!("expected leading Identity, got {other:?}"),
        }
        assert!(matches!(evs[1], AgentEvent::Waiting { .. }));
    }

    #[test]
    fn codex_user_prompt_submit_creates_agent_via_session_start() {
        // UserPromptSubmit is a Codex agent-creation signal: it carries source +
        // cwd and decodes to a SessionStart the reducer turns into a cx· agent. We
        // emit it here IN ADDITION to Codex's own SessionStart hook because Codex
        // hook firing is version-unstable (see the UserPromptSubmit arm), so the
        // agent registers whether or not SessionStart fired. No Identity attached —
        // the SessionStart already carries full identity.
        let ev = decode_single(json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "codex-sess",
            "_pixtuoid_source": "codex",
            "cwd": "/Users/me/work/myrepo",
            "transcript_path": "/Users/me/.codex/sessions/x.jsonl"
        }));
        match ev {
            AgentEvent::SessionStart {
                agent_id,
                source,
                cwd,
                ..
            } => {
                assert_eq!(source, "codex");
                assert_eq!(cwd, std::path::PathBuf::from("/Users/me/work/myrepo"));
                // Coalescing contract: Codex keys on session_id, NOT the
                // (here non-null) transcript_path — so hook events and the
                // JSONL source (which keys on the rollout-filename UUID ==
                // session_id) hash to the SAME AgentId. Keying on the path
                // would produce two sprites for one session.
                assert_eq!(agent_id, AgentId::from_parts("codex", "codex-sess"));
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn codex_stop_maps_to_activity_end_with_no_identity() {
        // An end for an unknown agent proves nothing worth registering — the
        // Stop arm must NOT attach an Identity (the reducer's end-events-
        // don't-synthesize boundary keeps its bite).
        let ev = decode_single(json!({
            "hook_event_name": "Stop",
            "session_id": "s",
            "_pixtuoid_source": "codex"
        }));
        assert!(matches!(ev, AgentEvent::ActivityEnd { .. }));
    }

    // #221: the tool/permission arms attach the payload's identity context
    // (source / session_id / cwd) ahead of the activity event, so the
    // reducer's proof-of-life registration lands with REAL identity instead
    // of a blank `#N` slot.
    #[test]
    fn pre_tool_use_decodes_to_identity_plus_activity_start() {
        let evs = decode_hook_payload(json!({
            "hook_event_name": "PreToolUse",
            "session_id": "ses-abc",
            "transcript_path": "/p/ses-abc.jsonl",
            "cwd": "/Users/me/repo",
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "tool_use_id": "t1"
        }))
        .expect("decodes");
        assert_eq!(evs.len(), 2, "Identity + ActivityStart, got {evs:?}");
        match &evs[0] {
            AgentEvent::Identity {
                agent_id,
                source,
                session_id,
                cwd,
                pid: None,
            } => {
                assert_eq!(
                    *agent_id,
                    AgentId::from_parts(crate::source::claude_code::SOURCE_NAME, "ses-abc"),
                    "Identity must coalesce with the activity event's id"
                );
                assert_eq!(source, crate::source::claude_code::SOURCE_NAME);
                assert_eq!(session_id, "ses-abc");
                assert_eq!(cwd.as_deref(), Some(std::path::Path::new("/Users/me/repo")));
            }
            other => panic!("expected leading Identity, got {other:?}"),
        }
        match &evs[1] {
            AgentEvent::ActivityStart { tool_use_id, .. } => {
                assert_eq!(tool_use_id.as_deref(), Some("t1"));
            }
            other => panic!("expected ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn post_tool_use_without_cwd_decodes_to_identity_with_none_cwd() {
        // Real CC PostToolUse payloads can omit cwd — Identity still fixes
        // source/session_id; cwd: None (never Some("")).
        let evs = decode_hook_payload(json!({
            "hook_event_name": "PostToolUse",
            "session_id": "ses-abc",
            "transcript_path": "/p/ses-abc.jsonl",
            "tool_name": "Bash",
            "tool_use_id": "t1"
        }))
        .expect("decodes");
        assert_eq!(evs.len(), 2, "Identity + ActivityEnd, got {evs:?}");
        match &evs[0] {
            AgentEvent::Identity {
                source,
                session_id,
                cwd,
                ..
            } => {
                assert_eq!(source, crate::source::claude_code::SOURCE_NAME);
                assert_eq!(session_id, "ses-abc");
                assert_eq!(*cwd, None, "absent cwd must map to None");
            }
            other => panic!("expected leading Identity, got {other:?}"),
        }
        assert!(matches!(evs[1], AgentEvent::ActivityEnd { .. }));
    }

    #[test]
    fn empty_cwd_on_tool_hook_decodes_to_identity_with_none_cwd() {
        // Present-but-empty cwd is as good as absent: Some("") would route
        // the reducer's registration around the unknown-cwd reap exemption.
        let evs = decode_hook_payload(json!({
            "hook_event_name": "Notification",
            "session_id": "ses-abc",
            "transcript_path": "/p/ses-abc.jsonl",
            "cwd": "",
            "message": "permission?"
        }))
        .expect("decodes");
        match &evs[0] {
            AgentEvent::Identity { cwd, .. } => {
                assert_eq!(*cwd, None, "empty cwd must map to None, not Some(\"\")");
            }
            other => panic!("expected leading Identity, got {other:?}"),
        }
    }

    #[test]
    fn notification_decodes_to_identity_plus_waiting() {
        let evs = decode_hook_payload(json!({
            "hook_event_name": "Notification",
            "session_id": "ses-abc",
            "transcript_path": "/p/ses-abc.jsonl",
            "cwd": "/Users/me/repo",
            "message": "permission?"
        }))
        .expect("decodes");
        assert_eq!(evs.len(), 2, "Identity + Waiting, got {evs:?}");
        match &evs[0] {
            AgentEvent::Identity { cwd, .. } => {
                assert_eq!(cwd.as_deref(), Some(std::path::Path::new("/Users/me/repo")));
            }
            other => panic!("expected leading Identity, got {other:?}"),
        }
        assert!(matches!(&evs[1], AgentEvent::Waiting { reason, .. } if reason == "permission?"));
    }

    #[test]
    fn session_start_and_session_end_carry_no_identity() {
        // SessionStart already carries full identity; an end for an unknown
        // agent proves nothing worth registering (boundary 2).
        for (payload, name) in [
            (
                json!({
                    "hook_event_name": "SessionStart",
                    "session_id": "s",
                    "transcript_path": "/p/s.jsonl",
                    "cwd": "/repo"
                }),
                "SessionStart",
            ),
            (
                json!({
                    "hook_event_name": "SessionEnd",
                    "session_id": "s",
                    "transcript_path": "/p/s.jsonl",
                    "cwd": "/repo"
                }),
                "SessionEnd",
            ),
        ] {
            let evs = decode_hook_payload(payload).expect("decodes");
            assert_eq!(evs.len(), 1, "{name}: exactly one event, got {evs:?}");
            assert!(
                !matches!(evs[0], AgentEvent::Identity { .. }),
                "{name} must not emit Identity"
            );
        }
    }

    // Regression: CC's SessionStart hook payload carries `source: "startup"`
    // (the start *reason* — startup/resume/clear/compact), which is NOT a CLI
    // name. Reading it as the CLI source namespaced the agent under "startup",
    // splitting it from the claude-code-keyed tool/JSONL/SessionEnd events — an
    // un-reapable `startup·…` ghost. The public `source` field must never drive
    // CLI attribution; only the shim-owned `_pixtuoid_source` does.
    #[test]
    fn cc_session_start_reason_source_does_not_hijack_cli_source() {
        let ev = decode_single(json!({
            "hook_event_name": "SessionStart",
            "session_id": "ses-abc",
            "transcript_path": "/Users/me/.claude/projects/x/ses-abc.jsonl",
            "cwd": "/repo",
            "source": "startup"
        }));
        match ev {
            AgentEvent::SessionStart {
                agent_id, source, ..
            } => {
                assert_eq!(source, crate::source::claude_code::SOURCE_NAME);
                assert_eq!(
                    agent_id,
                    // CC keys on the session UUID (IdKey::SessionId), which ==
                    // the transcript filename stem the watcher/per-line decode
                    // derive — so this coalesces with tool/JSONL/SessionEnd
                    // events on the claude-code id. The public `source`
                    // ("startup") must NOT drive CLI attribution.
                    AgentId::from_parts(crate::source::claude_code::SOURCE_NAME, "ses-abc"),
                    "must coalesce with tool/JSONL/SessionEnd events on the claude-code id"
                );
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn pixtuoid_source_private_key_drives_cli_attribution() {
        // The shim stamps the trusted CLI source under `_pixtuoid_source`.
        let ev = decode_single(json!({
            "hook_event_name": "Stop",
            "session_id": "codex-sess",
            "_pixtuoid_source": "codex"
        }));
        assert_eq!(
            ev.agent_id(),
            AgentId::from_parts("codex", "codex-sess"),
            "Codex Stop keys on session_id under the codex namespace"
        );
    }

    // Deliberate narrowing (vs pre-registry): SubagentStart/Stop decode only
    // through a source's OWN custom decoder (CC's and Codex's rows carry one,
    // #241); a payload stamped with a source whose row has none bails instead
    // of minting a child keyed on a raw agent_id that could never coalesce
    // with that source's own keying.
    #[test]
    fn subagent_hooks_from_sources_without_a_custom_decoder_bail() {
        for event in ["SubagentStart", "SubagentStop"] {
            let ev = decode_hook_payload(json!({
                "hook_event_name": event,
                "session_id": "s",
                "agent_id": "child",
                "cwd": "/repo",
                // antigravity's row has no custom fn
                "_pixtuoid_source": "antigravity"
            }));
            assert!(ev.is_err(), "antigravity-attributed {event} must bail");
        }
    }

    // End-to-end pin for the alien-envelope claim-fully contract: an UNKNOWN
    // reasonix event must Err out of `decode_hook_payload` itself — proving
    // the registry dispatch routed it to the rx custom decoder AND that the
    // decoder never returns Ok(None) for its own envelope (a fall-through
    // would hit the shared arms' "missing hook_event_name" with a misleading
    // error, or worse, decode under CC-shaped semantics).
    #[test]
    fn unknown_reasonix_event_errs_end_to_end_not_falls_through() {
        let ev = decode_hook_payload(json!({
            "_pixtuoid_source": "reasonix",
            "event": "PreCompact",
            "cwd": "/repo"
        }));
        let msg = ev.expect_err("unknown rx event must bail").to_string();
        assert!(
            msg.contains("reasonix"),
            "error must come from the rx decoder (claimed fully), got: {msg}"
        );
    }

    // Version-skew pin: a shim stamping a source this binary doesn't know yet
    // (mid-rollout of a new CLI) must degrade gracefully — CC-shaped decode
    // under the UNKNOWN source's own namespace (no ghost merge into cc, no
    // bail). This is the registry's `descriptor_for → None` fallback path.
    #[test]
    fn unknown_source_decodes_cc_shaped_under_its_own_namespace() {
        let ev = decode_single(json!({
            "hook_event_name": "Stop",
            "session_id": "s-1",
            "_pixtuoid_source": "some-future-cli"
        }));
        assert_eq!(
            ev.agent_id(),
            AgentId::from_parts("some-future-cli", "s-1"),
            "unknown source keys under its own namespace, not claude-code's"
        );
    }

    #[test]
    fn absent_source_still_defaults_to_claude() {
        // A payload with no `source` (legacy / un-stamped) must remain CC.
        let ev = decode_single(json!({
            "hook_event_name": "SessionStart",
            "session_id": "s",
            "transcript_path": "/p/a.jsonl",
            "cwd": "/repo"
        }));
        match ev {
            AgentEvent::SessionStart { source, .. } => {
                assert_eq!(source, crate::source::claude_code::SOURCE_NAME)
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn ellipsize_caps_on_chars_only_past_the_limit() {
        // Exactly AT the limit → unchanged (the negative branch of the cap),
        // multi-byte chars so a byte-slicing regression would panic/garble.
        let at = "é".repeat(MAX_DECODED_FIELD_CHARS);
        assert_eq!(ellipsize(&at, MAX_DECODED_FIELD_CHARS), at);
        // One char past → capped at the limit + '…'.
        let over = "é".repeat(MAX_DECODED_FIELD_CHARS + 1);
        let capped = ellipsize(&over, MAX_DECODED_FIELD_CHARS);
        assert_eq!(capped.chars().count(), MAX_DECODED_FIELD_CHARS + 1);
        assert!(capped.ends_with('…'), "cap must be marked: {capped:?}");
    }

    // conf-35 (#262 item 5): a Notification `message` is content-derived and
    // a hook line can legally be ~1 MiB — the Waiting reason must be capped
    // where it ENTERS (pitfall 3), like describe_tool_target already does.
    #[test]
    fn notification_reason_is_capped_at_the_decode_boundary() {
        let long = "メ".repeat(MAX_DECODED_FIELD_CHARS * 100);
        let evs = decode_hook_payload(json!({
            "hook_event_name": "Notification",
            "session_id": "ses-abc",
            "transcript_path": "/p/ses-abc.jsonl",
            "cwd": "/repo",
            "message": long
        }))
        .expect("decodes");
        match &evs[1] {
            AgentEvent::Waiting { reason, .. } => {
                assert_eq!(reason.chars().count(), MAX_DECODED_FIELD_CHARS + 1);
                assert!(reason.ends_with('…'));
            }
            other => panic!("expected Waiting, got {other:?}"),
        }
        // A legitimate short reason passes through untouched — pinned by
        // notification_decodes_to_identity_plus_waiting above ("permission?").
    }

    // The cwd is transcript/hook content too, and a SLASHLESS crafted value
    // makes the whole string the basename — the chokepoint shared by all three
    // derivers must cap it.
    #[test]
    fn cwd_basename_label_caps_a_content_derived_basename() {
        let long = "é".repeat(MAX_DECODED_FIELD_CHARS * 10);
        let label = cwd_basename_label("cc", Path::new(&long)).expect("a basename exists");
        assert_eq!(
            label.chars().count(),
            "cc·".chars().count() + MAX_DECODED_FIELD_CHARS + 1
        );
        assert!(label.ends_with('…'));
        // A legitimate cwd passes through unchanged.
        assert_eq!(
            cwd_basename_label("cc", Path::new("/repo/app")),
            Some("cc·app".to_string())
        );
    }

    /// The empty / filesystem-root cwd degrades to `None` (each source's own
    /// fallback then applies). `file_name()` is the one gate — this pins the
    /// removal of the former redundant `cwd == "" || cwd == "/"` pre-check.
    #[test]
    fn cwd_basename_label_is_none_for_empty_and_root() {
        assert_eq!(cwd_basename_label("cc", Path::new("")), None);
        assert_eq!(cwd_basename_label("cc", Path::new("/")), None);
    }

    /// Pin: every transcript-bearing source's `LabelDeriver`, on an EMPTY cwd,
    /// falls back to EXACTLY its registry `label_prefix` — no deriver hardcodes
    /// a prefix that could silently drift from the registry (invariant #3). The
    /// codex/copilot/antigravity derivers share [`derive_prefixed_label`]; CC's
    /// bespoke deriver (subagent + project-dir branches) is exercised directly.
    #[test]
    fn transcript_deriver_empty_cwd_fallback_equals_registry_prefix() {
        use crate::source::{claude_code, registry};
        // `line_decoder().is_some()` == transcript-bearing == has a LabelDeriver.
        for d in registry::REGISTRY
            .iter()
            .filter(|d| d.line_decoder().is_some())
        {
            let got = if d.name == claude_code::SOURCE_NAME {
                claude_code::cc_derive_label(Path::new(""), d.name, Path::new(""))
            } else {
                derive_prefixed_label(d.name, Path::new(""))
            };
            assert_eq!(
                got, d.label_prefix,
                "{} deriver empty-cwd fallback must equal its registry prefix",
                d.name
            );
        }
    }

    /// CC's per-tool target keys: the file-tool family reads `file_path`,
    /// Bash reads `command`, Grep/Glob read `pattern`, anything else has no
    /// keyed target. Deleting a match arm silently drops the `: target`
    /// suffix from every display of that family.
    #[test]
    fn describe_tool_target_keys_each_cc_tool_family() {
        for tool in ["Write", "Edit", "MultiEdit", "Read"] {
            assert_eq!(
                describe_tool_target(tool, Some(&json!({"file_path": "/a/b.rs"}))),
                Some("/a/b.rs"),
                "{tool} must key on file_path"
            );
        }
        assert_eq!(
            describe_tool_target("Bash", Some(&json!({"command": "ls"}))),
            Some("ls")
        );
        assert_eq!(
            describe_tool_target("Grep", Some(&json!({"pattern": "fn "}))),
            Some("fn ")
        );
        assert_eq!(
            describe_tool_target("WebFetch", Some(&json!({"url": "u"}))),
            None
        );
    }

    use crate::test_capture::capture_logs;

    /// The `unknown_dispatch` breadcrumb (upstream-drift defense #2) fires
    /// EXACTLY when the semantic signal caught a dispatch under an
    /// unrecognised name — never for the known `Agent` dispatch, whose warn
    /// would be per-tool-call noise drowning the real drift signal.
    #[test]
    fn unknown_dispatch_breadcrumb_fires_only_for_a_renamed_dispatch() {
        let renamed = capture_logs(|| {
            let d = make_tool_detail(
                "claude-code",
                "DelegateZ",
                Some(&json!({"subagent_type": "explorer"})),
            );
            assert!(d.is_task());
        });
        assert!(
            renamed.contains("unknown_dispatch") && renamed.contains("DelegateZ"),
            "a dispatch under an unrecognised name must leave the drift breadcrumb, got:\n{renamed}"
        );
        let known = capture_logs(|| {
            let d = make_tool_detail(
                "claude-code",
                "Agent",
                Some(&json!({"subagent_type": "explorer"})),
            );
            assert!(d.is_task());
        });
        assert!(
            !known.contains("unknown_dispatch"),
            "the known dispatch name must stay breadcrumb-silent, got:\n{known}"
        );
    }

    // tool_name is wire/transcript content landing in Active.detail → the
    // unbounded headless summary — capped in the Generic display like its
    // target.
    #[test]
    fn generic_tool_name_is_capped_in_the_display() {
        let long = "T".repeat(MAX_DECODED_FIELD_CHARS * 10);
        match make_tool_detail("test", &long, None) {
            ToolDetail::Generic { display } => {
                assert_eq!(display.chars().count(), MAX_DECODED_FIELD_CHARS + 1);
                assert!(display.ends_with('…'));
            }
            other => panic!("expected Generic, got {other:?}"),
        }
        // A legitimate short name passes through unchanged.
        match make_tool_detail("test", "Read", None) {
            ToolDetail::Generic { display } => assert_eq!(display, "Read"),
            other => panic!("expected Generic, got {other:?}"),
        }
    }

    // The shared non-CC last mile: `generic_keyed_detail` scans the source's
    // key list for the target, then routes it through the SAME cap chokepoint —
    // so a source can't scan a target and format it raw past
    // MAX_TOOL_TARGET_CHARS (the bypass class this helper closes structurally).
    #[test]
    fn generic_keyed_detail_scans_keys_then_caps_the_target() {
        const KEYS: &[&str] = &["command", "path"];
        // First present key wins; the target rides the `: …` suffix.
        match generic_keyed_detail("bash", Some(&json!({"command": "ls -la"})), KEYS) {
            ToolDetail::Generic { display } => assert_eq!(display, "bash: ls -la"),
            other => panic!("expected Generic, got {other:?}"),
        }
        // An over-cap target is ellipsized at MAX_TOOL_TARGET_CHARS through the
        // composed path — the guarantee a copy-pasted source now inherits.
        let long = "T".repeat(MAX_TOOL_TARGET_CHARS * 5);
        match generic_keyed_detail("run", Some(&json!({ "path": long })), KEYS) {
            ToolDetail::Generic { display } => {
                let target = display.strip_prefix("run: ").expect("has a target suffix");
                assert_eq!(target.chars().count(), MAX_TOOL_TARGET_CHARS + 1);
                assert!(target.ends_with('…'));
            }
            other => panic!("expected Generic, got {other:?}"),
        }
        // No matching key → bare tool name, no suffix.
        match generic_keyed_detail("noop", Some(&json!({"other": "x"})), KEYS) {
            ToolDetail::Generic { display } => assert_eq!(display, "noop"),
            other => panic!("expected Generic, got {other:?}"),
        }
    }

    // Every non-daemon source's REAL decoder must route its tool display through
    // the shared cap chokepoints (`generic_tool_display`/`ellipsize`), so an
    // over-cap tool name/target can never leak past the bound. The documented
    // residual (#272) is a per-decoder BYPASS — a display minted from raw wire
    // content without `ellipsize` (3 hook-only decoders, copilot twice). Before
    // this, only CC had a firing cap test; the other 9 were unpinned. The
    // completeness assert vs REGISTERED_SOURCES-minus-daemons makes a NEW source
    // fail until it gets a cap row (#612 FIND-36).
    #[test]
    fn every_agent_decoder_caps_its_tool_display() {
        use crate::source::{
            antigravity, claude_code, codewhale, codex, copilot, cursor, grok, hermes, kimi, omp,
            opencode, reasonix, registry,
        };
        use serde_json::json;
        use std::collections::HashSet;

        let name_s = "N".repeat(MAX_DECODED_FIELD_CHARS * 2);
        let tgt_s = "T".repeat(MAX_TOOL_TARGET_CHARS * 5);
        // codewhale's `tool_args` is a JSON STRING, not an object — embed the
        // over-cap target inside it so its target chokepoint is exercised too.
        let cw_args_s = format!("{{\"command\":\"{tgt_s}\"}}");
        // Borrow as &str (Copy) so each row's closure captures by copy — a String
        // would be MOVED into its first json! and break the `Fn` table.
        let (name, tgt, cw_args) = (name_s.as_str(), tgt_s.as_str(), cw_args_s.as_str());
        // Widest a capped display can be: capped name + ": " + capped target,
        // each gaining one '…'. A raw-content bypass (200+ chars) blows past it.
        let bound = MAX_DECODED_FIELD_CHARS + 1 + ": ".len() + MAX_TOOL_TARGET_CHARS + 1;

        type Row<'a> = (&'static str, Box<dyn Fn() -> Vec<AgentEvent> + 'a>);
        let table: Vec<Row> = vec![
            (
                claude_code::SOURCE_NAME,
                Box::new(|| {
                    claude_code::decode_cc_line(
                        "/p/ses-a.jsonl",
                        "claude-code",
                        json!({"type":"assistant","message":{"content":[
                            {"type":"tool_use","id":"t1","name":name,"input":{"file_path":tgt}}]}}),
                    )
                    .expect("cc decodes")
                }),
            ),
            (
                codex::SOURCE_NAME,
                Box::new(|| {
                    codex::decode_codex_line(
                        "/p/rollout.jsonl",
                        "codex",
                        json!({"type":"response_item","payload":{"type":"function_call","name":name}}),
                    )
                    .expect("codex decodes")
                }),
            ),
            (
                antigravity::SOURCE_NAME,
                Box::new(|| {
                    antigravity::decode_ag_line(
                        "/x/transcript.jsonl",
                        "antigravity",
                        json!({"type":"PLANNER_RESPONSE","step_index":0,"tool_calls":[
                            {"name":name,"args":{"CommandLine":tgt}}]}),
                    )
                    .expect("antigravity decodes")
                }),
            ),
            (
                copilot::SOURCE_NAME,
                Box::new(|| {
                    copilot::decode_copilot_line(
                        "/c/id/events.jsonl",
                        "copilot",
                        json!({"type":"tool.execution_start","data":{
                            "toolCallId":"tc1","toolName":name,"arguments":{"command":tgt}}}),
                    )
                    .expect("copilot decodes")
                }),
            ),
            (
                omp::SOURCE_NAME,
                Box::new(|| {
                    omp::decode_omp_line(
                        "/o/s.jsonl",
                        "omp",
                        json!({"type":"message","message":{"role":"assistant","content":[
                            {"type":"toolCall","id":"t1","name":name,"arguments":{"command":tgt}}]}}),
                    )
                    .expect("omp decodes")
                }),
            ),
            (
                reasonix::SOURCE_NAME,
                Box::new(|| {
                    reasonix::decode_rx_hook_payload(&json!({
                        "event":"PreToolUse","cwd":"/r","toolName":name,"toolArgs":{"command":tgt}}))
                    .expect("reasonix decodes")
                }),
            ),
            (
                codewhale::SOURCE_NAME,
                Box::new(|| {
                    codewhale::decode_cw_hook_payload(&json!({
                        "event":"tool_call_before","cwd":"/r","tool":name,"tool_args":cw_args}))
                    .expect("codewhale decodes")
                }),
            ),
            (
                opencode::SOURCE_NAME,
                Box::new(|| {
                    opencode::decode_oc_hook_payload(&json!({
                        "type":"message.part.updated","properties":{"sessionID":"ses-1","part":{
                            "type":"tool","callID":"c1","tool":name,
                            "state":{"status":"running","input":{"command":tgt}}}}}))
                    .expect("opencode decodes")
                }),
            ),
            (
                cursor::SOURCE_NAME,
                Box::new(|| {
                    cursor::decode_cursor_hook_payload(&json!({
                        "hook_event_name":"preToolUse","session_id":"s",
                        "tool_name":name,"tool_input":{"command":tgt}}))
                    .expect("cursor decodes")
                }),
            ),
            (
                hermes::SOURCE_NAME,
                Box::new(|| {
                    hermes::decode_hermes_hook_payload(&json!({
                        "hook_event_name":"pre_tool_call","session_id":"s","cwd":"/r",
                        "tool_name":name,"tool_input":{"command":tgt}}))
                    .expect("hermes decodes")
                }),
            ),
            (
                grok::SOURCE_NAME,
                Box::new(|| {
                    grok::decode_grok_hook_payload(&json!({
                        "hookEventName":"pre_tool_use","sessionId":"s","cwd":"/r",
                        "workspaceRoot":"/r","toolName":name,"toolUseId":"c1",
                        "toolInput":{"command":tgt},"toolInputTruncated":false}))
                    .expect("grok decodes")
                }),
            ),
            (
                // Kimi rides the SHARED CC-shaped arms (its Extend decoder declines
                // PreToolUse), so route the whole payload through the dispatcher —
                // the shared `make_tool_detail` is the chokepoint under test.
                kimi::SOURCE_NAME,
                Box::new(|| {
                    decode_hook_payload(json!({
                        "hook_event_name":"PreToolUse","session_id":"s","cwd":"/r",
                        "tool_name":name,"tool_input":{"command":tgt},"tool_use_id":"c1",
                        "_pixtuoid_source":"kimi"}))
                    .expect("kimi decodes")
                }),
            ),
        ];

        for (src, decode) in &table {
            let evs = decode();
            let display = evs
                .iter()
                .find_map(|e| match e {
                    AgentEvent::ActivityStart {
                        detail: Some(d), ..
                    } => Some(d.display().to_string()),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{src}: decoder emitted no ActivityStart with a detail"));
            assert!(
                display.chars().count() <= bound,
                "{src}: tool display {} chars > cap bound {bound} — a chokepoint bypass leaks raw content",
                display.chars().count()
            );
            // '…' proves the cap FIRED (a decoder that silently dropped the
            // over-cap field would pass the length check vacuously).
            assert!(
                display.ends_with('…'),
                "{src}: display {display:?} did not end with the ellipsis — cap did not fire"
            );
        }

        // Completeness (anti-drift teeth): the table must cover EXACTLY the
        // non-daemon registered sources — a new one reds until it gets a row.
        let covered: HashSet<&str> = table.iter().map(|(n, _)| *n).collect();
        for &s in crate::source::REGISTERED_SOURCES {
            let daemon = registry::descriptor_for(s).is_some_and(|d| d.is_daemon());
            if !daemon {
                assert!(covered.contains(s), "add {s} to the decoder cap table");
            }
        }
        for &c in &covered {
            let daemon = registry::descriptor_for(c).is_some_and(|d| d.is_daemon());
            assert!(!daemon, "{c} is a daemon — remove it from the cap table");
        }
    }

    // A DAEMON source's payload decodes to ZERO AgentEvents — the `is_daemon()`
    // short-circuit that replaced the deleted `decode_openclaw_hook_custom`. Pins
    // that a daemon envelope (alien `{type:…}`, no `hook_event_name`) never reaches
    // the shared agent arms (which would bail on the missing field) — registry-
    // driven, so a 2nd daemon is covered for free.
    #[test]
    fn daemon_source_payload_decodes_to_zero_agent_events() {
        let v = json!({"_pixtuoid_source": "openclaw", "type": "gateway_start", "_pid": 1});
        let evs = decode_hook_payload(v).expect("a daemon payload must not error");
        assert!(
            evs.is_empty(),
            "a daemon source decodes to zero AgentEvents (presence rides the sibling channel), got {evs:?}"
        );
    }

    // ---- the grok cross-fire guard ----

    /// One grok envelope, byte-identical except for the shim's source tag.
    fn grok_envelope(tag: &str) -> Value {
        json!({
            "_pixtuoid_source": tag,
            "hookEventName": "pre_tool_use",
            "sessionId": "0197fa30-sess",
            "cwd": "/repo",
            "workspaceRoot": "/repo",
            "timestamp": "2026-07-16T12:00:00Z",
            "toolName": "run_terminal_command",
            "toolUseId": "call_1",
            "toolInput": {"command": "ls"},
            "toolInputTruncated": false
        })
    }

    #[test]
    fn grok_tagged_grok_envelope_decodes_via_the_custom_decoder() {
        let evs = decode_hook_payload(grok_envelope("grok")).expect("decodes");
        assert_eq!(evs.len(), 2, "Identity + ActivityStart");
        assert!(evs
            .iter()
            .all(|e| e.agent_id() == crate::AgentId::from_parts("grok", "0197fa30-sess")));
    }

    #[test]
    fn cross_fired_grok_envelopes_are_dropped_quietly() {
        // grok executes the hook commands pixtuoid installed in
        // ~/.claude/settings.json and ~/.cursor/hooks.json (compat scan, on by
        // default) — the SAME grok envelope then arrives tagged as those
        // sources. The `hookEventName` key is grok's envelope fingerprint;
        // mis-tagged copies are known duplicates of the grok-tagged one and
        // must decode to NOTHING (Ok, not Err — an Err would warn on every
        // tool call of every grok session).
        for tag in ["claude-code", "cursor"] {
            let evs = decode_hook_payload(grok_envelope(tag))
                .unwrap_or_else(|e| panic!("{tag}: cross-fired envelope must be Ok, got {e}"));
            assert!(
                evs.is_empty(),
                "{tag}: cross-fired grok envelope must decode to zero events, got {evs:?}"
            );
        }
        // An UNTAGGED grok envelope (a user wired the bare shim command into
        // grok's hooks by hand): the missing tag defaults to claude-code —
        // same quiet drop, never a ghost.
        let mut untagged = grok_envelope("x");
        untagged.as_object_mut().unwrap().remove("_pixtuoid_source");
        assert!(decode_hook_payload(untagged).expect("Ok").is_empty());
    }

    #[test]
    fn cc_envelopes_still_decode_normally_despite_the_guard() {
        // The guard keys on the `hookEventName` KEY, which no CC/cursor
        // envelope carries — a real CC payload must be untouched by it.
        let evs = decode_hook_payload(json!({
            "hook_event_name": "PreToolUse",
            "session_id": "cc-sess",
            "cwd": "/repo",
            "tool_name": "Bash",
            "tool_use_id": "toolu_1",
            "tool_input": {"command": "ls"}
        }))
        .expect("decodes");
        assert!(!evs.is_empty());
    }
}
