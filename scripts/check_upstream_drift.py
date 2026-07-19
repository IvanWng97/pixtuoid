#!/usr/bin/env python3
"""Upstream wire-format drift watch.

pixtuoid decodes the CC and Codex CLI wire formats (hook event names, the
subagent-dispatch tool name). Those names change upstream WITHOUT notice — the
`Task` -> `Agent` rename shipped undocumented and silently disabled subagent
suppression. This script verifies that the names we depend on still exist at the
canonical upstream sources, so CI can flag a break before it reaches a user.

It reads what we depend on directly from our own source (no snapshot file to rot)
and compares against the live upstream:

  * Codex hook events  -> `CODEX_EVENTS` in crates/pixtuoid/src/install/codex.rs
                          vs the `HookEventName` enum in openai/codex protocol.rs
  * Codex rollout types-> the `("event_msg"|"response_item", …)` decode arms in
                          crates/pixtuoid-core/src/source/codex.rs vs the `EventMsg`
                          enum (protocol.rs) + the `ResponseItem` enum (models.rs).
                          The transcript decoder drops an unknown type SILENTLY
                          (`_ => vec![]`, no breadcrumb), so this positive check —
                          each depended type still exists upstream — is its ONLY backstop
  * CC hook events     -> `EVENTS` in crates/pixtuoid/src/install/claude.rs
                          vs the hook-event summary table in code.claude.com
                          hooks.md (CC is a closed binary; the docs markdown is
                          the only watchable surface)
  * CC dispatch tool   -> the known names in `make_tool_detail`
                          vs the tool list in code.claude.com tools-reference
  * Reasonix hooks     -> `REASONIX_EVENTS` in crates/pixtuoid/src/install/reasonix.rs
                          + the payload fields decode_rx_hook_payload reads
                          vs the `Event` consts / json tags in
                          esengine/DeepSeek-Reasonix internal/hook/hook.go
  * CodeWhale hooks    -> `CODEWHALE_EVENTS` in crates/pixtuoid/src/install/codewhale.rs
                          vs the snake_case `HookEvent` enum in
                          Hmbown/CodeWhale crates/tui/src/hooks.rs
  * grok hooks/wire    -> `GROK_EVENTS` in crates/pixtuoid/src/install/grok.rs
                          vs the `HookEventName` enum + envelope/payload serde in
                          xai-org/grok-build xai-grok-hooks/src/event.rs, PLUS the
                          transcript xAI-update vocabulary (extensions/
                          notification.rs) and the active_sessions.json registry
                          struct (the liveness ladder's parse surface)
  * burn-tier fields   -> codex turn_context.{model,effort} (TurnContextItem,
                        protocol.rs) + copilot data.model (schema) + opencode
                        info.model (session.ts) + CC ultra attachment markers
                        (docs appearance watch) — see #541
* opencode events    -> the EventV2 `type`s the decoder maps (the `match event`
                          block in crates/pixtuoid-core/src/source/opencode.rs)
                          vs the `EventV2.define` type literals in
                          anomalyco/opencode packages/schema/src/v1/session.ts +
                          packages/schema/src/permission.ts
                          (one-directional: only a VANISHED depended type alarms)
  * Copilot events     -> the event `type`s the decoder maps (the `match kind`
                          block in crates/pixtuoid-core/src/source/copilot.rs)
                          vs the per-event `type` consts in the published
                          @github/copilot-<os>-<arch> session-events JSON schema (unpkg)
                          (one-directional: Copilot emits ~100 event types and we
                          map ~10 by design, so only a VANISHED depended type alarms)
  * Cursor hooks       -> the camelCase `hook_event_name`s we register
                          (CURSOR_EVENTS in crates/pixtuoid/src/install/cursor.rs)
                          vs the hook-event names on cursor.com/docs/hooks
                          (one-directional: Cursor exposes ~18 hook events and we
                          map ~5 by design, so only a VANISHED depended event alarms)
  * Hermes hooks       -> `HERMES_EVENTS` in crates/pixtuoid/src/install/hermes.rs
                          vs the `_DEFAULT_PAYLOADS` shell-hook event keys in
                          NousResearch/hermes-agent hermes_cli/hooks.py
                          (one-directional: Hermes fires ~15 shell-hook events and
                          we register 4 by design, so only a VANISHED depended event alarms)
  * Kimi hooks         -> `KIMI_EVENTS` in crates/pixtuoid/src/install/kimi.rs
                          vs the PascalCase hook-event names in the raw
                          MoonshotAI/kimi-code docs/en/customization/hooks.md
                          (one-directional: Kimi fires 16 events and we register 8
                          by design, so only a VANISHED depended event alarms)

Beyond the event/TYPE lists above, FIELD-NAME drift is watched wherever the
upstream field owner is fetchable (a rename → the decoder reads None and the
sprite silently breaks — the same class as a vanished type): Reasonix payload
json tags; Codex EventMsg/ResponseItem rollout types + FunctionCall name/arguments;
CodeWhale DEEPSEEK_* env vars (HookContext::to_env_vars); opencode Struct fields;
Copilot schema `properties`; OpenClaw hook-types fields; Hermes _serialize_payload
keys (agent/shell_hooks.py). Cursor + CC (closed binaries, docs-prose only) and
Antigravity (no fetchable schema) CANNOT be field-watched — the in-code drift
breadcrumbs (defense #2: drift::missing_field/unknown_event) are the limit there.

Exit codes:
  0  no drift
  1  actionable drift (a name we depend on vanished, or a new upstream Codex
     or CC hook event we neither register nor intentionally omit) -> open a
     tracking issue
  2  could not check (network/HTTP error) -> transient, do NOT alarm

See crates/pixtuoid-core/CLAUDE.md "Keeping the decode mapping current".
"""

from __future__ import annotations

import http.client
import json
import pathlib
import re
import sys
import traceback
import urllib.error
import urllib.request

# What a fetch can raise transiently. URLError covers connect-phase failures
# (urllib wraps OSErrors only during do_open) and HTTP 4xx/5xx (HTTPError
# subclasses it), but the READ phase inside fetch() raises raw
# socket.timeout / ConnectionResetError (OSError subclasses, NOT URLError)
# and http.client.IncompleteRead (HTTPException) — left uncaught they exit 1
# and the workflow files a junk "confirmed drift" issue from an empty report.
# URLError is itself an OSError subclass; kept explicit to document intent.
FETCH_ERRORS = (urllib.error.URLError, OSError, http.client.HTTPException)

# A permanent HTTP status means the URL itself is wrong/gone — our pinned
# upstream path moved, so the watch is BLIND for that source until fixed. This is
# BREAKING, never transient. Everything else (403/429 throttling behind a CDN,
# 5xx server hiccups, connect/read timeouts) is genuinely retry-later. The trap
# this guards: `HTTPError` subclasses `URLError` ⊂ FETCH_ERRORS, so a 404 used to
# fall into the transient bucket and the weekly job stayed green while silently
# watching nothing.
PERMANENT_HTTP_STATUS = frozenset({404, 410, 451})

REPO = pathlib.Path(__file__).resolve().parent.parent

CODEX_PROTOCOL_URL = (
    "https://raw.githubusercontent.com/openai/codex/main/"
    "codex-rs/protocol/src/protocol.rs"
)
# The ROLLOUT `response_item` types (function_call, …) live in the sibling
# models.rs (`crate::models::ResponseItem`), NOT protocol.rs; the `event_msg`
# types are the `EventMsg` enum in protocol.rs (reused above).
CODEX_MODELS_URL = (
    "https://raw.githubusercontent.com/openai/codex/main/"
    "codex-rs/protocol/src/models.rs"
)
CC_TOOLS_URL = "https://code.claude.com/docs/en/tools-reference.md"
CC_HOOKS_URL = "https://code.claude.com/docs/en/hooks.md"

# CC durable-end-marker + sessions-registry watch. CC is a closed binary, so —
# exactly like the dispatch-tool check below — the docs markdown is the only
# watchable surface; this is an APPEARANCE watch (the inverse of the
# vanished-identifier checks): pixtuoid treats CC lifecycle as hook + idle
# sweep ONLY, because CC persists NO structural end record in transcripts
# today (135-transcript corpus, 2026-06; the content-based /exit matcher was
# removed — chat content must never drive lifecycle). Two surfaces we want to
# ADOPT the moment they exist upstream:
#   * a structural transcript end record (`subtype:"session_end"`) —
#     `cc_session_ended` already decodes it; the docs mentioning it means CC
#     started persisting it and the JSONL transport gains a durable end signal.
#     Adoption note: the liveness-probe first-sight bypass (`probe_admits` in
#     core's source/jsonl.rs) deliberately skips the gate's ended tail-scan
#     because no such marker exists today — when one lands, admission needs an
#     ended-check before bypassing the gate.
#   * the `~/.claude/sessions/<pid>.json` registry ({pid, sessionId, startedAt,
#     cwd, procStart, status}) — the input the liveness probe consumes
#     (#224/#227; shape drift is consumer-warned in live_cc_session_ids, #247).
# All markers are ABSENT from hooks.md at add time (verified live); a hit is
# review-class drift (something new to adopt), never breaking. `session_end`
# is snake_case on purpose: the SessionEnd HOOK name appears throughout
# hooks.md and must not match.
# CC hook-payload surfaces we DEPEND on that are DOCUMENTED (hooks.md) — the
# inverse direction of the appearance markers below: these strings VANISHING
# from hooks.md is review-class drift (the docs moved/renamed a surface the
# burn-tier decoder reads). `CLAUDE_EFFORT` pins the effort row (the decoder
# reads `effort.level` off tool-context payloads; ultracode reports as xhigh);
# the model sentence pins SessionStart's optional `model` field.
CC_DEPENDED_DOC_MARKERS = {
    "CLAUDE_EFFORT": "the hook-payload effort surface (effort.level, burn tier)",
    "receive a `model` field": "SessionStart's optional model field (burn tier)",
}

CC_LIFECYCLE_SURFACE_MARKERS = {
    "session_end": 'a structural transcript end record (subtype:"session_end")',
    ".claude/sessions/": "the ~/.claude/sessions/<pid>.json session registry",
    "procStart": "the sessions-registry procStart field",
    # burn tier (#541): the periodic ultra-effort attachment markers the CC
    # decoder synthesizes effort labels from (undocumented wire, verified live
    # 2026-07-10). CC is a closed binary, so like the registry above this is an
    # APPEARANCE watch: the docs mentioning them = upstream started documenting
    # the surface — a review ping to re-verify our synthesized labels/shape.
    "ultra_effort_enter": "the ultra-effort transcript attachment marker",
    "ultrathink_effort": "the ultrathink transcript attachment marker",
    "ultra_effort_exit": "the ultra-effort EXIT attachment marker (instant flame-off)",
}

# Codex hook events we DELIBERATELY do not register — they are not agent
# activity a visualizer cares about. A new upstream hook NOT in this set is
# surfaced for review (it might be a lifecycle signal worth handling).
CODEX_KNOWN_OMITTED = {"PreCompact", "PostCompact"}

# CC hook events we DELIBERATELY do not register (vs install/claude.rs EVENTS,
# which since #241 includes SubagentStart/SubagentStop). A NEW upstream event
# beyond both sets is surfaced for review — the weekly "evaluate this" ping.
# Verified against hooks.md 2026-06: per-turn / content noise (UserPromptSubmit,
# UserPromptExpansion, MessageDisplay, Stop, StopFailure, PostToolBatch,
# PostToolUseFailure), permission detail already covered by Notification
# (PermissionRequest, PermissionDenied), task/teammate bookkeeping (TaskCreated,
# TaskCompleted, TeammateIdle), environment/config plumbing (Setup,
# InstructionsLoaded, ConfigChange, CwdChanged, FileChanged, WorktreeCreate,
# WorktreeRemove, Elicitation, ElicitationResult), compaction internals
# (PreCompact, PostCompact).
CC_KNOWN_OMITTED = {
    "Setup",
    "UserPromptSubmit",
    "UserPromptExpansion",
    "PermissionRequest",
    "PermissionDenied",
    "PostToolUseFailure",
    "PostToolBatch",
    "MessageDisplay",
    "TaskCreated",
    "TaskCompleted",
    "Stop",
    "StopFailure",
    "TeammateIdle",
    "InstructionsLoaded",
    "ConfigChange",
    "CwdChanged",
    "FileChanged",
    "WorktreeCreate",
    "WorktreeRemove",
    "PreCompact",
    "PostCompact",
    "Elicitation",
    "ElicitationResult",
}

REASONIX_HOOK_URL = (
    "https://raw.githubusercontent.com/esengine/DeepSeek-Reasonix/main-v2/"
    "internal/hook/hook.go"
)

# Reasonix hook events we DELIBERATELY do not register: PostLLMCall fires per
# model turn (noise), PreCompact is a compaction internal, SubagentStop carries
# no ids and is already covered by the parent's `task` PostToolUse.
REASONIX_KNOWN_OMITTED = {"PostLLMCall", "PreCompact", "SubagentStop"}

# Payload fields decode_rx_hook_payload reads — a renamed json tag upstream
# silently zeroes the decode (`event`/`cwd` are load-bearing: a payload without
# them is rejected as malformed; `subject` feeds the PermissionRequest→Waiting
# reason, #302).
REASONIX_PAYLOAD_FIELDS = {"event", "cwd", "toolName", "toolArgs", "subject", "message"}

CODEWHALE_HOOK_URL = (
    "https://raw.githubusercontent.com/Hmbown/CodeWhale/main/"
    "crates/tui/src/hooks.rs"
)

# CodeWhale hook events we DELIBERATELY do not register (snake_case wire names):
# turn_end is per-turn telemetry, and mode_change/on_error/shell_env are not
# agent activity a visualizer shows. (subagent_spawn/subagent_complete ARE
# registered — they drive child sprites.)
CODEWHALE_KNOWN_OMITTED = {
    "turn_end",
    "mode_change",
    "on_error",
    "shell_env",
}

# CodeWhale ENV-MODE identity: the shim (pixtuoid-hook) folds these DEEPSEEK_*
# env vars into the cwd-keyed `{cwd, tool, tool_args}` envelope the decoder reads
# (source/codewhale.rs). The envelope FIELD names are our own shim contract (they
# can't drift), but the DEEPSEEK_* names are CodeWhale's — set by
# `HookContext::to_env_vars` in the SAME hooks.rs the event check fetches. WORKSPACE
# is load-bearing: it becomes the envelope `cwd` = the AgentId KEY, so a rename →
# the shim reads None → empty cwd → the decoder drops EVERY session (no sprite).
# (DEEPSEEK_SESSION_ID is deliberately NOT read — proven inconsistent — so it's
# not a dependency.) The RAW subagent-JSON fields (agent_id/workspace) are NOT
# watched here: their owner is a fuzzy ui.rs `json!` macro, and the decoder's own
# `ok_or_else`/parentless-degrade (defense #2) covers them.
CODEWHALE_ENV_FIELDS = {"DEEPSEEK_WORKSPACE", "DEEPSEEK_TOOL_NAME", "DEEPSEEK_TOOL_ARGS"}

# opencode is open TS: the EventV2 `type` strings the plugin forwards + the
# decoder maps live in these files. The check is ONE-DIRECTIONAL — opencode emits
# ~50 event types and we intentionally map only a handful, so "new upstream event"
# is noise; we only alarm when a type WE DEPEND ON vanishes (a rename the plugin
# would forward but the decoder would map to nothing).
# NB: the repo's default branch is `dev` (not `main`) — the `main` branch was
# retired, which 404'd these URLs and (pre-`try_fetch`) was silently bucketed as
# transient, blinding the opencode watch. Track `dev` (the active default).
# NB2: opencode moved the schema definitions out of `packages/core/` into a
# dedicated `packages/schema/` package (the old `core/src/v1/session.ts` is now a
# re-export shim with no `type:` literals — it 200s but greps empty, which read as
# a false "every event GONE" until these paths were repointed, #406). The session
# lifecycle + `message.part.updated` live in `schema/src/v1/session.ts`; the v2
# `permission.v2.asked` lives in the top-level `schema/src/permission.ts`.
OPENCODE_EVENT_URLS = (
    "https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/schema/src/v1/session.ts",
    "https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/schema/src/permission.ts",
)

# `permission.asked` is forwarded/decoded DEFENSIVELY (a V1/alias spelling); only
# `permission.v2.asked` is a guaranteed standalone upstream EventV2 definition, so
# don't alarm if the bare form isn't found as a `type:` literal.
OPENCODE_TOLERATED = {"permission.asked"}

# opencode payload FIELD names decode_oc_hook_payload reads (beyond the `type`
# discriminator): `info.{id,parentID,directory}` (id = the ses_* identity KEY;
# parentID = subagent link) and `part.{type,callID,tool,state.{status,input}}`.
# A rename → the decoder reads None → wrong-register / no-link / no-activity. They
# appear as `field: …` property lines in the Schema.Struct defs (session.ts).
# Checked ONE-DIRECTIONAL against the SAME concatenated schema `text`.
OPENCODE_PAYLOAD_FIELDS = {
    "info", "id", "parentID", "directory",
    "part", "sessionID", "callID", "tool", "state", "status", "input",
    # burn tier (#541): session.created carries `info.model.{id, providerID}`
    # (SessionInfo.model → SessionModel in session.ts); the decoder reads
    # `model.id` — `id` is watched above, this watches the wrapper.
    "model",
}

# Copilot CLI publishes a session-events JSON schema; unpkg serves the file
# directly (the bare path 302-redirects to the latest published version, which
# urllib follows — intentionally UNPINNED: a drift watch wants the latest shape,
# not a frozen one). Each event is a `definitions.<Name>` object whose
# `properties.type.const` is the wire `type` string. The check is ONE-DIRECTIONAL
# (like opencode): Copilot emits ~100 event types and copilot.rs intentionally maps
# only ~10, so "new upstream event" is noise — we alarm only when a type WE DEPEND
# ON vanishes (a rename the transcript still carries but the decoder maps to nothing).
# NB: `@github/copilot` is now a thin loader stub (its tarball is just package.json
# + npm-loader.js that pulls a `@github/copilot-<os>-<arch>` binary package at
# runtime), so the schema 404'd at the old root path (#406). The schema ships
# inside the platform packages at `schemas/session-events.schema.json`; we fetch
# the linux-x64 one (matches the CI host — every platform package carries the
# identical schema, and unpkg serves the single file without the 100MB tarball).
COPILOT_SCHEMA_URL = "https://unpkg.com/@github/copilot-linux-x64/schemas/session-events.schema.json"

# Copilot payload FIELD names decode_copilot_line / extract_copilot_cwd read
# (beyond the `type` discriminator): the `data` ENVELOPE wrapper (every tool /
# permission / identity field below lives under it — decode_copilot_line's
# `obj.get("data")`, extract_copilot_cwd's `v.get("data")`), so a rename of this
# one key silently nulls ALL of them while the nested fields still resolve under
# the new wrapper (the union check would NOT alarm) — it is a top-level
# `properties` key on every event, so the all-depth union finds it and the add is
# false-alarm-safe; identity/link (`agentId` — the child key, == data.toolCallId;
# `sessionId`, `context`, `cwd`), tool (`toolCallId`, `toolName`, `arguments`),
# display (`agentDisplayName`) and permission (`permissionRequest`, `result`,
# `kind`). The wire `parentId` is deliberately NOT here — sub-agents link via the
# envelope `agentId`, not a parent field, so watching `parentId` would false-alarm
# on a field we don't depend on. Curated (NOT scraped — a scrape drags in opaque
# tool-arg keys + fixture JSON). Checked against the union of every `properties`
# key at ANY depth (envelope + nested `data.properties`) in the SAME schema `text`
# (a depended field GONE = breaking).
COPILOT_PAYLOAD_FIELDS = {
    "data",
    "agentId", "sessionId", "context", "cwd",
    "toolCallId", "toolName", "arguments", "agentDisplayName",
    "permissionRequest", "result", "kind",
    # burn tier (#541): the per-tool model (ToolExecutionCompleteData.model,
    # schema-verified 2026-07-10) — a rename silently darkens the cp· badge.
    "model",
}

# Cursor CLI (`cursor-agent`) is HOOK-ONLY; the events we register/decode are
# camelCase `hook_event_name`s (`source/cursor.rs`). Cursor is a closed binary,
# so — like CC — the docs markdown is the only watchable surface. ONE-DIRECTIONAL
# (like opencode): Cursor exposes ~18 hook events and we map ~5 by design, so a
# "new upstream event" is noise; we alarm only when an event WE DEPEND ON
# vanishes (a rename the CLI would fire but the decoder maps to nothing). The
# common-word event `stop` is intrinsically low-confidence (the docs page
# contains the word regardless), so its disappearance can be masked — the
# distinctive `sessionStart`/`sessionEnd`/`preToolUse`/`postToolUse` carry the check.
CURSOR_HOOKS_URL = "https://cursor.com/docs/hooks"

# OpenClaw is a daemon gateway; pixtuoid ships a TS plugin that registers a
# handful of lifecycle hooks (`OPENCLAW_EVENTS` in install/openclaw.rs) and
# forwards their timing to the wandering lobster mascot. OpenClaw is open TS:
# the canonical hook-name union lives in `src/plugins/hook-types.ts` as quoted
# string literals. ONE-DIRECTIONAL (like opencode/cursor): OpenClaw defines ~40
# hook types and we register 6 by design, so a "new upstream event" is noise —
# we alarm only when an event WE REGISTER vanishes (a rename means the plugin
# registers a hook OpenClaw never fires, so presence silently goes dark).
OPENCLAW_HOOK_TYPES_URL = (
    "https://raw.githubusercontent.com/openclaw/openclaw/main/src/plugins/hook-types.ts"
)

# OpenClaw payload FIELD names decode_openclaw_presence reads (beyond `type`):
# `runId` (the in-flight run key), `sessionId` (fallback key + label) and
# `success` (agent_end → Degraded gate). `_pid` is plugin-stamped process.pid (no
# upstream coupling); sessionKey/reason/messageCount are forwarded-but-unread.
# Checked ONE-DIRECTIONAL (bare `\b` word-boundary) against the SAME hook-types.ts
# `text`. NB `success` is a common word — like the cursor `stop` caveat, a rename
# of THE depended field could be masked by an unrelated occurrence (low-confidence
# false-negative); the distinctive `runId`/`sessionId` carry the check.
OPENCLAW_PAYLOAD_FIELDS = {"runId", "sessionId", "success"}

# Hermes Agent is a hook-only source: we install SHELL hooks into config.yaml and
# register 4 of its lifecycle events (`HERMES_EVENTS` in install/hermes.rs). Hermes
# is open Python: the canonical shell-hook event set is the KEYS of `_DEFAULT_PAYLOADS`
# in hermes_cli/hooks.py (the `hermes hooks test`/`doctor` fixtures, whose kwargs
# mirror the real invoke_hook() call sites). ONE-DIRECTIONAL (like opencode/openclaw):
# Hermes fires ~15 events and we register 4, so only an event WE REGISTER vanishing is
# breaking (a rename → the shell hook we install fires nothing → no sprite).
HERMES_HOOK_URL = (
    "https://raw.githubusercontent.com/NousResearch/hermes-agent/main/hermes_cli/hooks.py"
)
# The Hermes shell-hook PAYLOAD (field names, not the event list) is assembled by
# `_serialize_payload()` in agent/shell_hooks.py — a DIFFERENT file from the
# event-list source (hooks.py). The decoder reads `session_id`/`cwd`/`tool_name`/
# `tool_input`; a rename → the shell-hook JSON omits it → the decoder reads None
# (no key → no coalesce, no tool label). Two orthogonal checks, two files.
HERMES_SHELL_HOOK_URL = (
    "https://raw.githubusercontent.com/NousResearch/hermes-agent/main/agent/shell_hooks.py"
)
# Hermes payload FIELD names decode_hermes_hook_payload reads (the `session_id`
# coalesce key + `cwd` label + `tool_name`/`tool_input` for the tool detail).
# `hook_event_name` is the discriminator (event check covers it). Checked as
# dict-key literals in _serialize_payload; ONE-DIRECTIONAL (a depended field gone).
HERMES_PAYLOAD_FIELDS = {"session_id", "cwd", "tool_name", "tool_input"}

# Kimi Code CLI (MoonshotAI/kimi-code) is HOOK-ONLY; the events we register/decode
# are Claude-Code-shaped PascalCase `hook_event_name`s (`KIMI_EVENTS` in
# install/kimi.rs). Kimi is a pnpm/TS monorepo, but the canonical hook-event list
# lives in the docs (each name appears verbatim in the hooks page — a summary
# table AND the payload examples), so — like Cursor — the raw markdown is the
# watchable surface. ONE-DIRECTIONAL (like cursor/hermes/openclaw): Kimi exposes
# 16 hook events and we register 8 by design, so a "new upstream event" is noise;
# we alarm only when an event WE DEPEND ON vanishes (a rename the CLI would fire
# but the decoder maps to nothing). The common-word event `Stop` is intrinsically
# low-confidence (the doc contains the word regardless), so its disappearance can
# be masked — the distinctive `PreToolUse`/`PostToolUseFailure`/`SessionStart`/
# `SessionEnd`/`PermissionRequest` carry the check (the cursor `stop` caveat).
KIMI_HOOKS_URL = (
    "https://raw.githubusercontent.com/MoonshotAI/kimi-code/main/"
    "docs/en/customization/hooks.md"
)

# grok (Grok Build, xai-org/grok-build) is open Rust with THREE depended
# surfaces in three files: the hook event enum + payload serde (event.rs), the
# transcript xAI-extension vocabulary (extensions/notification.rs — the ACP
# half lives in the external agent-client-protocol crate and is deliberately
# not fetched: a versioned protocol, pinned in-repo by grok's own wire_tags
# guard test), and the active_sessions.json liveness-registry struct.
GROK_HOOK_URL = (
    "https://raw.githubusercontent.com/xai-org/grok-build/main/"
    "crates/codegen/xai-grok-hooks/src/event.rs"
)
GROK_NOTIFICATION_URL = (
    "https://raw.githubusercontent.com/xai-org/grok-build/main/"
    "crates/codegen/xai-grok-shell/src/extensions/notification.rs"
)
GROK_ACTIVE_SESSIONS_URL = (
    "https://raw.githubusercontent.com/xai-org/grok-build/main/"
    "crates/codegen/xai-grok-shell/src/active_sessions.rs"
)
# grok hook events we DELIBERATELY do not register: compaction internals, not
# agent activity (the CC/Codex PreCompact/PostCompact precedent). SubagentEnd
# IS registered (upstream's finish site fires the alias, docs name SubagentStop
# — we register both spellings, so neither appears here).
GROK_KNOWN_OMITTED = {"PreCompact", "PostCompact"}
# Hook fields decode_grok_hook_payload reads, split by serde origin in
# event.rs: the ENVELOPE fields are camelCase via struct-level rename_all (the
# wire name never appears literally — check the `pub <ident>:` declarations),
# while the PAYLOAD fields carry explicit `rename = "<camel>"` attrs (check the
# rename literals), and two payload fields are un-renamed snake_case idents.
GROK_ENVELOPE_IDENTS = {"hook_event_name", "session_id", "cwd", "workspace_root"}
GROK_PAYLOAD_RENAMES = {
    "toolName",
    "toolUseId",
    "toolInput",
    "notificationType",
    "subagentId",
    "subagentType",
    "modelId",
}
GROK_PAYLOAD_IDENTS = {"message", "description"}
# Transcript vocabulary decode_grok_line reads from the xAI SessionUpdate enum
# (variant IDENTS — the snake_case tags derive via rename_all, so the wire
# string never appears literally) + its field idents (verbatim snake_case).
GROK_XAI_VARIANTS = {
    "SubagentSpawned",
    "SubagentFinished",
    "ModelChanged",
    "HookExecution",
    "TurnCompleted",
}
GROK_XAI_FIELDS = {
    "subagent_id",
    "child_session_id",
    "model_id",
    "reasoning_effort",
    "event_name",
    "subagent_type",
    "description",
}
# The active_sessions.json registry struct grok_ids_from_registry parses (no
# rename_all — field idents ARE the wire names). A rename silently degrades the
# whole liveness ladder (probe → instant exit → negative vouch) to mtime gating.
GROK_ACTIVE_SESSION_FIELDS = {"session_id", "pid", "cwd", "opened_at"}

# Oh My Pi (omp) is TRANSCRIPT-ONLY: the decoder tails the session JSONL, so the
# depended names are the entry `type` discriminators (source/omp.rs `match kind`)
# plus a handful of field literals, split across the upstream files that define
# them (open TS, can1357/oh-my-pi). ONE-DIRECTIONAL like copilot: omp persists
# ~15 entry types and we map 3 by design — only a name WE DEPEND ON vanishing is
# breaking (a rename → the transcript still flows but decodes to nothing).
OMP_SESSION_ENTRIES_URL = (
    "https://raw.githubusercontent.com/can1357/oh-my-pi/main/packages/coding-agent/src/session/session-entries.ts"
)
# Field names defined in session-entries.ts: the header `cwd` (the label/first-
# sight identity) + the `customType` discriminator decode_omp_line keys custom
# entries on — checked as TS property keys (`cwd: string`), not bare words. The
# entry `type` values from read_omp_entry_types are checked against the SAME
# file as QUOTED literals (`type: "message"`): these are generic English words,
# so a \b word match would survive an upstream rename on any stray prose use.
OMP_SESSION_ENTRY_FIELDS = {"cwd", "customType"}
# The clean-teardown marker (SESSION_EXIT_CUSTOM_TYPE) lives in exit-diagnostics.ts
# — the session-ended checker + the SessionEnd decode both key on it.
OMP_EXIT_DIAG_URL = (
    "https://raw.githubusercontent.com/can1357/oh-my-pi/main/packages/coding-agent/src/session/exit-diagnostics.ts"
)
# The message-level names (roles + tool-call block shape) live in the pi-ai LLM
# types: `role:"assistant"`/`"toolResult"`, the `"toolCall"` content-block type
# (quoted TS literals), plus the result's `toolCallId` back-reference, the
# call's `arguments`, and the assistant message's bare `model` (the burn-tier
# carrier, #545 — AssistantMessage requires it) as property keys.
OMP_AI_TYPES_URL = (
    "https://raw.githubusercontent.com/can1357/oh-my-pi/main/packages/ai/src/types.ts"
)
OMP_MESSAGE_LITERALS = {"assistant", "toolResult", "toolCall"}
OMP_MESSAGE_FIELDS = {"toolCallId", "arguments", "model"}
# The ask tool (#519): its toolCall NAME is STATE-bearing — decode_omp_line
# maps an assistant `ask` block to Waiting — and the first question's text
# feeds the Waiting reason. Checked against the tool's own source (`readonly
# name = "ask"` + the arkType schema property keys). `arguments.i` (the
# intent fallback) is the harness-wide tool-call intent key, NOT defined in
# ask.ts — its loss only degrades the reason label, so it is deliberately
# unwatched.
OMP_ASK_URL = (
    "https://raw.githubusercontent.com/can1357/oh-my-pi/main/packages/coding-agent/src/tools/ask.ts"
)
OMP_ASK_FIELDS = {"questions", "question"}


def fetch(url: str) -> str:
    req = urllib.request.Request(url, headers={"User-Agent": "pixtuoid-drift-watch"})
    with urllib.request.urlopen(req, timeout=30) as resp:  # noqa: S310 (trusted hosts)
        return resp.read().decode("utf-8", "replace")


def try_fetch(
    url: str, label: str, breaking: list[str], errors: list[str]
) -> str | None:
    """Fetch `url`, classifying failures so a PERMANENT upstream move is loud.

    A `PERMANENT_HTTP_STATUS` (404/410/451) means our pinned URL is wrong/gone →
    `breaking` (the watch is blind for this source until the `*_URL` constant is
    fixed). 403/429/5xx + connect/read timeouts → `errors` (transient). Returns
    the body, or None on any failure (the caller skips that source's checks).
    Centralizes the try/except every fetch site repeated, AND fixes the
    `HTTPError ⊂ URLError ⊂ FETCH_ERRORS` swallow that bucketed 404 as transient.
    """
    try:
        return fetch(url)
    except urllib.error.HTTPError as e:
        if e.code in PERMANENT_HTTP_STATUS:
            breaking.append(
                f"{label}: HTTP {e.code} at {url} — the upstream URL moved or was "
                f"removed; drift-watch is BLIND for this source until the *_URL "
                f"constant / parser is updated."
            )
        else:
            errors.append(f"{label}: transient HTTP {e.code} at {url}: {e}")
        return None
    except FETCH_ERRORS as e:
        errors.append(f"{label}: fetch failed (transient?): {e}")
        return None


def read_codex_events() -> set[str]:
    src = (REPO / "crates/pixtuoid/src/install/codex.rs").read_text()
    m = re.search(r"const CODEX_EVENTS[^=]*=\s*&\[(.*?)\];", src, re.S)
    if not m:
        raise RuntimeError("could not locate CODEX_EVENTS in install/codex.rs")
    return set(re.findall(r'"(\w+)"', m.group(1)))


def read_codex_rollout_types() -> tuple[set[str], set[str]]:
    """The (event_msg, response_item) inner `type` strings the codex TRANSCRIPT
    decoder matches on (`source/codex.rs` `match (outer, inner)`). Unlike the hook
    events these are registered NOWHERE, and the decoder's `_ => vec![]` drops an
    unrecognized one SILENTLY (no `unknown_event` breadcrumb, unlike the hook
    decoders) — so a positive "each depended type still exists upstream" check is
    the only backstop against an upstream rename going dark."""
    src = (REPO / "crates/pixtuoid-core/src/source/codex.rs").read_text()
    event_msg = set(re.findall(r'\(\s*"event_msg"\s*,\s*"(\w+)"\s*\)', src))
    response_item = set(re.findall(r'\(\s*"response_item"\s*,\s*"(\w+)"\s*\)', src))
    if not event_msg or not response_item:
        raise RuntimeError(
            "could not locate codex ('event_msg'|'response_item', …) decode arms "
            "in source/codex.rs — the transcript decoder was refactored; update "
            "the parser."
        )
    return event_msg, response_item


def read_cc_events() -> set[str]:
    src = (REPO / "crates/pixtuoid/src/install/claude.rs").read_text()
    m = re.search(r"const EVENTS[^=]*=\s*&\[(.*?)\];", src, re.S)
    if not m:
        raise RuntimeError("could not locate EVENTS in install/claude.rs")
    return set(re.findall(r'"(\w+)"', m.group(1)))


def read_dispatch_names() -> set[str]:
    src = (REPO / "crates/pixtuoid-core/src/source/decoder.rs").read_text()
    m = re.search(r"known_name\s*=\s*([^;]+);", src)
    if not m:
        raise RuntimeError("could not locate the dispatch known_name check in decoder.rs")
    return set(re.findall(r'"(\w+)"', m.group(1)))


def upstream_codex_hooks(text: str) -> set[str] | None:
    m = re.search(r"enum HookEventName\s*\{(.*?)\}", text, re.S)
    if not m:
        return None
    # variant identifiers (drop comments/attrs by keeping CamelCase words)
    return set(re.findall(r"\b([A-Z][A-Za-z]+)\b", m.group(1)))


def _snake_case(camel: str) -> str:
    return re.sub(r"(?<!^)(?=[A-Z])", "_", camel).lower()


def _enum_body(text: str, enum_name: str) -> str | None:
    """The brace-balanced body of `enum <enum_name> { … }` (the `?` non-greedy
    regexes elsewhere stop at the FIRST `}`, which a struct-variant body would
    truncate — so balance explicitly)."""
    m = re.search(rf"enum\s+{enum_name}\s*\{{", text)
    if not m:
        return None
    start = m.end() - 1  # index of the opening `{`
    depth = 0
    for i in range(start, len(text)):
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
            if depth == 0:
                return text[start + 1 : i]
    return None


def _strip_nested(s: str) -> str:
    """Remove line/doc comments then iteratively strip innermost `(…)`/`{…}`
    (tuple params, struct-variant bodies, AND attr parens) so only top-level
    variant idents survive — else a CamelCase field/param TYPE reads as a variant."""
    s = re.sub(r"//[^\n]*", "", s)
    prev = None
    while prev != s:
        prev = s
        s = re.sub(r"\([^()]*\)", "", s)
        s = re.sub(r"\{[^{}]*\}", "", s)
    return s


def upstream_codex_enum_types(text: str, enum_name: str) -> set[str] | None:
    """Serialized `type` tags of a codex `#[serde(tag="type", rename_all="snake_case")]`
    enum (`EventMsg` in protocol.rs, `ResponseItem` in models.rs). Each variant
    contributes snake_case(name), plus every explicit `#[serde(rename="…")]` /
    `alias="…"` literal. This over-includes (a renamed variant keeps its
    snake_case form too), which is HARMLESS: the check is one-directional — it
    only confirms a DEPENDED type is still present, never that a name is absent.
    Returns None if the enum can't be located (→ a loud "upstream moved it")."""
    body = _enum_body(text, enum_name)
    if body is None:
        return None
    # rename/alias literals must be read BEFORE `_strip_nested` eats the attr parens.
    names = set(re.findall(r'(?:rename|alias)\s*=\s*"([^"]+)"', re.sub(r"//[^\n]*", "", body)))
    names.update(_snake_case(v) for v in re.findall(r"\b([A-Z][A-Za-z0-9]*)\b", _strip_nested(body)))
    return names or None


def codex_function_call_fields(text: str) -> set[str] | None:
    """The field idents of the INLINE `ResponseItem::FunctionCall { … }` variant
    (models.rs). Returns None if it isn't an inline struct — a GRACEFUL SKIP, not
    an alarm: a tuple-variant refactor (`FunctionCall(FunctionCallItem)`)
    serializes the SAME JSON, so the decoder's `.get("name"/"arguments")` still
    works; only this bonus field check goes quiet (the type-existence check above
    still covers `function_call`). Selftested so OUR regex breaking is caught."""
    m = re.search(r"FunctionCall\s*\{([^}]*)\}", text)
    if not m:
        return None
    return set(re.findall(r"\b([a-z_][a-z0-9_]*)\s*:", m.group(1)))


def codex_turn_context_fields(text: str) -> set[str] | None:
    """The field idents of the `TurnContextItem` struct (protocol.rs) — the
    burn-tier feature reads `model` + `effort` off every `turn_context`
    rollout line (source/codex.rs). Same graceful-skip contract as
    `codex_function_call_fields`: None = the struct moved/changed shape, the
    caller alarms on that separately."""
    m = re.search(r"pub struct TurnContextItem\s*\{([^}]*)\}", text)
    if not m:
        return None
    return set(re.findall(r"\bpub ([a-z_][a-z0-9_]*)\s*:", m.group(1)))


def upstream_cc_hook_events(text: str) -> set[str] | None:
    """The hook-event summary table near the top of hooks.md ("| Event | When
    it fires |") is the canonical event list — parse only its rows (other
    tables in the doc repeat event names with different columns)."""
    m = re.search(r"^\|\s*Event\s*\|[^\n]*\n\|[\s:|-]*\n((?:\|[^\n]*\n)+)", text, re.M)
    if not m:
        return None
    return set(re.findall(r"^\|\s*`(\w+)`\s*\|", m.group(1), re.M)) or None


def read_reasonix_events() -> set[str]:
    src = (REPO / "crates/pixtuoid/src/install/reasonix.rs").read_text()
    m = re.search(r"const REASONIX_EVENTS[^=]*=\s*&\[(.*?)\];", src, re.S)
    if not m:
        raise RuntimeError("could not locate REASONIX_EVENTS in install/reasonix.rs")
    return set(re.findall(r'"(\w+)"', m.group(1)))


def upstream_reasonix_hooks(text: str) -> set[str] | None:
    # Go consts: `PreToolUse Event = "PreToolUse"` — take the string values.
    found = set(re.findall(r'\w+\s+Event\s*=\s*"(\w+)"', text))
    return found or None


def read_codewhale_events() -> set[str]:
    src = (REPO / "crates/pixtuoid/src/install/codewhale.rs").read_text()
    m = re.search(r"const CODEWHALE_EVENTS[^=]*=\s*&\[(.*?)\];", src, re.S)
    if not m:
        raise RuntimeError("could not locate CODEWHALE_EVENTS in install/codewhale.rs")
    return set(re.findall(r'"(\w+)"', m.group(1)))


def read_opencode_events() -> set[str]:
    """The EventV2 `type` strings the decoder maps, read from the `match event`
    block in source/opencode.rs (the source of truth — stays in sync with the
    decoder by construction)."""
    src = (REPO / "crates/pixtuoid-core/src/source/opencode.rs").read_text()
    m = re.search(r"match event \{(.*?)\n    \}", src, re.S)
    if not m:
        raise RuntimeError("could not locate the `match event` block in source/opencode.rs")
    return set(re.findall(r'"((?:session|message|permission)\.[a-z0-9.]+)"', m.group(1)))


def read_omp_entry_types() -> set[str]:
    """The session-entry `type` strings decode_omp_line maps, read from the
    `match kind` block in source/omp.rs (the source of truth — stays in sync
    with the decoder by construction). Scoped to the match block so the unit
    tests further down the file (which embed the same strings as JSON) don't
    leak in."""
    src = (REPO / "crates/pixtuoid-core/src/source/omp.rs").read_text()
    m = re.search(r"let out = match kind \{(.*?)\n    \};", src, re.S)
    if not m:
        raise RuntimeError("could not locate the `match kind` block in source/omp.rs")
    # Arm-position capture (a line-leading quoted pattern, `"session" => {` or a
    # guarded `"custom"` on its own line) — a 4th decode arm is picked up
    # automatically, and arm-BODY literals (`"toolCall"`, `"session_exit"`) that
    # belong to the other two upstream checks never leak in.
    return set(re.findall(r'(?m)^\s*"(\w+)"\s*(?:=>|if\b|$)', m.group(1)))


def read_copilot_events() -> set[str]:
    """The event `type` strings the decoder maps, read from the `match kind`
    block in source/copilot.rs (the source of truth — stays in sync with the
    decoder by construction). Scoped to the match block so the test fixtures
    further down the file (which embed the same strings as JSON) don't leak in."""
    src = (REPO / "crates/pixtuoid-core/src/source/copilot.rs").read_text()
    m = re.search(r"let out = match kind \{(.*?)\n    \};", src, re.S)
    if not m:
        raise RuntimeError("could not locate the `match kind` block in source/copilot.rs")
    return set(re.findall(r'"((?:session|tool|subagent|permission)\.[a-z._]+)"', m.group(1)))


def read_cursor_events() -> set[str]:
    """The camelCase hook events we register/decode, read from the explicit
    `CURSOR_EVENTS` const in install/cursor.rs — the same registered list the
    `every_registered_cursor_event_decodes` test pins, and a leak-free source of
    truth (mirrors read_reasonix_events / read_codewhale_events). Reading the
    decoder's `match event` block instead would risk a future camelCase field
    lookup in an arm leaking a phantom event into the drift set."""
    src = (REPO / "crates/pixtuoid/src/install/cursor.rs").read_text()
    m = re.search(r"const CURSOR_EVENTS[^=]*=\s*&\[(.*?)\];", src, re.S)
    if not m:
        raise RuntimeError("could not locate CURSOR_EVENTS in install/cursor.rs")
    return set(re.findall(r'"(\w+)"', m.group(1)))


def read_openclaw_events() -> set[str]:
    """The OpenClaw gateway hook events we register/decode, read from the
    `OPENCLAW_EVENTS` const in install/openclaw.rs — the SAME list the plugin
    HOOKS array and the decoder arms are pinned to by
    `openclaw_events_plugin_decoder_and_const_agree`, so this is a leak-free
    source of truth (mirrors read_cursor_events / read_codewhale_events)."""
    src = (REPO / "crates/pixtuoid/src/install/openclaw.rs").read_text()
    m = re.search(r"const OPENCLAW_EVENTS[^=]*=\s*&\[(.*?)\];", src, re.S)
    if not m:
        raise RuntimeError("could not locate OPENCLAW_EVENTS in install/openclaw.rs")
    return set(re.findall(r'"(\w+)"', m.group(1)))


def read_hermes_events() -> set[str]:
    """The Hermes shell-hook events we register/decode, read from the
    `HERMES_EVENTS` const in install/hermes.rs — pinned to the decoder arms by
    `every_registered_hermes_event_decodes` (install/hermes.rs), so this is a
    leak-free source of truth (mirrors read_cursor_events / read_openclaw_events)."""
    src = (REPO / "crates/pixtuoid/src/install/hermes.rs").read_text()
    m = re.search(r"const HERMES_EVENTS[^=]*=\s*&\[(.*?)\];", src, re.S)
    if not m:
        raise RuntimeError("could not locate HERMES_EVENTS in install/hermes.rs")
    return set(re.findall(r'"(\w+)"', m.group(1)))


def read_grok_events() -> set[str]:
    """The grok hook events we register/decode, read from the `GROK_EVENTS`
    const in install/grok.rs — pinned to the decoder arms by
    `every_registered_grok_event_decodes`, so this is a leak-free source of
    truth (mirrors read_cursor_events / read_hermes_events)."""
    src = (REPO / "crates/pixtuoid/src/install/grok.rs").read_text()
    m = re.search(r"const GROK_EVENTS[^=]*=\s*&\[(.*?)\];", src, re.S)
    if not m:
        raise RuntimeError("could not locate GROK_EVENTS in install/grok.rs")
    return set(re.findall(r'"(\w+)"', m.group(1)))


def read_kimi_events() -> set[str]:
    """The PascalCase hook events we register/decode, read from the `KIMI_EVENTS`
    const in install/kimi.rs — the SAME registered list the
    `every_registered_kimi_event_decodes` test pins to the decode path (the shared
    CC-shaped arms + the source's custom Extend decoder), so this is a leak-free
    source of truth (mirrors read_cursor_events / read_hermes_events)."""
    src = (REPO / "crates/pixtuoid/src/install/kimi.rs").read_text()
    m = re.search(r"const KIMI_EVENTS[^=]*=\s*&\[(.*?)\];", src, re.S)
    if not m:
        raise RuntimeError("could not locate KIMI_EVENTS in install/kimi.rs")
    return set(re.findall(r'"(\w+)"', m.group(1)))


def upstream_grok_hooks(text: str) -> set[str] | None:
    """The HookEventName enum variants (bare Rust idents — registration keys
    accept the PascalCase spelling, so these ARE the names we register)."""
    m = re.search(r"pub enum HookEventName \{(.*?)\n\}", text, re.S)
    if not m:
        return None
    found = set(re.findall(r"(?m)^\s*([A-Z]\w+),", m.group(1)))
    return found or None


def upstream_copilot_events(text: str) -> set[str] | None:
    """The per-event `type` consts from the @github/copilot session-events JSON
    schema. Each event is a `definitions.<Name>` object whose `properties.type`
    pins the wire string as a `const` (or a single-element `enum`)."""
    try:
        defs = json.loads(text).get("definitions", {})
    except (json.JSONDecodeError, AttributeError):
        return None
    consts: set[str] = set()
    for sch in defs.values():
        if not isinstance(sch, dict):
            continue
        t = sch.get("properties", {}).get("type")
        if not isinstance(t, dict):
            continue
        c = t.get("const")
        if c is None:
            enum = t.get("enum")
            if isinstance(enum, list) and len(enum) == 1:
                c = enum[0]
        if isinstance(c, str):
            consts.add(c)
    return consts or None


def upstream_copilot_field_names(text: str) -> set[str] | None:
    """The union of every `properties` key at ANY depth in the @github/copilot
    session-events schema — the envelope fields (agentId/sessionId) AND the
    nested `data.properties` fields (toolCallId/toolName/arguments/…). Used
    one-directional: a field the decoder READS that is absent from the whole
    schema is a rename. Returns None if the JSON won't parse (→ loud breaking)."""
    try:
        root = json.loads(text)
    except json.JSONDecodeError:
        return None
    names: set[str] = set()

    def walk(node: object) -> None:
        if isinstance(node, dict):
            props = node.get("properties")
            if isinstance(props, dict):
                names.update(k for k in props if isinstance(k, str))
            for v in node.values():
                walk(v)
        elif isinstance(node, list):
            for v in node:
                walk(v)

    walk(root)
    return names or None


def upstream_codewhale_hooks(text: str) -> set[str] | None:
    # The TUI shell-command hook enum `pub enum HookEvent { SessionStart, ... }`
    # in crates/tui/src/hooks.rs (NOT the app-server `codewhale-hooks` sink enum
    # in crates/hooks). serde `rename_all = "snake_case"`, so convert each
    # CamelCase variant to the wire name we register. Variant lines are bare
    # `Identifier,` — doc comments (`///`) start with `/` and are skipped.
    m = re.search(r"pub enum HookEvent\s*\{(.*?)\}", text, re.S)
    if not m:
        return None
    variants = re.findall(r"^\s*([A-Z][A-Za-z0-9]+)\s*,", m.group(1), re.M)
    snake = {re.sub(r"(?<!^)(?=[A-Z])", "_", v).lower() for v in variants}
    return snake or None


def cc_doc_marker_findings(hooks_doc: str) -> list[str]:
    # Both marker directions over an already-fetched hooks.md text, as a pure
    # function so the selftest can exercise the DETECTION (not just the
    # parsers): depended markers alarm on VANISH, surface markers on
    # APPEARANCE.
    review: list[str] = []
    for marker, what in sorted(CC_DEPENDED_DOC_MARKERS.items()):
        if marker not in hooks_doc:
            review.append(
                f"CC hooks.md no longer mentions `{marker}` — {what} may have "
                f"moved/renamed; re-verify the burn-tier hook decode."
            )
    for marker, what in sorted(CC_LIFECYCLE_SURFACE_MARKERS.items()):
        if marker in hooks_doc:
            review.append(
                f"CC hooks doc now mentions `{marker}` — {what} may have "
                f"landed upstream. Adopt it (a durable end signal for the "
                f"JSONL transport / the liveness-probe registry) and "
                f"update this watch."
            )
    return review


def run_checks(
    codex_ours: set[str] | None,
    codex_rollout: tuple[set[str], set[str]] | None,
    cc_ours: set[str] | None,
    dispatch_names: set[str] | None,
    reasonix_ours: set[str] | None,
    codewhale_ours: set[str] | None,
    opencode_ours: set[str] | None,
    copilot_ours: set[str] | None,
    omp_ours: set[str] | None,
    cursor_ours: set[str] | None,
    openclaw_ours: set[str] | None,
    hermes_ours: set[str] | None,
    grok_ours: set[str] | None,
    kimi_ours: set[str] | None,
    breaking: list[str],
    review: list[str],
    errors: list[str],
) -> None:
    """The upstream comparisons. Split from main() so an UNEXPECTED exception
    here (a script bug, an exotic network failure outside FETCH_ERRORS) can be
    routed to the transient bucket with the partial report intact — without it
    the interpreter exits 1 and the workflow files a junk "confirmed drift"
    issue from an empty report. The deliberate read-our-own-source LOUD path
    stays inside main(), before this is called, and still exits 1."""
    # --- Codex hook events + rollout decode vocabulary (only the FETCH is
    #     transient). protocol.rs holds BOTH the HookEventName enum (hooks) and
    #     the EventMsg enum (rollout `event_msg` types); the `response_item` types
    #     live in the sibling models.rs (ResponseItem). ------------------------
    if codex_ours is not None or codex_rollout is not None:
        text = try_fetch(CODEX_PROTOCOL_URL, "Codex source", breaking, errors)
        if text is not None and codex_ours is not None:
            upstream = upstream_codex_hooks(text)
            if upstream is None:
                breaking.append(
                    "Codex `HookEventName` enum not found at the pinned path "
                    "(codex-rs/protocol/src/protocol.rs) — upstream moved it; "
                    "update CODEX_PROTOCOL_URL / the parser."
                )
            else:
                for ev in sorted(codex_ours):
                    if ev not in upstream:
                        breaking.append(
                            f"Codex hook `{ev}` (registered in CODEX_EVENTS) is GONE "
                            f"from upstream HookEventName — likely renamed; the "
                            f"decoder will silently drop it."
                        )
                for ev in sorted(upstream - codex_ours - CODEX_KNOWN_OMITTED):
                    review.append(
                        f"new Codex hook `{ev}` upstream — we neither register nor "
                        f"intentionally omit it (add a decoder arm + CODEX_EVENTS, "
                        f"or add it to CODEX_KNOWN_OMITTED)."
                    )
        # Rollout `event_msg` types → the EventMsg enum in the SAME protocol.rs.
        # ONE-DIRECTIONAL: codex emits many EventMsg/ResponseItem types we ignore,
        # so only a VANISHED depended type alarms (a new one is not a ping). This
        # is the ONLY backstop — the transcript decoder's `_ => vec![]` drops an
        # unknown type silently, with no `unknown_event` breadcrumb.
        if text is not None and codex_rollout is not None:
            event_msg_ours, _ = codex_rollout
            up_ev = upstream_codex_enum_types(text, "EventMsg")
            if up_ev is None:
                breaking.append(
                    "Codex `EventMsg` enum not found in protocol.rs — upstream "
                    "moved it; update the parser."
                )
            else:
                for t in sorted(event_msg_ours):
                    if t not in up_ev:
                        breaking.append(
                            f"Codex rollout event_msg `{t}` (decoded in "
                            f"source/codex.rs) is GONE from upstream `EventMsg` — "
                            f"renamed; the transcript decoder drops it SILENTLY "
                            f"(`_ => vec![]`, no drift breadcrumb)."
                        )
        # `turn_context` FIELD survival (burn tier, #541): the transcript
        # decoder reads `model` + `effort` off every turn_context line
        # (source/codex.rs) — a rename silently kills the model badge/flame
        # (fail-quiet by design, so this watch is the only alarm).
        if text is not None:
            tc_fields = codex_turn_context_fields(text)
            if tc_fields is None:
                breaking.append(
                    "Codex `TurnContextItem` struct not found in protocol.rs — "
                    "upstream moved it; update the parser (burn-tier model/effort "
                    "watch is blind)."
                )
            else:
                for f in ("model", "effort"):
                    if f not in tc_fields:
                        breaking.append(
                            f"Codex turn_context field `{f}` is GONE from "
                            f"TurnContextItem in protocol.rs — renamed; the "
                            f"burn-tier decoder reads None (model badge/flame "
                            f"silently dark for codex agents)."
                        )
        # Rollout `response_item` types → the ResponseItem enum in models.rs.
        if codex_rollout is not None:
            _, response_item_ours = codex_rollout
            models = try_fetch(CODEX_MODELS_URL, "Codex models", breaking, errors)
            if models is not None:
                up_ri = upstream_codex_enum_types(models, "ResponseItem")
                if up_ri is None:
                    breaking.append(
                        "Codex `ResponseItem` enum not found in models.rs — "
                        "upstream moved it; update CODEX_MODELS_URL / the parser."
                    )
                else:
                    for t in sorted(response_item_ours):
                        if t not in up_ri:
                            breaking.append(
                                f"Codex rollout response_item `{t}` (decoded in "
                                f"source/codex.rs) is GONE from upstream "
                                f"`ResponseItem` — renamed; the transcript decoder "
                                f"drops it SILENTLY."
                            )
                    # FunctionCall FIELD survival: codex_tool_start reads `name`
                    # + `arguments` off a function_call item; a rename → silent
                    # mislabel / the approval gate never fires. Rides `models`.
                    # None = not an inline struct → graceful skip (see the helper).
                    fc_fields = codex_function_call_fields(models)
                    if fc_fields is not None:
                        for f in ("name", "arguments"):
                            if f not in fc_fields:
                                breaking.append(
                                    f"Codex function_call field `{f}` is GONE from "
                                    f"ResponseItem::FunctionCall in models.rs — renamed; "
                                    f"the decoder reads None (mislabels the tool / never "
                                    f"gates on approval)."
                                )

    # --- Reasonix hook events + payload fields (only the FETCH is transient)
    if reasonix_ours is not None:
        text = try_fetch(REASONIX_HOOK_URL, "Reasonix source", breaking, errors)
        if text is not None:
            upstream = upstream_reasonix_hooks(text)
            if upstream is None:
                breaking.append(
                    "Reasonix `Event` consts not found at the pinned path "
                    "(internal/hook/hook.go) — upstream moved it; update "
                    "REASONIX_HOOK_URL / the parser."
                )
            else:
                for ev in sorted(reasonix_ours):
                    if ev not in upstream:
                        breaking.append(
                            f"Reasonix hook `{ev}` (registered in REASONIX_EVENTS) is "
                            f"GONE from upstream hook.go — likely renamed; the decoder "
                            f"will silently drop it."
                        )
                for ev in sorted(upstream - reasonix_ours - REASONIX_KNOWN_OMITTED):
                    review.append(
                        f"new Reasonix hook `{ev}` upstream — we neither register nor "
                        f"intentionally omit it (add a decoder arm + REASONIX_EVENTS, "
                        f"or add it to REASONIX_KNOWN_OMITTED)."
                    )
                for field in sorted(REASONIX_PAYLOAD_FIELDS):
                    if f'json:"{field}' not in text:
                        breaking.append(
                            f"Reasonix payload field `{field}` (read by "
                            f"decode_rx_hook_payload) has no json tag in upstream "
                            f"hook.go — likely renamed; the decode will silently zero."
                        )

    # --- CodeWhale hook events (only the FETCH is transient) ---------------
    if codewhale_ours is not None:
        text = try_fetch(CODEWHALE_HOOK_URL, "CodeWhale source", breaking, errors)
        if text is not None:
            upstream = upstream_codewhale_hooks(text)
            if upstream is None:
                breaking.append(
                    "CodeWhale `pub enum HookEvent` not found at the pinned path "
                    "(crates/tui/src/hooks.rs) — upstream moved it; update "
                    "CODEWHALE_HOOK_URL / the parser."
                )
            else:
                for ev in sorted(codewhale_ours):
                    if ev not in upstream:
                        breaking.append(
                            f"CodeWhale hook `{ev}` (registered in CODEWHALE_EVENTS) is "
                            f"GONE from upstream HookEvent — likely renamed; the decoder "
                            f"will silently drop it."
                        )
                for ev in sorted(upstream - codewhale_ours - CODEWHALE_KNOWN_OMITTED):
                    review.append(
                        f"new CodeWhale hook `{ev}` upstream — we neither register nor "
                        f"intentionally omit it (add a decoder arm + CODEWHALE_EVENTS, "
                        f"or add it to CODEWHALE_KNOWN_OMITTED)."
                    )
            # Env-mode identity fields: the DEEPSEEK_* names CodeWhale sets in
            # `HookContext::to_env_vars` (same hooks.rs). ONE-DIRECTIONAL.
            for field in sorted(CODEWHALE_ENV_FIELDS):
                if f'"{field}"' not in text:
                    breaking.append(
                        f"CodeWhale env var `{field}` (folded by the shim's env-mode "
                        f"into the {{cwd,tool,tool_args}} envelope) is GONE from "
                        f"hooks.rs `to_env_vars` — renamed; the shim reads None, the "
                        f"envelope omits its field, and the cwd-keyed decoder drops "
                        f"the event (empty cwd = no sprite / no activity)."
                    )

    # --- opencode EventV2 types (only the FETCH is transient) --------------
    if opencode_ours is not None:
        parts = [
            try_fetch(u, "opencode source", breaking, errors)
            for u in OPENCODE_EVENT_URLS
        ]
        # If ANY url failed, skip the check — a partial concat would
        # false-positive a depended type as "GONE" just because it lived in the
        # half we couldn't fetch. (try_fetch already classified each failure.)
        text = "\n".join(parts) if all(p is not None for p in parts) else None
        if text is not None:
            for ev in sorted(opencode_ours - OPENCODE_TOLERATED):
                # The type strings appear as `type: "session.created"` etc. in
                # the EventV2.define / Schema.Literal definitions.
                if f'"{ev}"' not in text:
                    breaking.append(
                        f"opencode event `{ev}` (decoded in source/opencode.rs) is GONE "
                        f"from upstream — likely renamed; the plugin still forwards it but "
                        f"the decoder maps it to nothing (no sprite / no activity)."
                    )
            # Payload FIELD names — each a `field:` property line in the schema
            # Struct defs. ONE-DIRECTIONAL (a depended field vanishing alarms).
            for field in sorted(OPENCODE_PAYLOAD_FIELDS):
                if not re.search(rf"(?m)^\s*{re.escape(field)}:", text):
                    breaking.append(
                        f"opencode field `{field}` (read by source/opencode.rs) is GONE "
                        f"from the schema Struct defs — likely renamed; the plugin still "
                        f"forwards the event but the decoder reads None (wrong-register / "
                        f"no-link / no-activity)."
                    )

    # --- grok hook events + payload/transcript/registry names (FETCH transient)
    if grok_ours is not None:
        text = try_fetch(GROK_HOOK_URL, "grok hooks source", breaking, errors)
        if text is not None:
            upstream = upstream_grok_hooks(text)
            if upstream is None:
                breaking.append(
                    "grok `pub enum HookEventName` not found at the pinned path "
                    "(xai-grok-hooks/src/event.rs) — upstream moved it; update "
                    "GROK_HOOK_URL / the parser."
                )
            else:
                for ev in sorted(grok_ours):
                    if ev not in upstream:
                        breaking.append(
                            f"grok hook `{ev}` (registered in GROK_EVENTS) is GONE from "
                            f"upstream event.rs — likely renamed; the registered key "
                            f"stops matching and that event silently never fires."
                        )
                for ev in sorted(upstream - grok_ours - GROK_KNOWN_OMITTED):
                    review.append(
                        f"new grok hook `{ev}` upstream — we neither register nor "
                        f"intentionally omit it (add a decoder arm + GROK_EVENTS, or "
                        f"add it to GROK_KNOWN_OMITTED)."
                    )
            for ident in sorted(GROK_ENVELOPE_IDENTS):
                if not re.search(rf"(?m)^\s*pub {ident}:", text):
                    breaking.append(
                        f"grok envelope field `{ident}` is GONE from HookEventEnvelope "
                        f"in event.rs — renamed; its camelCase wire name shifts and "
                        f"decode_grok_hook_payload reads None (no key / no cwd)."
                    )
            for rename in sorted(GROK_PAYLOAD_RENAMES):
                if f'rename = "{rename}"' not in text:
                    breaking.append(
                        f"grok payload rename `{rename}` is GONE from event.rs — the "
                        f"wire field decode_grok_hook_payload reads was renamed; the "
                        f"decode silently zeroes (no tool label / no child key)."
                    )
            for ident in sorted(GROK_PAYLOAD_IDENTS):
                if not re.search(rf"(?m)^\s*{ident}:", text):
                    breaking.append(
                        f"grok payload field `{ident}` is GONE from event.rs — renamed; "
                        f"the decoder's fallback reads None (Waiting reason / child "
                        f"label degrade)."
                    )
        text = try_fetch(GROK_NOTIFICATION_URL, "grok notification source", breaking, errors)
        if text is not None:
            for variant in sorted(GROK_XAI_VARIANTS):
                if not re.search(rf"(?m)^\s*{variant}\b", text):
                    breaking.append(
                        f"grok xAI update variant `{variant}` is GONE from "
                        f"extensions/notification.rs — its snake_case sessionUpdate tag "
                        f"shifts and decode_grok_line maps the line to nothing "
                        f"(no subagent link / no model info / no end marker)."
                    )
            for ident in sorted(GROK_XAI_FIELDS):
                if not re.search(rf"(?m)^\s*(pub )?{ident}:", text):
                    breaking.append(
                        f"grok xAI update field `{ident}` is GONE from "
                        f"extensions/notification.rs — renamed; decode_grok_line reads "
                        f"None (child un-keyed / model dropped / end marker missed)."
                    )
        text = try_fetch(GROK_ACTIVE_SESSIONS_URL, "grok active-sessions source", breaking, errors)
        if text is not None:
            for ident in sorted(GROK_ACTIVE_SESSION_FIELDS):
                if not re.search(rf"(?m)^\s*(pub )?{ident}:", text):
                    breaking.append(
                        f"grok active_sessions field `{ident}` is GONE from "
                        f"active_sessions.rs — renamed; grok_ids_from_registry stops "
                        f"parsing and the WHOLE liveness ladder (probe / instant exit / "
                        f"negative vouch / focus) degrades to mtime gating."
                    )

    # --- Copilot event types (only the FETCH is transient) -----------------
    if copilot_ours is not None:
        text = try_fetch(COPILOT_SCHEMA_URL, "Copilot schema", breaking, errors)
        if text is not None:
            upstream = upstream_copilot_events(text)
            if upstream is None:
                breaking.append(
                    "Copilot session-events schema has no parseable `type` consts "
                    "(definitions empty / shape changed) — upstream restructured the "
                    "schema; update COPILOT_SCHEMA_URL / upstream_copilot_events."
                )
            else:
                for ev in sorted(copilot_ours):
                    if ev not in upstream:
                        breaking.append(
                            f"Copilot event `{ev}` (decoded in source/copilot.rs) is GONE "
                            f"from the @github/copilot schema — likely renamed; the "
                            f"transcript still carries it but the decoder maps it to "
                            f"nothing (no sprite / no activity)."
                        )
            # Payload FIELD names — the union of every `properties` key (envelope
            # + nested data.*). ONE-DIRECTIONAL (a depended field vanishing alarms).
            fields_up = upstream_copilot_field_names(text)
            if fields_up is None:
                breaking.append(
                    "Copilot schema won't parse for field names — upstream "
                    "restructured it; update upstream_copilot_field_names."
                )
            else:
                for field in sorted(COPILOT_PAYLOAD_FIELDS):
                    if field not in fields_up:
                        breaking.append(
                            f"Copilot field `{field}` (read by decode_copilot_line / "
                            f"extract_copilot_cwd) is GONE from the schema properties — "
                            f"renamed; the decoder reads None (wrong-register / no-link / "
                            f"no tool label / permission never gates)."
                        )

    # --- omp session-entry types + wire names (only the FETCH is transient) --
    if omp_ours is not None:
        text = try_fetch(OMP_SESSION_ENTRIES_URL, "omp session-entries", breaking, errors)
        if text is not None:
            # Entry `type` discriminators are QUOTED TS literal types
            # (`type: "message"`); the names are generic English words, so a
            # bare \b match would stay green on prose/comment uses after an
            # upstream rename — quote-anchored on purpose.
            for name in sorted(omp_ours):
                if f'"{name}"' not in text:
                    breaking.append(
                        f"omp entry type `{name}` (decoded in source/omp.rs) is GONE "
                        f"from session-entries.ts — likely renamed; the transcript "
                        f"still flows but the decoder maps it to nothing (no sprite "
                        f"/ no activity)."
                    )
            # Field names appear as TS property keys (`cwd: string`).
            for field in sorted(OMP_SESSION_ENTRY_FIELDS):
                if not re.search(rf"(?m)^\s*(?:readonly\s+)?{re.escape(field)}\??\s*:", text):
                    breaking.append(
                        f"omp field `{field}` (read by decode_omp_line) is GONE from "
                        f"session-entries.ts property keys — renamed; the decoder "
                        f"reads None (no cwd label / no session_exit end)."
                    )
        diag = try_fetch(OMP_EXIT_DIAG_URL, "omp exit-diagnostics", breaking, errors)
        if diag is not None and '"session_exit"' not in diag:
            breaking.append(
                "omp customType `session_exit` (the clean-teardown marker the "
                "session-ended checker + SessionEnd decode key on) is GONE from "
                "exit-diagnostics.ts — renamed; finished sessions resurrect at "
                "first sight and never SessionEnd."
            )
        ai = try_fetch(OMP_AI_TYPES_URL, "omp pi-ai types", breaking, errors)
        if ai is not None:
            for name in sorted(OMP_MESSAGE_LITERALS):
                if f'"{name}"' not in ai:
                    breaking.append(
                        f"omp message literal `{name}` (read by decode_omp_line) is "
                        f"GONE from pi-ai types.ts — renamed; tool rounds decode to "
                        f"nothing."
                    )
            for field in sorted(OMP_MESSAGE_FIELDS):
                if not re.search(rf"(?m)^\s*(?:readonly\s+)?{re.escape(field)}\??\s*:", ai):
                    breaking.append(
                        f"omp message field `{field}` (read by decode_omp_line) is "
                        f"GONE from pi-ai types.ts property keys — renamed; tool "
                        f"rounds lose their key/target."
                    )
        ask = try_fetch(OMP_ASK_URL, "omp ask tool", breaking, errors)
        if ask is not None:
            if '"ask"' not in ask:
                breaking.append(
                    "omp tool name `ask` (drives the ask→Waiting decode) is GONE "
                    "from tools/ask.ts — renamed; a session parked on a user "
                    "question renders active instead of waiting."
                )
            for field in sorted(OMP_ASK_FIELDS):
                if not re.search(rf"(?m)^\s*(?:readonly\s+)?{re.escape(field)}\??\s*:", ask):
                    breaking.append(
                        f"omp ask field `{field}` (feeds the Waiting reason) is GONE "
                        f"from tools/ask.ts property keys — renamed; the Waiting "
                        f"reason degrades to the intent/bare-name fallback."
                    )

    # --- Cursor hook events (only the FETCH is transient) ------------------
    if cursor_ours is not None:
        text = try_fetch(CURSOR_HOOKS_URL, "Cursor hooks doc", breaking, errors)
        if text is not None:
            for ev in sorted(cursor_ours):
                # Word-boundary token match (the docs render the names inline /
                # in tables, not as quoted literals). ONE-DIRECTIONAL: a depended
                # event missing from the page is breaking; a new upstream event
                # is intentionally ignored (we map ~5 of ~18 by design).
                if not re.search(rf"\b{re.escape(ev)}\b", text):
                    breaking.append(
                        f"Cursor hook `{ev}` (decoded in source/cursor.rs) is GONE from "
                        f"cursor.com/docs/hooks — likely renamed; the CLI still fires it but "
                        f"the decoder maps it to nothing (no sprite / no activity)."
                    )

    # --- OpenClaw gateway hook events (only the FETCH is transient) ---------
    if openclaw_ours is not None:
        text = try_fetch(OPENCLAW_HOOK_TYPES_URL, "OpenClaw hook-types", breaking, errors)
        if text is not None:
            for ev in sorted(openclaw_ours):
                # The union lists each hook as a quoted string literal
                # (`| "before_agent_run"` / `"before_agent_run",`). ONE-DIRECTIONAL:
                # a registered event missing upstream is breaking; new upstream
                # hooks are ignored (we register 6 of ~40 by design).
                if f'"{ev}"' not in text:
                    breaking.append(
                        f"OpenClaw hook `{ev}` (registered in OPENCLAW_EVENTS / the TS "
                        f"plugin) is GONE from src/plugins/hook-types.ts — likely renamed; "
                        f"the plugin registers a hook OpenClaw never fires, so the lobster "
                        f"mascot silently stops reacting (no presence)."
                    )
            # Payload FIELD names read by decode_openclaw_presence. ONE-DIRECTIONAL.
            for field in sorted(OPENCLAW_PAYLOAD_FIELDS):
                if not re.search(rf"\b{re.escape(field)}\b", text):
                    breaking.append(
                        f"OpenClaw field `{field}` (read by decode_openclaw_presence) is "
                        f"GONE from src/plugins/hook-types.ts — renamed; the decoder reads "
                        f"None (wrong run-key / no Degraded gate / no presence)."
                    )

    # --- Hermes shell-hook events + payload fields (only the FETCH is transient)
    if hermes_ours is not None:
        text = try_fetch(HERMES_HOOK_URL, "Hermes hooks", breaking, errors)
        if text is not None:
            for ev in sorted(hermes_ours):
                # `_DEFAULT_PAYLOADS` lists each event as a quoted dict key
                # (`"on_session_start":`). ONE-DIRECTIONAL: a registered event
                # missing upstream is breaking; new upstream events are ignored
                # (we register 4 of ~15 by design).
                if f'"{ev}"' not in text:
                    breaking.append(
                        f"Hermes hook `{ev}` (registered in HERMES_EVENTS) is GONE from "
                        f"hermes_cli/hooks.py _DEFAULT_PAYLOADS — likely renamed; Hermes still "
                        f"runs but the shell hook we install into config.yaml fires nothing "
                        f"(no sprite / no activity)."
                    )
        # Payload FIELD names — assembled by _serialize_payload in the SEPARATE
        # agent/shell_hooks.py (a second fetch). ONE-DIRECTIONAL.
        shell = try_fetch(HERMES_SHELL_HOOK_URL, "Hermes shell_hooks", breaking, errors)
        if shell is not None:
            for field in sorted(HERMES_PAYLOAD_FIELDS):
                if f'"{field}"' not in shell:
                    breaking.append(
                        f"Hermes payload field `{field}` (read by "
                        f"decode_hermes_hook_payload) is GONE from agent/shell_hooks.py "
                        f"_serialize_payload — renamed; the shell-hook JSON omits it and the "
                        f"decoder reads None (no coalesce key / no tool label)."
                    )

    # --- Kimi hook events (only the FETCH is transient) --------------------
    if kimi_ours is not None:
        text = try_fetch(KIMI_HOOKS_URL, "Kimi hooks doc", breaking, errors)
        if text is not None:
            for ev in sorted(kimi_ours):
                # Word-boundary token match (the doc renders each PascalCase name
                # inline / in a summary table, not as a quoted literal). Mirrors
                # the Cursor check. ONE-DIRECTIONAL: a depended event missing from
                # the page is breaking; a new upstream event is intentionally
                # ignored (we map 8 of 16 by design).
                if not re.search(rf"\b{re.escape(ev)}\b", text):
                    breaking.append(
                        f"Kimi hook `{ev}` (registered in KIMI_EVENTS) is GONE from "
                        f"kimi-code docs/en/customization/hooks.md — likely renamed; "
                        f"Kimi still fires it but the decoder maps it to nothing "
                        f"(no sprite / no activity)."
                    )

    # --- CC subagent-dispatch tool (only the FETCH is transient) -----------
    if dispatch_names is not None:
        tools = try_fetch(CC_TOOLS_URL, "CC tools-reference", breaking, errors)
        if tools is not None:
            # At least one name we'd detect by-name must still be the documented
            # dispatch tool. (Losing a legacy name like `Task` is fine.)
            present = [n for n in dispatch_names if re.search(rf"`{re.escape(n)}`", tools)]
            if not present:
                breaking.append(
                    f"None of our known dispatch tool names {sorted(dispatch_names)} "
                    f"appear in CC tools-reference — the subagent tool was likely "
                    f"renamed again. Update make_tool_detail's known names. (Semantic "
                    f"subagent_type detection still works, but the name fallback is "
                    f"stale.)"
                )

    # --- CC hook-event list + lifecycle surfaces (ONE hooks.md fetch) ------
    # The event-list diff mirrors the Codex HookEventName check (CC is a
    # closed binary, so the docs markdown is the only watchable surface); the
    # lifecycle-marker scan is unconditional (nothing to read from our source
    # first — we depend on those surfaces' ABSENCE; see
    # CC_LIFECYCLE_SURFACE_MARKERS).
    hooks_doc = try_fetch(CC_HOOKS_URL, "CC hooks doc", breaking, errors)
    if hooks_doc is not None:
        if cc_ours is not None:
            upstream = upstream_cc_hook_events(hooks_doc)
            if upstream is None:
                breaking.append(
                    "CC hook-event summary table not found in hooks.md — the "
                    "docs were restructured; update upstream_cc_hook_events' "
                    "parser."
                )
            else:
                for ev in sorted(cc_ours):
                    if ev not in upstream:
                        breaking.append(
                            f"CC hook `{ev}` (registered in install/claude.rs "
                            f"EVENTS) is GONE from hooks.md — likely renamed; "
                            f"the decoder will silently drop it."
                        )
                for ev in sorted(upstream - cc_ours - CC_KNOWN_OMITTED):
                    review.append(
                        f"new CC hook `{ev}` upstream — we neither register nor "
                        f"intentionally omit it (add a decoder arm + "
                        f"install/claude.rs EVENTS, or add it to "
                        f"CC_KNOWN_OMITTED)."
                    )
        review.extend(cc_doc_marker_findings(hooks_doc))



def main() -> int:
    breaking: list[str] = []
    review: list[str] = []
    errors: list[str] = []

    # Read what WE depend on from our OWN source first. A failure here means the
    # monitor itself is broken (decoder.rs / install/codex.rs refactored away from
    # what the parsers expect) — that is a LOUD breaking signal, never a transient
    # one, or drift monitoring would silently stop with zero alarm.
    codex_ours = None
    codex_rollout = None
    cc_ours = None
    dispatch_names = None
    reasonix_ours = None
    codewhale_ours = None
    opencode_ours = None
    copilot_ours = None
    omp_ours = None
    cursor_ours = None
    openclaw_ours = None
    hermes_ours = None
    grok_ours = None
    kimi_ours = None
    try:
        codex_ours = read_codex_events()
        codex_rollout = read_codex_rollout_types()
        cc_ours = read_cc_events()
        dispatch_names = read_dispatch_names()
        reasonix_ours = read_reasonix_events()
        codewhale_ours = read_codewhale_events()
        opencode_ours = read_opencode_events()
        copilot_ours = read_copilot_events()
        omp_ours = read_omp_entry_types()
        cursor_ours = read_cursor_events()
        openclaw_ours = read_openclaw_events()
        hermes_ours = read_hermes_events()
        grok_ours = read_grok_events()
        kimi_ours = read_kimi_events()
    except Exception as e:  # noqa: BLE001
        breaking.append(
            f"drift-watch cannot read our own source ({e}) — the parsers in "
            f"check_upstream_drift.py are stale (decoder.rs / install refactored?). "
            f"The monitor is blind until the script is fixed."
        )

    try:
        run_checks(
            codex_ours,
            codex_rollout,
            cc_ours,
            dispatch_names,
            reasonix_ours,
            codewhale_ours,
            opencode_ours,
            copilot_ours,
            omp_ours,
            cursor_ours,
            openclaw_ours,
            hermes_ours,
            grok_ours,
            kimi_ours,
            breaking,
            review,
            errors,
        )
    except Exception as e:  # noqa: BLE001
        traceback.print_exc()
        errors.append(
            f"unexpected error during the upstream checks "
            f"({type(e).__name__}: {e}) — treating as transient; the report "
            f"covers only the checks that completed (traceback on stderr)"
        )

    # --- report ------------------------------------------------------------
    out = ["# pixtuoid upstream wire-format drift report", ""]
    if breaking:
        out.append("## ⛔ Breaking drift — decoder will silently drop events")
        out += [f"- {b}" for b in breaking]
        out.append("")
    if review:
        out.append("## 🔎 New upstream events to review")
        out += [f"- {r}" for r in review]
        out.append("")
    if errors:
        out.append("## ⚠️ Could not verify (transient network/HTTP — not drift)")
        out += [f"- {e}" for e in errors]
        out.append("")
    if not (breaking or review or errors):
        out.append("✅ No drift. Every name we depend on is present upstream.")
    print("\n".join(out))

    if breaking or review:
        return 1
    if errors:
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
