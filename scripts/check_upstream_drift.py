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
  * CC dispatch tool   -> the known names in `make_tool_detail`
                          vs the tool list in code.claude.com tools-reference
  * Reasonix hooks     -> `REASONIX_EVENTS` in crates/pixtuoid/src/install/reasonix.rs
                          + the payload fields decode_rx_hook_payload reads
                          vs the `Event` consts / json tags in
                          esengine/DeepSeek-Reasonix internal/hook/hook.go

Exit codes:
  0  no drift
  1  actionable drift (a name we depend on vanished, or a new upstream Codex
     hook we neither register nor intentionally omit) -> open a tracking issue
  2  could not check (network/HTTP error) -> transient, do NOT alarm

See crates/pixtuoid-core/CLAUDE.md "Keeping the decode mapping current".
"""

from __future__ import annotations

import pathlib
import re
import sys
import urllib.error
import urllib.request

REPO = pathlib.Path(__file__).resolve().parent.parent

CODEX_PROTOCOL_URL = (
    "https://raw.githubusercontent.com/openai/codex/main/"
    "codex-rs/protocol/src/protocol.rs"
)
CC_TOOLS_URL = "https://code.claude.com/docs/en/tools-reference.md"

# Codex hook events we DELIBERATELY do not register — they are not agent
# activity a visualizer cares about. A new upstream hook NOT in this set is
# surfaced for review (it might be a lifecycle signal worth handling).
CODEX_KNOWN_OMITTED = {"PreCompact", "PostCompact"}

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


def main() -> int:
    breaking: list[str] = []
    review: list[str] = []
    errors: list[str] = []

    # Read what WE depend on from our OWN source first. A failure here means the
    # monitor itself is broken (decoder.rs / install/codex.rs refactored away from
    # what the parsers expect) — that is a LOUD breaking signal, never a transient
    # one, or drift monitoring would silently stop with zero alarm.
    codex_ours = None
    dispatch_names = None
    reasonix_ours = None
    try:
        codex_ours = read_codex_events()
        dispatch_names = read_dispatch_names()
        reasonix_ours = read_reasonix_events()
    except Exception as e:  # noqa: BLE001
        breaking.append(
            f"drift-watch cannot read our own source ({e}) — the parsers in "
            f"check_upstream_drift.py are stale (decoder.rs / install refactored?). "
            f"The monitor is blind until the script is fixed."
        )

    # --- Codex hook events (only the FETCH is transient) -------------------
    if codex_ours is not None:
        try:
            text = fetch(CODEX_PROTOCOL_URL)
        except urllib.error.URLError as e:
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
        except urllib.error.URLError as e:
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

    # --- CC subagent-dispatch tool (only the FETCH is transient) -----------
    if dispatch_names is not None:
        try:
            tools = fetch(CC_TOOLS_URL)
        except urllib.error.URLError as e:
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
