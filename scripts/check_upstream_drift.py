#!/usr/bin/env python3
"""Upstream wire-format drift watch.

pixtuoid decodes agent-CLI wire formats (hook event names, the subagent-dispatch
tool name, transcript type tags) whose names change upstream WITHOUT notice — the
`Task` -> `Agent` rename shipped undocumented and silently disabled subagent
suppression. This verifies that every name we depend on still exists at its
canonical upstream source, reading what we depend on from our OWN source (no
snapshot file to rot).

Findings carry a DISPOSITION (see `Report`), because "upstream changed" and "our
probe missed" need different work and only one of them is a statement about
upstream. Every document swept for name-presence declares an ANCHOR (see
`ANCHORS`) proving it is still the document that owns those names — no anchor, no
sweep: a stale pin that still returns 200 as a re-export facade otherwise reads as
mass drift, and #793's report would have renamed three WORKING env vars.

Exit codes:
  0  no findings
  1  actionable (verified drift, a review ping, OR probe health — all three need
     a human) -> open a tracking issue
  2  could not check (network/HTTP error) -> transient, do NOT alarm

See crates/pixtuoid-core/CLAUDE.md "Keeping the decode mapping current".
"""

from __future__ import annotations

import dataclasses
import http.client
import json
import pathlib
import re
import sys
import traceback
import typing
import urllib.error
import urllib.request

# URLError alone is not enough: the READ phase inside fetch() raises raw
# socket.timeout / ConnectionResetError (OSError, NOT URLError) and
# http.client.IncompleteRead (HTTPException) — left uncaught they exit 1 and the
# workflow files a junk drift-titled issue from an empty report.
FETCH_ERRORS = (urllib.error.URLError, OSError, http.client.HTTPException)

# A permanent status means OUR pinned path is gone, so the watch is BLIND for that
# source: probe health, never transient. The trap it guards: `HTTPError` subclasses
# `URLError` ⊂ FETCH_ERRORS, so a 404 used to bucket as transient and the weekly
# job stayed green while silently watching nothing.
PERMANENT_HTTP_STATUS = frozenset({404, 410, 451})

REPO = pathlib.Path(__file__).resolve().parent.parent

CODEX_PROTOCOL_URL = (
    "https://raw.githubusercontent.com/openai/codex/main/"
    "codex-rs/protocol/src/protocol.rs"
)
# The ROLLOUT `response_item` types live in the sibling models.rs
# (`crate::models::ResponseItem`), NOT protocol.rs.
CODEX_MODELS_URL = (
    "https://raw.githubusercontent.com/openai/codex/main/"
    "codex-rs/protocol/src/models.rs"
)
CC_TOOLS_URL = "https://code.claude.com/docs/en/tools-reference.md"
CC_HOOKS_URL = "https://code.claude.com/docs/en/hooks.md"

# Documented hooks.md surfaces the burn-tier decoder DEPENDS on: a string
# VANISHING here is review-class drift (the docs renamed a surface we read).
CC_DEPENDED_DOC_MARKERS = {
    "CLAUDE_EFFORT": "the hook-payload effort surface (effort.level, burn tier)",
    "receive a `model` field": "SessionStart's optional model field (burn tier)",
}

# APPEARANCE watch, the inverse direction: these are ABSENT from hooks.md today
# and a HIT is review-class (a surface to adopt), never breaking. CC is a closed
# binary, so the docs are the only watchable surface. `session_end` is snake_case
# on purpose — the SessionEnd HOOK name is all over hooks.md and must not match.
# Adopting a structural end record also means the liveness-probe first-sight
# bypass (`probe_admits`, core's source/jsonl.rs) needs an ended-check: it skips
# the gate's ended tail-scan only because no such marker exists.
CC_LIFECYCLE_SURFACE_MARKERS = {
    "session_end": 'a structural transcript end record (subtype:"session_end")',
    ".claude/sessions/": "the ~/.claude/sessions/<pid>.json session registry",
    "procStart": "the sessions-registry procStart field",
    # The CC decoder synthesizes effort labels from these undocumented markers;
    # documenting them upstream is a ping to re-verify our synthesized shape.
    "ultra_effort_enter": "the ultra-effort transcript attachment marker",
    "ultrathink_effort": "the ultrathink transcript attachment marker",
    "ultra_effort_exit": "the ultra-effort EXIT attachment marker (instant flame-off)",
}

# Codex hooks we DELIBERATELY do not register: compaction internals, not agent
# activity a visualizer cares about.
CODEX_KNOWN_OMITTED = {"PreCompact", "PostCompact"}

# CC hooks we DELIBERATELY do not register: per-turn/content noise, permission
# detail already covered by Notification, task/teammate bookkeeping,
# environment/config plumbing, compaction internals.
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

# Reasonix hooks we DELIBERATELY do not register: PostLLMCall is per-turn noise,
# PreCompact a compaction internal, SubagentStop carries no ids. Registering
# PostToolUseFailure/StopFailure would DOUBLE-FIRE every failed tool/turn — the
# runner already re-fires the native PostToolUse/Stop hooks we install with the
# event merely re-labeled.
REASONIX_KNOWN_OMITTED = {
    "PostLLMCall",
    "PreCompact",
    "SubagentStop",
    "PostToolUseFailure",
    "StopFailure",
}

# Payload fields decode_rx_hook_payload reads — a renamed json tag silently
# zeroes the decode (`event`/`cwd` are load-bearing: a payload lacking them is
# rejected as malformed).
REASONIX_PAYLOAD_FIELDS = {"event", "cwd", "toolName", "toolArgs", "subject", "message"}

CODEWHALE_HOOK_URL = (
    "https://raw.githubusercontent.com/Hmbown/CodeWhale/main/"
    "crates/tui/src/hooks/config.rs"
)
CODEWHALE_EXECUTOR_URL = (
    "https://raw.githubusercontent.com/Hmbown/CodeWhale/main/"
    "crates/tui/src/hooks/executor.rs"
)

# CodeWhale hooks we DELIBERATELY do not register (snake_case wire names):
# turn_end is per-turn telemetry, the rest are not agent activity we show.
CODEWHALE_KNOWN_OMITTED = {
    "turn_end",
    "mode_change",
    "on_error",
    "shell_env",
}

# CodeWhale ENV-MODE identity: the DEEPSEEK_* names our shim folds into the
# cwd-keyed envelope, set by `HookContext::to_env_vars`. WORKSPACE is
# load-bearing — it becomes the envelope `cwd` = the AgentId KEY, so a rename
# drops EVERY session (no sprite). DEEPSEEK_SESSION_ID is deliberately NOT read
# (proven inconsistent), and the raw subagent-JSON fields are left to the
# decoder's own parentless-degrade because a fuzzy `json!` macro owns them.
CODEWHALE_ENV_FIELDS = {"DEEPSEEK_WORKSPACE", "DEEPSEEK_TOOL_NAME", "DEEPSEEK_TOOL_ARGS"}

# ONE-DIRECTIONAL: opencode emits far more EventV2 types than we map, so a new
# upstream event is noise; only a type WE DEPEND ON vanishing alarms. Track the
# `dev` branch (the active default) and `packages/schema/` (the old
# `packages/core` path is a re-export shim that 200s but greps empty).
OPENCODE_EVENT_URLS = (
    "https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/schema/src/v1/session.ts",
    "https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/schema/src/permission.ts",
)

# `permission.asked` is decoded DEFENSIVELY (a V1/alias spelling); only
# `permission.v2.asked` is a guaranteed standalone upstream EventV2 definition, so
# don't alarm if the bare form isn't found as a `type:` literal.
OPENCODE_TOLERATED = {"permission.asked"}

# Payload FIELD names decode_oc_hook_payload reads, as `field:` property lines in
# the schema Struct defs. A rename → the decoder reads None → wrong-register /
# no-link / no-activity.
OPENCODE_PAYLOAD_FIELDS = {
    "info", "id", "parentID", "directory",
    "part", "sessionID", "callID", "tool", "state", "status", "input",
    # The `info.model` wrapper (the decoder reads `model.id`; `id` is above).
    "model",
}

# Deliberately UNPINNED: the bare unpkg path 302-redirects to the latest
# published version, and a drift watch wants the latest shape, not a frozen one.
# The schema ships inside the platform packages (`@github/copilot` itself is now a
# loader stub); linux-x64 matches the CI host and every platform package carries
# an identical copy. ONE-DIRECTIONAL like opencode: only a depended type vanishing
# alarms.
COPILOT_SCHEMA_URL = "https://unpkg.com/@github/copilot-linux-x64/schemas/session-events.schema.json"

# Payload FIELD names decode_copilot_line / extract_copilot_cwd read, CURATED —
# not scraped, which would drag in opaque tool-arg keys and fixture JSON. `data`
# is the envelope every other field below lives under, so renaming that one key
# nulls all of them while the nested names still resolve (the all-depth union
# check would NOT alarm). The wire `parentId` is deliberately absent: sub-agents
# link via the envelope `agentId`, so watching it would alarm on a non-dependency.
COPILOT_PAYLOAD_FIELDS = {
    "data",
    "agentId", "sessionId", "context", "cwd",
    "toolCallId", "toolName", "arguments", "agentDisplayName",
    "permissionRequest", "result", "kind",
    "model",
}

# Cursor is a closed binary, so — like CC — the docs are the only watchable
# surface. ONE-DIRECTIONAL: only a depended event vanishing alarms. The
# common-word event `stop` is intrinsically low-confidence (the page contains the
# word regardless), so its disappearance can be masked; the distinctive
# `sessionStart`/`sessionEnd`/`preToolUse`/`postToolUse` carry the check.
CURSOR_HOOKS_URL = "https://cursor.com/docs/hooks"

# The canonical OpenClaw hook-name union, as quoted string literals.
# ONE-DIRECTIONAL: a hook WE REGISTER vanishing means the plugin registers
# something OpenClaw never fires, so the mascot silently goes dark.
OPENCLAW_HOOK_TYPES_URL = (
    "https://raw.githubusercontent.com/openclaw/openclaw/main/src/plugins/hook-types.ts"
)

# Payload FIELD names decode_openclaw_presence reads: `runId` (the in-flight run
# key), `sessionId` (fallback key + label), `success` (agent_end → Degraded gate).
# `success` is a common word, so a rename of it could be masked by an unrelated
# occurrence; the distinctive `runId`/`sessionId` carry the check.
OPENCLAW_PAYLOAD_FIELDS = {"runId", "sessionId", "success"}

# The gateway PORT is pixtuoid's runtime identity for one gateway, read off
# `gateway_start`'s event/ctx: a rename leaves every envelope stamped with the
# registration-time FALLBACK, so two live gateways collapse onto one mascot. The
# type names are watched instead of the common word `port`.
OPENCLAW_GATEWAY_PORT_TYPES = {"PluginHookGatewayStartEvent", "PluginHookGatewayContext"}

# The plugin cannot IMPORT upstream's `DEFAULT_GATEWAY_PORT` (it lives in
# OpenClaw's state dir, outside any `node_modules/openclaw`), so the literal is
# copied into `openclaw_plugin.js` — a copied constant is a latent drift bug
# unless something watches it. This is that watch.
OPENCLAW_PATHS_URL = (
    "https://raw.githubusercontent.com/openclaw/openclaw/main/src/config/paths.ts"
)

# The canonical Hermes shell-hook event set is the KEYS of `_DEFAULT_PAYLOADS`.
# ONE-DIRECTIONAL: an event WE REGISTER vanishing means the shell hook we install
# into config.yaml fires nothing (no sprite).
HERMES_HOOK_URL = (
    "https://raw.githubusercontent.com/NousResearch/hermes-agent/main/hermes_cli/hooks.py"
)
# The shell-hook PAYLOAD is assembled by `_serialize_payload()` in a DIFFERENT
# file from the event list — two orthogonal checks, two files.
HERMES_SHELL_HOOK_URL = (
    "https://raw.githubusercontent.com/NousResearch/hermes-agent/main/agent/shell_hooks.py"
)
# Field names decode_hermes_hook_payload reads, as dict-key literals in
# _serialize_payload; a rename → the JSON omits it → the decoder reads None.
HERMES_PAYLOAD_FIELDS = {"session_id", "cwd", "tool_name", "tool_input"}

# Kimi is a pnpm/TS monorepo, but the canonical hook-event list lives in the docs
# (each name appears verbatim in the summary table AND the payload examples), so —
# like Cursor — the raw markdown is the watchable surface. ONE-DIRECTIONAL. The
# common-word event `Stop` is intrinsically low-confidence (the doc contains the
# word regardless); the distinctive PascalCase names carry the check.
KIMI_HOOKS_URL = (
    "https://raw.githubusercontent.com/MoonshotAI/kimi-code/main/"
    "docs/en/customization/hooks.md"
)

# Three depended surfaces in three files: the hook event enum + payload serde,
# the transcript xAI-extension vocabulary, and the active_sessions.json
# liveness-registry struct. The ACP half of the transcript vocabulary lives in the
# external agent-client-protocol crate and is deliberately NOT fetched here — a
# versioned protocol, pinned in-repo by grok's own wire_tags guard test.
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
# grok hooks we DELIBERATELY do not register: compaction internals, not agent
# activity. SubagentEnd/SubagentStop are BOTH registered (upstream's finish site
# fires one spelling, the docs name the other), so neither belongs here.
GROK_KNOWN_OMITTED = {"PreCompact", "PostCompact"}
# Hook fields decode_grok_hook_payload reads, split by serde origin: ENVELOPE
# fields are camelCase via struct-level rename_all, so the wire name never appears
# literally and the `pub <ident>:` declarations are what to check; PAYLOAD fields
# carry explicit `rename = "…"` attrs; two payload fields are un-renamed idents.
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
# Transcript vocabulary decode_grok_line reads from the xAI SessionUpdate enum:
# variant IDENTS, since the snake_case tags derive via rename_all and the wire
# string never appears literally.
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
# whole liveness ladder to mtime gating.
GROK_ACTIVE_SESSION_FIELDS = {"session_id", "pid", "cwd", "opened_at"}
# grok pins `features = ["unstable"]`, so its real ACP surface is the UNION of the
# v1 stable + v1 unstable tag sets — hence two fetches. v2 is a separate, partly
# non-overlapping line grok does NOT speak, so fetching it would emit false
# "adopt terminal_update" noise.
ACP_V1_SCHEMA_URL = (
    "https://raw.githubusercontent.com/agentclientprotocol/"
    "agent-client-protocol/main/schema/v1/schema.json"
)
ACP_V1_SCHEMA_UNSTABLE_URL = (
    "https://raw.githubusercontent.com/agentclientprotocol/"
    "agent-client-protocol/main/schema/v1/schema.unstable.json"
)

# omp is TRANSCRIPT-ONLY, and its depended names are split across the upstream
# files that define them. ONE-DIRECTIONAL like copilot: only a name WE DEPEND ON
# vanishing is breaking (the transcript still flows but decodes to nothing).
OMP_SESSION_ENTRIES_URL = (
    "https://raw.githubusercontent.com/can1357/oh-my-pi/main/packages/coding-agent/src/session/session-entries.ts"
)
OMP_SESSION_ENTRY_FIELDS = {"cwd", "customType"}
# The clean-teardown marker both the session-ended checker and the SessionEnd
# decode key on.
OMP_EXIT_DIAG_URL = (
    "https://raw.githubusercontent.com/can1357/oh-my-pi/main/packages/coding-agent/src/session/exit-diagnostics.ts"
)
# Message-level names (roles + tool-call block shape) live in the pi-ai LLM types:
# quoted TS literals for the roles/block type, property keys for the fields.
OMP_AI_TYPES_URL = (
    "https://raw.githubusercontent.com/can1357/oh-my-pi/main/packages/ai/src/types.ts"
)
OMP_MESSAGE_LITERALS = {"assistant", "toolResult", "toolCall"}
OMP_MESSAGE_FIELDS = {"toolCallId", "arguments", "model"}
# The ask tool's NAME is STATE-bearing — decode_omp_line maps an assistant `ask`
# block to Waiting — and the first question's text feeds the Waiting reason.
# `arguments.i` (the intent fallback) is harness-wide, not defined in ask.ts, and
# its loss only degrades the label, so it is deliberately unwatched.
OMP_ASK_URL = (
    "https://raw.githubusercontent.com/can1357/oh-my-pi/main/packages/coding-agent/src/tools/ask.ts"
)
OMP_ASK_FIELDS = {"questions", "question"}


class Anchor(typing.NamedTuple):
    """The declaration that OWNS the names we check a fetched document for."""

    pattern: str
    owns: str


# Absence is only evidence of an upstream RENAME if the document still contains
# the declaration that OWNS the name; drop that premise and a stale pin reads as
# mass drift (#793 reported three working env vars as renamed, because a moved
# module left a `pub use` facade that fetched 200 and greps empty).
#
# Choosing one, in descending strength — take the strongest available, because
# THIS comment is what picks the next anchor:
#   1. The DECLARATION that owns the checked names, so an upstream move takes
#      both and the sweep cannot run against a document missing them.
#   2. Failing that, a declaration co-located with them in the same file — this
#      proves file IDENTITY, which is weaker: "declaration X moved out while Y
#      stayed" satisfies the anchor and still reports phantom renames. Rows
#      marked `identity` below are that weaker grade; they are not upgradeable
#      without a parser, and a docs PAGE can only ever be this.
#   3. Never one of the checked names itself — that is circular and makes the
#      check vacuous (a rename would take the anchor too, so it could never fire).
ANCHORS: dict[str, Anchor] = {
    # owner-grade: each anchor is the declaration the checked names live inside.
    CODEWHALE_EXECUTOR_URL: Anchor(r"fn to_env_vars", "`HookContext::to_env_vars`"),
    GROK_ACTIVE_SESSIONS_URL: Anchor(r"pub struct ActiveSession", "`ActiveSession`"),
    HERMES_HOOK_URL: Anchor(r"_DEFAULT_PAYLOADS", "`_DEFAULT_PAYLOADS`"),
    HERMES_SHELL_HOOK_URL: Anchor(r"_serialize_payload", "`_serialize_payload`"),
    OPENCLAW_HOOK_TYPES_URL: Anchor(r"export type PluginHookName", "the `PluginHookName` union"),
    # NOT `SessionNotification` (the obvious pick): it sits earlier in the file and
    # does not own the variants, so moving the enum out would leave it satisfied
    # while every variant read as renamed.
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


# Documents where a PARSER is the identity proof instead. Written down so the
# selftest can assert this set equals the live `try_fetch` call sites — otherwise
# the next unanchored sweep just uses that spelling to escape the gate.
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


@dataclasses.dataclass
class Report:
    """One bucket per DISPOSITION, because each calls for a different action:

    `breaking` = verified upstream change, fix the decoder; `review` = a new
    surface to adopt or ignore; `blind` = probe health, repin and verify by hand;
    `errors` = transient. Collapsing `blind` into `breaking` is what made #793
    report five phantom renames. File through `add_*` only — pinned by the
    selftest.
    """

    breaking: list[str] = dataclasses.field(default_factory=list)
    review: list[str] = dataclasses.field(default_factory=list)
    blind: list[str] = dataclasses.field(default_factory=list)
    errors: list[str] = dataclasses.field(default_factory=list)

    def add_breaking(self, line: str) -> None:
        self.breaking.append(line)

    def add_review(self, line: str) -> None:
        self.review.append(line)

    def add_error(self, line: str) -> None:
        self.errors.append(line)

    def add_blind(
        self, what: str, where: str, consequence: str, *, our_source: bool = False
    ) -> None:
        """A lookup missed. Composed here so the disclaimer always ships with it.

        `our_source=True` when the unreadable thing is OURS — the default wording
        blames upstream, which nothing verified.
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
        self.blind.append(
            f"could not verify {what} at {where} — {cause} {consequence} {action}"
        )

    def title(self) -> str:
        """The report's H1, which IS the GitHub issue title.

        upstream-drift.yml reads it back off the rendered report rather than
        keeping a second, unpinned copy of these strings. Ordered by disposition
        so the issue list itself carries the strongest finding.
        """
        if self.breaking:
            return "Upstream CLI wire-format drift detected"
        if self.review:
            return "New upstream events to review"
        if self.blind:
            return "Upstream drift watch could not verify — repin needed"
        return "Upstream wire-format watch: no drift"

    def render(self) -> str:
        out = [f"# {self.title()}", ""]
        if self.breaking:
            out.append("## ⛔ Verified upstream change — decoder will silently drop events")
            out.append("")
            out.append(
                "Each line was read off the upstream surface that OWNS the name: the "
                "declaration is present and the name is gone. Fix the decoder."
            )
            out.append("")
            out += [f"- {b}" for b in self.breaking]
            out.append("")
        if self.review:
            out.append("## 🔎 New upstream events to review")
            out += [f"- {r}" for r in self.review]
            out.append("")
        if self.blind:
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
            out += [f"- {b}" for b in self.blind]
            out.append("")
        if self.errors:
            out.append("## ⚠️ Could not verify (transient network/HTTP — not drift)")
            out += [f"- {e}" for e in self.errors]
            out.append("")
        if not (self.breaking or self.review or self.blind or self.errors):
            out.append("✅ No drift. Every name we depend on is present upstream.")
        return "\n".join(out)

    def exit_code(self) -> int:
        """0 no findings / 1 actionable / 2 could not check (transient).

        `blind` is actionable — the watch is DARK for that source until someone
        repins — so it exits 1 even though the report no longer calls it drift.
        Drop it from this condition and a report saying the watch is dark exits 0:
        both `exit == '1'` steps in upstream-drift.yml skip and the weekly run
        goes green (#454's fail-open).
        """
        if self.breaking or self.review or self.blind:
            return 1
        if self.errors:
            return 2
        return 0


def fetch(url: str) -> str:
    req = urllib.request.Request(url, headers={"User-Agent": "pixtuoid-drift-watch"})
    with urllib.request.urlopen(req, timeout=30) as resp:  # noqa: S310 (trusted hosts)
        return resp.read().decode("utf-8", "replace")


def try_fetch(url: str, label: str, report: Report) -> str | None:
    """Fetch `url`, classifying failures so a PERMANENT upstream move is loud.

    A `PERMANENT_HTTP_STATUS` means our pin is wrong/gone → `blind` (a fact about
    OUR pin, not about upstream's wire format); everything else → `errors`.
    Returns None on any failure, and the caller skips that source's checks.
    """
    try:
        return fetch(url)
    except urllib.error.HTTPError as e:
        if e.code in PERMANENT_HTTP_STATUS:
            report.add_blind(
                f"{label}",
                f"{url} (HTTP {e.code})",
                "Every check for this source was SKIPPED.",
            )
        else:
            report.add_error(f"{label}: transient HTTP {e.code} at {url}: {e}")
        return None
    except FETCH_ERRORS as e:
        report.add_error(f"{label}: fetch failed (transient?): {e}")
        return None


def fetch_anchored(url: str, label: str, report: Report) -> str | None:
    """`try_fetch` plus the identity proof required before sweeping a document.

    Returns the body only when the document still contains its `ANCHORS` entry.
    A missing anchor means the fetch succeeded but landed on the wrong content
    (a re-export facade, a restructured page), so every presence check that would
    have run is SKIPPED and reported as probe health instead of drift.

    An undeclared URL is reported, never swept — deliberately not raised:
    `run_checks` routes exceptions to the TRANSIENT bucket, so a bare `KeyError`
    would degrade "someone added an unproven sweep" into a warning on a green run.
    """
    anchor = ANCHORS.get(url)
    if anchor is None:
        report.add_blind(
            f"{label}: no ANCHORS entry declares what proves this document's identity",
            url,
            "It was NOT swept — an unproven document cannot distinguish an "
            "upstream rename from a stale pin. Add its anchor to ANCHORS "
            "(and a sample to the selftest's ANCHOR_SAMPLES).",
        )
        return None
    text = try_fetch(url, label, report)
    if text is None:
        return None
    if not re.search(anchor.pattern, text):
        report.add_blind(
            f"{label}: the document no longer contains {anchor.owns}",
            url,
            "It still fetches, so the probe landed on the wrong content — "
            "most likely a stale pin (a moved declaration, or a re-export "
            "facade left at the old path) rather than an upstream rename. "
            "Every presence check riding this document was SKIPPED, NOT "
            "reported as drift.",
        )
        return None
    return text


def rust_const_str_array(rel_path: str, const_name: str) -> set[str]:
    """The quoted words of a `const NAME: &[&str] = &[ … ];` array, COMMENTS EXCLUDED.

    Comment-stripping is load-bearing, not tidiness: a bare `"(\\w+)"` scrape over
    the raw block also captures every quoted word inside an explanatory comment.
    It fired for real — a WHY comment inside CODEX_EVENTS mentioning a `reason`
    payload const made the watcher read that const as a registered hook event and
    file a phantom breaking-drift issue. Every event reader shares this parser.
    """
    got = parse_rust_const_str_array((REPO / rel_path).read_text(), const_name)
    if got is None:
        raise RuntimeError(f"could not locate {const_name} in {rel_path}")
    return got


def strip_rust_comments(body: str) -> str:
    """Remove Rust comments, PRESERVING string literals and honouring nesting.

    A scanner rather than a pair of `re.sub`s, because both regex approaches drop
    a REAL registered event and so fail SILENTLY OPEN — the watcher stops checking
    a name and nothing says so:

    - blind `//[^\\n]*` eats to end of line from a `//` inside a STRING, taking
      every later entry on that line with it;
    - `/\\*.*?\\*/` stops at the first `*/`, so a nested block comment (legal Rust)
      leaves its tail behind and re-admits words from inside it.
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

    Bounding the scrape keeps out text the decoder does not depend on — a
    `#[cfg(test)] mod tests` constructing the same shape leaks a phantom, and a
    phantom makes the watcher alarm on a name upstream never had. Run the source
    through `strip_rust_comments` first so a brace inside a comment or string
    cannot move the bounds.
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
    """The pure half of `rust_const_str_array` — `None` when the const is absent."""
    m = re.search(rf"const {const_name}[^=]*=\s*&\[(.*?)\];", src, re.S)
    if not m:
        return None
    return set(re.findall(r'"(\w+)"', strip_rust_comments(m.group(1))))


def read_codex_events() -> set[str]:
    return rust_const_str_array("crates/pixtuoid/src/install/codex.rs", "CODEX_EVENTS")


def read_codex_rollout_types() -> tuple[set[str], set[str]]:
    """The (event_msg, response_item) inner `type` strings the codex TRANSCRIPT
    decoder matches on. These are registered NOWHERE and the decoder's
    `_ => vec![]` drops an unrecognized one SILENTLY (no `unknown_event`
    breadcrumb, unlike the hook decoders), so this positive check is the only
    backstop against an upstream rename going dark."""
    src = (REPO / "crates/pixtuoid-core/src/source/codex.rs").read_text()
    # Bounded to the decoder's own match block so the file's `mod tests` cannot
    # leak a phantom type into the depended set.
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
    """The rollout OUTER `type` discriminators the transcript decoder RECOGNIZES.

    The tail breadcrumbs any outer OUTSIDE this set and `drift::unknown_event` has
    NO dedup, so a new upstream variant would flood the warn-floor on every line
    of it — the report diffs it against upstream to ping BEFORE that happens."""
    return rust_const_str_array("crates/pixtuoid-core/src/source/codex.rs", "KNOWN_OUTERS")


def read_cc_events() -> set[str]:
    return rust_const_str_array("crates/pixtuoid/src/install/claude.rs", "EVENTS")


def read_dispatch_names() -> set[str]:
    src = (REPO / "crates/pixtuoid-core/src/source/decoder.rs").read_text()
    m = re.search(r"known_name\s*=\s*([^;]+);", src)
    if not m:
        raise RuntimeError("could not locate the dispatch known_name check in decoder.rs")
    # The captured span is an EXPRESSION, so a trailing comment naming a dropped
    # legacy tool name would otherwise read as a known name.
    return set(re.findall(r'"(\w+)"', strip_rust_comments(m.group(1))))


def upstream_codex_hooks(text: str) -> set[str] | None:
    body = _enum_body(text, "HookEventName")
    if body is None:
        return None
    return set(re.findall(r"\b([A-Z][A-Za-z]+)\b", body)) or None


def _snake_case(camel: str) -> str:
    return re.sub(r"(?<!^)(?=[A-Z])", "_", camel).lower()


def _enum_body(text: str, enum_name: str) -> str | None:
    """The brace-balanced body of `enum <enum_name> { … }`.

    THE enum-body reader for every Rust surface we watch — do not go back to a
    regex, because both spellings guess where the body ENDS and break on a
    harmless upstream refactor:

    * `(.*?)\\}` stops at the FIRST `}`, so one struct variant truncates the enum
      and every variant after it reads as GONE — a phantom rename per variant.
    * `(.*?)\\n\\}` demands a column-0 closing brace, so an INDENTED enum runs on
      to the next top-level one and a variant scrape admits the following `impl`
      block's CamelCase idents.

    Comments are stripped FIRST because they are scanned for braces otherwise: a
    `// see Foo { bar }` line inside the enum would unbalance the count.
    `strip_rust_comments` preserves string literals, so `rename = "…"` attrs that
    callers read out of the returned body survive.
    """
    text = strip_rust_comments(text)
    # `\\s*\\{` (not `\\b`) so a prefix name can't match a longer enum:
    # `HookEvent` must not bind to `enum HookEventName {`.
    m = re.search(rf"enum\s+{enum_name}\s*\{{", text)
    if not m:
        return None
    start = m.end() - 1
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
    """Iteratively strip innermost `(…)`/`{…}` so only top-level variant idents
    survive — else a CamelCase field/param TYPE reads as a variant.

    PRECONDITION: comments are already gone (every caller passes an `_enum_body`
    result). Do NOT re-strip them here with a naive `//[^\\n]*`: that eats to
    end-of-line from a `//` inside a STRING LITERAL, so one `rename = "http://…"`
    attr would silently delete every variant after it on that line.
    """
    prev = None
    while prev != s:
        prev = s
        s = re.sub(r"\([^()]*\)", "", s)
        s = re.sub(r"\{[^{}]*\}", "", s)
    return s


def upstream_codex_enum_types(text: str, enum_name: str) -> set[str] | None:
    """Serialized `type` tags of a codex `#[serde(tag="type", rename_all="snake_case")]`
    enum. Over-includes (a renamed variant keeps its snake_case form too), which is
    HARMLESS given a one-directional check. Returns None if the enum can't be
    located → the caller files probe health rather than claiming a rename."""
    body = _enum_body(text, enum_name)
    if body is None:
        return None
    # rename/alias literals must be read BEFORE `_strip_nested` eats the attr parens.
    names = set(re.findall(r'(?:rename|alias)\s*=\s*"([^"]+)"', body))
    names.update(_snake_case(v) for v in re.findall(r"\b([A-Z][A-Za-z0-9]*)\b", _strip_nested(body)))
    return names or None


def codex_function_call_fields(text: str) -> set[str] | None:
    """The field idents of the INLINE `ResponseItem::FunctionCall { … }` variant.

    Returns None if it isn't an inline struct — a GRACEFUL SKIP, not an alarm: a
    tuple-variant refactor serializes the SAME JSON, so the decoder still works
    and only this bonus check goes quiet."""
    m = re.search(r"FunctionCall\s*\{([^}]*)\}", text)
    if not m:
        return None
    return set(re.findall(r"\b([a-z_][a-z0-9_]*)\s*:", m.group(1)))


def codex_turn_context_fields(text: str) -> set[str] | None:
    """The field idents of the `TurnContextItem` struct — the burn-tier feature
    reads `model` + `effort` off every `turn_context` rollout line. Same
    graceful-skip contract as `codex_function_call_fields`."""
    m = re.search(r"pub struct TurnContextItem\s*\{([^}]*)\}", text)
    if not m:
        return None
    return set(re.findall(r"\bpub ([a-z_][a-z0-9_]*)\s*:", m.group(1)))


def upstream_cc_hook_events(text: str) -> set[str] | None:
    """The canonical event list is the "| Event | When it fires |" summary table —
    parse only its rows, since other tables repeat names with different columns."""
    m = re.search(r"^\|\s*Event\s*\|[^\n]*\n\|[\s:|-]*\n((?:\|[^\n]*\n)+)", text, re.M)
    if not m:
        return None
    return set(re.findall(r"^\|\s*`(\w+)`\s*\|", m.group(1), re.M)) or None


def read_reasonix_events() -> set[str]:
    return rust_const_str_array("crates/pixtuoid/src/install/reasonix.rs", "REASONIX_EVENTS")


def upstream_reasonix_hooks(text: str) -> set[str] | None:
    found = set(re.findall(r'\w+\s+Event\s*=\s*"(\w+)"', text))
    return found or None


def read_codewhale_events() -> set[str]:
    return rust_const_str_array("crates/pixtuoid/src/install/codewhale.rs", "CODEWHALE_EVENTS")


def read_opencode_events() -> set[str]:
    """The EventV2 `type` strings the decoder maps, read from the decoder's own
    `match event` block so the two stay in sync by construction."""
    src = (REPO / "crates/pixtuoid-core/src/source/opencode.rs").read_text()
    m = re.search(r"match event \{(.*?)\n    \}", src, re.S)
    if not m:
        raise RuntimeError("could not locate the `match event` block in source/opencode.rs")
    return set(re.findall(r'"((?:session|message|permission)\.[a-z0-9.]+)"', m.group(1)))


def read_omp_entry_types() -> set[str]:
    """The session-entry `type` strings decode_omp_line maps, read from the
    decoder's own `match kind` block so the two stay in sync by construction.
    Scoped to that block so the unit tests below (which embed the same strings as
    JSON) don't leak in."""
    src = (REPO / "crates/pixtuoid-core/src/source/omp.rs").read_text()
    m = re.search(r"let out = match kind \{(.*?)\n    \};", src, re.S)
    if not m:
        raise RuntimeError("could not locate the `match kind` block in source/omp.rs")
    # Arm-POSITION capture: a new decode arm is picked up automatically, while
    # arm-body literals belonging to the other upstream checks never leak in.
    return set(re.findall(r'(?m)^\s*"(\w+)"\s*(?:=>|if\b|$)', m.group(1)))


def read_omp_known_types() -> set[str]:
    """The COMPLETE session-entry `type` set the transcript tail flood-guards —
    the full upstream union, NOT just the arms we decode (`read_omp_entry_types`).
    The tail breadcrumbs a `type` outside it with no dedup, so the report diffs it
    against upstream to ping BEFORE a new type floods the warn-floor."""
    return rust_const_str_array("crates/pixtuoid-core/src/source/omp.rs", "KNOWN_ENTRY_TYPES")


def read_copilot_events() -> set[str]:
    """The event `type` strings the decoder maps, read from the decoder's own
    `match kind` block. Scoped to that block so the test fixtures below (which
    embed the same strings as JSON) don't leak in."""
    src = (REPO / "crates/pixtuoid-core/src/source/copilot.rs").read_text()
    m = re.search(r"let out = match kind \{(.*?)\n    \};", src, re.S)
    if not m:
        raise RuntimeError("could not locate the `match kind` block in source/copilot.rs")
    return set(re.findall(r'"((?:session|tool|subagent|permission)\.[a-z._]+)"', m.group(1)))


def read_copilot_namespaces() -> set[str]:
    """The event-`type` NAMESPACE families the transcript tail flood-guards. The
    tail breadcrumbs a namespace outside this set with no dedup, so the report
    diffs it against upstream to ping BEFORE a new family floods the warn-floor."""
    return rust_const_str_array("crates/pixtuoid-core/src/source/copilot.rs", "KNOWN_NAMESPACES")


def read_cursor_events() -> set[str]:
    """The camelCase hook events we register/decode. Read from the const, NOT the
    decoder's `match event` block: a future camelCase field lookup in an arm would
    leak a phantom event into the drift set."""
    return rust_const_str_array("crates/pixtuoid/src/install/cursor.rs", "CURSOR_EVENTS")


def read_openclaw_events() -> set[str]:
    return rust_const_str_array("crates/pixtuoid/src/install/openclaw.rs", "OPENCLAW_EVENTS")


def openclaw_plugin_default_port() -> str | None:
    """The `DEFAULT_GATEWAY_PORT` literal the bundled OpenClaw plugin falls back
    to, read from the JS template itself (not a second copy here) so the watch
    compares upstream against the value that actually ships."""
    try:
        src = (REPO / "crates/pixtuoid/src/install/openclaw_plugin.js").read_text()
    except OSError:
        return None
    m = re.search(r"const DEFAULT_GATEWAY_PORT\s*=\s*(\d+)", src)
    return m.group(1) if m else None


def read_hermes_events() -> set[str]:
    return rust_const_str_array("crates/pixtuoid/src/install/hermes.rs", "HERMES_EVENTS")


def read_grok_events() -> set[str]:
    return rust_const_str_array("crates/pixtuoid/src/install/grok.rs", "GROK_EVENTS")


def read_acp_tags() -> set[str]:
    """The ACP v1 `sessionUpdate` tag vocabulary the shared `source/acp.rs`
    flood-guards. A tag outside it is breadcrumbed with no dedup (a per-token
    `*_message_chunk` would flood hardest), so the report diffs it against the
    live v1 schema to ping BEFORE that happens."""
    return rust_const_str_array("crates/pixtuoid-core/src/source/acp.rs", "KNOWN_ACP_TAGS")


def read_kimi_events() -> set[str]:
    return rust_const_str_array("crates/pixtuoid/src/install/kimi.rs", "KIMI_EVENTS")


def upstream_grok_hooks(text: str) -> set[str] | None:
    """The HookEventName enum variants (bare Rust idents — registration keys
    accept the PascalCase spelling, so these ARE the names we register).

    TWO declaration shapes, tried in order, because upstream now GENERATES the
    enum from a `hook_events! { … }` macro table. The plain-enum regex still
    matches the macro DEFINITION's body, but that body holds `$variant`
    placeholders and reads empty — hence the fall-THROUGH: an empty plain-enum
    parse is not an answer, it is the signal to read the table.
    """
    m = re.search(r"pub enum HookEventName \{(.*?)\n\}", text, re.S)
    if m:
        found = set(re.findall(r"(?m)^\s*([A-Z]\w+),", m.group(1)))
        if found:
            return found
    # Comments first: `rust_block_after` measures braces, and a doc comment on a
    # row would otherwise read as a variant.
    block = rust_block_after(strip_rust_comments(text), r"(?m)^\s*hook_events!\s*")
    if block is None:
        return None
    # Row headers only — the row BODY is `key: value` lines whose aliases/traits
    # carry CamelCase tokens we must not admit.
    found = set(re.findall(r"(?m)^\s*([A-Z]\w+)\s*\{", block))
    return found or None


def upstream_acp_session_update_tags(text: str) -> set[str] | None:
    """The ACP `SessionUpdate` discriminator tags from a v1 JSON schema. Returns
    None if the schema won't parse or the union is absent → the caller files probe
    health and SKIPS the check."""
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
    as a `const` or a single-element `enum`. Shared by the event and namespace
    readers so the two can't drift on how a type-tag is expressed."""
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
    schema."""
    try:
        defs = json.loads(text).get("definitions", {})
    except (json.JSONDecodeError, AttributeError):
        return None
    consts = {c for sch in defs.values() if (c := _copilot_type_const(sch))}
    return consts or None


def upstream_copilot_namespaces(text: str) -> set[str] | None:
    """The NAMESPACE families of the copilot `SessionEvent` union. Scoped to the
    anyOf union ON PURPOSE: a naive walk of ALL `definitions` also pulls in
    nested-content type-tags (`audio`/`text`/`image`/…) that share the
    `type.const` shape but are never a top-level envelope `type`, inflating the
    set with phantom families. Returns None → the caller files probe health and
    SKIPS every Copilot check."""
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
    """The COMPLETE omp session-entry `type` set. Two forms: direct
    `type: "literal"` discriminators, and `type: typeof CONST` refs whose value is
    a `CONST = "literal"` binding. Returns None if neither is found → the caller
    files probe health and SKIPS the check."""
    literals = set(re.findall(r'type:\s*"(\w+)"', text))
    for const_name in re.findall(r"type:\s*typeof\s+(\w+)", text):
        m = re.search(rf'{re.escape(const_name)}\s*=\s*"(\w+)"', text)
        if m:
            literals.add(m.group(1))
    return literals or None


def upstream_copilot_field_names(text: str) -> set[str] | None:
    """The union of every `properties` key at ANY depth — envelope fields AND the
    nested `data.properties` ones. Used one-directional: a field the decoder READS
    that is absent from the whole schema is a rename."""
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

    NOT the app-server `codewhale-hooks` sink enum in `crates/hooks` — a different
    mechanism sharing no configuration. serde `rename_all = "snake_case"`, so each
    CamelCase variant converts to the name we register.
    """
    body = _enum_body(text, "HookEvent")
    if body is None:
        return None
    variants = re.findall(r"^\s*([A-Z][A-Za-z0-9]+)\s*,", body, re.M)
    snake = {_snake_case(v) for v in variants}
    return snake or None


def cc_doc_marker_findings(hooks_doc: str) -> list[str]:
    # Depended markers alarm on VANISH, surface markers on APPEARANCE. Not named
    # `review`: the selftest bans a bucket-named local outside `Report`.
    findings: list[str] = []
    for marker, what in sorted(CC_DEPENDED_DOC_MARKERS.items()):
        if marker not in hooks_doc:
            findings.append(
                f"CC hooks.md no longer mentions `{marker}` — {what} may have "
                f"moved/renamed; re-verify the burn-tier hook decode."
            )
    for marker, what in sorted(CC_LIFECYCLE_SURFACE_MARKERS.items()):
        if marker in hooks_doc:
            findings.append(
                f"CC hooks doc now mentions `{marker}` — {what} may have "
                f"landed upstream. Adopt it (a durable end signal for the "
                f"JSONL transport / the liveness-probe registry) and "
                f"update this watch."
            )
    return findings


@dataclasses.dataclass
class OurNames:
    """What WE depend on, read from our OWN source — one field per READER.

    `None` means that reader failed; every consumer guards on `is not None`, so a
    stale parser darkens exactly what it fed and nothing else."""

    codex: set[str] | None = None
    codex_rollout: tuple[set[str], set[str]] | None = None
    cc: set[str] | None = None
    dispatch_names: set[str] | None = None
    reasonix: set[str] | None = None
    codewhale: set[str] | None = None
    opencode: set[str] | None = None
    copilot: set[str] | None = None
    omp: set[str] | None = None
    cursor: set[str] | None = None
    openclaw: set[str] | None = None
    hermes: set[str] | None = None
    grok: set[str] | None = None
    kimi: set[str] | None = None
    codex_rollout_outers: set[str] | None = None
    copilot_namespaces: set[str] | None = None
    omp_known_types: set[str] | None = None
    acp_tags: set[str] | None = None


# (OurNames field, reader, the surface it reads — the wording of its probe-health
# line). A table because each row needs its OWN `try`; rows hold function OBJECTS
# captured at import, so a test injecting a stale reader must replace the ROW, not
# the module attribute.
READERS: tuple[tuple[str, typing.Callable[[], typing.Any], str], ...] = (
    ("codex", read_codex_events, "CODEX_EVENTS in install/codex.rs"),
    ("codex_rollout", read_codex_rollout_types, "the decode arms in source/codex.rs"),
    ("cc", read_cc_events, "EVENTS in install/claude.rs"),
    ("dispatch_names", read_dispatch_names, "the dispatch tool names in source/decoder.rs"),
    ("reasonix", read_reasonix_events, "REASONIX_EVENTS in install/reasonix.rs"),
    ("codewhale", read_codewhale_events, "CODEWHALE_EVENTS in install/codewhale.rs"),
    ("opencode", read_opencode_events, "the `match event` arms in source/opencode.rs"),
    ("copilot", read_copilot_events, "the `match kind` arms in source/copilot.rs"),
    ("omp", read_omp_entry_types, "the entry types in source/omp.rs"),
    ("cursor", read_cursor_events, "CURSOR_EVENTS in install/cursor.rs"),
    ("openclaw", read_openclaw_events, "OPENCLAW_EVENTS in install/openclaw.rs"),
    ("hermes", read_hermes_events, "HERMES_EVENTS in install/hermes.rs"),
    ("grok", read_grok_events, "GROK_EVENTS in install/grok.rs"),
    ("kimi", read_kimi_events, "KIMI_EVENTS in install/kimi.rs"),
    ("codex_rollout_outers", read_codex_rollout_outers, "KNOWN_OUTERS in source/codex.rs"),
    ("copilot_namespaces", read_copilot_namespaces, "KNOWN_NAMESPACES in source/copilot.rs"),
    ("omp_known_types", read_omp_known_types, "KNOWN_ENTRY_TYPES in source/omp.rs"),
    ("acp_tags", read_acp_tags, "KNOWN_ACP_TAGS in source/acp.rs"),
)


def read_our_names(report: Report) -> OurNames:
    """Read every depended name from our own source, ISOLATING each reader.

    A stale parser here means the MONITOR is broken: loud probe health (exit 1),
    never transient. Per-row `try` — one shared one leaves every later field
    `None`."""
    ours = OurNames()
    for field, reader, what in READERS:
        try:
            setattr(ours, field, reader())
        except Exception as e:  # noqa: BLE001
            report.add_blind(
                f"what WE depend on for `{field}`, reading {what} ({e})",
                f"check_upstream_drift.py's {reader.__name__}",
                f"That parser is stale (was it refactored?), so the `{field}` "
                f"upstream checks were SKIPPED. Every OTHER source still ran.",
                our_source=True,
            )
    return ours


def run_checks(ours: OurNames, *, report: Report) -> None:
    """The upstream comparisons, filing what they find into `report`.

    Split from main() so an UNEXPECTED exception here can be routed to the
    transient bucket with the partial report intact — without it the interpreter
    exits 1 and the workflow files a junk drift-titled issue from an empty report.

    Every block guards on its own `ours.<field> is not None`, so one stale reader
    costs exactly what it fed. No `read_*` may be called from here: an inline read
    unwinds to that catch-all and is filed TRANSIENT — a warning on a green run —
    which is strictly worse than the probe-health line `read_our_names` files."""
    # protocol.rs holds BOTH the HookEventName enum and the EventMsg enum.
    if ours.codex is not None or ours.codex_rollout is not None:
        text = try_fetch(CODEX_PROTOCOL_URL, "Codex source", report)
        if text is not None and ours.codex is not None:
            upstream = upstream_codex_hooks(text)
            if upstream is None:
                report.add_blind(
                    "the Codex `HookEventName` enum",
                    "codex-rs/protocol/src/protocol.rs",
                    "The Codex hook-event watch was SKIPPED.",
                )
            else:
                for ev in sorted(ours.codex):
                    if ev not in upstream:
                        report.add_breaking(
                            f"Codex hook `{ev}` (registered in CODEX_EVENTS) is GONE "
                            f"from upstream HookEventName — likely renamed; the "
                            f"decoder will silently drop it."
                        )
                for ev in sorted(upstream - ours.codex - CODEX_KNOWN_OMITTED):
                    report.add_review(
                        f"new Codex hook `{ev}` upstream — we neither register nor "
                        f"intentionally omit it (add a decoder arm + CODEX_EVENTS, "
                        f"or add it to CODEX_KNOWN_OMITTED)."
                    )
        # ONE-DIRECTIONAL: codex emits many EventMsg/ResponseItem types we ignore,
        # so only a VANISHED depended type alarms.
        if text is not None and ours.codex_rollout is not None:
            event_msg_ours, _ = ours.codex_rollout
            up_ev = upstream_codex_enum_types(text, "EventMsg")
            if up_ev is None:
                report.add_blind(
                    "the Codex `EventMsg` enum",
                    "protocol.rs",
                    "The rollout event_msg watch was SKIPPED.",
                )
            else:
                for t in sorted(event_msg_ours):
                    if t not in up_ev:
                        report.add_breaking(
                            f"Codex rollout event_msg `{t}` (decoded in "
                            f"source/codex.rs) is GONE from upstream `EventMsg` — "
                            f"renamed; the transcript decoder drops it SILENTLY "
                            f"(`_ => vec![]`, no drift breadcrumb)."
                        )
        # The reverse direction (a KNOWN_OUTERS member gone upstream) is a benign
        # stale silent-set entry — review only, never breaking.
        if text is not None and ours.codex_rollout is not None:
            up_outers = upstream_codex_enum_types(text, "RolloutItem")
            if up_outers is None:
                report.add_blind(
                    "the Codex `RolloutItem` enum",
                    "protocol.rs",
                    "The rollout OUTER flood guard was SKIPPED.",
                )
            elif ours.codex_rollout_outers is not None:
                for t in sorted(up_outers - ours.codex_rollout_outers):
                    report.add_review(
                        f"new Codex rollout OUTER `{t}` upstream (`RolloutItem`) not "
                        f"in KNOWN_OUTERS (source/codex.rs) — the transcript tail will "
                        f"breadcrumb EVERY line of it (drift flood); add it to "
                        f"KNOWN_OUTERS (decode it, or knowingly ignore it)."
                    )
        # The burn-tier decode is fail-quiet by design (a rename just darkens the
        # model badge/flame), so this watch is its only alarm.
        if text is not None:
            tc_fields = codex_turn_context_fields(text)
            if tc_fields is None:
                report.add_blind(
                    "the Codex `TurnContextItem` struct",
                    "protocol.rs",
                    "The burn-tier model/effort watch was SKIPPED.",
                )
            else:
                for f in ("model", "effort"):
                    if f not in tc_fields:
                        report.add_breaking(
                            f"Codex turn_context field `{f}` is GONE from "
                            f"TurnContextItem in protocol.rs — renamed; the "
                            f"burn-tier decoder reads None (model badge/flame "
                            f"silently dark for codex agents)."
                        )
        if ours.codex_rollout is not None:
            _, response_item_ours = ours.codex_rollout
            models = try_fetch(CODEX_MODELS_URL, "Codex models", report)
            if models is not None:
                up_ri = upstream_codex_enum_types(models, "ResponseItem")
                if up_ri is None:
                    report.add_blind(
                        "the Codex `ResponseItem` enum",
                        "models.rs",
                        "The rollout response_item watch was SKIPPED.",
                    )
                else:
                    for t in sorted(response_item_ours):
                        if t not in up_ri:
                            report.add_breaking(
                                f"Codex rollout response_item `{t}` (decoded in "
                                f"source/codex.rs) is GONE from upstream "
                                f"`ResponseItem` — renamed; the transcript decoder "
                                f"drops it SILENTLY."
                            )
                    # A rename here silently mislabels the tool / never fires the
                    # approval gate. None = graceful skip (see the helper).
                    fc_fields = codex_function_call_fields(models)
                    if fc_fields is not None:
                        for f in ("name", "arguments"):
                            if f not in fc_fields:
                                report.add_breaking(
                                    f"Codex function_call field `{f}` is GONE from "
                                    f"ResponseItem::FunctionCall in models.rs — renamed; "
                                    f"the decoder reads None (mislabels the tool / never "
                                    f"gates on approval)."
                                )

    if ours.reasonix is not None:
        text = try_fetch(REASONIX_HOOK_URL, "Reasonix source", report)
        if text is not None:
            upstream = upstream_reasonix_hooks(text)
            if upstream is None:
                report.add_blind(
                    "the Reasonix `Event` consts",
                    "internal/hook/hook.go",
                    "The Reasonix event AND payload-field watches were SKIPPED.",
                )
            else:
                for ev in sorted(ours.reasonix):
                    if ev not in upstream:
                        report.add_breaking(
                            f"Reasonix hook `{ev}` (registered in REASONIX_EVENTS) is "
                            f"GONE from upstream hook.go — likely renamed; the decoder "
                            f"will silently drop it."
                        )
                for ev in sorted(upstream - ours.reasonix - REASONIX_KNOWN_OMITTED):
                    report.add_review(
                        f"new Reasonix hook `{ev}` upstream — we neither register nor "
                        f"intentionally omit it (add a decoder arm + REASONIX_EVENTS, "
                        f"or add it to REASONIX_KNOWN_OMITTED)."
                    )
                for field in sorted(REASONIX_PAYLOAD_FIELDS):
                    if f'json:"{field}' not in text:
                        report.add_breaking(
                            f"Reasonix payload field `{field}` (read by "
                            f"decode_rx_hook_payload) has no json tag in upstream "
                            f"hook.go — likely renamed; the decode will silently zero."
                        )

    if ours.codewhale is not None:
        text = try_fetch(CODEWHALE_HOOK_URL, "CodeWhale source", report)
        if text is not None:
            upstream = upstream_codewhale_hooks(text)
            if upstream is None:
                report.add_blind(
                    "the CodeWhale `pub enum HookEvent`",
                    "crates/tui/src/hooks/config.rs",
                    "The CodeWhale event watch was SKIPPED.",
                )
            else:
                for ev in sorted(ours.codewhale):
                    if ev not in upstream:
                        report.add_breaking(
                            f"CodeWhale hook `{ev}` (registered in CODEWHALE_EVENTS) is "
                            f"GONE from upstream HookEvent — likely renamed; the decoder "
                            f"will silently drop it."
                        )
                for ev in sorted(upstream - ours.codewhale - CODEWHALE_KNOWN_OMITTED):
                    report.add_review(
                        f"new CodeWhale hook `{ev}` upstream — we neither register nor "
                        f"intentionally omit it (add a decoder arm + CODEWHALE_EVENTS, "
                        f"or add it to CODEWHALE_KNOWN_OMITTED)."
                    )
        # Its own fetch of a SEPARATE file, so a failure to read the executor
        # can't be mistaken for every env var vanishing.
        exec_text = fetch_anchored(CODEWHALE_EXECUTOR_URL, "CodeWhale executor", report)
        if exec_text is not None:
            for field in sorted(CODEWHALE_ENV_FIELDS):
                if f'"{field}"' not in exec_text:
                    report.add_breaking(
                        f"CodeWhale env var `{field}` (folded by the shim's env-mode "
                        f"into the {{cwd,tool,tool_args}} envelope) is GONE from "
                        f"hooks/executor.rs `to_env_vars` — renamed; the shim reads "
                        f"None, the envelope omits its field, and the cwd-keyed "
                        f"decoder drops the event (empty cwd = no sprite / no activity)."
                    )

    if ours.opencode is not None:
        parts = [
            fetch_anchored(u, "opencode source", report)
            for u in OPENCODE_EVENT_URLS
        ]
        # Skip unless BOTH halves read: a partial concat false-positives a
        # depended type as "GONE" because it lived in the half we couldn't read.
        readable = [p for p in parts if p is not None]
        text = "\n".join(readable) if len(readable) == len(parts) else None
        if text is not None:
            for ev in sorted(ours.opencode - OPENCODE_TOLERATED):
                if f'"{ev}"' not in text:
                    report.add_breaking(
                        f"opencode event `{ev}` (decoded in source/opencode.rs) is GONE "
                        f"from upstream — likely renamed; the plugin still forwards it but "
                        f"the decoder maps it to nothing (no sprite / no activity)."
                    )
            for field in sorted(OPENCODE_PAYLOAD_FIELDS):
                if not re.search(rf"(?m)^\s*{re.escape(field)}:", text):
                    report.add_breaking(
                        f"opencode field `{field}` (read by source/opencode.rs) is GONE "
                        f"from the schema Struct defs — likely renamed; the plugin still "
                        f"forwards the event but the decoder reads None (wrong-register / "
                        f"no-link / no-activity)."
                    )

    if ours.grok is not None:
        text = fetch_anchored(GROK_HOOK_URL, "grok hooks source", report)
        if text is not None:
            upstream = upstream_grok_hooks(text)
            if upstream is None:
                report.add_blind(
                    "the grok `HookEventName` variants (plain enum or "
                    "`hook_events!` table)",
                    "xai-grok-hooks/src/event.rs",
                    "The grok event watch was SKIPPED.",
                )
            else:
                for ev in sorted(ours.grok):
                    if ev not in upstream:
                        report.add_breaking(
                            f"grok hook `{ev}` (registered in GROK_EVENTS) is GONE from "
                            f"upstream event.rs — likely renamed; the registered key "
                            f"stops matching and that event silently never fires."
                        )
                for ev in sorted(upstream - ours.grok - GROK_KNOWN_OMITTED):
                    report.add_review(
                        f"new grok hook `{ev}` upstream — we neither register nor "
                        f"intentionally omit it (add a decoder arm + GROK_EVENTS, or "
                        f"add it to GROK_KNOWN_OMITTED)."
                    )
            for ident in sorted(GROK_ENVELOPE_IDENTS):
                if not re.search(rf"(?m)^\s*pub {ident}:", text):
                    report.add_breaking(
                        f"grok envelope field `{ident}` is GONE from HookEventEnvelope "
                        f"in event.rs — renamed; its camelCase wire name shifts and "
                        f"decode_grok_hook_payload reads None (no key / no cwd)."
                    )
            for rename in sorted(GROK_PAYLOAD_RENAMES):
                if f'rename = "{rename}"' not in text:
                    report.add_breaking(
                        f"grok payload rename `{rename}` is GONE from event.rs — the "
                        f"wire field decode_grok_hook_payload reads was renamed; the "
                        f"decode silently zeroes (no tool label / no child key)."
                    )
            for ident in sorted(GROK_PAYLOAD_IDENTS):
                if not re.search(rf"(?m)^\s*{ident}:", text):
                    report.add_breaking(
                        f"grok payload field `{ident}` is GONE from event.rs — renamed; "
                        f"the decoder's fallback reads None (Waiting reason / child "
                        f"label degrade)."
                    )
        text = fetch_anchored(GROK_NOTIFICATION_URL, "grok notification source", report)
        if text is not None:
            for variant in sorted(GROK_XAI_VARIANTS):
                if not re.search(rf"(?m)^\s*{variant}\b", text):
                    report.add_breaking(
                        f"grok xAI update variant `{variant}` is GONE from "
                        f"extensions/notification.rs — its snake_case sessionUpdate tag "
                        f"shifts and decode_grok_line maps the line to nothing "
                        f"(no subagent link / no model info / no end marker)."
                    )
            for ident in sorted(GROK_XAI_FIELDS):
                if not re.search(rf"(?m)^\s*(pub )?{ident}:", text):
                    report.add_breaking(
                        f"grok xAI update field `{ident}` is GONE from "
                        f"extensions/notification.rs — renamed; decode_grok_line reads "
                        f"None (child un-keyed / model dropped / end marker missed)."
                    )
        text = fetch_anchored(GROK_ACTIVE_SESSIONS_URL, "grok active-sessions source", report)
        if text is not None:
            for ident in sorted(GROK_ACTIVE_SESSION_FIELDS):
                if not re.search(rf"(?m)^\s*(pub )?{ident}:", text):
                    report.add_breaking(
                        f"grok active_sessions field `{ident}` is GONE from "
                        f"active_sessions.rs — renamed; grok_ids_from_registry stops "
                        f"parsing and the WHOLE liveness ladder (probe / instant exit / "
                        f"negative vouch / focus) degrades to mtime gating."
                    )

    # Review-class, never breaking: the reverse direction (a KNOWN_ACP_TAGS member
    # gone upstream) is a benign stale entry.
    up_acp: set[str] = set()
    acp_parsed = False
    for url, label in (
        (ACP_V1_SCHEMA_URL, "ACP v1 schema"),
        (ACP_V1_SCHEMA_UNSTABLE_URL, "ACP v1 unstable schema"),
    ):
        text = try_fetch(url, label, report)
        if text is not None:
            tags = upstream_acp_session_update_tags(text)
            if tags is None:
                report.add_blind(
                    "the ACP `SessionUpdate` oneOf union",
                    label,
                    "The ACP tag flood guard was SKIPPED.",
                )
            else:
                up_acp |= tags
                acp_parsed = True
    if acp_parsed and ours.acp_tags is not None:
        for tag in sorted(up_acp - ours.acp_tags):
            report.add_review(
                f"new ACP v1 `sessionUpdate` tag `{tag}` upstream not in KNOWN_ACP_TAGS "
                f"(source/acp.rs) — the ACP tag tier will breadcrumb EVERY line of it "
                f"(drift flood); add it to KNOWN_ACP_TAGS (decode it, or knowingly ignore it)."
            )

    if ours.copilot is not None:
        text = try_fetch(COPILOT_SCHEMA_URL, "Copilot schema", report)
        # The `SessionEvent` union is this document's ANCHOR: a parseable JSON is
        # NOT enough, one stray `properties` object reports every field renamed.
        up_ns = upstream_copilot_namespaces(text) if text is not None else None
        if text is not None and up_ns is None:
            report.add_blind(
                "the Copilot `SessionEvent` anyOf union",
                COPILOT_SCHEMA_URL,
                "EVERY Copilot check (event types, payload fields, the "
                "namespace flood guard) was SKIPPED — an unproven schema "
                "cannot tell a rename from a restructure.",
            )
        if text is not None and up_ns is not None:
            upstream = upstream_copilot_events(text)
            if upstream is None:
                report.add_blind(
                    "any parseable `type` const in the Copilot session-events schema",
                    COPILOT_SCHEMA_URL,
                    "The Copilot event watch was SKIPPED.",
                )
            else:
                for ev in sorted(ours.copilot):
                    if ev not in upstream:
                        report.add_breaking(
                            f"Copilot event `{ev}` (decoded in source/copilot.rs) is GONE "
                            f"from the @github/copilot schema — likely renamed; the "
                            f"transcript still carries it but the decoder maps it to "
                            f"nothing (no sprite / no activity)."
                        )
            fields_up = upstream_copilot_field_names(text)
            if fields_up is None:
                report.add_blind(
                    "the Copilot schema `properties` keys",
                    COPILOT_SCHEMA_URL,
                    "The Copilot payload-field watch was SKIPPED.",
                )
            else:
                for field in sorted(COPILOT_PAYLOAD_FIELDS):
                    if field not in fields_up:
                        report.add_breaking(
                            f"Copilot field `{field}` (read by decode_copilot_line / "
                            f"extract_copilot_cwd) is GONE from the schema properties — "
                            f"renamed; the decoder reads None (wrong-register / no-link / "
                            f"no tool label / permission never gates)."
                        )
            # Review-class only: the reverse direction (a KNOWN_NAMESPACES member
            # gone upstream) is a benign stale entry.
            if ours.copilot_namespaces is not None:
                for ns in sorted(up_ns - ours.copilot_namespaces):
                    report.add_review(
                        f"new Copilot event NAMESPACE `{ns}` upstream (`SessionEvent`) "
                        f"not in KNOWN_NAMESPACES (source/copilot.rs) — the transcript "
                        f"tail will breadcrumb EVERY line of it (drift flood); add it "
                        f"to KNOWN_NAMESPACES (decode it, or knowingly ignore it)."
                    )

    if ours.omp is not None:
        text = fetch_anchored(OMP_SESSION_ENTRIES_URL, "omp session-entries", report)
        if text is not None:
            # Quote-anchored on purpose: the entry types are generic English words,
            # so a bare \b match would stay green on a prose use after a rename.
            for name in sorted(ours.omp):
                if f'"{name}"' not in text:
                    report.add_breaking(
                        f"omp entry type `{name}` (decoded in source/omp.rs) is GONE "
                        f"from session-entries.ts — likely renamed; the transcript "
                        f"still flows but the decoder maps it to nothing (no sprite "
                        f"/ no activity)."
                    )
            for field in sorted(OMP_SESSION_ENTRY_FIELDS):
                if not re.search(rf"(?m)^\s*(?:readonly\s+)?{re.escape(field)}\??\s*:", text):
                    report.add_breaking(
                        f"omp field `{field}` (read by decode_omp_line) is GONE from "
                        f"session-entries.ts property keys — renamed; the decoder "
                        f"reads None (no cwd label / no session_exit end)."
                    )
            up_types = upstream_omp_entry_types(text)
            if up_types is None:
                report.add_blind(
                    "the omp entry-`type` literals",
                    "session-entries.ts",
                    "The omp entry-type flood guard was SKIPPED.",
                )
            elif ours.omp_known_types is not None:
                for t in sorted(up_types - ours.omp_known_types):
                    report.add_review(
                        f"new omp entry TYPE `{t}` upstream (session-entries.ts) not "
                        f"in KNOWN_ENTRY_TYPES (source/omp.rs) — the transcript tail "
                        f"will breadcrumb EVERY line of it (drift flood); add it to "
                        f"KNOWN_ENTRY_TYPES (decode it, or knowingly ignore it)."
                    )
        diag = fetch_anchored(OMP_EXIT_DIAG_URL, "omp exit-diagnostics", report)
        if diag is not None and '"session_exit"' not in diag:
            report.add_breaking(
                "omp customType `session_exit` (the clean-teardown marker the "
                "session-ended checker + SessionEnd decode key on) is GONE from "
                "exit-diagnostics.ts — renamed; finished sessions resurrect at "
                "first sight and never SessionEnd."
            )
        ai = fetch_anchored(OMP_AI_TYPES_URL, "omp pi-ai types", report)
        if ai is not None:
            for name in sorted(OMP_MESSAGE_LITERALS):
                if f'"{name}"' not in ai:
                    report.add_breaking(
                        f"omp message literal `{name}` (read by decode_omp_line) is "
                        f"GONE from pi-ai types.ts — renamed; tool rounds decode to "
                        f"nothing."
                    )
            for field in sorted(OMP_MESSAGE_FIELDS):
                if not re.search(rf"(?m)^\s*(?:readonly\s+)?{re.escape(field)}\??\s*:", ai):
                    report.add_breaking(
                        f"omp message field `{field}` (read by decode_omp_line) is "
                        f"GONE from pi-ai types.ts property keys — renamed; tool "
                        f"rounds lose their key/target."
                    )
        ask = fetch_anchored(OMP_ASK_URL, "omp ask tool", report)
        if ask is not None:
            if '"ask"' not in ask:
                report.add_breaking(
                    "omp tool name `ask` (drives the ask→Waiting decode) is GONE "
                    "from tools/ask.ts — renamed; a session parked on a user "
                    "question renders active instead of waiting."
                )
            for field in sorted(OMP_ASK_FIELDS):
                if not re.search(rf"(?m)^\s*(?:readonly\s+)?{re.escape(field)}\??\s*:", ask):
                    report.add_breaking(
                        f"omp ask field `{field}` (feeds the Waiting reason) is GONE "
                        f"from tools/ask.ts property keys — renamed; the Waiting "
                        f"reason degrades to the intent/bare-name fallback."
                    )

    if ours.cursor is not None:
        text = fetch_anchored(CURSOR_HOOKS_URL, "Cursor hooks doc", report)
        if text is not None:
            for ev in sorted(ours.cursor):
                # Word-boundary, not quote-anchored: the docs render the names
                # inline / in tables, never as quoted literals.
                if not re.search(rf"\b{re.escape(ev)}\b", text):
                    report.add_breaking(
                        f"Cursor hook `{ev}` (decoded in source/cursor.rs) is GONE from "
                        f"cursor.com/docs/hooks — likely renamed; the CLI still fires it but "
                        f"the decoder maps it to nothing (no sprite / no activity)."
                    )

    if ours.openclaw is not None:
        text = fetch_anchored(OPENCLAW_HOOK_TYPES_URL, "OpenClaw hook-types", report)
        if text is not None:
            for ev in sorted(ours.openclaw):
                if f'"{ev}"' not in text:
                    report.add_breaking(
                        f"OpenClaw hook `{ev}` (registered in OPENCLAW_EVENTS / the TS "
                        f"plugin) is GONE from src/plugins/hook-types.ts — likely renamed; "
                        f"the plugin registers a hook OpenClaw never fires, so the lobster "
                        f"mascot silently stops reacting (no presence)."
                    )
            for field in sorted(OPENCLAW_PAYLOAD_FIELDS):
                if not re.search(rf"\b{re.escape(field)}\b", text):
                    report.add_breaking(
                        f"OpenClaw field `{field}` (read by decode_openclaw_presence) is "
                        f"GONE from src/plugins/hook-types.ts — renamed; the decoder reads "
                        f"None (wrong run-key / no Degraded gate / no presence)."
                    )
            for ty in sorted(OPENCLAW_GATEWAY_PORT_TYPES):
                if ty not in text:
                    report.add_breaking(
                        f"OpenClaw type `{ty}` (the plugin reads the gateway `port` off it "
                        f"for the mascot's instance identity) is GONE from "
                        f"src/plugins/hook-types.ts — renamed; every envelope would carry "
                        f"the registration-time fallback port, so concurrent gateways "
                        f"collapse onto one mascot."
                    )

    text = try_fetch(OPENCLAW_PATHS_URL, "OpenClaw config/paths", report)
    if text is not None:
        our_port = openclaw_plugin_default_port()
        m = re.search(r"DEFAULT_GATEWAY_PORT\s*=\s*(\d+)", text)
        if m is None:
            report.add_blind(
                "OpenClaw's `DEFAULT_GATEWAY_PORT`",
                "src/config/paths.ts",
                "The plugin's fallback-port comparison was SKIPPED.",
            )
        elif our_port is None:
            report.add_blind(
                "our own `DEFAULT_GATEWAY_PORT` copy",
                "install/openclaw_plugin.js",
                "The upstream-port comparison had nothing to compare against "
                "and was SKIPPED (did OUR const get renamed?).",
                our_source=True,
            )
        elif m.group(1) != our_port:
            report.add_breaking(
                f"OpenClaw's DEFAULT_GATEWAY_PORT is now {m.group(1)} but "
                f"openclaw_plugin.js still falls back to {our_port} — a gateway on the new "
                f"default would be stamped with the stale port (a phantom second mascot "
                f"until its TTL sweeps it)."
            )

    if ours.hermes is not None:
        text = fetch_anchored(HERMES_HOOK_URL, "Hermes hooks", report)
        if text is not None:
            for ev in sorted(ours.hermes):
                if f'"{ev}"' not in text:
                    report.add_breaking(
                        f"Hermes hook `{ev}` (registered in HERMES_EVENTS) is GONE from "
                        f"hermes_cli/hooks.py _DEFAULT_PAYLOADS — likely renamed; Hermes still "
                        f"runs but the shell hook we install into config.yaml fires nothing "
                        f"(no sprite / no activity)."
                    )
        shell = fetch_anchored(HERMES_SHELL_HOOK_URL, "Hermes shell_hooks", report)
        if shell is not None:
            for field in sorted(HERMES_PAYLOAD_FIELDS):
                if f'"{field}"' not in shell:
                    report.add_breaking(
                        f"Hermes payload field `{field}` (read by "
                        f"decode_hermes_hook_payload) is GONE from agent/shell_hooks.py "
                        f"_serialize_payload — renamed; the shell-hook JSON omits it and the "
                        f"decoder reads None (no coalesce key / no tool label)."
                    )

    if ours.kimi is not None:
        text = fetch_anchored(KIMI_HOOKS_URL, "Kimi hooks doc", report)
        if text is not None:
            for ev in sorted(ours.kimi):
                # Word-boundary like the Cursor check: the doc renders each name
                # inline / in a summary table, never as a quoted literal.
                if not re.search(rf"\b{re.escape(ev)}\b", text):
                    report.add_breaking(
                        f"Kimi hook `{ev}` (registered in KIMI_EVENTS) is GONE from "
                        f"kimi-code docs/en/customization/hooks.md — likely renamed; "
                        f"Kimi still fires it but the decoder maps it to nothing "
                        f"(no sprite / no activity)."
                    )

    if ours.dispatch_names is not None:
        tools = fetch_anchored(CC_TOOLS_URL, "CC tools-reference", report)
        if tools is not None:
            # At least one name we'd detect by-name must still be the documented
            # dispatch tool. (Losing a legacy name like `Task` is fine.)
            present = [n for n in ours.dispatch_names if re.search(rf"`{re.escape(n)}`", tools)]
            if not present:
                report.add_breaking(
                    f"None of our known dispatch tool names {sorted(ours.dispatch_names)} "
                    f"appear in CC tools-reference — the subagent tool was likely "
                    f"renamed again. Update make_tool_detail's known names. (Semantic "
                    f"subagent_type detection still works, but the name fallback is "
                    f"stale.)"
                )

    # The lifecycle-marker scan is unconditional: there is nothing to read from
    # our source first, since we depend on those surfaces' ABSENCE.
    hooks_doc = fetch_anchored(CC_HOOKS_URL, "CC hooks doc", report)
    if hooks_doc is not None:
        if ours.cc is not None:
            upstream = upstream_cc_hook_events(hooks_doc)
            if upstream is None:
                report.add_blind(
                    "the CC hook-event summary table",
                    "hooks.md",
                    "The CC event watch was SKIPPED.",
                )
            else:
                for ev in sorted(ours.cc):
                    if ev not in upstream:
                        report.add_breaking(
                            f"CC hook `{ev}` (registered in install/claude.rs "
                            f"EVENTS) is GONE from hooks.md — likely renamed; "
                            f"the decoder will silently drop it."
                        )
                for ev in sorted(upstream - ours.cc - CC_KNOWN_OMITTED):
                    report.add_review(
                        f"new CC hook `{ev}` upstream — we neither register nor "
                        f"intentionally omit it (add a decoder arm + "
                        f"install/claude.rs EVENTS, or add it to "
                        f"CC_KNOWN_OMITTED)."
                    )
        for finding in cc_doc_marker_findings(hooks_doc):
            report.add_review(finding)



def main() -> int:
    report = Report()

    ours = read_our_names(report)

    try:
        run_checks(ours, report=report)
    except Exception as e:  # noqa: BLE001
        traceback.print_exc()
        report.add_error(
            f"unexpected error during the upstream checks "
            f"({type(e).__name__}: {e}) — treating as transient; the report "
            f"covers only the checks that completed (traceback on stderr)"
        )

    print(report.render())
    return report.exit_code()


if __name__ == "__main__":
    sys.exit(main())
