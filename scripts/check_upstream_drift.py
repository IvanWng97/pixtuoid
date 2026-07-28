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
                          The transcript decoder now BREADCRUMBS an unknown OUTER
                          type (`drift::unknown_event`, defense #2) but is silent on
                          an unknown INNER under a known outer, so this positive check
                          — each depended INNER type still exists upstream — is the
                          backstop for the INNER direction
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
                          Hmbown/CodeWhale crates/tui/src/hooks/config.rs (the
                          DEEPSEEK_* env vars ride its sibling hooks/executor.rs)
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

Findings carry a DISPOSITION, because "upstream changed" and "our probe missed"
need different work and only one of them is a statement about upstream:

  * VERIFIED CHANGE (`breaking`) — the surface that OWNS the name was read
    successfully and the name is gone. Fix the decoder.
  * REVIEW (`review`) — a new upstream event/type/namespace to adopt or
    knowingly ignore.
  * PROBE HEALTH (`blind`) — a lookup missed: a 404, an absent anchor, a parser
    that found nothing. All the script knows is that ITS OWN PROBE failed, so
    the affected checks are SKIPPED rather than reported as renames. Repin and
    verify by hand.
  * TRANSIENT (`errors`) — network/HTTP trouble; retry later.

Keeping those apart is not cosmetic. #793 filed five phantom renames under
"decoder will silently drop events": CodeWhale had split `crates/tui/src/hooks.rs`
into a module directory, the stale pin still returned 200 as a `mod`/`pub use`
facade, and the unanchored sweep read the facade's silence as three env-var
renames. Following that report would have renamed three WORKING env vars. Every
document swept for name-presence therefore declares an ANCHOR (see `ANCHORS`)
proving it is still the document that owns those names — no anchor, no sweep.

Exit codes:
  0  no findings
  1  actionable (verified drift, a review ping, OR probe health — all three need
     a human) -> open a tracking issue
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
import typing
import urllib.error
import urllib.request

# What a fetch can raise transiently. URLError covers connect-phase failures
# (urllib wraps OSErrors only during do_open) and HTTP 4xx/5xx (HTTPError
# subclasses it), but the READ phase inside fetch() raises raw
# socket.timeout / ConnectionResetError (OSError subclasses, NOT URLError)
# and http.client.IncompleteRead (HTTPException) — left uncaught they exit 1
# and the workflow files a junk drift-titled issue from an empty report.
# URLError is itself an OSError subclass; kept explicit to document intent.
FETCH_ERRORS = (urllib.error.URLError, OSError, http.client.HTTPException)

# A permanent HTTP status means the URL itself is wrong/gone — our pinned
# upstream path moved, so the watch is BLIND for that source until fixed. This is
# PROBE HEALTH (our pin is wrong), never transient. Everything else (403/429 throttling behind a CDN,
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
# PostToolUseFailure/StopFailure (#710): NATIVE hooks registered under
# PostToolUse/Stop already receive failures — the runner re-fires them with the
# event re-labeled (internal/hook/runner.go `PostToolUseFailure`/`StopResult`:
# `legacy := r.nativeHooks(PostToolUse|Stop); p.Event = ...; Run(...)`), and
# our install writes native-format hooks. Registering the failure events TOO
# would double-fire every failed tool/turn; both paths decode to the same
# ActivityEnd anyway.
REASONIX_KNOWN_OMITTED = {
    "PostLLMCall",
    "PreCompact",
    "SubagentStop",
    "PostToolUseFailure",
    "StopFailure",
}

# Payload fields decode_rx_hook_payload reads — a renamed json tag upstream
# silently zeroes the decode (`event`/`cwd` are load-bearing: a payload without
# them is rejected as malformed; `subject` feeds the PermissionRequest→Waiting
# reason, #302).
REASONIX_PAYLOAD_FIELDS = {"event", "cwd", "toolName", "toolArgs", "subject", "message"}

# CodeWhale split `crates/tui/src/hooks.rs` into a module DIRECTORY (#793): the
# old path still 200s as the module root, but it is now a `mod`/`pub use` shim,
# so both depended surfaces read empty there — the enum moved to `hooks/config.rs`
# and `HookContext::to_env_vars` to `hooks/executor.rs`. They are two files now,
# hence two URLs; neither surface's CONTENT changed.
CODEWHALE_HOOK_URL = (
    "https://raw.githubusercontent.com/Hmbown/CodeWhale/main/"
    "crates/tui/src/hooks/config.rs"
)
CODEWHALE_EXECUTOR_URL = (
    "https://raw.githubusercontent.com/Hmbown/CodeWhale/main/"
    "crates/tui/src/hooks/executor.rs"
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
# `HookContext::to_env_vars` in hooks/executor.rs (its own fetch since the
# hooks.rs -> hooks/ split; it used to share the event check's file). WORKSPACE
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

# The gateway PORT is pixtuoid's runtime identity for one gateway (the inner key of
# `SceneState::daemons`), and the plugin gets it from `gateway_start`'s event/ctx —
# `PluginHookGatewayStartEvent = { port: number }` / `PluginHookGatewayContext`.
# That `port` field is therefore a depended wire name exactly like `runId`: a rename
# leaves every envelope stamped with the registration-time FALLBACK, so two live
# gateways collapse onto one mascot again (or one splits into two phantoms). Checked
# in the SAME hook-types.ts text, one-directional. NB `port` is a common word —
# the `PluginHookGatewayStartEvent` type name below carries the precise half.
OPENCLAW_GATEWAY_PORT_TYPES = {"PluginHookGatewayStartEvent", "PluginHookGatewayContext"}

# The plugin's fallback when no hook has handed it the real bound port yet (a hot
# reload replays no `gateway_start`): upstream's `DEFAULT_GATEWAY_PORT` in
# `src/config/paths.ts`. We cannot IMPORT it (the plugin lives in OpenClaw's state
# dir, outside any `node_modules/openclaw` for a global install), so the literal is
# copied into `openclaw_plugin.js` — and a copied constant is a latent drift bug
# unless something watches it. This is that watch.
OPENCLAW_PATHS_URL = (
    "https://raw.githubusercontent.com/openclaw/openclaw/main/src/config/paths.ts"
)

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
# The ACP (Agent Client Protocol) v1 `SessionUpdate` tag vocabulary the SHARED
# `source/acp.rs` flood-guards (grok is the sole ACP-transcript source today; #766).
# The canonical schema lives in the `agentclientprotocol` org (the `zed-industries`
# slug 301-redirects there). grok pins `features = ["unstable"]`, so its real
# surface is the UNION of the v1 stable + v1 unstable tag sets — we fetch both and
# union. v2 is a SEPARATE, partly non-overlapping line grok does NOT speak → NOT
# fetched (its tags would be false "adopt terminal_update" noise). `main` tracks the
# latest v1, matching KNOWN_ACP_TAGS' "latest v1, no version-fallback" anchor.
ACP_V1_SCHEMA_URL = (
    "https://raw.githubusercontent.com/agentclientprotocol/"
    "agent-client-protocol/main/schema/v1/schema.json"
)
ACP_V1_SCHEMA_UNSTABLE_URL = (
    "https://raw.githubusercontent.com/agentclientprotocol/"
    "agent-client-protocol/main/schema/v1/schema.unstable.json"
)

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


class Anchor(typing.NamedTuple):
    """The declaration that OWNS the names we check a fetched document for."""

    pattern: str
    """Regex matching that declaration in the document."""
    owns: str
    """Human name of the declaration, for the probe-health message."""


# Every document we sweep for "is this name still present upstream?" declares the
# anchor that proves it is STILL the document owning those names.
#
# WHY this exists (#793): absence is only evidence of an upstream RENAME if the
# document still contains the declaration that owns the name. Drop that premise
# and a stale pin reads as mass drift. CodeWhale split `crates/tui/src/hooks.rs`
# into a module directory; the old path kept returning 200 as a `mod`/`pub use`
# facade, so `try_fetch` was satisfied and every depended `DEEPSEEK_*` name was
# missing from it. The watcher reported all three as upstream renames and filed
# an issue saying the decoder was broken. Nothing upstream had changed — acting
# on that report would have renamed three WORKING env vars.
#
# Choosing one, in descending strength — take the strongest available, because
# THIS comment is what picks anchor #17:
#   1. The DECLARATION that owns the checked names, so an upstream move takes
#      both and the sweep cannot run against a document missing them.
#   2. Failing that, a declaration co-located with them in the same file — this
#      proves file IDENTITY, which is weaker: "declaration X moved out while Y
#      stayed" satisfies the anchor and still reports phantom renames. Rows
#      marked `identity` below are that weaker grade; they are not upgradeable
#      without a parser, and a docs PAGE (cursor/kimi/CC) can only ever be this.
#   3. Never one of the checked names itself — that is circular and makes the
#      check vacuous (a rename would take the anchor too, so it could never fire).
# Every pattern below was verified against the live document when it was added.
ANCHORS: dict[str, Anchor] = {
    # owner-grade: each anchor is the declaration the checked names live inside.
    CODEWHALE_EXECUTOR_URL: Anchor(r"fn to_env_vars", "`HookContext::to_env_vars`"),
    GROK_ACTIVE_SESSIONS_URL: Anchor(r"pub struct ActiveSession", "`ActiveSession`"),
    HERMES_HOOK_URL: Anchor(r"_DEFAULT_PAYLOADS", "`_DEFAULT_PAYLOADS`"),
    HERMES_SHELL_HOOK_URL: Anchor(r"_serialize_payload", "`_serialize_payload`"),
    OPENCLAW_HOOK_TYPES_URL: Anchor(r"export type PluginHookName", "the `PluginHookName` union"),
    # `SessionUpdate` owns the checked xAI variants; `SessionNotification` (the
    # obvious pick) sits 13KB earlier and does NOT — moving the enum out would
    # leave it satisfied while every variant read as renamed.
    GROK_NOTIFICATION_URL: Anchor(r"pub enum SessionUpdate", "the `SessionUpdate` enum"),
    # The const IDENT owns the checked VALUE `"session_exit"`; a value rename
    # keeps the identifier, so the anchor holds and the check still fires.
    OMP_EXIT_DIAG_URL: Anchor(r"SESSION_EXIT_CUSTOM_TYPE", "`SESSION_EXIT_CUSTOM_TYPE`"),
    OMP_ASK_URL: Anchor(r"export class AskTool", "the `AskTool` class"),
    # identity-grade: co-located, not owning. A union head or page title.
    OPENCODE_EVENT_URLS[0]: Anchor(r"(?m)^export const Event = \{", "the `Event` inventory"),
    OPENCODE_EVENT_URLS[1]: Anchor(r"(?m)^export const Event = \{", "the `Event` inventory"),
    GROK_HOOK_URL: Anchor(r"pub struct HookEventEnvelope", "`HookEventEnvelope`"),
    OMP_SESSION_ENTRIES_URL: Anchor(r"export type SessionEntry", "the `SessionEntry` union"),
    OMP_AI_TYPES_URL: Anchor(r"export type Message =", "the `Message` union"),
    CURSOR_HOOKS_URL: Anchor(r"hook_event_name", "the hook-event payload docs"),
    KIMI_HOOKS_URL: Anchor(r"hook_event_name", "the hook-event payload docs"),
    CC_TOOLS_URL: Anchor(r"(?m)^# Tools reference", "the tools-reference page"),
    CC_HOOKS_URL: Anchor(r"(?m)^# Hooks reference", "the hooks-reference page"),
}


# The other half of the ANCHORS population: documents fetched with plain
# `try_fetch`, where a PARSER is the identity proof (it returns None on the wrong
# content, and the caller files probe health). Written down so the selftest can
# assert this set equals the live `try_fetch` call sites — otherwise "every swept
# document is anchored" is only enforced for the `fetch_anchored` spelling, and
# the next unanchored sweep just uses the other one.
UNANCHORED_BY_DESIGN: dict[str, str] = {
    CODEX_PROTOCOL_URL: "parser-gated: _enum_body / codex_turn_context_fields return None",
    CODEX_MODELS_URL: "parser-gated: _enum_body(ResponseItem) returns None",
    REASONIX_HOOK_URL: "parser-gated: upstream_reasonix_hooks returns None (gates the field sweep)",
    CODEWHALE_HOOK_URL: "parser-gated: upstream_codewhale_hooks -> _enum_body returns None",
    COPILOT_SCHEMA_URL: "parser-gated: the SessionEvent anyOf union gates all three sweeps",
    OPENCLAW_PATHS_URL: "value comparison: both sides are read, no presence sweep",
    ACP_V1_SCHEMA_URL: "parser-gated: upstream_acp_session_update_tags returns None",
    ACP_V1_SCHEMA_UNSTABLE_URL: "parser-gated: same",
}


def probe_failed(
    what: str, where: str, consequence: str, *, our_source: bool = False
) -> str:
    """One wording for every probe-health line.

    The distinction this enforces is the whole point of the `blind` bucket: all
    the script knows when a lookup misses is that ITS OWN PROBE missed. "Upstream
    moved it" is a guess, and #793 shipped that guess as fact across five
    false-positive drift lines. Verified drift says what upstream did; this says
    what we failed to read, and points at the pin rather than the decoder.

    `our_source=True` for a failure to read OUR OWN source — the plugin template, this
    script's parsers. Nothing upstream is even consulted on those paths, so the
    default's "upstream may have moved it … re-verify upstream by hand" would be
    the wrong cause AND the wrong action. A change whose thesis is "don't assert
    a cause you didn't verify" must not assert one in the other direction.
    """
    if our_source:
        cause = "Our own parser or constant is stale; nothing upstream was consulted."
        action = "Fix the script — this says nothing about upstream."
    else:
        cause = (
            "Upstream may have moved or reshaped it, or our pin/parser may be "
            "stale; this is NOT evidence that upstream changed."
        )
        action = (
            "Re-verify upstream by hand, then repin — do NOT change a decoder "
            "on this alone."
        )
    return f"could not verify {what} at {where} — {cause} {consequence} {action}"


def fetch(url: str) -> str:
    req = urllib.request.Request(url, headers={"User-Agent": "pixtuoid-drift-watch"})
    with urllib.request.urlopen(req, timeout=30) as resp:  # noqa: S310 (trusted hosts)
        return resp.read().decode("utf-8", "replace")


def try_fetch(
    url: str, label: str, blind: list[str], errors: list[str]
) -> str | None:
    """Fetch `url`, classifying failures so a PERMANENT upstream move is loud.

    A `PERMANENT_HTTP_STATUS` (404/410/451) means our pinned URL is wrong/gone →
    `blind` (probe health: the watch cannot see this source until the `*_URL`
    constant is fixed — a fact about OUR pin, not about upstream's wire format).
    403/429/5xx + connect/read timeouts → `errors` (transient). Returns the body,
    or None on any failure (the caller skips that source's checks). Centralizes
    the try/except every fetch site repeated, AND fixes the
    `HTTPError ⊂ URLError ⊂ FETCH_ERRORS` swallow that bucketed 404 as transient.
    """
    try:
        return fetch(url)
    except urllib.error.HTTPError as e:
        if e.code in PERMANENT_HTTP_STATUS:
            blind.append(
                probe_failed(
                    f"{label}",
                    f"{url} (HTTP {e.code})",
                    "Every check for this source was SKIPPED.",
                )
            )
        else:
            errors.append(f"{label}: transient HTTP {e.code} at {url}: {e}")
        return None
    except FETCH_ERRORS as e:
        errors.append(f"{label}: fetch failed (transient?): {e}")
        return None


def fetch_anchored(
    url: str, label: str, blind: list[str], errors: list[str]
) -> str | None:
    """`try_fetch` plus the identity proof required before sweeping a document.

    Returns the body only when the document still contains its `ANCHORS` entry.
    A missing anchor means the fetch succeeded but landed on the wrong content
    (a re-export facade, a restructured page), so every presence check that
    would have run is SKIPPED and reported as probe health instead of drift —
    the #793 class, made unrepresentable rather than merely fixed once.

    An undeclared URL is reported, never swept. It cannot raise: `run_checks` is
    wrapped in `except Exception` that routes bugs to the TRANSIENT bucket
    (exit 2, warn-only) to avoid filing junk issues — so a bare `KeyError` here
    would degrade "someone added an unproven sweep" into a green-ish warning,
    which is the fail-open shape this whole change exists to remove. The
    selftest's `test_every_swept_url_declares_an_anchor` is the development-time
    gate; this is the runtime backstop, and it is deliberately loud.
    """
    anchor = ANCHORS.get(url)
    if anchor is None:
        blind.append(
            probe_failed(
                f"{label}: no ANCHORS entry declares what proves this document's identity",
                url,
                "It was NOT swept — an unproven document cannot distinguish an "
                "upstream rename from a stale pin. Add its anchor to ANCHORS "
                "(and a sample to the selftest's ANCHOR_SAMPLES).",
            )
        )
        return None
    text = try_fetch(url, label, blind, errors)
    if text is None:
        return None
    if not re.search(anchor.pattern, text):
        blind.append(
            probe_failed(
                f"{label}: the document no longer contains {anchor.owns}",
                url,
                "It still fetches, so the probe landed on the wrong content — "
                "most likely a stale pin (a moved declaration, or a re-export "
                "facade left at the old path) rather than an upstream rename. "
                "Every presence check riding this document was SKIPPED, NOT "
                "reported as drift.",
            )
        )
        return None
    return text


def rust_const_str_array(rel_path: str, const_name: str) -> set[str]:
    """The quoted words of a `const NAME: &[&str] = &[ … ];` array, COMMENTS EXCLUDED.

    Comment-stripping is load-bearing, not tidiness. A bare `"(\\w+)"` scrape over
    the raw block also captures every quoted word inside an explanatory comment —
    and this repo's comment convention actively encourages those. It fired for
    real: a WHY comment added inside CODEX_EVENTS mentioned the SessionEnd
    payload's `reason const "other"`, the watcher read `other` as a REGISTERED
    hook event, found no such variant upstream, and reported a phantom
    "⛔ Breaking drift" heading of the day, auto-filing an
    issue and failed the run. A watcher that cries wolf gets its real alarms
    ignored, so every event reader shares this one parser.
    """
    got = parse_rust_const_str_array((REPO / rel_path).read_text(), const_name)
    if got is None:
        raise RuntimeError(f"could not locate {const_name} in {rel_path}")
    return got


def strip_rust_comments(body: str) -> str:
    """Remove Rust comments, PRESERVING string literals and honouring nesting.

    A scanner rather than a pair of `re.sub`s, because both regex approaches drop
    a REAL registered event and so fail SILENTLY OPEN — the watcher stops checking
    a name and nothing says so, which is worse than the phantom it replaced:

    - blind `//[^\\n]*` eats to end of line from a `//` inside a STRING, taking
      every later entry on that line with it;
    - `/\\*.*?\\*/` stops at the first `*/`, so a nested block comment (legal Rust)
      leaves its tail behind and re-admits words from inside it.

    Neither can fire on today's `\\w+` hook names, so this is robustness for the
    general parser the docstring below promises, not a live defect.
    """
    out: list[str] = []
    i, n, depth = 0, len(body), 0
    while i < n:
        pair = body[i : i + 2]
        if depth:
            if pair == "/*":
                depth += 1
                i += 2
            elif pair == "*/":
                depth -= 1
                i += 2
            else:
                i += 1
            continue
        if pair == "/*":
            depth = 1
            i += 2
            continue
        if pair == "//":
            nl = body.find("\n", i)
            i = n if nl < 0 else nl
            continue
        if body[i] == '"':
            j = i + 1
            while j < n:
                if body[j] == "\\":
                    j += 2
                    continue
                if body[j] == '"':
                    j += 1
                    break
                j += 1
            out.append(body[i:j])
            i = j
            continue
        out.append(body[i])
        i += 1
    return "".join(out)


def rust_block_after(src: str, anchor_re: str) -> str | None:
    """The `{ … }` block following the first `anchor_re` match, `None` if absent.

    Scraping a whole FILE for a decoder's arms is the same class as scraping a
    whole const-array block including its comments: it admits text the decoder
    does not actually depend on — a `#[cfg(test)] mod tests` constructing the same
    tuple shape leaks a phantom, and a phantom makes the watcher alarm on a name
    upstream never had to have. Run the source through `strip_rust_comments`
    first so a brace inside a comment or string cannot move the bounds.
    """
    m = re.search(anchor_re, src)
    if not m:
        return None
    start = src.find("{", m.end())
    if start < 0:
        return None
    depth = 0
    for i in range(start, len(src)):
        if src[i] == "{":
            depth += 1
        elif src[i] == "}":
            depth -= 1
            if depth == 0:
                return src[start : i + 1]
    return None


def parse_rust_const_str_array(src: str, const_name: str) -> set[str] | None:
    """The pure half of `rust_const_str_array` — `None` when the const is absent.

    Split out so the comment-blindness above is testable from a synthetic snippet:
    the shape assertions in the selftest cannot catch it, since a phantom scraped
    out of a comment is an ordinary-looking word that passes every shape regex.
    """
    m = re.search(rf"const {const_name}[^=]*=\s*&\[(.*?)\];", src, re.S)
    if not m:
        return None
    return set(re.findall(r'"(\w+)"', strip_rust_comments(m.group(1))))


def read_codex_events() -> set[str]:
    return rust_const_str_array("crates/pixtuoid/src/install/codex.rs", "CODEX_EVENTS")


def read_codex_rollout_types() -> tuple[set[str], set[str]]:
    """The (event_msg, response_item) inner `type` strings the codex TRANSCRIPT
    decoder matches on (`source/codex.rs` `match (outer, inner)`). Unlike the hook
    events these are registered NOWHERE, and the decoder's `_ => vec![]` drops an
    unrecognized one SILENTLY (no `unknown_event` breadcrumb, unlike the hook
    decoders) — so a positive "each depended type still exists upstream" check is
    the only backstop against an upstream rename going dark."""
    src = (REPO / "crates/pixtuoid-core/src/source/codex.rs").read_text()
    # Bounded to the decoder's own match block, NOT the whole file: the file also
    # carries `#[cfg(test)] mod tests`, and a future test constructing a tuple of
    # this shape with a type the decoder does not depend on would leak a phantom
    # into the depended set — the watcher would then alarm on a name upstream
    # never had to have. Comments are stripped first so a brace inside one cannot
    # move the block's bounds.
    block = rust_block_after(strip_rust_comments(src), r"match \(outer, inner\)")
    if block is None:
        raise RuntimeError(
            "could not locate the codex `match (outer, inner)` decode block in "
            "source/codex.rs — the transcript decoder was refactored; update the parser."
        )
    event_msg = set(re.findall(r'\(\s*"event_msg"\s*,\s*"(\w+)"\s*\)', block))
    response_item = set(re.findall(r'\(\s*"response_item"\s*,\s*"(\w+)"\s*\)', block))
    if not event_msg or not response_item:
        raise RuntimeError(
            "could not locate codex ('event_msg'|'response_item', …) decode arms "
            "in source/codex.rs — the transcript decoder was refactored; update "
            "the parser."
        )
    return event_msg, response_item


def read_codex_rollout_outers() -> set[str]:
    """The rollout OUTER `type` discriminators the transcript decoder RECOGNIZES,
    read from the `KNOWN_OUTERS` const in `source/codex.rs`. The tail breadcrumbs
    any outer OUTSIDE this set, and `drift::unknown_event` has NO dedup — so an
    upstream `RolloutItem` variant missing from this set would flood the
    warn-floor on every line of it. The report diffs this against the live
    `RolloutItem` enum so a new upstream outer is a review ping BEFORE it floods."""
    return rust_const_str_array("crates/pixtuoid-core/src/source/codex.rs", "KNOWN_OUTERS")


def read_cc_events() -> set[str]:
    return rust_const_str_array("crates/pixtuoid/src/install/claude.rs", "EVENTS")


def read_dispatch_names() -> set[str]:
    src = (REPO / "crates/pixtuoid-core/src/source/decoder.rs").read_text()
    m = re.search(r"known_name\s*=\s*([^;]+);", src)
    if not m:
        raise RuntimeError("could not locate the dispatch known_name check in decoder.rs")
    # Same comment-blindness class as the array readers: the captured span is an
    # EXPRESSION, so a trailing `// the legacy "Task" name was dropped` — exactly
    # the history CLAUDE.md records for this line — would read as a known name.
    return set(re.findall(r'"(\w+)"', strip_rust_comments(m.group(1))))


def upstream_codex_hooks(text: str) -> set[str] | None:
    """The Codex `HookEventName` variants. Reads the body through `_enum_body`
    (see there for why guessing where an enum ends is what broke #793)."""
    body = _enum_body(text, "HookEventName")
    if body is None:
        return None
    # variant identifiers (drop comments/attrs by keeping CamelCase words)
    return set(re.findall(r"\b([A-Z][A-Za-z]+)\b", body)) or None


def _snake_case(camel: str) -> str:
    return re.sub(r"(?<!^)(?=[A-Z])", "_", camel).lower()


def _enum_body(text: str, enum_name: str) -> str | None:
    """The brace-balanced body of `enum <enum_name> { … }`.

    THE enum-body reader for every Rust surface we watch. The ad-hoc regexes it
    replaced each guessed where the body ENDS, and both guesses break on a
    harmless upstream refactor:

    * `(.*?)\\}` stops at the FIRST `}`, so a single struct variant
      (`ToolCallBefore { name: String }`) truncates the enum and every variant
      after it reads as GONE — a phantom rename per variant.
    * `(.*?)\\n\\}` instead demands a column-0 closing brace, so an INDENTED
      enum does not stop at its own brace; it runs on to the next top-level one,
      over-capturing whatever lies between. Measured against grok's real
      event.rs: 2324 characters, spilling into the following `impl` block —
      whose CamelCase idents a variant scrape then admits as variants.

    Balancing counts braces instead. NOT what fixed grok in #793, to be clear:
    there the enum is macro-GENERATED, so its declaration holds `$variant`
    placeholders and no amount of balancing recovers the names —
    `upstream_grok_hooks` falls through to the `hook_events!` invocation table
    for that. This removes a separate latent defect living on the same code.

    Comments are stripped FIRST because they are scanned for braces otherwise:
    a `// see Foo { bar }` doc line inside the enum would unbalance the count.
    `strip_rust_comments` preserves string literals, so `rename = "…"` attrs
    that callers read out of the returned body survive.
    """
    text = strip_rust_comments(text)
    # `\\s*\\{` (not `\\b`) so a prefix name can't match a longer enum:
    # `HookEvent` must not bind to `enum HookEventName {`.
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
    """Iteratively strip innermost `(…)`/`{…}` (tuple params, struct-variant
    bodies, AND attr parens) so only top-level variant idents survive — else a
    CamelCase field/param TYPE reads as a variant.

    PRECONDITION: comments are already gone — every caller receives an
    `_enum_body` result, which strips them with `strip_rust_comments`. This used
    to re-strip with a naive `//[^\\n]*`, which is not merely redundant: that
    pattern eats to end-of-line from a `//` inside a STRING LITERAL, so one
    `rename = "http://…"` attr would silently delete every variant after it on
    that line. Two lexers, one un-doing the other's guarantee.
    """
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
    Returns None if the enum can't be located (→ probe health: the caller files a
    `blind` line and SKIPS the check, rather than claiming upstream moved it)."""
    body = _enum_body(text, enum_name)
    if body is None:
        return None
    # rename/alias literals must be read BEFORE `_strip_nested` eats the attr parens.
    names = set(re.findall(r'(?:rename|alias)\s*=\s*"([^"]+)"', body))
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
    return rust_const_str_array("crates/pixtuoid/src/install/reasonix.rs", "REASONIX_EVENTS")


def upstream_reasonix_hooks(text: str) -> set[str] | None:
    # Go consts: `PreToolUse Event = "PreToolUse"` — take the string values.
    found = set(re.findall(r'\w+\s+Event\s*=\s*"(\w+)"', text))
    return found or None


def read_codewhale_events() -> set[str]:
    return rust_const_str_array("crates/pixtuoid/src/install/codewhale.rs", "CODEWHALE_EVENTS")


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


def read_omp_known_types() -> set[str]:
    """The COMPLETE session-entry `type` set the transcript tail flood-guards, read
    from the `KNOWN_ENTRY_TYPES` const in `source/omp.rs` (the full `RawFileEntry`
    union — NOT just the arms we decode, which is `read_omp_entry_types`). The tail
    breadcrumbs a `type` OUTSIDE this set, and `drift::unknown_event` has NO dedup,
    so an upstream entry type missing from it would flood the warn-floor on every
    line of it. The report diffs this against the live `RawFileEntry` literals so a
    new upstream type is a review ping BEFORE it floods."""
    return rust_const_str_array("crates/pixtuoid-core/src/source/omp.rs", "KNOWN_ENTRY_TYPES")


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


def read_copilot_namespaces() -> set[str]:
    """The event-`type` NAMESPACE families the transcript tail flood-guards, read
    from the `KNOWN_NAMESPACES` const in `source/copilot.rs`. The tail breadcrumbs a
    `type` whose namespace is OUTSIDE this set, and `drift::unknown_event` has NO
    dedup — so an upstream `SessionEvent` namespace missing from it would flood the
    warn-floor on every line of that family. The report diffs this against the live
    `SessionEvent` union so a new upstream namespace is a review ping BEFORE it
    floods."""
    return rust_const_str_array("crates/pixtuoid-core/src/source/copilot.rs", "KNOWN_NAMESPACES")


def read_cursor_events() -> set[str]:
    """The camelCase hook events we register/decode, read from the explicit
    `CURSOR_EVENTS` const in install/cursor.rs — the same registered list the
    `every_registered_cursor_event_decodes` test pins, and a leak-free source of
    truth (mirrors read_reasonix_events / read_codewhale_events). Reading the
    decoder's `match event` block instead would risk a future camelCase field
    lookup in an arm leaking a phantom event into the drift set."""
    return rust_const_str_array("crates/pixtuoid/src/install/cursor.rs", "CURSOR_EVENTS")


def read_openclaw_events() -> set[str]:
    """The OpenClaw gateway hook events we register/decode, read from the
    `OPENCLAW_EVENTS` const in install/openclaw.rs — the SAME list the plugin
    HOOKS array and the decoder arms are pinned to by
    `openclaw_events_plugin_decoder_and_const_agree`, so this is a leak-free
    source of truth (mirrors read_cursor_events / read_codewhale_events)."""
    return rust_const_str_array("crates/pixtuoid/src/install/openclaw.rs", "OPENCLAW_EVENTS")


def openclaw_plugin_default_port() -> str | None:
    """The `DEFAULT_GATEWAY_PORT` literal the bundled OpenClaw plugin falls back to.

    Read from the JS template itself (not a second copy here) so the watch compares
    upstream against the value that actually ships.
    """
    try:
        src = (REPO / "crates/pixtuoid/src/install/openclaw_plugin.js").read_text()
    except OSError:
        return None
    m = re.search(r"const DEFAULT_GATEWAY_PORT\s*=\s*(\d+)", src)
    return m.group(1) if m else None


def read_hermes_events() -> set[str]:
    """The Hermes shell-hook events we register/decode, read from the
    `HERMES_EVENTS` const in install/hermes.rs — pinned to the decoder arms by
    `every_registered_hermes_event_decodes` (install/hermes.rs), so this is a
    leak-free source of truth (mirrors read_cursor_events / read_openclaw_events)."""
    return rust_const_str_array("crates/pixtuoid/src/install/hermes.rs", "HERMES_EVENTS")


def read_grok_events() -> set[str]:
    """The grok hook events we register/decode, read from the `GROK_EVENTS`
    const in install/grok.rs — pinned to the decoder arms by
    `every_registered_grok_event_decodes`, so this is a leak-free source of
    truth (mirrors read_cursor_events / read_hermes_events)."""
    return rust_const_str_array("crates/pixtuoid/src/install/grok.rs", "GROK_EVENTS")


def read_acp_tags() -> set[str]:
    """The ACP v1 `sessionUpdate` tag vocabulary the shared `source/acp.rs`
    flood-guards, read from its `KNOWN_ACP_TAGS` const. The ACP tag tier
    breadcrumbs a tag OUTSIDE this set, and `drift::unknown_event` has NO dedup —
    so an upstream v1 tag missing from it (esp. a per-token `*_message_chunk`)
    would flood. The report diffs this against the live v1 schema so a new upstream
    tag is a review ping BEFORE it floods."""
    return rust_const_str_array("crates/pixtuoid-core/src/source/acp.rs", "KNOWN_ACP_TAGS")


def read_kimi_events() -> set[str]:
    """The PascalCase hook events we register/decode, read from the `KIMI_EVENTS`
    const in install/kimi.rs — the SAME registered list the
    `every_registered_kimi_event_decodes` test pins to the decode path (the shared
    CC-shaped arms + the source's custom Extend decoder), so this is a leak-free
    source of truth (mirrors read_cursor_events / read_hermes_events)."""
    return rust_const_str_array("crates/pixtuoid/src/install/kimi.rs", "KIMI_EVENTS")


def upstream_grok_hooks(text: str) -> set[str] | None:
    """The HookEventName enum variants (bare Rust idents — registration keys
    accept the PascalCase spelling, so these ARE the names we register).

    TWO declaration shapes, tried in order. Upstream originally wrote a plain
    `pub enum HookEventName { … }`; it now GENERATES that enum from a
    `hook_events! { … }` macro table (one row per event, carrying the event's
    display name, deserialize aliases and trait triple). The plain-enum regex
    still matches the macro DEFINITION's body — but that body holds `$variant`
    placeholders, not variants, so it reads empty and the whole 15-variant set
    looked "not found at the pinned path" (#793). Hence the fall-THROUGH: an
    empty plain-enum parse is not an answer, it's a signal to read the table.
    """
    m = re.search(r"pub enum HookEventName \{(.*?)\n\}", text, re.S)
    if m:
        found = set(re.findall(r"(?m)^\s*([A-Z]\w+),", m.group(1)))
        if found:
            return found
    # Comments first: `rust_block_after` measures braces, and a doc comment on a
    # row would otherwise read as a variant (upstream annotates Stop/SubagentEnd).
    block = rust_block_after(strip_rust_comments(text), r"(?m)^\s*hook_events!\s*")
    if block is None:
        return None
    # Row headers only (`Variant {`) — the row BODY is `key: value` lines whose
    # aliases/traits carry CamelCase tokens (Observe/Tested) we must not admit.
    found = set(re.findall(r"(?m)^\s*([A-Z]\w+)\s*\{", block))
    return found or None


def upstream_acp_session_update_tags(text: str) -> set[str] | None:
    """The ACP `SessionUpdate` discriminator tags from a v1 JSON schema — the
    `sessionUpdate` `const` of each member of the `$defs.SessionUpdate` closed
    `oneOf` union (members carry the const INLINE; a `$ref` member is resolved).
    Returns None if the schema won't parse or the union is absent (→ the caller
    files probe health and SKIPS the check; the ACP tag flood guard is blind)."""
    try:
        root = json.loads(text)
    except json.JSONDecodeError:
        return None
    defs = root.get("$defs") or root.get("definitions") or {}
    if not isinstance(defs, dict):
        return None
    su = defs.get("SessionUpdate")
    members = su.get("oneOf") or su.get("anyOf") if isinstance(su, dict) else None
    if not isinstance(members, list):
        return None
    tags: set[str] = set()
    for member in members:
        if not isinstance(member, dict):
            continue
        node = member
        ref = member.get("$ref", "")
        if ref:
            node = defs.get(ref.rsplit("/", 1)[-1], {})
        const = (
            node.get("properties", {}).get("sessionUpdate", {}).get("const")
            if isinstance(node, dict)
            else None
        )
        if isinstance(const, str):
            tags.add(const)
    return tags or None


def _copilot_type_const(sch: object) -> str | None:
    """The wire `type` string a copilot schema definition pins — `properties.type`
    as a `const` (or a single-element `enum`). Shared by upstream_copilot_events
    (walks ALL definitions) and upstream_copilot_namespaces (the SessionEvent
    union only) so the two can't drift on how a type-tag is expressed."""
    if not isinstance(sch, dict):
        return None
    t = sch.get("properties", {}).get("type")
    if not isinstance(t, dict):
        return None
    c = t.get("const")
    if c is None:
        enum = t.get("enum")
        if isinstance(enum, list) and len(enum) == 1:
            c = enum[0]
    return c if isinstance(c, str) else None


def upstream_copilot_events(text: str) -> set[str] | None:
    """The per-event `type` consts from the @github/copilot session-events JSON
    schema. Each event is a `definitions.<Name>` object whose `properties.type`
    pins the wire string as a `const` (or a single-element `enum`)."""
    try:
        defs = json.loads(text).get("definitions", {})
    except (json.JSONDecodeError, AttributeError):
        return None
    consts = {c for sch in defs.values() if (c := _copilot_type_const(sch))}
    return consts or None


def upstream_copilot_namespaces(text: str) -> set[str] | None:
    """The NAMESPACE families (prefix before the first `.`) of the copilot
    `SessionEvent` union — the `definitions.SessionEvent.anyOf` members' own `type`
    consts, grouped to their family. Scoped to the anyOf union ON PURPOSE: a naive
    walk of ALL `definitions` (upstream_copilot_events) also pulls in nested-content
    type-tags (`audio`/`text`/`image`/`file`/…) that share the `type.const` shape
    but are never a top-level envelope `type`, inflating the set with ~30 phantom
    families. Returns None if the schema won't parse or the union is absent (→ the
    caller files probe health and SKIPS every Copilot check)."""
    try:
        defs = json.loads(text).get("definitions", {})
    except (json.JSONDecodeError, AttributeError):
        return None
    anyof = defs.get("SessionEvent", {}).get("anyOf") if isinstance(defs, dict) else None
    if not isinstance(anyof, list):
        return None
    namespaces: set[str] = set()
    for member in anyof:
        ref = member.get("$ref", "") if isinstance(member, dict) else ""
        name = ref.rsplit("/", 1)[-1] if ref else ""
        c = _copilot_type_const(defs.get(name))
        if c:
            namespaces.add(c.split(".", 1)[0])
    return namespaces or None


def upstream_omp_entry_types(text: str) -> set[str] | None:
    """The COMPLETE omp session-entry `type` set — the `RawFileEntry` union in
    session-entries.ts. Two forms: direct `type: "literal"` discriminators, and
    `type: typeof CONST` refs whose value is a `CONST = "literal"` binding (the
    `title` / `title_change` slots). Returns None if neither form is found (→ the
    caller files probe health and SKIPS the check; the entry-type flood guard is blind)."""
    literals = set(re.findall(r'type:\s*"(\w+)"', text))
    for const_name in re.findall(r"type:\s*typeof\s+(\w+)", text):
        m = re.search(rf'{re.escape(const_name)}\s*=\s*"(\w+)"', text)
        if m:
            literals.add(m.group(1))
    return literals or None


def upstream_copilot_field_names(text: str) -> set[str] | None:
    """The union of every `properties` key at ANY depth in the @github/copilot
    session-events schema — the envelope fields (agentId/sessionId) AND the
    nested `data.properties` fields (toolCallId/toolName/arguments/…). Used
    one-directional: a field the decoder READS that is absent from the whole
    schema is a rename. Returns None if the JSON won't parse (→ probe health, not a drift claim)."""
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
    """The CodeWhale TUI shell-command hook wire names.

    Reads `pub enum HookEvent` in `crates/tui/src/hooks/config.rs` — NOT the
    app-server `codewhale-hooks` sink enum in `crates/hooks`, a different
    mechanism sharing no configuration. serde `rename_all = "snake_case"`, so
    each CamelCase variant converts to the name we register. Body via
    `_enum_body` (see there on why the end of an enum is not guessable).
    """
    body = _enum_body(text, "HookEvent")
    if body is None:
        return None
    variants = re.findall(r"^\s*([A-Z][A-Za-z0-9]+)\s*,", body, re.M)
    snake = {_snake_case(v) for v in variants}
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
    blind: list[str],
    errors: list[str],
) -> None:
    """The upstream comparisons. Split from main() so an UNEXPECTED exception
    here (a script bug, an exotic network failure outside FETCH_ERRORS) can be
    routed to the transient bucket with the partial report intact — without it
    the interpreter exits 1 and the workflow files a junk drift-titled
    issue from an empty report. The deliberate read-our-own-source LOUD path
    stays inside main(), before this is called, and still exits 1.

    Four buckets, because they call for four different actions. `breaking` is a
    VERIFIED upstream change: we read the surface that owns a name and the name
    is gone — fix the decoder. `blind` is probe health: a lookup missed, so all
    we know is that OUR probe failed — repin, and verify by hand before touching
    anything. `review` is a new upstream surface. `errors` is transient.
    Collapsing `blind` into `breaking` is what made #793 report five phantom
    renames as "decoder will silently drop events"."""
    # --- Codex hook events + rollout decode vocabulary (only the FETCH is
    #     transient). protocol.rs holds BOTH the HookEventName enum (hooks) and
    #     the EventMsg enum (rollout `event_msg` types); the `response_item` types
    #     live in the sibling models.rs (ResponseItem). ------------------------
    if codex_ours is not None or codex_rollout is not None:
        text = try_fetch(CODEX_PROTOCOL_URL, "Codex source", blind, errors)
        if text is not None and codex_ours is not None:
            upstream = upstream_codex_hooks(text)
            if upstream is None:
                blind.append(
                    probe_failed(
                        "the Codex `HookEventName` enum",
                        "codex-rs/protocol/src/protocol.rs",
                        "The Codex hook-event watch was SKIPPED.",
                    )
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
                blind.append(
                    probe_failed(
                        "the Codex `EventMsg` enum",
                        "protocol.rs",
                        "The rollout event_msg watch was SKIPPED.",
                    )
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
        # Rollout OUTER types → the `RolloutItem` enum in the SAME protocol.rs.
        # A NEW upstream outer NOT in KNOWN_OUTERS makes the transcript tail
        # `drift::unknown_event` on EVERY line of it (no dedup) → warn-floor
        # flood, so a new outer is a REVIEW ping to add it to KNOWN_OUTERS
        # (decode it or knowingly ignore it). The reverse (a KNOWN_OUTERS member
        # gone upstream) is a benign stale silent-set entry — no breaking alarm.
        if text is not None and codex_rollout is not None:
            up_outers = upstream_codex_enum_types(text, "RolloutItem")
            if up_outers is None:
                blind.append(
                    probe_failed(
                        "the Codex `RolloutItem` enum",
                        "protocol.rs",
                        "The rollout OUTER flood guard was SKIPPED.",
                    )
                )
            else:
                known_outers = read_codex_rollout_outers()
                for t in sorted(up_outers - known_outers):
                    review.append(
                        f"new Codex rollout OUTER `{t}` upstream (`RolloutItem`) not "
                        f"in KNOWN_OUTERS (source/codex.rs) — the transcript tail will "
                        f"breadcrumb EVERY line of it (drift flood); add it to "
                        f"KNOWN_OUTERS (decode it, or knowingly ignore it)."
                    )
        # `turn_context` FIELD survival (burn tier, #541): the transcript
        # decoder reads `model` + `effort` off every turn_context line
        # (source/codex.rs) — a rename silently kills the model badge/flame
        # (fail-quiet by design, so this watch is the only alarm).
        if text is not None:
            tc_fields = codex_turn_context_fields(text)
            if tc_fields is None:
                blind.append(
                    probe_failed(
                        "the Codex `TurnContextItem` struct",
                        "protocol.rs",
                        "The burn-tier model/effort watch was SKIPPED.",
                    )
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
            models = try_fetch(CODEX_MODELS_URL, "Codex models", blind, errors)
            if models is not None:
                up_ri = upstream_codex_enum_types(models, "ResponseItem")
                if up_ri is None:
                    blind.append(
                        probe_failed(
                            "the Codex `ResponseItem` enum",
                            "models.rs",
                            "The rollout response_item watch was SKIPPED.",
                        )
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
        text = try_fetch(REASONIX_HOOK_URL, "Reasonix source", blind, errors)
        if text is not None:
            upstream = upstream_reasonix_hooks(text)
            if upstream is None:
                blind.append(
                    probe_failed(
                        "the Reasonix `Event` consts",
                        "internal/hook/hook.go",
                        "The Reasonix event AND payload-field watches were SKIPPED.",
                    )
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
        text = try_fetch(CODEWHALE_HOOK_URL, "CodeWhale source", blind, errors)
        if text is not None:
            upstream = upstream_codewhale_hooks(text)
            if upstream is None:
                blind.append(
                    probe_failed(
                        "the CodeWhale `pub enum HookEvent`",
                        "crates/tui/src/hooks/config.rs",
                        "The CodeWhale event watch was SKIPPED.",
                    )
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
        # `HookContext::to_env_vars`, a SEPARATE file from the enum since the
        # hooks.rs -> hooks/ split. ONE-DIRECTIONAL. Its own fetch, so a failure
        # to read the executor can't be mistaken for every env var vanishing.
        exec_text = fetch_anchored(CODEWHALE_EXECUTOR_URL, "CodeWhale executor", blind, errors)
        if exec_text is not None:
            for field in sorted(CODEWHALE_ENV_FIELDS):
                if f'"{field}"' not in exec_text:
                    breaking.append(
                        f"CodeWhale env var `{field}` (folded by the shim's env-mode "
                        f"into the {{cwd,tool,tool_args}} envelope) is GONE from "
                        f"hooks/executor.rs `to_env_vars` — renamed; the shim reads "
                        f"None, the envelope omits its field, and the cwd-keyed "
                        f"decoder drops the event (empty cwd = no sprite / no activity)."
                    )

    # --- opencode EventV2 types (only the FETCH is transient) --------------
    if opencode_ours is not None:
        parts = [
            fetch_anchored(u, "opencode source", blind, errors)
            for u in OPENCODE_EVENT_URLS
        ]
        # If ANY url failed OR lost its anchor, skip the check — a partial concat
        # would false-positive a depended type as "GONE" just because it lived in
        # the half we couldn't read. (fetch_anchored already classified each.)
        readable = [p for p in parts if p is not None]
        text = "\n".join(readable) if len(readable) == len(parts) else None
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
        text = fetch_anchored(GROK_HOOK_URL, "grok hooks source", blind, errors)
        if text is not None:
            upstream = upstream_grok_hooks(text)
            if upstream is None:
                blind.append(
                    probe_failed(
                        "the grok `HookEventName` variants (plain enum or "
                        "`hook_events!` table)",
                        "xai-grok-hooks/src/event.rs",
                        "The grok event watch was SKIPPED.",
                    )
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
        text = fetch_anchored(GROK_NOTIFICATION_URL, "grok notification source", blind, errors)
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
        text = fetch_anchored(GROK_ACTIVE_SESSIONS_URL, "grok active-sessions source", blind, errors)
        if text is not None:
            for ident in sorted(GROK_ACTIVE_SESSION_FIELDS):
                if not re.search(rf"(?m)^\s*(pub )?{ident}:", text):
                    breaking.append(
                        f"grok active_sessions field `{ident}` is GONE from "
                        f"active_sessions.rs — renamed; grok_ids_from_registry stops "
                        f"parsing and the WHOLE liveness ladder (probe / instant exit / "
                        f"negative vouch / focus) degrades to mtime gating."
                    )

    # --- ACP v1 SessionUpdate tags — the shared source/acp.rs flood guard (#766) --
    # A NEW upstream v1 tag NOT in KNOWN_ACP_TAGS makes `acp::decode_session_update`
    # `drift::unknown_event` on EVERY line of it (no dedup) → warn-floor flood, so a
    # new tag is a REVIEW ping to add it (decode it or knowingly ignore it). grok
    # pins features=["unstable"], so its surface is the UNION of v1 stable + unstable;
    # we fetch both. Review-class, never breaking — the reverse (a KNOWN_ACP_TAGS
    # member gone upstream) is a benign stale entry.
    known_acp = read_acp_tags()
    up_acp: set[str] = set()
    acp_parsed = False
    for url, label in (
        (ACP_V1_SCHEMA_URL, "ACP v1 schema"),
        (ACP_V1_SCHEMA_UNSTABLE_URL, "ACP v1 unstable schema"),
    ):
        text = try_fetch(url, label, blind, errors)
        if text is not None:
            tags = upstream_acp_session_update_tags(text)
            if tags is None:
                blind.append(
                    probe_failed(
                        "the ACP `SessionUpdate` oneOf union",
                        label,
                        "The ACP tag flood guard was SKIPPED.",
                    )
                )
            else:
                up_acp |= tags
                acp_parsed = True
    if acp_parsed:
        for tag in sorted(up_acp - known_acp):
            review.append(
                f"new ACP v1 `sessionUpdate` tag `{tag}` upstream not in KNOWN_ACP_TAGS "
                f"(source/acp.rs) — the ACP tag tier will breadcrumb EVERY line of it "
                f"(drift flood); add it to KNOWN_ACP_TAGS (decode it, or knowingly ignore it)."
            )

    # --- Copilot event types (only the FETCH is transient) -----------------
    if copilot_ours is not None:
        text = try_fetch(COPILOT_SCHEMA_URL, "Copilot schema", blind, errors)
        # The `SessionEvent` union is this document's ANCHOR — it is the
        # declaration that owns every envelope `type` and, transitively, every
        # `data.properties` key we check. It gets the same no-anchor-no-sweep
        # treatment as `fetch_anchored`'s documents, just expressed structurally
        # because the proof is a JSON shape rather than a regex.
        #
        # It is NOT enough that the JSON parses: `upstream_copilot_field_names`
        # unions every `properties` key at ANY depth, so a restructured schema
        # with ONE unrelated `properties` object satisfies its `is None` guard
        # and reports all 12 depended fields as verified renames — 13 phantom
        # "decoder will silently drop events" lines, the #793 shape exactly.
        # Not hypothetical: `@github/copilot` already became a loader stub once
        # (#406), and COPILOT_SCHEMA_URL is deliberately UNPINNED (it follows
        # unpkg's redirect to latest), so the shape can change under us.
        up_ns = upstream_copilot_namespaces(text) if text is not None else None
        if text is not None and up_ns is None:
            blind.append(
                probe_failed(
                    "the Copilot `SessionEvent` anyOf union",
                    COPILOT_SCHEMA_URL,
                    "EVERY Copilot check (event types, payload fields, the "
                    "namespace flood guard) was SKIPPED — an unproven schema "
                    "cannot tell a rename from a restructure.",
                )
            )
        if text is not None and up_ns is not None:
            upstream = upstream_copilot_events(text)
            if upstream is None:
                blind.append(
                    probe_failed(
                        "any parseable `type` const in the Copilot session-events schema",
                        COPILOT_SCHEMA_URL,
                        "The Copilot event watch was SKIPPED.",
                    )
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
                blind.append(
                    probe_failed(
                        "the Copilot schema `properties` keys",
                        COPILOT_SCHEMA_URL,
                        "The Copilot payload-field watch was SKIPPED.",
                    )
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
            # Event NAMESPACES → the family axis the transcript tail flood-guards.
            # A NEW upstream namespace NOT in KNOWN_NAMESPACES makes the tail
            # `drift::unknown_event` on EVERY line of that family (no dedup) →
            # warn-floor flood, so a new namespace is a REVIEW ping to add it to
            # KNOWN_NAMESPACES (decode it or knowingly ignore it). The reverse (a
            # KNOWN_NAMESPACES member gone upstream) is a benign stale entry.
            # `up_ns` is the anchor parsed above — reused, not re-derived.
            known_ns = read_copilot_namespaces()
            for ns in sorted(up_ns - known_ns):
                review.append(
                        f"new Copilot event NAMESPACE `{ns}` upstream (`SessionEvent`) "
                        f"not in KNOWN_NAMESPACES (source/copilot.rs) — the transcript "
                        f"tail will breadcrumb EVERY line of it (drift flood); add it "
                        f"to KNOWN_NAMESPACES (decode it, or knowingly ignore it)."
                    )

    # --- omp session-entry types + wire names (only the FETCH is transient) --
    if omp_ours is not None:
        text = fetch_anchored(OMP_SESSION_ENTRIES_URL, "omp session-entries", blind, errors)
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
            # Entry TYPES → the axis the transcript tail flood-guards. A NEW
            # upstream entry type NOT in KNOWN_ENTRY_TYPES makes the tail
            # `drift::unknown_event` on EVERY line of it (no dedup) → warn-floor
            # flood, so a new type is a REVIEW ping to add it to KNOWN_ENTRY_TYPES
            # (decode it or knowingly ignore it).
            up_types = upstream_omp_entry_types(text)
            if up_types is None:
                blind.append(
                    probe_failed(
                        "the omp entry-`type` literals",
                        "session-entries.ts",
                        "The omp entry-type flood guard was SKIPPED.",
                    )
                )
            else:
                known_types = read_omp_known_types()
                for t in sorted(up_types - known_types):
                    review.append(
                        f"new omp entry TYPE `{t}` upstream (session-entries.ts) not "
                        f"in KNOWN_ENTRY_TYPES (source/omp.rs) — the transcript tail "
                        f"will breadcrumb EVERY line of it (drift flood); add it to "
                        f"KNOWN_ENTRY_TYPES (decode it, or knowingly ignore it)."
                    )
        diag = fetch_anchored(OMP_EXIT_DIAG_URL, "omp exit-diagnostics", blind, errors)
        if diag is not None and '"session_exit"' not in diag:
            breaking.append(
                "omp customType `session_exit` (the clean-teardown marker the "
                "session-ended checker + SessionEnd decode key on) is GONE from "
                "exit-diagnostics.ts — renamed; finished sessions resurrect at "
                "first sight and never SessionEnd."
            )
        ai = fetch_anchored(OMP_AI_TYPES_URL, "omp pi-ai types", blind, errors)
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
        ask = fetch_anchored(OMP_ASK_URL, "omp ask tool", blind, errors)
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
        text = fetch_anchored(CURSOR_HOOKS_URL, "Cursor hooks doc", blind, errors)
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
        text = fetch_anchored(OPENCLAW_HOOK_TYPES_URL, "OpenClaw hook-types", blind, errors)
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
            # The gateway-identity carriers the plugin reads `port` off.
            for ty in sorted(OPENCLAW_GATEWAY_PORT_TYPES):
                if ty not in text:
                    breaking.append(
                        f"OpenClaw type `{ty}` (the plugin reads the gateway `port` off it "
                        f"for the mascot's instance identity) is GONE from "
                        f"src/plugins/hook-types.ts — renamed; every envelope would carry "
                        f"the registration-time fallback port, so concurrent gateways "
                        f"collapse onto one mascot."
                    )

    # --- OpenClaw default gateway port (the plugin's copied fallback) --------
    text = try_fetch(OPENCLAW_PATHS_URL, "OpenClaw config/paths", blind, errors)
    if text is not None:
        ours = openclaw_plugin_default_port()
        m = re.search(r"DEFAULT_GATEWAY_PORT\s*=\s*(\d+)", text)
        if m is None:
            blind.append(
                probe_failed(
                    "OpenClaw's `DEFAULT_GATEWAY_PORT`",
                    "src/config/paths.ts",
                    "The plugin's fallback-port comparison was SKIPPED.",
                )
            )
        elif ours is None:
            blind.append(
                probe_failed(
                    "our own `DEFAULT_GATEWAY_PORT` copy",
                    "install/openclaw_plugin.js",
                    "The upstream-port comparison had nothing to compare against "
                    "and was SKIPPED (did OUR const get renamed?).",
                    our_source=True,
                )
            )
        elif m.group(1) != ours:
            breaking.append(
                f"OpenClaw's DEFAULT_GATEWAY_PORT is now {m.group(1)} but "
                f"openclaw_plugin.js still falls back to {ours} — a gateway on the new "
                f"default would be stamped with the stale port (a phantom second mascot "
                f"until its TTL sweeps it)."
            )

    # --- Hermes shell-hook events + payload fields (only the FETCH is transient)
    if hermes_ours is not None:
        text = fetch_anchored(HERMES_HOOK_URL, "Hermes hooks", blind, errors)
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
        shell = fetch_anchored(HERMES_SHELL_HOOK_URL, "Hermes shell_hooks", blind, errors)
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
        text = fetch_anchored(KIMI_HOOKS_URL, "Kimi hooks doc", blind, errors)
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
        tools = fetch_anchored(CC_TOOLS_URL, "CC tools-reference", blind, errors)
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
    hooks_doc = fetch_anchored(CC_HOOKS_URL, "CC hooks doc", blind, errors)
    if hooks_doc is not None:
        if cc_ours is not None:
            upstream = upstream_cc_hook_events(hooks_doc)
            if upstream is None:
                blind.append(
                    probe_failed(
                        "the CC hook-event summary table",
                        "hooks.md",
                        "The CC event watch was SKIPPED.",
                    )
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
    blind: list[str] = []
    errors: list[str] = []

    # Read what WE depend on from our OWN source first. A failure here means the
    # monitor itself is broken (decoder.rs / install/codex.rs refactored away from
    # what the parsers expect) — that is a LOUD PROBE-HEALTH signal (still exit 1;
    # our own parsers being stale is the textbook case of "the probe missed"),
    # never a transient one, or drift monitoring would silently stop with zero alarm.
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
        blind.append(
            probe_failed(
                f"what WE depend on, reading our own source ({e})",
                "check_upstream_drift.py's parsers",
                "The parsers are stale (decoder.rs / install refactored?) and the "
                "monitor is blind until the script is fixed. Nothing upstream was "
                "checked.",
                our_source=True,
            )
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
            blind,
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
    # The H1 IS the GitHub issue title (upstream-drift.yml reads it back rather
    # than keeping a second, unpinned copy of these strings).
    title = (
        "Upstream CLI wire-format drift detected"
        if breaking
        else "New upstream events to review"
        if review
        else "Upstream drift watch could not verify — repin needed"
        if blind
        else "Upstream wire-format watch: no drift"
    )
    out = [f"# {title}", ""]
    if breaking:
        out.append("## ⛔ Verified upstream change — decoder will silently drop events")
        out.append("")
        out.append(
            "Each line was read off the upstream surface that OWNS the name: the "
            "declaration is present and the name is gone. Fix the decoder."
        )
        out.append("")
        out += [f"- {b}" for b in breaking]
        out.append("")
    if review:
        out.append("## 🔎 New upstream events to review")
        out += [f"- {r}" for r in review]
        out.append("")
    if blind:
        out.append("## 🩺 Probe could NOT verify — this is not evidence of upstream change")
        out.append("")
        out.append(
            "A lookup missed, so all the watcher knows is that **its own probe "
            "failed**. A stale pin, a moved declaration, or a refactor that left a "
            "re-export facade at the old path all land here, and every check "
            "riding the affected surface was SKIPPED rather than reported as "
            "drift. **Verify upstream by hand and repin — do NOT change a decoder "
            "on these lines alone** (#793 did, against three working env vars)."
        )
        out.append("")
        out += [f"- {b}" for b in blind]
        out.append("")
    if errors:
        out.append("## ⚠️ Could not verify (transient network/HTTP — not drift)")
        out += [f"- {e}" for e in errors]
        out.append("")
    if not (breaking or review or blind or errors):
        out.append("✅ No drift. Every name we depend on is present upstream.")
    print("\n".join(out))

    # `blind` is actionable (the watch is dark until repinned) so it keeps
    # exit 1 and files an issue — but the report above no longer calls it drift.
    if breaking or review or blind:
        return 1
    if errors:
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
