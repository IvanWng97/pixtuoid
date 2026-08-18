#!/usr/bin/env python3
"""Upstream wire-format drift watch.

pixtuoid decodes agent-CLI wire formats (hook event names, the subagent-dispatch
tool name, transcript type tags) whose names change upstream WITHOUT notice — the
`Task` -> `Agent` rename shipped undocumented and silently disabled subagent
suppression. This verifies that every name we depend on still exists at its
canonical upstream source.

What we depend on is read from `drift-surface.json` — one per crate, GENERATED
by a test in the crate that owns those (private) names. This file never parses
our Rust: a scraped `match` arm goes stale silently, so a rename would narrow
the watch with nothing to show for it.

WHICH surfaces owe a VANISH row is decided by transport and by whether the
decoder can speak — `crates/pixtuoid-core/src/source/drift.rs`'s header is the
only statement of that rule. The appearance watches (adopt-a-surface markers,
safety premises, the sibling sweeps) are declared at their own sweep sites.

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

REASONIX_HOOK_URL = (
    "https://raw.githubusercontent.com/esengine/DeepSeek-Reasonix/main-v2/"
    "internal/hook/hook.go"
)

CODEWHALE_HOOK_URL = (
    "https://raw.githubusercontent.com/Hmbown/CodeWhale/main/"
    "crates/tui/src/hooks/config.rs"
)

CODEX_PROTOCOL_URL = (
    "https://raw.githubusercontent.com/openai/codex/main/"
    "codex-rs/protocol/src/protocol.rs"
)

# We install a SHELL hook, so the only thing that can make our always-exit-0
# shim ANSWER an approval is `_BLOCKING_EVENTS` — the set gating
# `returncode == BLOCK_EXIT_CODE`. An APPEARANCE watch, the opposite direction
# from every name check here: today the set holds only `pre_tool_call`, where
# exit 0 means "proceed" and the shim is a correct no-op.
HERMES_SHELL_HOOK_URL = (
    "https://raw.githubusercontent.com/NousResearch/hermes-agent/main/agent/shell_hooks.py"
)

# Registered events whose blocking would make exit 0 a DECISION, not a no-op.
# `install/hermes.rs` states the observer-only premise; this is what checks it.
HERMES_BLOCKING_UNSAFE = {"pre_approval_request"}

HERMES_PLUGINS_URL = (
    "https://raw.githubusercontent.com/NousResearch/hermes-agent/main/hermes_cli/plugins.py"
)

# The ROLLOUT `response_item` types live in this sibling of protocol.rs, not in
# protocol.rs itself.
CODEX_MODELS_URL = (
    "https://raw.githubusercontent.com/openai/codex/main/"
    "codex-rs/protocol/src/models.rs"
)

# omp's clean-teardown marker VALUE. The decoder's guard falls through to
# `_ => vec![]`, so a rename means no omp session ever ends cleanly — every one
# lingers to a stale sweep, with nothing said.
OMP_EXIT_DIAG_URL = (
    "https://raw.githubusercontent.com/can1357/oh-my-pi/main/"
    "packages/coding-agent/src/session/exit-diagnostics.ts"
)

# omp's message-level vocabulary is SPLIT across two upstream files: the roles
# and block type live in the shared LLM types, the `ask` tool in its own module.
# Each is an equality guard whose miss decodes the turn to nothing.
OMP_AI_TYPES_URL = (
    "https://raw.githubusercontent.com/can1357/oh-my-pi/main/packages/ai/src/types.ts"
)

OMP_ASK_URL = (
    "https://raw.githubusercontent.com/can1357/oh-my-pi/main/"
    "packages/coding-agent/src/tools/ask.ts"
)

OMP_SESSION_ENTRIES_URL = (
    "https://raw.githubusercontent.com/can1357/oh-my-pi/main/packages/coding-agent/src/session/session-entries.ts"
)

CODEX_ROLLOUT_ITEM_URL = (
    "https://raw.githubusercontent.com/openai/codex/main/codex-rs/history/src/lib.rs"
)

GROK_HOOK_URL = (
    "https://raw.githubusercontent.com/xai-org/grok-build/main/"
    "crates/codegen/xai-grok-hooks/src/event.rs"
)

# The module that WRITES the method into updates.jsonl, so it OWNS the name we
# gate on — not the notification module, which only mentions it in a `///`.
GROK_SESSION_STORAGE_URL = (
    "https://raw.githubusercontent.com/xai-org/grok-build/main/"
    "crates/codegen/xai-grok-shell/src/session/storage/mod.rs"
)

# The xAI `SessionUpdate` variants. Upstream declares PascalCase idents whose
# snake_case wire tags derive via `rename_all`, so the tag never appears
# literally and the check converts before searching.
GROK_NOTIFICATION_URL = (
    "https://raw.githubusercontent.com/xai-org/grok-build/main/"
    "crates/codegen/xai-grok-shell/src/extensions/notification.rs"
)

# OpenClaw's own `DEFAULT_GATEWAY_PORT`. A VALUE watch, not a name sweep: the
# resolved port is the daemon's runtime IDENTITY, so an upstream bump our plugin
# does not follow stamps a gateway with a stale port and two live gateways
# collapse onto one mascot.
OPENCLAW_PATHS_URL = (
    "https://raw.githubusercontent.com/openclaw/openclaw/main/src/config/paths.ts"
)

OPENCLAW_HOOK_TYPES_URL = (
    "https://raw.githubusercontent.com/openclaw/openclaw/main/src/plugins/hook-types.ts"
)

# `permission.asked` is decoded DEFENSIVELY (a V1/alias spelling); only
# `permission.v2.asked` is a guaranteed standalone upstream EventV2 definition,
# so don't alarm if the bare form isn't found as a `type:` literal.
OPENCODE_TOLERATED = {"permission.asked"}

OPENCODE_EVENT_URLS = (
    "https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/schema/src/v1/session.ts",
    "https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/schema/src/permission.ts",
)

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

# The two emitted halves of the drift surface. Each is generated by a test in the
# crate that OWNS those names (`src/drift_surface.rs`), so the watcher never
# parses our source: the crate fails its own test rather than this file silently
# reading a smaller set. Registration and decoding are separate documents because
# they answer different questions — see the `*_EVENTS` sharp edge in
# `crates/pixtuoid/SHARP-EDGES.md`.
CORE_LIB_FRAGMENT = "crates/pixtuoid-core/drift-surface.json"
CORE_BIN_FRAGMENT = "crates/pixtuoid/drift-surface.json"

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


# Deliberately UNPINNED: the bare unpkg path 302-redirects to the latest
# published version, and a drift watch wants the latest shape, not a frozen one.
# The schema ships inside the platform packages (`@github/copilot` itself is now a
# loader stub); linux-x64 matches the CI host and every platform package carries
# an identical copy. ONE-DIRECTIONAL like opencode: only a depended type vanishing
# alarms.
COPILOT_SCHEMA_URL = "https://unpkg.com/@github/copilot-linux-x64/schemas/session-events.schema.json"


# Cursor is a closed binary, so — like CC — the docs are the only watchable
# surface. ONE-DIRECTIONAL: only a depended event vanishing alarms. The
# common-word event `stop` is intrinsically low-confidence (the page contains the
# word regardless), so its disappearance can be masked; the distinctive
# `sessionStart`/`sessionEnd`/`preToolUse`/`postToolUse` carry the check.
CURSOR_HOOKS_URL = "https://cursor.com/docs/hooks.md"

# Kimi is a pnpm/TS monorepo, but the canonical hook-event list lives in the docs
# (each name appears verbatim in the summary table AND the payload examples), so —
# like Cursor — the raw markdown is the watchable surface. ONE-DIRECTIONAL. The
# common-word event `Stop` is intrinsically low-confidence (the doc contains the
# word regardless); the distinctive PascalCase names carry the check.
KIMI_HOOKS_URL = (
    "https://raw.githubusercontent.com/MoonshotAI/kimi-code/main/"
    "docs/en/customization/hooks.md"
)

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
# The three JSON Schemas are parsed STRUCTURALLY (`$defs`/`definitions` walked by
# `upstream_acp_session_update_tags` / `upstream_copilot_*`), so a text anchor
# would add nothing a failed parse does not already say. Every OTHER swept
# document is prose and must declare one — `every_swept_url_declares_an_anchor`
# is what stops a new sweep quietly skipping the #793 gate.
UNANCHORED_BY_DESIGN: frozenset[str] = frozenset(
    {ACP_V1_SCHEMA_URL, ACP_V1_SCHEMA_UNSTABLE_URL, COPILOT_SCHEMA_URL}
)

# The value must be read from upstream's DECLARATION, never scanned for as a
# substring: a rename leaves the old literal standing in a `///` and a
# `#[cfg(test)]` fixture, so `"…" in text` never fires.
GROK_XAI_METHOD_CONST = r"const XAI_SESSION_UPDATE_METHOD\s*:\s*&(?:'static\s+)?str"
GROK_XAI_METHOD_DECL = GROK_XAI_METHOD_CONST + r'\s*=\s*"([^"]+)"'

ANCHORS: dict[str, Anchor] = {
    # owner-grade: the anchor IS the declaration the checked names live inside,
    # so it cannot hold while the names have moved out from under it.
    CODEWHALE_HOOK_URL: Anchor(r"pub enum HookEvent\b", "`HookEvent`"),
    CODEX_MODELS_URL: Anchor(r"pub enum ResponseItem\b", "`ResponseItem`"),
    CODEX_PROTOCOL_URL: Anchor(r"pub enum HookEventName\b", "`HookEventName`"),
    CODEX_ROLLOUT_ITEM_URL: Anchor(r"pub enum RolloutItem\b", "`RolloutItem`"),
    GROK_HOOK_URL: Anchor(r"pub enum HookEventName\b", "`HookEventName`"),
    GROK_NOTIFICATION_URL: Anchor(r"pub enum SessionUpdate\b", "`SessionUpdate`"),
    GROK_SESSION_STORAGE_URL: Anchor(
        GROK_XAI_METHOD_CONST, "`XAI_SESSION_UPDATE_METHOD`"
    ),
    HERMES_PLUGINS_URL: Anchor(r"VALID_HOOKS", "`VALID_HOOKS`"),
    HERMES_SHELL_HOOK_URL: Anchor(r"_BLOCKING_EVENTS\s*=", "`_BLOCKING_EVENTS`"),
    OPENCLAW_HOOK_TYPES_URL: Anchor(r"export type PluginHookName\s*=", "`PluginHookName`"),
    OPENCLAW_PATHS_URL: Anchor(r"DEFAULT_GATEWAY_PORT\s*=", "`DEFAULT_GATEWAY_PORT`"),
    OPENCODE_EVENT_URLS[0]: Anchor(r"(?m)^export const Event = \{", "the `Event` inventory"),
    OPENCODE_EVENT_URLS[1]: Anchor(r"(?m)^export const Event = \{", "the `Event` inventory"),
    # identity-grade: co-located, not owning — a section head, a page title, or a
    # union whose MEMBERS declare the checked literals. The names could move out
    # while this still matches.
    OMP_AI_TYPES_URL: Anchor(r"toolCall", "the message block types"),
    OMP_ASK_URL: Anchor(r"export class AskTool", "the `AskTool` class"),
    OMP_EXIT_DIAG_URL: Anchor(r"SESSION_EXIT_CUSTOM_TYPE", "`SESSION_EXIT_CUSTOM_TYPE`"),
    OMP_SESSION_ENTRIES_URL: Anchor(r"export type SessionEntry\b", "the session-entry union"),
    CC_HOOKS_URL: Anchor(r"(?m)^# Hooks reference", "the hooks-reference page"),
    CC_TOOLS_URL: Anchor(r"(?m)^# Tools reference", "the tools-reference page"),
    CURSOR_HOOKS_URL: Anchor(r"(?m)^#{2,4} Hook events", "the hook-events section"),
    KIMI_HOOKS_URL: Anchor(r"hook_event_name", "the hook-event payload docs"),
    REASONIX_HOOK_URL: Anchor(r"Event\s*=\s*\"", "the Event consts"),
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


# Codex variants we know about and deliberately do not decode. Each would START
# an activity with no upstream event to END it, so decoding one strands the
# session Active until the stale sweep — worse than not decoding it.
CODEX_KNOWN_OMITTED: dict[str, str] = {
    "local_shell_call": "no `local_shell_call_output` sibling exists to end it",
    "image_generation_call": "no `image_generation_call_output` sibling exists to end it",
}


# grok xAI variants we know about and deliberately do not decode.
GROK_XAI_KNOWN_OMITTED: dict[str, str] = {
    "hook_annotation": "TUI scrollback text; carries no state we render",
    "subagent_progress": "cumulative per-child totals (turns, tool calls); "
    "summing a running total into the delta-accumulating reducer double-counts "
    "(codex's `token_count_emits_fresh_usage_from_last_reading` is the precedent)",
}


def sibling_families(decoded: set[str]) -> set[str]:
    """The trailing word of each name we decode — `function_call` → `call`.

    A vocabulary MISS matters when the new name is a sibling of one we already
    handle (`custom_tool_call` beside `function_call`, #933's origin), not when
    upstream merely grows. Only usable where the suffix is a real family: codex's
    `EventMsg` suffixes are generic lifecycle words (`begin`/`end`/`started`)
    shared across unrelated subsystems, so it is excluded on purpose.
    """
    return {n.rsplit("_", 1)[-1] for n in decoded if "_" in n}


def upstream_acp_tool_call_statuses(text: str) -> set[str] | None:
    """The `ToolCallStatus` values a v1 schema declares, as `oneOf` consts."""
    try:
        root = json.loads(text)
    except json.JSONDecodeError:
        return None
    defs = root.get("$defs") or root.get("definitions") or {}
    members = defs.get("ToolCallStatus", {}).get("oneOf") if isinstance(defs, dict) else None
    if not isinstance(members, list):
        return None
    out = {
        m["const"]
        for m in members
        if isinstance(m, dict) and isinstance(m.get("const"), str)
    }
    return out or None


def upstream_cursor_hook_events(text: str) -> set[str] | None:
    """The per-hook SECTION HEADINGS of cursor's hooks page.

    Word-boundary counting over the page is fail-open here: the names appear
    5-17 times each in prose and code samples, so a rename would have to erase
    every occurrence for the check to fire. A heading is the page's own
    declaration of "this hook exists" and vanishes with the hook.
    """
    heads = re.findall(r"(?m)^#{2,4} ([a-z][A-Za-z]+)\s*$", text)
    return set(heads) or None


def upstream_cc_hook_events(text: str) -> set[str] | None:
    """The canonical event list is the "| Event | When it fires |" summary table —
    parse only its rows, since other tables repeat names with different columns."""
    m = re.search(r"^\|\s*Event\s*\|[^\n]*\n\|[\s:|-]*\n((?:\|[^\n]*\n)+)", text, re.M)
    if not m:
        return None
    return set(re.findall(r"^\|\s*`(\w+)`\s*\|", m.group(1), re.M)) or None


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

    acp_decoded_tags: set[str] | None = None
    acp_terminal_statuses: set[str] | None = None
    cc: set[str] | None = None
    dispatch_names: set[str] | None = None
    codex: set[str] | None = None
    codewhale: set[str] | None = None
    hermes: set[str] | None = None
    grok: set[str] | None = None
    grok_xai_method: set[str] | None = None
    grok_xai_tags: set[str] | None = None
    omp: set[str] | None = None
    omp_exit_marker: set[str] | None = None
    omp_message_vocab: set[str] | None = None
    codex_event_msg: set[str] | None = None
    codex_response_item: set[str] | None = None
    codex_outers: set[str] | None = None
    codex_escalation: set[str] | None = None
    openclaw: set[str] | None = None
    openclaw_gateway_port: set[str] | None = None
    opencode: set[str] | None = None
    opencode_part_statuses: set[str] | None = None
    reasonix: set[str] | None = None
    copilot: set[str] | None = None
    copilot_fields: set[str] | None = None
    cursor: set[str] | None = None
    kimi: set[str] | None = None


# The document each source's believability gate doubts — named in the probe-health
# line so a maintainer knows which pin to re-check.
PARSE_SOURCES: dict[str, str] = {
    "cc": "the hook-event summary table in hooks.md",
    "cursor": "the per-hook section headings on cursor.com/docs/hooks.md",
    "codex": "the HookEventName enum in codex-rs/protocol/src/protocol.rs",
    "codewhale": "the HookEvent enum in crates/tui/src/hooks/config.rs",
    "hermes": "VALID_HOOKS in hermes_cli/plugins.py",
    "grok": "the HookEventName enum in xai-grok-hooks/src/event.rs",
    "reasonix": "the Event consts in internal/hook/hook.go",
}


def parse_is_believable(source: str, upstream: set[str], ours: OurNames, report: Report) -> bool:
    """The floor, asked BEFORE EITHER direction — files probe health and returns
    False when the parse is too small to believe.

    It used to gate only the review half. The VANISH half — which files ⛔ "fix
    the decoder", the highest severity this report has — was gated on bare
    non-emptiness, so a reader returning 12 of 37 names could file five ⛔ against
    working decoders and only then be stopped. That is not hypothetical: #929's
    false ⛔ against `pre_approval_request` came from exactly a partial document.
    """
    # The key IS the `OurNames` field — pinned by the selftest's census.
    handled = getattr(ours, source) or set()
    # A reader that finds fewer names than we already handle is broken, or — where
    # the two directions read different populations (cursor's anchor index, kimi's
    # Event Reference table, against a vanish half that searches raw prose) — it is
    # degraded. Probe health is the right answer to both.
    floor = len(handled)
    if len(upstream) < floor:
        report.add_blind(
            f"the {source} name set upstream actually offers",
            PARSE_SOURCES[source],
            f"the reader returned {len(upstream)} names (floor {floor}), so the "
            f"vanish check was SKIPPED for {source}.",
        )
        return False
    return True


# Each row is one OurNames field and where the emitting crate puts it. The
# fragments are GENERATED by a test in the crate that owns the names
# (`src/drift_surface.rs` in each), so a rename is a test failure THERE rather
# than a parser here quietly returning a smaller set.
SURFACE_ROWS: tuple[tuple[str, str, str, str], ...] = (
    ("cc", CORE_BIN_FRAGMENT, "registered", "claude-code"),
    ("cursor", CORE_BIN_FRAGMENT, "registered", "cursor"),
    ("kimi", CORE_BIN_FRAGMENT, "registered", "kimi"),
    ("codex", CORE_BIN_FRAGMENT, "registered", "codex"),
    ("codewhale", CORE_BIN_FRAGMENT, "registered", "codewhale"),
    ("hermes", CORE_BIN_FRAGMENT, "registered", "hermes"),
    ("grok", CORE_BIN_FRAGMENT, "registered", "grok"),
    ("grok_xai_method", CORE_LIB_FRAGMENT, "decoded", "grok.xai_method"),
    ("grok_xai_tags", CORE_LIB_FRAGMENT, "decoded", "grok.xai_tags"),
    ("omp", CORE_LIB_FRAGMENT, "decoded", "omp.entry_types"),
    ("omp_exit_marker", CORE_LIB_FRAGMENT, "decoded", "omp.exit_marker"),
    ("omp_message_vocab", CORE_LIB_FRAGMENT, "decoded", "omp.message_vocab"),
    ("codex_event_msg", CORE_LIB_FRAGMENT, "decoded", "codex.event_msg"),
    ("codex_response_item", CORE_LIB_FRAGMENT, "decoded", "codex.response_item"),
    ("codex_outers", CORE_LIB_FRAGMENT, "decoded", "codex.rollout_outers"),
    ("codex_escalation", CORE_LIB_FRAGMENT, "decoded", "codex.escalation"),
    ("openclaw", CORE_BIN_FRAGMENT, "registered", "openclaw"),
    ("openclaw_gateway_port", CORE_BIN_FRAGMENT, "shipped", "openclaw.default_gateway_port"),
    ("reasonix", CORE_BIN_FRAGMENT, "registered", "reasonix"),
    ("opencode", CORE_LIB_FRAGMENT, "decoded", "opencode.hook_events"),
    ("opencode_part_statuses", CORE_LIB_FRAGMENT, "decoded", "opencode.part_statuses"),
    ("acp_decoded_tags", CORE_LIB_FRAGMENT, "decoded", "acp.session_update_tags"),
    ("acp_terminal_statuses", CORE_LIB_FRAGMENT, "decoded", "acp.terminal_statuses"),
    ("copilot", CORE_LIB_FRAGMENT, "decoded", "copilot.kinds"),
    ("copilot_fields", CORE_LIB_FRAGMENT, "decoded", "copilot.payload_fields"),
    ("dispatch_names", CORE_LIB_FRAGMENT, "decoded", "decoder.dispatch_names"),
)


def load_fragment(rel: str) -> dict:
    """One emitted drift-surface half. Raises so the caller can file probe health
    naming the fragment — a missing file is OUR build being wrong, never evidence
    about upstream."""
    return json.loads((REPO / rel).read_text())


def read_our_names(report: Report) -> OurNames:
    """Read every depended name from the EMITTED drift surface.

    Per-KEY isolation, as the 16 hand-written parsers had per-reader: a fragment
    that loads but lacks one row darkens exactly that field. A fragment that does
    not load at all darkens every field it feeds, which is honest — the file is
    generated as a whole, so it is never partially stale.

    A gap here means the MONITOR is broken: loud probe health (exit 1), never
    transient."""
    ours = OurNames()
    frags: dict[str, dict] = {}
    for rel in (CORE_LIB_FRAGMENT, CORE_BIN_FRAGMENT):
        try:
            frags[rel] = load_fragment(rel)
        except Exception as e:  # noqa: BLE001
            report.add_blind(
                f"what WE depend on, reading {rel} ({e})",
                f"the drift surface emitted by {rel.rsplit('/', 1)[0]}",
                "That fragment is missing or unparseable, so every check it feeds "
                "was SKIPPED. Regenerate with `just gen-drift-surface`.",
                our_source=True,
            )
    for field, rel, group, key in SURFACE_ROWS:
        frag = frags.get(rel)
        if frag is None:
            continue
        got = frag.get(group, {}).get(key)
        if not got:
            report.add_blind(
                f"what WE depend on for `{field}`, reading {group}.{key} in {rel}",
                "the emitted drift surface",
                f"That row is absent or empty, so the `{field}` upstream checks "
                f"were SKIPPED. Every OTHER source still ran.",
                our_source=True,
            )
            continue
        setattr(ours, field, set(got))
    return ours


def strip_rust_comments(body: str) -> str:
    """Remove Rust `//` and `/* */` comments, PRESERVING string literals.

    A scanner rather than a pair of `re.sub`s, because both regex approaches drop
    a REAL depended name and so fail SILENTLY OPEN — the watcher stops checking
    a name and nothing says so:

    - blind `//[^\\n]*` eats to end of line from a `//` inside a STRING, taking
      every later entry on that line with it;
    - `/\\*.*?\\*/` stops at the first `*/`, so a nested block comment (legal Rust)
      leaves its tail behind and re-admits words from inside it.

    `"` is the only string opener tracked: an apostrophe here is far more often a
    lifetime (`&'a str`), and an ODD count of them would swallow the FOLLOWING
    comment into a fake literal, re-admitting every word inside it (pinned by
    `test_rust_comment_strip_is_not_confused_by_lifetimes`).
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


def _snake_case(camel: str) -> str:
    return re.sub(r"(?<!^)(?=[A-Z])", "_", camel).lower()


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


def upstream_codex_hooks(text: str) -> set[str] | None:
    body = _enum_body(text, "HookEventName")
    if body is None:
        return None
    return set(re.findall(r"\b([A-Z][A-Za-z]+)\b", body)) or None


def upstream_reasonix_hooks(text: str) -> set[str] | None:
    found = set(re.findall(r'\w+\s+Event\s*=\s*"(\w+)"', text))
    return found or None


def sole_match(pattern: str, text: str) -> re.Match[str] | None:
    """The ONE match, or None when there are none OR several.

    A first-match read is silently wrong the day upstream grows a second
    declaration above the real one — a `#[cfg(test)]` twin, a `macro_rules!`
    body, a raw string. Ambiguity is probe health, never an answer."""
    found = list(re.finditer(pattern, text))
    return found[0] if len(found) == 1 else None


def python_set_literal(src: str, decl: str) -> set[str] | None:
    """The string members of a `NAME: Set[str] = { … }` block.

    Comments are stripped BEFORE the brace scan, for the reason `_enum_body` and
    `rust_block_after` both state: a brace inside a comment moves the bounds. The
    first cut scanned raw source, and upstream interleaves prose here — one of its
    comments quotes `{"action": "continue"}`, and a stray `}` truncated the set
    SILENTLY while the size floor still passed.
    """
    if src.count(decl) != 1:
        return None
    i = src.find(decl)
    code = "\n".join(line.split("#", 1)[0] for line in src[i:].splitlines())
    j = code.find("{")
    if j < 0:
        return None
    depth = 0
    for k in range(j, len(code)):
        if code[k] == "{":
            depth += 1
        elif code[k] == "}":
            depth -= 1
            if depth == 0:
                return set(re.findall(r'"([a-z_][a-z0-9_]*)"', code[j + 1 : k])) or None
    return None


def run_checks(ours: OurNames, *, report: Report) -> None:
    """The upstream comparisons, filing what they find into `report`.

    Split from main() so an UNEXPECTED exception here can be routed to the
    transient bucket with the partial report intact — without it the interpreter
    exits 1 and the workflow files a junk drift-titled issue from an empty report.

    Every block guards on its own `ours.<field> is not None`, so one stale reader
    costs exactly what it fed. No `read_*` may be called from here: an inline read
    unwinds to that catch-all and is filed TRANSIENT — a warning on a green run —
    which is strictly worse than the probe-health line `read_our_names` files."""
    # The ACP method `session/update` and the two `sessionUpdate` tags
    # `decode_session_update` turns into events. v1 only — grok does not speak v2.
    # xAI's OWN `_x.ai/session/update` is watched separately, off its declaration.
    up_acp: set[str] = set()
    up_acp_status: set[str] = set()
    acp_method_surface = False
    acp_method_declared = False
    acp_parsed = False
    for url, label in (
        (ACP_V1_SCHEMA_URL, "ACP v1 schema"),
        (ACP_V1_SCHEMA_UNSTABLE_URL, "ACP v1 unstable schema"),
    ):
        acp_text = try_fetch(url, label, report)
        if acp_text is None:
            continue
        tags = upstream_acp_session_update_tags(acp_text)
        if tags is None:
            report.add_blind(
                "the ACP `SessionUpdate` oneOf union",
                label,
                "The ACP decoded-tag watch was SKIPPED.",
            )
        else:
            up_acp |= tags
            up_acp_status |= upstream_acp_tool_call_statuses(acp_text) or set()
            acp_parsed = True
            # `"x-method"` is the surface that OWNS the method names, and it is a
            # generator-emitted vendor extension — a codegen change can move it
            # wholesale. Its PRESENCE is this check's anchor: absent, the probe
            # landed on a schema that no longer declares methods (probe health);
            # present without ours, the name is genuinely gone.
            acp_method_surface |= '"x-method"' in acp_text
            acp_method_declared |= '"session/update"' in acp_text
    if acp_parsed and not acp_method_surface:
        report.add_blind(
            "the ACP `x-method` declarations",
            "the ACP v1 schemas",
            "The ACP method watch was SKIPPED — the schema parses but declares no "
            "methods at all, so this is a restructure, NOT evidence of a rename.",
        )
    elif acp_parsed and not acp_method_declared:
        report.add_breaking(
            "the ACP method `session/update` is GONE from the ACP schema — renamed; "
            "`decode_grok_line` gates whether the tag is INTERPRETED, so every "
            "ACP-standard activity event is silently lost (`_ => Ok(vec![])`, no "
            "breadcrumb)."
        )
    if acp_parsed and ours.acp_terminal_statuses is not None:
        if up_acp_status:
            for st in sorted(ours.acp_terminal_statuses):
                if st not in up_acp_status:
                    report.add_breaking(
                        f"ACP `ToolCallStatus` value `{st}` (read by "
                        f"decode_session_update) is GONE from the ACP schema — "
                        f"renamed; the tool_call_update arm falls through to "
                        f"`_ => vec![]` and every ACP tool call stays Active forever."
                    )
        else:
            report.add_blind(
                "the ACP `ToolCallStatus` union",
                "the ACP v1 schemas",
                "The terminal-status watch was SKIPPED.",
            )

    if acp_parsed:
        # The appearing direction, scoped to the one family we decode: a new
        # `tool_call_*` tag (a confirmation, a retry) decodes to nothing and
        # its calls render wrong for every ACP-speaking source.
        for tag in sorted(up_acp - set(ours.acp_decoded_tags or ())):
            if tag.startswith("tool_call"):
                report.add_review(
                    f"ACP grew `{tag}`, a `tool_call` sibling decode_session_update "
                    f"does not handle — every ACP-speaking source decodes it to "
                    f"nothing."
                )
        for tag in sorted(ours.acp_decoded_tags or ()):
            if tag not in up_acp:
                report.add_breaking(
                    f"ACP v1 `sessionUpdate` tag `{tag}` (decoded in source/acp.rs) is "
                    f"GONE from the ACP schema — renamed; every ACP-speaking source "
                    f"loses that half of its activity decode."
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
                "EVERY Copilot check (event types, payload fields) was SKIPPED — "
                "an unproven schema cannot tell a rename from a restructure.",
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
                for field in sorted(ours.copilot_fields or ()):
                    if field not in fields_up:
                        report.add_breaking(
                            f"Copilot field `{field}` (read by decode_copilot_line / "
                            f"extract_copilot_cwd) is GONE from the schema properties — "
                            f"renamed; the decoder reads None (wrong-register / no-link / "
                            f"no tool label / permission never gates)."
                        )


    if ours.cursor is not None:
        text = fetch_anchored(CURSOR_HOOKS_URL, "Cursor hooks doc", report)
        if text is not None:
            upstream = upstream_cursor_hook_events(text)
            if upstream is None:
                report.add_blind(
                    "the cursor hook sections upstream publishes",
                    PARSE_SOURCES["cursor"],
                    "The page parsed to no hook sections, so the cursor vanish "
                    "check was SKIPPED.",
                )
            elif parse_is_believable("cursor", upstream, ours, report):
                for ev in sorted(ours.cursor - upstream):
                    report.add_breaking(
                        f"Cursor hook `{ev}` (decoded in source/cursor.rs) no longer has a "
                        f"section on cursor.com/docs/hooks — likely renamed; the CLI still "
                        f"fires it but the decoder maps it to nothing (no sprite)."
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
            elif parse_is_believable("cc", upstream, ours, report):
                for ev in sorted(ours.cc):
                    if ev not in upstream:
                        report.add_breaking(
                            f"CC hook `{ev}` (registered in install/claude.rs "
                            f"EVENTS) is GONE from hooks.md — likely renamed; "
                            f"the decoder will silently drop it."
                        )
        for finding in cc_doc_marker_findings(hooks_doc):
            report.add_review(finding)


    # HOOK-REGISTERED sources — the inert-registration clause of
    # `source/drift.rs`'s header.
    if ours.reasonix is not None:
        text = fetch_anchored(REASONIX_HOOK_URL, "Reasonix hook source", report)
        if text is not None:
            upstream = upstream_reasonix_hooks(text)
            if upstream is None:
                report.add_blind(
                    "the reasonix hook names upstream declares",
                    PARSE_SOURCES["reasonix"],
                    "The parser found no declaration, so the reasonix vanish check "
                    "was SKIPPED.",
                )
            elif parse_is_believable("reasonix", upstream, ours, report):
                for ev in sorted(ours.reasonix - upstream):
                    report.add_breaking(
                        f"Reasonix hook `{ev}` (registered in REASONIX_EVENTS) is GONE "
                        f"from upstream hook.go — likely renamed; we register a hook it "
                        f"never fires, so the decoder is never reached (no sprite)."
                    )

    if ours.codewhale is not None:
        text = fetch_anchored(CODEWHALE_HOOK_URL, "CodeWhale hook source", report)
        if text is not None:
            upstream = upstream_codewhale_hooks(text)
            if upstream is None:
                report.add_blind(
                    "the codewhale hook names upstream declares",
                    PARSE_SOURCES["codewhale"],
                    "The parser found no declaration, so the codewhale vanish check "
                    "was SKIPPED.",
                )
            elif parse_is_believable("codewhale", upstream, ours, report):
                for ev in sorted(ours.codewhale - upstream):
                    report.add_breaking(
                        f"CodeWhale hook `{ev}` (registered in CODEWHALE_EVENTS) is GONE "
                        f"from upstream HookEvent — likely renamed; we register a hook it "
                        f"never fires, so the decoder is never reached (no sprite)."
                    )

    if ours.codex is not None:
        text = fetch_anchored(CODEX_PROTOCOL_URL, "Codex protocol source", report)
        if text is not None:
            upstream = upstream_codex_hooks(text)
            if upstream is None:
                report.add_blind(
                    "the codex hook names upstream declares",
                    PARSE_SOURCES["codex"],
                    "The parser found no declaration, so the codex vanish check "
                    "was SKIPPED.",
                )
            elif parse_is_believable("codex", upstream, ours, report):
                for ev in sorted(ours.codex - upstream):
                    report.add_breaking(
                        f"Codex hook `{ev}` (registered in CODEX_EVENTS) is GONE from "
                        f"upstream HookEventName — likely renamed; we register a hook it "
                        f"never fires, so the decoder is never reached (no sprite)."
                    )

    if ours.hermes is not None:
        text = fetch_anchored(HERMES_PLUGINS_URL, "Hermes plugins", report)
        if text is not None:
            valid = python_set_literal(text, "VALID_HOOKS: Set[str] = {")
            if valid is None:
                report.add_blind(
                    "the hermes hook names upstream declares",
                    PARSE_SOURCES["hermes"],
                    "The parser found no declaration, so the hermes vanish check "
                    "was SKIPPED.",
                )
            elif parse_is_believable("hermes", valid, ours, report):
                for ev in sorted(ours.hermes - valid):
                    report.add_breaking(
                        f"Hermes hook `{ev}` (registered in HERMES_EVENTS) is GONE from "
                        f"VALID_HOOKS in hermes_cli/plugins.py — likely renamed; the shell "
                        f"hook we install into config.yaml fires nothing (no sprite)."
                    )

    if ours.hermes is not None:
        # SHELL_UNSUPPORTED_HOOKS is the OTHER appearance direction on the
        # plugins document: a hook we register being reclassified as unservable
        # by a shell hook means the command we write into config.yaml fires
        # nothing — same silence as a rename, from the opposite edit.
        plugins = fetch_anchored(HERMES_PLUGINS_URL, "Hermes plugins", report)
        if plugins is not None:
            unsupported = python_set_literal(
                plugins, "SHELL_UNSUPPORTED_HOOKS: Set[str] = {"
            )
            if unsupported is None:
                report.add_blind(
                    "hermes's SHELL_UNSUPPORTED_HOOKS set",
                    PARSE_SOURCES["hermes"],
                    "The shell-serviceability check was SKIPPED, so a hook we "
                    "install could already be one a shell hook cannot serve.",
                )
            else:
                for ev in sorted(ours.hermes & unsupported):
                    report.add_breaking(
                        f"Hermes `{ev}` (registered in HERMES_EVENTS) is now in "
                        f"SHELL_UNSUPPORTED_HOOKS — a shell hook cannot serve it, so "
                        f"the command we install into config.yaml fires nothing "
                        f"(no sprite / no activity)."
                    )

        shell = fetch_anchored(HERMES_SHELL_HOOK_URL, "Hermes shell_hooks", report)
        if shell is not None:
            blocking = sole_match(r"_BLOCKING_EVENTS\s*=\s*frozenset\(\{([^}]*)\}", shell)
            if blocking is None:
                report.add_blind(
                    "whether a hermes shell hook can stall an approval",
                    "_BLOCKING_EVENTS in agent/shell_hooks.py",
                    "The blocking-event set moved or was renamed, so the premise behind "
                    "an always-exit-0 shim on a PERMISSION hook is unchecked.",
                )
            else:
                for ev in sorted(HERMES_BLOCKING_UNSAFE & ours.hermes):
                    if f'"{ev}"' in blocking.group(1):
                        report.add_breaking(
                            f"Hermes `{ev}` is now in `_BLOCKING_EVENTS` — a shell hook's "
                            f"exit code can stall it, so the shim's silent exit 0 would "
                            f"ANSWER a real approval prompt. Unregister it in "
                            f"install/hermes.rs, or make the shim decline explicitly."
                        )

    # omp's entry types are generic English words, so the match is QUOTE-anchored:
    # a bare word test stays green on prose.
    if ours.omp_message_vocab is not None:
        # The union IS the document: `ask` is declared only in ask.ts, the rest
        # only in types.ts, so a single fetch failure must not read as a vanish.
        docs = [
            t
            for u in (OMP_AI_TYPES_URL, OMP_ASK_URL)
            if (t := fetch_anchored(u, "omp message vocabulary", report)) is not None
        ]
        if len(docs) == 2:
            joined = "\n".join(docs)
            for name in sorted(ours.omp_message_vocab):
                if f'"{name}"' not in joined:
                    report.add_breaking(
                        f"omp message name `{name}` (decoded in source/omp.rs) is GONE "
                        f"from upstream — renamed; the turn decodes to nothing, or an "
                        f"`ask` round strands its Waiting gate forever."
                    )

    if ours.omp_exit_marker is not None:
        diag = fetch_anchored(OMP_EXIT_DIAG_URL, "omp exit-diagnostics", report)
        if diag is not None:
            for marker in sorted(ours.omp_exit_marker):
                if f'"{marker}"' not in diag:
                    report.add_breaking(
                        f"omp's clean-teardown marker `{marker}` (decoded in "
                        f"source/omp.rs) is GONE from exit-diagnostics.ts — renamed; "
                        f"no omp session ever ends cleanly again, each lingering to a "
                        f"stale sweep with no breadcrumb."
                    )

    if ours.omp is not None:
        text = fetch_anchored(OMP_SESSION_ENTRIES_URL, "omp session-entries", report)
        if text is not None:
            for name in sorted(ours.omp):
                if f'"{name}"' not in text:
                    report.add_breaking(
                        f"omp entry type `{name}` (decoded in source/omp.rs) is GONE from "
                        f"session-entries.ts — likely renamed; the transcript still flows "
                        f"but the decoder maps it to nothing (no sprite / no activity)."
                    )

    for field, url, enum, label in (
        ("codex_outers", CODEX_ROLLOUT_ITEM_URL, "RolloutItem", "rollout OUTER"),
        ("codex_event_msg", CODEX_PROTOCOL_URL, "EventMsg", "rollout event_msg"),
        ("codex_response_item", CODEX_MODELS_URL, "ResponseItem", "rollout response_item"),
    ):
        mine = getattr(ours, field)
        if mine is None:
            continue
        text = fetch_anchored(url, f"Codex `{enum}`", report)
        if text is None:
            continue
        upstream = upstream_codex_enum_types(text, enum)
        if upstream is None:
            report.add_blind(
                f"the Codex `{enum}` enum",
                url.rsplit("/", 2)[-1],
                f"The {label} watch was SKIPPED.",
            )
            continue
        for name in sorted(mine - upstream):
            report.add_breaking(
                f"Codex {label} `{name}` (decoded in source/codex.rs) is GONE from "
                f"upstream `{enum}` — renamed; the transcript decodes to nothing."
            )

    if ours.grok is not None:
        text = fetch_anchored(GROK_HOOK_URL, "grok hook source", report)
        if text is not None:
            upstream = upstream_grok_hooks(text)
            if upstream is None:
                report.add_blind(
                    "the grok hook names upstream declares",
                    PARSE_SOURCES["grok"],
                    "The parser found no declaration, so the grok vanish check was SKIPPED.",
                )
            elif parse_is_believable("grok", upstream, ours, report):
                for ev in sorted(ours.grok - upstream):
                    report.add_breaking(
                        f"grok hook `{ev}` (registered in GROK_EVENTS) is GONE from "
                        f"upstream event.rs — likely renamed; we register a hook it never "
                        f"fires, so the decoder is never reached (no sprite)."
                    )

    # grok's xAI extension is TRANSCRIPT vocabulary whose arm ends
    # `_ => Ok(vec![])` — the method gates the whole block and each tag decodes
    # to nothing once renamed, with no breadcrumb either way.
    # The PREFIX twin, for vocabularies whose family leads the name. The OTHER
    # vocabularies are out deliberately: copilot's `session.*` is a product-wide
    # event bus (52 same-namespace additions today — a ledger would mirror it);
    # omp's entry types are single words with no family signal, and every
    # renderable state also rides the message stream we decode; codex `EventMsg`
    # suffixes are generic lifecycle words; CC and antigravity breadcrumb their
    # unknown vocabulary (the runtime detector); opencode's plugin forwards
    # exactly what the decoder reads, pinned by test — an upstream addition
    # never arrives.
    if ours.grok_xai_tags is not None:
        text = fetch_anchored(GROK_NOTIFICATION_URL, "grok notification source", report)
        if text is not None:
            upstream = upstream_codex_enum_types(text, "SessionUpdate")
            if upstream is not None:
                families = {n.split("_", 1)[0] for n in ours.grok_xai_tags if "_" in n}
                gap = upstream - ours.grok_xai_tags - set(GROK_XAI_KNOWN_OMITTED)
                for name in sorted(gap):
                    if name.split("_", 1)[0] in families:
                        report.add_review(
                            f"grok's xAI `SessionUpdate` has `{name}`, a sibling of a "
                            f"family we already decode, and source/grok.rs decodes it "
                            f"to nothing. Decode it, or add it to "
                            f"GROK_XAI_KNOWN_OMITTED with the reason."
                        )

    if ours.codex_escalation is not None:
        # A BOOLEAN check with no breadcrumb possible: a renamed field or value
        # makes `is_escalated` silently false, and false is a legitimate answer,
        # so codex sessions would sit Active through every approval prompt. The
        # pair is split across two upstream files, so the union is the document.
        docs = [
            t
            for u in (CODEX_PROTOCOL_URL, CODEX_MODELS_URL)
            if (t := fetch_anchored(u, "Codex escalation names", report)) is not None
        ]
        if len(docs) == 2:
            joined = "\n".join(docs)
            for name in sorted(ours.codex_escalation):
                if not re.search(rf"\b{re.escape(name)}\b", joined):
                    report.add_breaking(
                        f"Codex escalation name `{name}` (read by source/codex.rs) is "
                        f"GONE from upstream — renamed; the approval gate never fires, "
                        f"so a codex session sits Active through every prompt."
                    )

    # The APPEARING direction (#933): a sibling of something we decode, that we
    # do not. One-directional the other way is what let `custom_tool_call` decode
    # to nothing for four tool calls a turn.
    for field, url, enum in (
        ("codex_response_item", CODEX_MODELS_URL, "ResponseItem"),
        ("codex_outers", CODEX_ROLLOUT_ITEM_URL, "RolloutItem"),
    ):
        mine = getattr(ours, field)
        if mine is None:
            continue
        text = fetch_anchored(url, f"Codex `{enum}`", report)
        if text is None:
            continue
        upstream = upstream_codex_enum_types(text, enum)
        if upstream is None:
            continue
        families = sibling_families(mine)
        for name in sorted(upstream - mine - set(CODEX_KNOWN_OMITTED)):
            if name.rsplit("_", 1)[-1] in families:
                report.add_review(
                    f"upstream `{enum}` has `{name}`, a sibling of the "
                    f"`{'`/`'.join(sorted(n for n in mine if n.rsplit('_', 1)[-1] == name.rsplit('_', 1)[-1])[:2])}` "
                    f"we already decode, and source/codex.rs decodes it to nothing. "
                    f"Decode it, or add it to CODEX_KNOWN_OMITTED with the reason."
                )

    if ours.grok_xai_method is not None:
        text = fetch_anchored(GROK_SESSION_STORAGE_URL, "grok session storage", report)
        if text is not None:
            decl = sole_match(GROK_XAI_METHOD_DECL, strip_rust_comments(text))
            if decl is None:
                report.add_blind(
                    "grok's `XAI_SESSION_UPDATE_METHOD` declaration",
                    "storage/mod.rs",
                    "The xAI method watch was SKIPPED.",
                )
            else:
                for method in sorted(ours.grok_xai_method):
                    if method != decl.group(1):
                        report.add_breaking(
                            f"grok's xAI method is now `{decl.group(1)}`, not `{method}` "
                            f"(source/grok.rs) — decode_grok_line gates the WHOLE xAI "
                            f"block on it, so every subagent link, model change and end "
                            f"marker is silently lost."
                        )

    if ours.grok_xai_tags is not None:
        text = fetch_anchored(GROK_NOTIFICATION_URL, "grok notification source", report)
        if text is not None:
            for tag in sorted(ours.grok_xai_tags):
                variant = "".join(p.title() for p in tag.split("_"))
                if not re.search(rf"(?m)^\s*{variant}\b", text):
                    report.add_breaking(
                        f"grok xAI update variant `{variant}` (tag `{tag}`, decoded in "
                        f"source/grok.rs) is GONE from extensions/notification.rs — its "
                        f"snake_case tag shifts and the line decodes to nothing "
                        f"(no subagent link / no model info / no end marker)."
                    )

    if ours.openclaw is not None:
        text = fetch_anchored(OPENCLAW_HOOK_TYPES_URL, "OpenClaw hook types", report)
        if text is not None:
            for ev in sorted(ours.openclaw):
                if f'"{ev}"' not in text:
                    report.add_breaking(
                        f"OpenClaw hook `{ev}` (registered in OPENCLAW_EVENTS / the TS "
                        f"plugin) is GONE from src/plugins/hook-types.ts — likely renamed; "
                        f"the plugin registers a hook OpenClaw never fires (no presence)."
                    )

    if ours.openclaw_gateway_port is not None:
        text = fetch_anchored(OPENCLAW_PATHS_URL, "OpenClaw config/paths", report)
        if text is not None:
            m = sole_match(r"DEFAULT_GATEWAY_PORT\s*=\s*(\d+)", text)
            if m is None:
                report.add_blind(
                    "OpenClaw's `DEFAULT_GATEWAY_PORT` value",
                    "src/config/paths.ts",
                    "The plugin's fallback-port comparison was SKIPPED.",
                )
            else:
                for ours_port in sorted(ours.openclaw_gateway_port):
                    if m.group(1) != ours_port:
                        report.add_breaking(
                            f"OpenClaw's DEFAULT_GATEWAY_PORT is now {m.group(1)} but "
                            f"openclaw_plugin.js still falls back to {ours_port} — a gateway "
                            f"on the new default is stamped with the stale port, so two live "
                            f"gateways collapse onto one mascot until a TTL sweeps it."
                        )

    if ours.opencode is not None:
        # The inventory is SPLIT across two modules — `permission.v2.asked` is
        # declared in permission.ts, not session.ts — so the union is the
        # document, and a single fetch failure must not read as a vanish.
        docs = [
            t
            for u in OPENCODE_EVENT_URLS
            if (t := fetch_anchored(u, "opencode event inventory", report)) is not None
        ]
        if len(docs) == len(OPENCODE_EVENT_URLS):
            joined = "\n".join(docs)
            for ev in sorted(ours.opencode - OPENCODE_TOLERATED):
                if f'"{ev}"' not in joined:
                    report.add_breaking(
                        f"opencode event `{ev}` (forwarded by our plugin, decoded in "
                        f"source/opencode.rs) is GONE from upstream — likely renamed; "
                        f"the plugin subscribes to an event it never fires (no sprite)."
                    )
            # The part-state statuses ride the same two schema documents. The
            # dispatch ends `_ => Ok(vec![])`, so a renamed `running` silently
            # stops every opencode tool activity.
            for st in sorted(ours.opencode_part_statuses or ()):
                # `Schema.Literal(...)` is the DECLARATION; a bare `"error"` also
                # matches two unrelated `Omit<…, "error">` sites, masking a rename.
                if f'Schema.Literal("{st}")' not in joined:
                    report.add_breaking(
                        f"opencode part status `{st}` (decoded in source/opencode.rs) "
                        f"is GONE from the schema — renamed; tool activity decodes to "
                        f"nothing (sprites never go Active, or never come back)."
                    )


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
