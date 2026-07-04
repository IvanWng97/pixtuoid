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
CC_LIFECYCLE_SURFACE_MARKERS = {
    "session_end": 'a structural transcript end record (subtype:"session_end")',
    ".claude/sessions/": "the ~/.claude/sessions/<pid>.json session registry",
    "procStart": "the sessions-registry procStart field",
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


def run_checks(
    codex_ours: set[str] | None,
    cc_ours: set[str] | None,
    dispatch_names: set[str] | None,
    reasonix_ours: set[str] | None,
    codewhale_ours: set[str] | None,
    opencode_ours: set[str] | None,
    copilot_ours: set[str] | None,
    cursor_ours: set[str] | None,
    openclaw_ours: set[str] | None,
    hermes_ours: set[str] | None,
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
    # --- Codex hook events (only the FETCH is transient) -------------------
    if codex_ours is not None:
        text = try_fetch(CODEX_PROTOCOL_URL, "Codex source", breaking, errors)
        if text is not None:
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

    # --- Hermes shell-hook events (only the FETCH is transient) ------------
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
        for marker, what in sorted(CC_LIFECYCLE_SURFACE_MARKERS.items()):
            if marker in hooks_doc:
                review.append(
                    f"CC hooks doc now mentions `{marker}` — {what} may have "
                    f"landed upstream. Adopt it (a durable end signal for the "
                    f"JSONL transport / the liveness-probe registry) and "
                    f"update this watch."
                )



def main() -> int:
    breaking: list[str] = []
    review: list[str] = []
    errors: list[str] = []

    # Read what WE depend on from our OWN source first. A failure here means the
    # monitor itself is broken (decoder.rs / install/codex.rs refactored away from
    # what the parsers expect) — that is a LOUD breaking signal, never a transient
    # one, or drift monitoring would silently stop with zero alarm.
    codex_ours = None
    cc_ours = None
    dispatch_names = None
    reasonix_ours = None
    codewhale_ours = None
    opencode_ours = None
    copilot_ours = None
    cursor_ours = None
    openclaw_ours = None
    hermes_ours = None
    try:
        codex_ours = read_codex_events()
        cc_ours = read_cc_events()
        dispatch_names = read_dispatch_names()
        reasonix_ours = read_reasonix_events()
        codewhale_ours = read_codewhale_events()
        opencode_ours = read_opencode_events()
        copilot_ours = read_copilot_events()
        cursor_ours = read_cursor_events()
        openclaw_ours = read_openclaw_events()
        hermes_ours = read_hermes_events()
    except Exception as e:  # noqa: BLE001
        breaking.append(
            f"drift-watch cannot read our own source ({e}) — the parsers in "
            f"check_upstream_drift.py are stale (decoder.rs / install refactored?). "
            f"The monitor is blind until the script is fixed."
        )

    try:
        run_checks(
            codex_ours,
            cc_ours,
            dispatch_names,
            reasonix_ours,
            codewhale_ours,
            opencode_ours,
            copilot_ours,
            cursor_ours,
            openclaw_ours,
            hermes_ours,
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
