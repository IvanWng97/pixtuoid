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
# them is rejected as malformed).
REASONIX_PAYLOAD_FIELDS = {"event", "cwd", "toolName", "toolArgs", "message"}

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


def fetch(url: str) -> str:
    req = urllib.request.Request(url, headers={"User-Agent": "pixtuoid-drift-watch"})
    with urllib.request.urlopen(req, timeout=30) as resp:  # noqa: S310 (trusted hosts)
        return resp.read().decode("utf-8", "replace")


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
        try:
            text = fetch(CODEX_PROTOCOL_URL)
        except FETCH_ERRORS as e:
            errors.append(f"Codex source fetch failed (transient?): {e}")
            text = None
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
        try:
            text = fetch(REASONIX_HOOK_URL)
        except FETCH_ERRORS as e:
            errors.append(f"Reasonix source fetch failed (transient?): {e}")
            text = None
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
        try:
            text = fetch(CODEWHALE_HOOK_URL)
        except FETCH_ERRORS as e:
            errors.append(f"CodeWhale source fetch failed (transient?): {e}")
            text = None
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

    # --- CC subagent-dispatch tool (only the FETCH is transient) -----------
    if dispatch_names is not None:
        try:
            tools = fetch(CC_TOOLS_URL)
        except FETCH_ERRORS as e:
            errors.append(f"CC tools-reference fetch failed (transient?): {e}")
            tools = None
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
    try:
        hooks_doc = fetch(CC_HOOKS_URL)
    except FETCH_ERRORS as e:
        errors.append(f"CC hooks doc fetch failed (transient?): {e}")
        hooks_doc = None
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
    try:
        codex_ours = read_codex_events()
        cc_ours = read_cc_events()
        dispatch_names = read_dispatch_names()
        reasonix_ours = read_reasonix_events()
        codewhale_ours = read_codewhale_events()
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
