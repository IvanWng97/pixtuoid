#!/usr/bin/env python3
"""Self-test for check_upstream_drift.py — a regex-parser regression is a SILENT
monitor death: the weekly job either alarms on junk or watches nothing.

Run: `python3 scripts/check_upstream_drift_selftest.py` (exit 0 = pass).
No pytest dependency on purpose — the repo has no Python test harness.
"""

from __future__ import annotations

import ast
import dataclasses
import io
import pathlib
import re
import sys
import typing
import urllib.error

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import check_upstream_drift as d  # noqa: E402

FAILS: list[str] = []

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent

READER_NAME_SUFFIXES = {"", "_events", "_types", "_entry_types"}


def check(cond: bool, msg: str) -> None:
    if not cond:
        FAILS.append(msg)


def _http_error(code: int) -> urllib.error.HTTPError:
    return urllib.error.HTTPError("https://x/y", code, "msg", {}, io.BytesIO(b""))


def test_try_fetch_classifies_permanent_vs_transient() -> None:
    real = d.fetch
    try:
        # Deliberately not `breaking`: a 404 says nothing about whether upstream
        # renamed anything, only that we are looking in the wrong place.
        for code in (404, 410, 451):
            d.fetch = lambda _u, c=code: (_ for _ in ()).throw(_http_error(c))
            r = d.Report()
            out = d.try_fetch("https://x/y", "T", r)
            check(out is None, f"{code}: returns None")
            check(
                len(r.blind) == 1 and not r.errors,
                f"{code}: -> blind (got blind={r.blind} errors={r.errors})",
            )
            check(str(code) in (r.blind[0] if r.blind else ""), f"{code}: message names the status")

        for code in (500, 502, 503, 429, 403):
            d.fetch = lambda _u, c=code: (_ for _ in ()).throw(_http_error(c))
            r = d.Report()
            d.try_fetch("https://x/y", "T", r)
            check(
                not r.blind and len(r.errors) == 1,
                f"{code}: -> transient (got blind={r.blind} errors={r.errors})",
            )

        d.fetch = lambda _u: (_ for _ in ()).throw(urllib.error.URLError("conn refused"))
        r = d.Report()
        d.try_fetch("https://x/y", "T", r)
        check(
            not r.blind and len(r.errors) == 1,
            f"URLError -> transient (got blind={r.blind} errors={r.errors})",
        )

        d.fetch = lambda _u: "BODY"
        r = d.Report()
        out = d.try_fetch("https://x/y", "T", r)
        check(out == "BODY" and not r.blind and not r.errors, "success returns body, no buckets")
    finally:
        d.fetch = real


def test_source_parsers_find_nonempty_well_shaped_sets() -> None:
    # (reader, shape regex, floor). The dispatch-tool name set is a legitimate
    # SINGLETON since CC dropped the `Task` alias, so it floors at 1.
    cases = [
        (d.read_codex_events, r"^[A-Za-z]\w+$", 2),
        (d.read_codex_rollout_outers, r"^[a-z][a-z_]*$", 4),
        (d.read_cc_events, r"^[A-Za-z]\w+$", 2),
        (d.read_dispatch_names, r"^[A-Za-z]\w+$", 1),
        (d.read_reasonix_events, r"^[A-Za-z]\w+$", 2),
        (d.read_codewhale_events, r"^[a-z][a-z_]*$", 2),
        (d.read_openclaw_events, r"^[a-z][a-z_]*$", 2),
        (d.read_opencode_events, r"^[a-z][a-z0-9._]*$", 2),
        (d.read_copilot_events, r"^[a-z][a-z0-9._]*$", 2),
        (d.read_copilot_namespaces, r"^[a-z][a-z_]*$", 20),
        (d.read_omp_entry_types, r"^[a-z]+$", 3),
        (d.read_omp_known_types, r"^[a-z][a-z_]*$", 10),
        (d.read_acp_tags, r"^[a-z][a-z_]*$", 10),
        (d.read_cursor_events, r"^[a-zA-Z]\w+$", 2),
        (d.read_hermes_events, r"^[a-z][a-z_]*$", 2),
        (d.read_kimi_events, r"^[A-Za-z]\w+$", 2),
        (d.read_grok_events, r"^[A-Z]\w+$", 2),
    ]
    for reader, shape, floor in cases:
        name = reader.__name__
        got = reader()
        check(isinstance(got, set) and len(got) >= floor, f"{name}: non-empty (>={floor}), got {got!r}")
        bad = [m for m in got if not re.match(shape, m)]
        check(not bad, f"{name}: members match {shape}; offenders={bad}")

    # N-of-N: a row with no floor case can return `set()` and pass vacuously.
    floored = {r.__name__ for r, _, _ in cases} | {"read_codex_rollout_types"}
    unfloored = sorted({r.__name__ for _, r, _ in d.READERS} - floored)
    check(not unfloored, f"a READERS reader has no non-empty floor case: {unfloored}")

    ev, ri = d.read_codex_rollout_types()
    check(len(ev) >= 2 and len(ri) >= 2, f"read_codex_rollout_types non-empty: ev={ev!r} ri={ri!r}")
    check("task_started" in ev, f"codex event_msg has task_started: {ev!r}")
    check("function_call" in ri, f"codex response_item has function_call: {ri!r}")
    # Synthetic, so the control survives codex.rs dropping its own `|` arm.
    synthetic = '("response_item", "alpha" | "beta" | "gamma") => {} ("response_item", "solo") => {}'
    check(
        d.match_arm_inner_types(synthetic, "response_item") == {"alpha", "beta", "gamma", "solo"},
        f"match_arm_inner_types must read every `|` alternative, got "
        f"{d.match_arm_inner_types(synthetic, 'response_item')!r}",
    )
    check("custom_tool_call" in ri, f"codex response_item reads BOTH halves of its `|` arm: {ri!r}")
    offenders = [m for m in (ev | ri) if not re.match(r"^[a-z][a-z_]*$", m)]
    check(not offenders, f"codex rollout members are snake_case; offenders={offenders}")

    # The port literal is COPIED into the plugin (un-importable from OpenClaw's
    # state dir) and the live comparison is PR-gated off, so renaming the const
    # would leave the weekly check comparing nothing.
    port = d.openclaw_plugin_default_port()
    check(
        port is not None and port.isdigit(),
        f"openclaw_plugin_default_port reads a numeric literal from the plugin, got {port!r}",
    )


def test_upstream_parsers_extract_from_a_snippet() -> None:
    codex = 'pub enum HookEventName {\n    SessionStart,\n    PreToolUse,\n    Stop,\n}'
    up = d.upstream_codex_hooks(codex)
    check(up is not None and {"SessionStart", "PreToolUse"} <= up, f"codex enum parse: {up}")

    schema = (
        '{"definitions":{"A":{"properties":{"type":{"const":"session.start"}}},'
        '"B":{"properties":{"type":{"const":"tool.execution_start"}}}}}'
    )
    up = d.upstream_copilot_events(schema)
    check(up is not None and {"session.start", "tool.execution_start"} <= up, f"copilot schema parse: {up}")

    check(d.upstream_copilot_events("not json") is None, "copilot bad json -> None")

    # `Blob` shares the `type.const` shape but sits outside the SessionEvent
    # union — it must not leak a phantom family.
    ns_schema = (
        '{"definitions":{'
        '"SessionEvent":{"anyOf":[{"$ref":"#/definitions/SessStart"},{"$ref":"#/definitions/ToolStart"}]},'
        '"SessStart":{"properties":{"type":{"const":"session.start"}}},'
        '"ToolStart":{"properties":{"type":{"const":"tool.execution_start"}}},'
        '"Blob":{"properties":{"type":{"const":"blob"}}}}}'
    )
    up = d.upstream_copilot_namespaces(ns_schema)
    check(up == {"session", "tool"}, f"copilot namespaces scoped to SessionEvent union (no Blob leak): {up}")
    check(d.upstream_copilot_namespaces("not json") is None, "copilot namespaces bad json -> None")
    check(d.upstream_copilot_namespaces('{"definitions":{}}') is None, "copilot namespaces no union -> None")

    omp_ts = (
        'const SESSION_TITLE_SLOT_ENTRY_TYPE = "title";\n'
        'interface Msg { type: "message"; }\n'
        'interface Slot { type: typeof SESSION_TITLE_SLOT_ENTRY_TYPE; }\n'
    )
    up = d.upstream_omp_entry_types(omp_ts)
    check(up == {"message", "title"}, f"omp entry types (literal + typeof-resolved): {up}")
    check(d.upstream_omp_entry_types("no types here") is None, "omp entry types none -> None")

    acp_schema = (
        '{"$defs":{"SessionUpdate":{"oneOf":['
        '{"properties":{"sessionUpdate":{"const":"tool_call"}}},'
        '{"properties":{"sessionUpdate":{"const":"user_message_chunk"}}},'
        '{"$ref":"#/$defs/PlanUpd"}]},'
        '"PlanUpd":{"properties":{"sessionUpdate":{"const":"plan_update"}}}}}'
    )
    up = d.upstream_acp_session_update_tags(acp_schema)
    check(
        up == {"tool_call", "user_message_chunk", "plan_update"},
        f"acp tags (inline const + $ref resolved): {up}",
    )
    check(d.upstream_acp_session_update_tags("not json") is None, "acp bad json -> None")
    check(d.upstream_acp_session_update_tags('{"$defs":{}}') is None, "acp no SessionUpdate -> None")

    copilot_fields = '{"definitions":{"A":{"properties":{"agentId":{},"data":{"properties":{"toolCallId":{}}}}}}}'
    up = d.upstream_copilot_field_names(copilot_fields)
    check(up is not None and {"agentId", "toolCallId"} <= up, f"copilot field union (recursive): {up}")
    check(d.upstream_copilot_field_names("not json") is None, "copilot fields bad json -> None")

    cc_md = (
        "| Event | When it fires |\n"
        "|---|---|\n"
        "| `PreToolUse` | before a tool call |\n"
        "| `PostToolUse` | after a tool call |\n"
    )
    up = d.upstream_cc_hook_events(cc_md)
    check(up is not None and {"PreToolUse", "PostToolUse"} <= up, f"cc table parse: {up}")
    check(d.upstream_cc_hook_events("no table here") is None, "cc no table -> None")

    reasonix_go = 'const (\n\tPreToolUse Event = "PreToolUse"\n\tStop Event = "Stop"\n)'
    up = d.upstream_reasonix_hooks(reasonix_go)
    check(up is not None and {"PreToolUse", "Stop"} <= up, f"reasonix consts parse: {up}")
    check(d.upstream_reasonix_hooks("no consts here") is None, "reasonix none -> None")

    codewhale_rs = "pub enum HookEvent {\n    SessionStart,\n    PreToolUse,\n}"
    up = d.upstream_codewhale_hooks(codewhale_rs)
    check(up is not None and {"session_start", "pre_tool_use"} <= up, f"codewhale enum parse: {up}")
    check(d.upstream_codewhale_hooks("no enum here") is None, "codewhale none -> None")

    # BOTH declaration shapes: the macro-def arm is the trap — its body carries a
    # literal `pub enum HookEventName {` with `$variant` placeholders and no real
    # variants, so a parser must fall THROUGH it to the invocation (#793).
    grok_plain = "pub enum HookEventName {\n    SessionStart,\n    PreToolUse,\n}"
    up = d.upstream_grok_hooks(grok_plain)
    check(up is not None and {"SessionStart", "PreToolUse"} <= up, f"grok plain enum parse: {up}")
    grok_macro = (
        "macro_rules! hook_events {\n"
        "    ($($variant:ident { display: $d:literal, }),* $(,)?) => {\n"
        "        pub enum HookEventName {\n"
        "            $($variant),*\n"
        "        }\n"
        "    };\n"
        "}\n"
        "\n"
        "hook_events! {\n"
        "    SessionStart {\n"
        '        display: "session_start",\n'
        '        aliases: ["SessionStart", "session_start"],\n'
        "        traits: (Observe, Tested, true),\n"
        "    },\n"
        "    /// A doc comment on a row must not be read as a variant.\n"
        "    SubagentEnd {\n"
        '        display: "subagent_stop",\n'
        '        aliases: ["SubagentEnd"],\n'
        "        traits: (Stop, Tested, true),\n"
        "    },\n"
        "}\n"
    )
    up = d.upstream_grok_hooks(grok_macro)
    check(
        up is not None and up == {"SessionStart", "SubagentEnd"},
        f"grok macro-table parse (exact, no alias/trait leakage): {up}",
    )
    grok_def_only = grok_macro.split("hook_events! {")[0]
    check(d.upstream_grok_hooks(grok_def_only) is None, "grok macro def without invocation -> None")
    check(d.upstream_grok_hooks("no enum here") is None, "grok none -> None")

    codex_enum = (
        "pub enum EventMsg {\n"
        '    #[serde(rename = "task_started", alias = "turn_started")]\n'
        "    TurnStarted(TurnStartedEvent),\n"
        "    ExecCommandEnd(ExecCommandEndEvent),\n"
        "    SessionConfigured { model: ModelInfo, cwd: PathBuf },\n"
        "    Other,\n"
        "}"
    )
    up = d.upstream_codex_enum_types(codex_enum, "EventMsg")
    check(
        up is not None and {"task_started", "turn_started", "exec_command_end"} <= up,
        f"codex EventMsg parse (rename+alias+snake): {up}",
    )
    check(up is not None and "model_info" not in up and "path_buf" not in up, f"codex struct-field type leaked: {up}")
    check(d.upstream_codex_enum_types("no enum here", "EventMsg") is None, "codex enum none -> None")

    fc_struct = "FunctionCall {\n    name: String,\n    arguments: String,\n    call_id: String,\n}"
    up = d.codex_function_call_fields(fc_struct)
    check(up is not None and {"name", "arguments"} <= up, f"codex FunctionCall fields: {up}")
    check(
        d.codex_function_call_fields("FunctionCall(FunctionCallItem),") is None,
        "codex FunctionCall tuple variant -> None (graceful skip, not an alarm)",
    )
    tc_struct = (
        "pub struct TurnContextItem {\n"
        "    #[serde(default, skip_serializing_if = \"Option::is_none\")]\n"
        "    pub turn_id: Option<String>,\n"
        "    pub cwd: AbsolutePathBuf,\n"
        "    pub model: String,\n"
        "    #[serde(skip_serializing_if = \"Option::is_none\")]\n"
        "    pub effort: Option<ReasoningEffortConfig>,\n"
        "}\n"
    )
    up = d.codex_turn_context_fields(tc_struct)
    check(up is not None and {"model", "effort"} <= up, f"codex TurnContextItem fields: {up}")
    check(
        d.codex_turn_context_fields("pub enum RolloutItem { TurnContext(TurnContextItem) }") is None
        or "model" not in (d.codex_turn_context_fields("pub enum RolloutItem { TurnContext(TurnContextItem) }") or set()),
        "codex TurnContextItem absent -> None (caller alarms)",
    )


def test_cc_doc_marker_detection_fires_both_directions() -> None:
    quiet = "\n".join(d.CC_DEPENDED_DOC_MARKERS)
    got = d.cc_doc_marker_findings(quiet)
    check(got == [], f"quiet doc -> no findings: {got!r}")

    missing = "\n".join(m for m in d.CC_DEPENDED_DOC_MARKERS if m != "CLAUDE_EFFORT")
    got = d.cc_doc_marker_findings(missing)
    check(len(got) == 1 and "CLAUDE_EFFORT" in got[0], f"vanish fires: {got!r}")

    got = d.cc_doc_marker_findings(quiet + "\nultra_effort_exit\n")
    check(len(got) == 1 and "ultra_effort_exit" in got[0], f"appearance fires: {got!r}")


def test_const_array_parser_ignores_words_quoted_inside_comments() -> None:
    src = '''
const DEMO_EVENTS: &[&str] = &[
    "SessionStart",
    // upstream's payload carries a reason const "other"; the field is
    // `hook_event_name:"SessionEnd"` — neither is a registered event here
    /* a block comment naming "PreToolUse" is not a registration either */
    "SessionEnd",
];
'''
    got = d.parse_rust_const_str_array(src, "DEMO_EVENTS")
    check(got == {"SessionStart", "SessionEnd"}, f"comments excluded: {got!r}")

    # An empty set would read as "nothing registered" and silence every
    # downstream check.
    check(
        d.parse_rust_const_str_array(src, "NOPE_EVENTS") is None,
        "a missing const returns None, never an empty set",
    )


def test_parser_never_drops_a_real_event() -> None:
    """Each case is a construct that made some formulation of this parser swallow
    a real entry — a dropped registration is SILENT, unlike a phantom.
    """
    cases = [
        # `//` inside a string literal: a line-comment strip runs to end of line.
        ('"SessionStart", "a//b", "SessionEnd"', {"SessionStart", "SessionEnd"}),
        ('"Alpha", "http://x", "Charlie"', {"Alpha", "Charlie"}),
        # Nested block comments are legal Rust; a non-greedy `/\\*.*?\\*/` closes
        # at the FIRST `*/` and re-admits words from the comment's tail.
        ('"Alpha", /* outer /* inner */ names "Phantom" */ "B"', {"Alpha", "B"}),
        # A `/*` that only ever appears inside a line comment must not open a
        # block that swallows the entries after it.
        ('"Alpha",\n // uses /* as a marker\n "Beta", /* real */', {"Alpha", "Beta"}),
        # Escaped quotes must not end the string early.
        (r'"Alpha", "say \"Beta\"", "Gamma"', {"Alpha", "Gamma"}),
        ('"Alpha",\n // see https://x "Notification"\n "Beta"', {"Alpha", "Beta"}),
    ]
    for body, want in cases:
        got = d.parse_rust_const_str_array(f"const E: &[&str] = &[{body}];", "E")
        check(got == want, f"no real event dropped from `{body}`: {got!r} != {want!r}")


def test_block_scrape_is_bounded_to_the_decoder() -> None:
    src = """
fn decode(v: Value) -> Vec<Event> {
    let out = match (outer, inner) {
        ("event_msg", "task_started") => vec![start()],
        ("response_item", "function_call") => { let f = |x| { x }; vec![f(call())] }
        _ => vec![],
    };
    out
}

#[cfg(test)]
mod tests {
    fn planted() { let _ = ("event_msg", "PHANTOM"); }
}
"""
    block = d.rust_block_after(d.strip_rust_comments(src), r"match \(outer, inner\)")
    check(block is not None, "the anchor's block is found")
    got = set(re.findall(r'\(\s*"event_msg"\s*,\s*"(\w+)"\s*\)', block or ""))
    # Nested braces from the closure/vec! must not close the block early, and the
    # planted test tuple after it must not be visible.
    check(got == {"task_started"}, f"bounded to the decoder's arms: {got!r}")
    check(
        "function_call" in (block or ""),
        "the nested-brace arm is INSIDE the block (it did not close early)",
    )

    check(
        d.rust_block_after("fn unrelated() { }", r"match \(outer, inner\)") is None,
        "a missing anchor returns None, never an empty block",
    )


def test_every_const_array_reader_uses_the_shared_parser() -> None:
    """No reader may hand-roll the scrape the shared parser exists to own."""
    src = (pathlib.Path(__file__).parent / "check_upstream_drift.py").read_text()
    offenders = [
        ln.strip()
        for ln in src.splitlines()
        if 'findall(r\'"(\\w+)"\'' in ln and "strip_rust_comments" not in ln
    ]
    check(
        not offenders,
        "every `\"(\\\\w+)\"` scrape must route through strip_rust_comments; "
        f"unrouted: {offenders!r}",
    )


# One snippet per anchored document; a new ANCHORS entry with no sample here
# fails the gate test below.
ANCHOR_SAMPLES: dict[str, str] = {
    d.HERMES_PLUGINS_URL: 'VALID_HOOKS: Set[str] = {\n    "pre_tool_call",\n}',
    d.CODEWHALE_EXECUTOR_URL: "pub fn to_env_vars(&self) -> HashMap<String, String> {",
    d.OPENCODE_EVENT_URLS[0]: "\nexport const Event = {\n  Created,\n}",
    d.OPENCODE_EVENT_URLS[1]: "\nexport const Event = {\n  Asked,\n}",
    d.GROK_HOOK_URL: "pub struct HookEventEnvelope {\n    pub cwd: String,\n}",
    d.GROK_NOTIFICATION_URL: "pub enum SessionUpdate {\n    SubagentSpawned,\n}",
    d.GROK_ACTIVE_SESSIONS_URL: "pub struct ActiveSession {\n    pub pid: u32,\n}",
    d.OMP_SESSION_ENTRIES_URL: "export type SessionEntry = MessageEntry | CustomEntry;",
    d.OMP_EXIT_DIAG_URL: 'export const SESSION_EXIT_CUSTOM_TYPE = "session_exit";',
    d.OMP_AI_TYPES_URL: "export type Message = UserMessage | AssistantMessage;",
    d.OMP_ASK_URL: "export class AskTool extends Tool {}",
    d.OMP_DIRS_URL: (
        "class DirResolver {\n"
        '  constructor() { process.env.PI_CONFIG_DIR; process.env.OMP_PROFILE;\n'
        '    process.env.PI_PROFILE; process.env.PI_CODING_AGENT_DIR;\n'
        '    process.env.XDG_DATA_HOME; }\n'
        "}\n"
    ),
    d.OMP_ENV_URL: "export function parseEnvFile(filePath: string): Record<string, string> {}",
    d.CURSOR_HOOKS_URL: '"hook_event_name": "beforeShellExecution"',
    d.OPENCLAW_HOOK_TYPES_URL: 'export type PluginHookName =\n  | "gateway_start"',
    d.HERMES_HOOK_URL: "_DEFAULT_PAYLOADS = {\n    'on_session_start': {},\n}",
    d.HERMES_SHELL_HOOK_URL: "def _serialize_payload(event: str) -> str:",
    d.HERMES_HOME_URL: (
        "def _hermes_home_from_env() -> Path:\n"
        '    val = os.environ.get("HERMES_HOME", "").strip()\n'
        '    return Path(val) if val else Path(os.environ.get("LOCALAPPDATA", ""))\n'
    ),
    d.KIMI_HOOKS_URL: '"hook_event_name": "PreToolUse"',
    d.CC_TOOLS_URL: "\n# Tools reference\n",
    d.CC_HOOKS_URL: "\n# Hooks reference\n",
}

# A document that satisfies NO anchor — the "upstream reorganized this file"
# stand-in: content that fetches fine and owns nothing.
_UNANCHORED = "mod config;\nmod executor;\npub use config::*;\n"


def test_anchor_gate_fires_in_both_directions() -> None:
    """A PURE UPSTREAM REFACTOR must produce probe-health, never drift (#793)."""
    real = d.fetch
    try:
        # A floor, because everything below iterates `ANCHORS`: an emptied table
        # would run zero loop bodies and this test would pass VACUOUSLY.
        check(len(d.ANCHORS) >= 19, f"ANCHORS covers every swept document, got {len(d.ANCHORS)}")
        missing_samples = sorted(set(d.ANCHORS) - set(ANCHOR_SAMPLES))
        check(not missing_samples, f"every ANCHORS entry needs a sample: {missing_samples}")

        for url, anchor in sorted(d.ANCHORS.items()):
            served: list[str] = []

            d.fetch = lambda u, _s=served: (_s.append(u), _UNANCHORED)[1]
            r = d.Report()
            out = d.fetch_anchored(url, "T", r)
            check(served == [url], f"{anchor.owns}: the stubbed fetch actually ran")
            check(out is None, f"{anchor.owns}: no anchor -> the sweep is skipped")
            check(len(r.blind) == 1 and not r.errors, f"{anchor.owns}: -> blind, got {r.blind}")
            # Guarded index: a regressed gate leaves `blind` empty, and that must
            # REPORT rather than abort the suite before the later tests run.
            check(
                "NOT evidence that upstream changed" in (r.blind[0] if r.blind else ""),
                f"{anchor.owns}: the line disclaims upstream causation, got {r.blind!r}",
            )

            sample = ANCHOR_SAMPLES.get(url, "")
            d.fetch = lambda _u, _s=sample: _s
            r = d.Report()
            out = d.fetch_anchored(url, "T", r)
            check(
                out == sample and not r.blind and not r.errors,
                f"{anchor.owns}: anchor present -> body returned, got blind={r.blind}",
            )
    finally:
        d.fetch = real


def test_report_is_the_only_way_to_file_a_finding() -> None:
    """`Report` owns the buckets, their wording, their order, and the exit code."""
    empty = d.Report()
    check(empty.exit_code() == 0, f"an empty report exits 0, got {empty.exit_code()}")
    check("✅ No drift" in empty.render(), f"an empty report says so:\n{empty.render()}")

    r = d.Report()
    r.add_breaking("B-LINE")
    r.add_review("R-LINE")
    r.add_error("E-LINE")
    r.add_blind("WHAT", "WHERE", "CONSEQUENCE")
    check(r.breaking == ["B-LINE"], f"add_breaking -> breaking, got {r.breaking}")
    check(r.review == ["R-LINE"], f"add_review -> review, got {r.review}")
    check(r.errors == ["E-LINE"], f"add_error -> errors, got {r.errors}")
    check(len(r.blind) == 1, f"add_blind -> blind, got {r.blind}")

    line = r.blind[0] if r.blind else ""
    check(
        all(p in line for p in ("WHAT", "WHERE", "CONSEQUENCE")),
        f"add_blind keeps the caller's three facts: {line!r}",
    )
    check(
        "NOT evidence that upstream changed" in line and "do NOT change a decoder" in line,
        f"the default wording disclaims upstream causation: {line!r}",
    )
    ours = d.Report()
    ours.add_blind("WHAT", "WHERE", "CONSEQUENCE", our_source=True)
    our_line = ours.blind[0] if ours.blind else ""
    check(
        "nothing upstream was consulted" in our_line.lower()
        and "Fix the script" in our_line,
        f"our_source=True blames the script, not upstream: {our_line!r}",
    )

    out = r.render()
    offsets = [out.find(x) for x in ("B-LINE", "R-LINE", "WHAT", "E-LINE")]
    check(
        all(x >= 0 for x in offsets) and offsets == sorted(offsets),
        f"every bucket renders, in disposition order (offsets {offsets}):\n{out}",
    )

    # errors-ONLY is exit 2 (transient) — the one case `main()` cannot reach
    # without faking the network.
    for adder, want in (("add_breaking", 1), ("add_review", 1), ("add_error", 2)):
        one = d.Report()
        getattr(one, adder)("LINE")
        check(
            one.exit_code() == want,
            f"a {adder}-only report exits {want}, got {one.exit_code()}",
        )
    blind_only = d.Report()
    blind_only.add_blind("w", "where", "c")
    check(blind_only.exit_code() == 1, f"a blind-only report exits 1, got {blind_only.exit_code()}")
    both = d.Report()
    both.add_error("E")
    both.add_blind("w", "where", "c")
    check(both.exit_code() == 1, f"actionable outranks transient, got {both.exit_code()}")

    # An AST walk, not a regex: a regex also fires on prose QUOTING the spelling.
    src = (pathlib.Path(__file__).parent / "check_upstream_drift.py").read_text()
    tree = ast.parse(src)
    report_cls = next(
        (n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "Report"), None
    )
    check(report_cls is not None, "the module defines a top-level `class Report`")
    inside = {id(n) for n in ast.walk(report_cls)} if report_cls else set()
    buckets = {"breaking", "review", "blind", "errors"}
    stray = []
    for node in ast.walk(tree):
        if id(node) in inside:
            continue
        if not (isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)):
            continue
        if node.func.attr != "append":
            continue
        target = node.func.value
        name = (
            target.id if isinstance(target, ast.Name)
            else target.attr if isinstance(target, ast.Attribute)
            else None
        )
        if name in buckets:
            stray.append(f"{name}.append at check_upstream_drift.py:{node.lineno}")
    check(
        not stray,
        f"a finding must be filed through `report.add_*`, never appended to a "
        f"bucket — and no local list outside `Report` may borrow a bucket's name "
        f"(call it `findings`): {stray}",
    )


def test_one_stale_reader_does_not_blind_the_sources_after_it() -> None:
    """A parser that goes stale must dark ITS source, not the rest of the file."""
    real_fetch, real_readers = d.fetch, d.READERS
    # Every fetched document lands on the wrong content: a source gone dark
    # fetches nothing and says nothing.
    fetched: set[str] = set()
    d.fetch = lambda u: (fetched.add(u), "unrelated body")[1]

    def run() -> tuple[set[str], str]:
        fetched.clear()
        buf, real_stdout = io.StringIO(), sys.stdout
        sys.stdout = buf
        try:
            d.main()
        finally:
            sys.stdout = real_stdout
        return set(fetched), buf.getvalue()

    try:
        base_urls, _ = run()
        check(len(base_urls) > 20, f"the baseline exercises the file, got {len(base_urls)} URLs")

        # Inject into the ROW, never `d.read_codex_events` — see the WHY on the
        # READERS declaration.
        first_field, first_reader, first_what = d.READERS[0]
        d.READERS = (
            (first_field, _raiser(first_reader.__name__), first_what),
            *d.READERS[1:],
        )
        broken_urls, out = run()

        check(
            first_reader.__name__ in out,
            f"the injected fault fired and named `{first_reader.__name__}`:\n{out}",
        )
        own = [ln for ln in out.splitlines() if first_reader.__name__ in ln]
        check(
            len(own) == 1
            and "Fix the script" in own[0]
            and "Re-verify upstream by hand" not in own[0],
            f"the stale OWN-source line blames the script, not upstream: {own}",
        )
        # These four sit at the far end of the table.
        for name in ("CURSOR_HOOKS_URL", "HERMES_HOOK_URL", "GROK_HOOK_URL", "KIMI_HOOKS_URL"):
            check(
                getattr(d, name) in broken_urls,
                f"a stale reader in row 1 must not dark {name} in row 10+ "
                f"(fetched {len(broken_urls)} of {len(base_urls)} URLs):\n{out}",
            )
        check(
            "Nothing upstream was checked" not in out,
            f"a single stale reader must not claim the whole run was blind:\n{out}",
        )
    finally:
        d.fetch, d.READERS = real_fetch, real_readers


def _raiser(name: str) -> typing.Callable[[], object]:
    """A reader that always fails, keeping `__name__` so the report can name it."""

    def broken() -> object:
        raise RuntimeError("parser stale")

    broken.__name__ = name
    return broken


def test_a_stale_flood_guard_skips_its_check_instead_of_flooding() -> None:
    """The flood guards are one-directional — `up - set()` reports the whole
    upstream surface as new — so a stale reader must SKIP its check.

    The upstream PARSER is stubbed, not just `fetch`: an unparseable body takes
    the `is None` probe-health branch and never reaches the guard.
    """
    real_fetch, real_readers = d.fetch, d.READERS
    parsers = ("upstream_omp_entry_types", "upstream_acp_session_update_tags")
    real_parsers = {n: getattr(d, n) for n in parsers}
    # Carries omp's ANCHORS pattern: without it `fetch_anchored` files probe
    # health and the omp guard is never reached (acp rides plain `try_fetch`).
    d.fetch = lambda _u: "unrelated body\nexport type SessionEntry = never;\n"
    d.upstream_omp_entry_types = lambda _t: {"phantom_entry_type"}
    d.upstream_acp_session_update_tags = lambda _t: {"phantom_acp_tag"}
    try:
        # Assert on the INJECTED upstream token, not the KNOWN_* name — the
        # probe-health line quotes KNOWN_* too, so matching that flags a clean run.
        for field, reader_name, phantom_name in (
            ("omp_known_types", "read_omp_known_types", "phantom_entry_type"),
            ("acp_tags", "read_acp_tags", "phantom_acp_tag"),
        ):
            d.READERS = tuple(
                (f, (_raiser(r.__name__) if f == field else r), w) for f, r, w in real_readers
            )
            buf, real_stdout = io.StringIO(), sys.stdout
            sys.stdout = buf
            try:
                d.main()
            finally:
                sys.stdout = real_stdout
            out = buf.getvalue()
            phantom = [ln for ln in out.splitlines() if phantom_name in ln]
            check(
                not phantom,
                f"a stale `{field}` must SKIP its guard, not compare against an "
                f"empty set: {phantom[:2]}",
            )
            named = [ln for ln in out.splitlines() if reader_name in ln]
            check(len(named) == 1, f"a stale `{field}` files ONE probe-health line: {named}")
    finally:
        d.fetch, d.READERS = real_fetch, real_readers
        for n, fn in real_parsers.items():
            setattr(d, n, fn)


def test_every_reader_row_matches_a_field_on_our_names() -> None:
    """The READERS table is stringly-keyed; nothing else pins it to `OurNames`."""
    rows = {field for field, _, _ in _reader_rows()}
    fields = {f.name for f in dataclasses.fields(d.OurNames)}
    check(
        rows == fields,
        f"every READERS row must name an OurNames field and every field must be "
        f"filled by a row — otherwise the source is silently unchecked with no "
        f"probe-health line. Mismatched: {rows ^ fields}. Add the missing row or "
        f"field, or drop the orphan.",
    )
    # None, not an empty container: a truthy-empty default sails past `is not None`.
    fresh = d.OurNames()
    non_none = sorted(f.name for f in dataclasses.fields(fresh) if getattr(fresh, f.name) is not None)
    check(not non_none, f"OurNames fields must default to None, got set: {non_none}")
    for field, reader_name, what in _reader_rows():
        check(callable(getattr(d, reader_name, None)), f"{reader_name} is a real reader")
        check(bool(what.strip()), f"{reader_name} declares what it reads")
        # `field in reader_name` is too weak: every flood-guard field is a
        # SUFFIX-EXTENSION of a source field, so the likeliest mispair —
        # ("copilot", read_copilot_namespaces) — is the one a substring test
        # cannot see.
        stem = reader_name.removeprefix("read_")
        suffix = stem[len(field):] if stem.startswith(field) else None
        check(
            suffix in READER_NAME_SUFFIXES,
            f"READERS pairs field `{field}` with `{reader_name}`: the reader's "
            f"name must be `read_{field}` plus one of {sorted(READER_NAME_SUFFIXES)}. "
            f"A mispaired row feeds one source's names to another source's sweep, "
            f"which reports them all as renames. Fix the pairing, or name the "
            f"field after its reader.",
        )


def test_no_reader_is_called_outside_the_readers_table() -> None:
    """An inline `read_*()` raise unwinds to `main()`'s catch-all and is filed
    TRANSIENT — exit 2, a warning on a GREEN run — instead of probe health.
    """
    src = (pathlib.Path(__file__).parent / "check_upstream_drift.py").read_text()
    tree = ast.parse(src)
    tabled = {row.elts[1].id for row in next(
        n.value for n in tree.body
        if isinstance(n, ast.AnnAssign) and getattr(n.target, "id", "") == "READERS"
    ).elts}
    check(
        tabled == {name for _, name, _ in _reader_rows()},
        "the READERS AST scan agrees with the runtime table",
    )
    # Whole module: a `read_*` inside a HELPER that run_checks calls unwinds to
    # the same catch-all. `read_our_names` is the sanctioned caller.
    exempt = {"read_our_names"}
    scan_roots = [
        n for n in tree.body
        if isinstance(n, ast.FunctionDef)
        and n.name not in exempt
        and not n.name.startswith("read_")
    ]
    stray = sorted({
        f"{n.func.id}:{n.lineno}"
        for root in scan_roots
        for n in ast.walk(root)
        if isinstance(n, ast.Call)
        and isinstance(n.func, ast.Name)
        and n.func.id.startswith("read_")
        and n.func.id not in exempt
    })
    check(
        not stray,
        f"a `read_*` is called inline instead of being consumed from `ours`, so "
        f"its failure lands in the TRANSIENT bucket (exit 2, a warning on a green "
        f"run) instead of probe health: {stray}. Read it from the OurNames field "
        f"its READERS row fills, and guard on `is not None`.",
    )
    # Name-independent twin: the check above keys on the `read_*` convention; a
    # direct `REPO` read is the form a rename cannot slip past.
    run_checks = next(
        n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "run_checks"
    )
    repo_reads = sorted(
        {n.lineno for n in ast.walk(run_checks) if isinstance(n, ast.Name) and n.id == "REPO"}
    )
    check(
        not repo_reads,
        f"run_checks reads a repo file directly at line(s) {repo_reads} — route it "
        f"through a READERS row so its failure is isolated probe health.",
    )


def _reader_rows() -> list[tuple[str, str, str]]:
    return [(field, reader.__name__, what) for field, reader, what in d.READERS]


def test_793_stale_pin_reads_as_probe_health_not_three_renames() -> None:
    """The #793 regression: the old path kept returning 200 as a `mod`/`pub use`
    facade, so the fetch succeeded, all three `DEEPSEEK_*` names were absent, and
    they were reported as three upstream renames.
    """
    real = d.fetch
    config_rs = "pub enum HookEvent {\n    SessionStart,\n    ToolCallBefore,\n}"
    facade = "mod config;\nmod executor;\npub use config::HookEvent;\n"
    executor = (
        "impl HookContext {\n"
        "    pub fn to_env_vars(&self) -> HashMap<String, String> {\n"
        '        env.insert("DEEPSEEK_WORKSPACE".to_string(), ws);\n'
        '        env.insert("DEEPSEEK_TOOL_NAME".to_string(), name);\n'
        '        env.insert("DEEPSEEK_TOOL_ARGS".to_string(), args);\n'
        "    }\n"
        "}\n"
    )

    def drive(executor_body: str) -> tuple[list[str], list[str], list[str]]:
        served: list[str] = []

        def stub(url: str) -> str:
            served.append(url)
            if url == d.CODEWHALE_HOOK_URL:
                return config_rs
            if url == d.CODEWHALE_EXECUTOR_URL:
                return executor_body
            raise urllib.error.URLError("not stubbed")  # -> transient, ignored below

        d.fetch = stub
        report = d.Report()
        # One named field, not a wall of positional `None`s where a miscount
        # silently drove the WRONG source.
        d.run_checks(
            d.OurNames(codewhale={"session_start", "tool_call_before"}),
            report=report,
        )
        check(
            d.CODEWHALE_EXECUTOR_URL in served,
            "the executor fetch actually ran (the injected fault fired)",
        )
        cw = lambda xs: [x for x in xs if "DEEPSEEK" in x or "CodeWhale" in x]  # noqa: E731
        return cw(report.breaking), cw(report.blind), served

    br, bl, _ = drive(facade)
    check(not br, f"a stale pin must claim NO upstream change, got {br}")
    check(len(bl) == 1, f"a stale pin is one probe-health line, got {bl}")
    check(
        all(v not in "".join(bl) for v in ("DEEPSEEK_WORKSPACE", "DEEPSEEK_TOOL_NAME")),
        f"the probe-health line must not name env vars as renamed: {bl!r}",
    )

    br, bl, _ = drive(executor.replace('        env.insert("DEEPSEEK_WORKSPACE".to_string(), ws);\n', ""))
    check(
        len(br) == 1 and "DEEPSEEK_WORKSPACE" in br[0],
        f"a REAL rename must still be breaking drift, got breaking={br} blind={bl}",
    )
    check(not bl, f"a readable document produces no probe-health noise, got {bl}")

    br, bl, _ = drive(executor)
    check(not br and not bl, f"unchanged upstream -> silence, got breaking={br} blind={bl}")
    d.fetch = real


def test_every_swept_url_declares_an_anchor() -> None:
    """A presence sweep may not run on an unproven document."""
    # A bare KeyError would land in the TRANSIENT bucket (exit 2, warn-only),
    # turning "someone shipped an unproven sweep" into a green-ish warning (#454).
    real = d.fetch
    try:
        d.fetch = lambda _u: "irrelevant body"
        r = d.Report()
        out = d.fetch_anchored("https://example.invalid/undeclared", "New", r)
        check(out is None, "an undeclared URL is not swept")
        check(
            len(r.blind) == 1 and not r.errors,
            f"undeclared -> blind, not transient: {r.blind}",
        )
        check(
            "ANCHORS" in (r.blind[0] if r.blind else ""),
            f"the line names the fix (add an ANCHORS entry): {r.blind!r}",
        )
    finally:
        d.fetch = real

    src = (pathlib.Path(__file__).parent / "check_upstream_drift.py").read_text()
    swept = set(re.findall(r"fetch_anchored\(\s*(\w+(?:\[\d\])?)\s*,", src))
    # A lowercase first arg is a loop variable; resolve it to the tuple it iterates.
    for loop_var in {n for n in swept if not n[0].isupper()}:
        swept.discard(loop_var)
        swept |= {
            f"{m}[{i}]"
            for m in re.findall(rf"for {loop_var} in ([A-Z_]+)", src)
            for i in range(len(getattr(d, m)))
        }
    declared = {
        name
        for name in swept
        if (base := name.split("[")[0]) and hasattr(d, base)
    }
    check(swept == declared, f"a swept URL is not a module constant: {swept - declared}")
    for name in sorted(swept):
        url = eval(f"d.{name}")  # noqa: S307 (module constants matched by the regex above)
        check(url in d.ANCHORS, f"{name} is swept but declares no ANCHORS entry")

    # Enumerating the *_URL CONSTANTS rather than the call sites, deliberately: a
    # call-site regex has to keep up with how the fetch is spelled. Every URL the
    # module declares must be classified exactly once.
    urls: set[str] = set()
    for name in dir(d):
        if not (name.endswith("_URL") or name.endswith("_URLS")):
            continue
        value = getattr(d, name)
        urls |= {value} if isinstance(value, str) else set(value)
    classified = set(d.ANCHORS) | set(d.UNANCHORED_BY_DESIGN)
    check(
        not (urls - classified),
        f"every fetched URL is anchored or justified in UNANCHORED_BY_DESIGN; "
        f"unclassified: {sorted(urls - classified)}",
    )
    check(
        not (classified - urls),
        f"no classification for a URL the module no longer declares: "
        f"{sorted(classified - urls)}",
    )
    check(
        not (set(d.ANCHORS) & set(d.UNANCHORED_BY_DESIGN)),
        "a document is either anchored or justified-unanchored, never both",
    )


def test_report_separates_verified_change_from_probe_health() -> None:
    """The two dispositions must not share a heading: the heading is the
    instruction, and a reader who trusts a wrong one edits a decoder (#793).
    """
    real_run, real_read = d.run_checks, d.read_codex_events
    try:
        def fake(*a: object, report: d.Report, **k: object) -> None:
            report.add_breaking("VERIFIED-CHANGE-LINE")
            report.add_blind("PROBE-HEALTH-LINE", "somewhere", "Checks were SKIPPED.")

        d.run_checks = fake
        buf = io.StringIO()
        real_stdout, sys.stdout = sys.stdout, buf
        try:
            code = d.main()
        finally:
            sys.stdout = real_stdout
        out = buf.getvalue()

        check(code == 1, f"either bucket is actionable, exit 1 (got {code})")
        verified_hd = "## ⛔ Verified upstream change"
        probe_hd = "## 🩺 Probe could NOT verify"
        check(verified_hd in out and probe_hd in out, f"both headings present:\n{out}")
        order = [
            out.find(verified_hd),
            out.find("VERIFIED-CHANGE-LINE"),
            out.find(probe_hd),
            out.find("PROBE-HEALTH-LINE"),
        ]
        check(
            all(x >= 0 for x in order) and order == sorted(order),
            f"each line filed under its own heading (offsets {order}):\n{out}",
        )
        check(
            "do NOT change a decoder" in out,
            "the probe-health section says what NOT to do",
        )

        # A blind-ONLY case, because the one above cannot test it: with a breaking
        # line injected, `breaking` carries the exit code and `blind`'s
        # contribution is never exercised.
        def blind_only(*a: object, report: d.Report, **k: object) -> None:
            report.add_blind("PROBE-HEALTH-ONLY", "somewhere", "Checks were SKIPPED.")

        d.run_checks = blind_only
        buf = io.StringIO()
        real_stdout, sys.stdout = sys.stdout, buf
        try:
            code = d.main()
        finally:
            sys.stdout = real_stdout
        check(code == 1, f"a blind-ONLY report is actionable and exits 1, got {code}")
        check(
            "Probe could NOT verify" in buf.getvalue()
            and "Verified upstream change" not in buf.getvalue(),
            "a blind-only report renders ONLY the probe-health section",
        )
    finally:
        d.run_checks, d.read_codex_events = real_run, real_read


def test_enum_body_survives_struct_variants_and_indentation() -> None:
    """`(.*?)\\}` stopped at the first `}` (a struct variant truncated the enum);
    `(.*?)\\n\\}` additionally demanded a column-0 closing brace, so an indented
    enum read as "moved upstream" (#793).
    """
    struct_variant = (
        "pub enum HookEvent {\n"
        "    SessionStart,\n"
        "    ToolCallBefore { name: String, args: Value },\n"
        "    SessionEnd,\n"
        "}\n"
    )
    got = d.upstream_codewhale_hooks(struct_variant)
    check(
        got is not None and "session_end" in got,
        f"a variant AFTER a struct variant survives (no first-`}}` truncation): {got}",
    )

    # The OVER-capture direction: `upstream_codex_hooks` scrapes every CamelCase
    # word, so a spill into the following `impl` becomes phantom variants — and a
    # phantom is worse than a miss, because it makes a real rename look present.
    indented = (
        "mod outer {\n"
        "    pub enum HookEventName {\n"
        "        PreToolUse,\n"
        "    }\n"
        "\n"
        "    impl HookEventName {\n"
        "        fn f(&self) -> PhantomVariant { PhantomVariant }\n"
        "    }\n"
        "}\n"
    )
    got = d.upstream_codex_hooks(indented)
    check(got is not None and "PreToolUse" in got, f"the real variant is found: {got}")
    check(
        got is not None and "PhantomVariant" not in got,
        f"the body STOPS at the enum's own brace — no spill into the impl: {got}",
    )

    commented = (
        "pub enum HookEvent {\n"
        "    /// Fires like `Foo { bar }` does.\n"
        "    SessionStart,\n"
        "    SessionEnd,\n"
        "}\n"
    )
    got = d.upstream_codewhale_hooks(commented)
    check(
        got is not None and {"session_start", "session_end"} <= got,
        f"a brace in a comment does not truncate the body: {got}",
    )

    # An empty set would report every registered event as GONE.
    check(d.upstream_codewhale_hooks("struct Other {}") is None, "absent enum -> None")
    check(d.upstream_codex_hooks("struct Other {}") is None, "absent enum -> None")
    check(
        d.upstream_codewhale_hooks("pub enum HookEventName {\n    PreToolUse,\n}") is None,
        "`HookEvent` must not match `enum HookEventName`",
    )


def test_report_h1_is_the_issue_title_and_carries_the_disposition() -> None:
    """The H1 is a CROSS-FILE contract: upstream-drift.yml titles the GitHub issue
    with `head -1 | sed 's/^# //'` rather than keeping its own copy of these strings.
    """
    real = d.run_checks
    cases = [
        ("breaking", "Upstream CLI wire-format drift detected"),
        ("review", "New upstream events to review"),
        ("blind", "Upstream drift watch could not verify — repin needed"),
        ("clean", "Upstream wire-format watch: no drift"),
    ]
    try:
        for name, want in cases:
            def fake(*a: object, report: d.Report, _n: str = name, **k: object) -> None:
                if _n == "blind":
                    report.add_blind("LINE", "somewhere", "Checks were SKIPPED.")
                elif _n != "clean":
                    getattr(report, f"add_{_n}")("LINE")

            d.run_checks = fake
            buf = io.StringIO()
            real_stdout, sys.stdout = sys.stdout, buf
            try:
                d.main()
            finally:
                sys.stdout = real_stdout
            got = buf.getvalue().splitlines()[0]
            check(got == f"# {want}", f"{name}: H1 is {want!r}, got {got!r}")
    finally:
        d.run_checks = real

    wf = (pathlib.Path(__file__).parents[1] / ".github/workflows/upstream-drift.yml").read_text()
    check(
        "head -1 drift-report.md" in wf and "s/^# //" in wf,
        "upstream-drift.yml still titles the issue from the report's H1",
    )
    check(
        "Verified upstream change" not in wf and "could not verify — repin" not in wf,
        "upstream-drift.yml keeps NO second copy of the report's strings",
    )



# A minimal `env.ts` satisfying every omp-env sweep, shaped like the real file in
# the two ways the sweep can be fooled: `.env`/`OMP_` appear in JSDoc as well as
# code, and the quote detector (a lone `"` inside a single-quoted string) sits
# ABOVE a comment that names checked symbols. Tracking only `"` desynchronises
# there and leaks every later comment back into the "code" — which is why the
# masking cases below live AFTER the detector, not before it.
_OMP_ENV_LIVE = (
    '/** Reads ".env" from each dir and aliases "OMP_" keys — JSDoc, not code. */\n'
    'import { getAgentDir, getConfigRootDir, refreshDirsFromEnv } from "./dirs";\n'
    "export function isValidEnvName(name: string): boolean { return true; }\n"
    "export function isSafeEnvValue(value: string): boolean { return true; }\n"
    "function parseEnvLine(line: string) {\n"
    "\tif (quote === '\"' || quote === \"'\" || quote === \"`\") return undefined;\n"
    "\treturn undefined;\n"
    "}\n"
    "export function parseEnvFile(filePath: string): Record<string, string> {\n"
    "\tconst result = {};\n"
    "\tfor (const line of content.split()) parseEnvLine(line);\n"
    '\tif (k.startsWith("OMP_")) result[`PI_${k.slice(4)}`] = result[k];\n'
    "\treturn result;\n"
    "}\n"
    "// Eagerly parse each dir's \".env\", then rebuild: getAgentDir() already\n"
    "// located one, and parseEnvLine gives it Bun's semantics. Prose, not code.\n"
    'parseEnvFile(path.join(getConfigRootDir(), ".env"));\n'
    'parseEnvFile(path.join(getAgentDir(), ".env"));\n'
    "refreshDirsFromEnv();\n"
)


def _drive_omp(bodies: dict[str, str], kind: str) -> list[str]:
    """Run the omp block over stubbed upstream files, returning only the lines
    the `kind` sweep emits — a sibling sweep's noise must not read as this one."""

    def stub(url: str) -> str:
        if url in bodies:
            return bodies[url]
        raise urllib.error.URLError("not stubbed")  # -> transient, filtered below

    real = d.fetch
    d.fetch = stub
    try:
        report = d.Report()
        d.run_checks(d.OurNames(omp={"session"}), report=report)
    finally:
        d.fetch = real
    return [x for x in report.breaking if kind in x]


def _drive_hermes(hooks_body: str) -> tuple[list[str], list[str]]:
    """Run the hermes block over a stubbed `hooks.py`, returning (breaking, blind)."""

    def stub(url: str) -> str:
        if url == d.HERMES_HOOK_URL:
            return hooks_body
        raise urllib.error.URLError("not stubbed")

    real = d.fetch
    d.fetch = stub
    try:
        report = d.Report()
        d.run_checks(d.OurNames(hermes={"pre_approval_request"}), report=report)
    finally:
        d.fetch = real
    return (
        [x for x in report.breaking if "_BLOCKING_EVENTS" in x],
        [x for x in report.blind if "observer-only" in x],
    )


def test_the_hermes_approval_gate_stays_observer_only() -> None:
    """Registering `pre_approval_request` is safe ONLY while a hook's exit code
    cannot affect it, and the shim always exits 0. Three outcomes, because the
    dangerous one is the checker going quiet rather than red."""
    head = '_DEFAULT_PAYLOADS = {"pre_approval_request": {}}\n'

    breaking, blind = _drive_hermes(head + '_BLOCKING_EVENTS = frozenset({"pre_tool_call"})\n')
    check(not breaking and not blind, f"observer-only upstream stays silent: {breaking} {blind}")

    breaking, _ = _drive_hermes(
        head + '_BLOCKING_EVENTS = frozenset({"pre_tool_call", "pre_approval_request"})\n'
    )
    check(
        len(breaking) == 1,
        f"the gate joining _BLOCKING_EVENTS must be BREAKING — our exit 0 would "
        f"answer a real prompt. Got {breaking}",
    )

    _, blind = _drive_hermes(head + 'BLOCKING = frozenset({"pre_tool_call"})\n')
    check(
        len(blind) == 1,
        f"a renamed/moved constant must report PROBE HEALTH, not read as safe. Got {blind}",
    )


def test_ts_comment_strip_survives_a_quote_detector_line() -> None:
    """A `"`-only scanner desynchronises on `'"'` and leaks the REST OF THE FILE's
    comments back into the "code" every presence sweep reads. upstream `env.ts`
    ships exactly that line, so this is the live shape, not a hypothetical."""
    detector = "if (quote === '\"' || quote === \"'\" || quote === \"`\") {}\n"
    body = detector + "// getAgentDir is named here in PROSE only\nconst x = 1;\n"
    code = d.strip_ts_comments(body)
    check("getAgentDir" not in code, "a comment below the quote detector must be stripped")
    check("const x = 1;" in code, "the code below it must survive")
    # …and the quote characters must still HIDE a comment marker inside a string.
    check(
        "http://x" in d.strip_ts_comments("const u = 'http://x';\n"),
        "a `//` inside a single-quoted string is not a comment",
    )
    check(
        "keep" in d.strip_ts_comments("const t = `a // keep`;\n"),
        "a `//` inside a template literal is not a comment",
    )


def test_rust_comment_strip_is_not_confused_by_lifetimes() -> None:
    """Rust's `'` is a lifetime far more often than a string opener. Tracking it
    makes an ODD number of lifetimes leave one apostrophe "open", so the scanner
    reads everything after it — comments included — as string content and stops
    stripping. That is the TS quote-detector bug with the languages swapped: a
    prose mention then masks a real rename, silently. Hence the per-language set
    rather than one shared one."""
    body = (
        "fn f<'a>(x: &'a str) -> &'a str { x }\n"
        "// RENAMED_AWAY is named here in PROSE only\n"
        'const KNOWN: &[&str] = &["a"];\n'
    )
    code = d.strip_rust_comments(body)
    check("RENAMED_AWAY" not in code, "a comment after an odd lifetime count is still stripped")
    check('&["a"]' in code, "the code after it survives")
    # The half Rust DOES share with TS: a `//` inside a string is not a comment.
    check(
        "keep" in d.strip_rust_comments('let u = "http://keep";\n'),
        "a `//` inside a Rust string is not a comment",
    )


def test_omp_env_sweeps_fire_and_stay_silent() -> None:
    """`with_omp_dotenv` only tracks omp while `refreshDirsFromEnv` still runs
    AFTER the `.env` files load. If that ordering goes, our overlay becomes the
    divergence — we would honour a file omp ignores — so this sweep needs the
    both-directions treatment, comment-masking included: `.env` and `OMP_` sit
    in the JSDoc as well as the code.
    """
    live = _OMP_ENV_LIVE
    roles = (("omp dotenv locator", d.OMP_ENV_LOCATORS), ("omp dotenv parser", d.OMP_ENV_PARSERS))
    # Floors first: every loop below iterates one of these, so an emptied set
    # would run zero bodies and pass VACUOUSLY.
    for label, size, floor in (
        ("OMP_ENV_LOCATORS", len(d.OMP_ENV_LOCATORS), 3),
        ("OMP_ENV_PARSERS", len(d.OMP_ENV_PARSERS), 3),
        ("OMP_ENV_LITERALS", len(d.OMP_ENV_LITERALS), 2),
    ):
        check(size >= floor, f"{label} covers the overlay's inputs, got {size}")

    for kind in ("omp dotenv locator", "omp dotenv parser", "omp dotenv literal"):
        got = _drive_omp({d.OMP_ENV_URL: live}, kind)
        check(not got, f"{kind}: an unchanged upstream must stay silent, got {got}")

    # Each role's message must name ITS OWN requirement, or it sends the
    # maintainer to the wrong half of source/omp.rs.
    for kind, table in roles:
        other = "parser" if "locator" in kind else "locator"
        for fn, mirror in sorted(table.items()):
            got = _drive_omp({d.OMP_ENV_URL: live.replace(fn, "renamed")}, kind)
            check(
                len(got) == 1
                and f"`{fn}`" in got[0]
                and f"`{mirror}`" in got[0]
                and other not in got[0],
                f"a vanished {fn} must fire once as a {kind} naming {mirror}, got {got}",
            )

    for lit in sorted(d.OMP_ENV_LITERALS):
        got = _drive_omp(
            {d.OMP_ENV_URL: live.replace(f'"{lit}"', '"renamed"')}, "omp dotenv literal"
        )
        check(
            len(got) == 1 and f"`{lit}`" in got[0],
            f"a vanished literal {lit} must fire exactly once, got {got}",
        )

    # Renaming the ANCHOR is a BLIND probe, not a rename line — the louder
    # signal, and the reason `parseEnvFile` is in neither role table.
    gone = live.replace("export function parseEnvFile", "function gone")
    got = _drive_omp({d.OMP_ENV_URL: gone}, "omp dotenv")
    check(not got, f"an anchor miss must not masquerade as a rename, got {got}")

    # The masking regression: the CODE occurrence goes, the comments keep it.
    # `.env` survives in BOTH the leading JSDoc and the trailing `//` prose; the
    # locator/parser names survive only in the trailing prose, which is the half
    # a `"`-only comment stripper leaks (the detector line sits above it).
    masked = live.replace('getConfigRootDir(), ".env"', 'getConfigRootDir(), ".renamed"')
    masked = masked.replace('getAgentDir(), ".env"', 'getAgentDir(), ".renamed"')
    check('".env"' in masked, "the JSDoc example must survive, or this proves nothing")
    got = _drive_omp({d.OMP_ENV_URL: masked}, "omp dotenv literal")
    check(
        len(got) == 1 and "`.env`" in got[0],
        f"a literal surviving only in JSDoc must still fire, got {got}",
    )

    for fn, kind in (("getAgentDir", "omp dotenv locator"), ("parseEnvLine", "omp dotenv parser")):
        # Rename every CODE occurrence; the trailing prose keeps the name.
        masked = "".join(
            ln if ln.lstrip().startswith(("//", "*", "/*")) else ln.replace(fn, "renamed")
            for ln in live.splitlines(keepends=True)
        )
        check(fn in masked, f"the prose mention of {fn} must survive, or this proves nothing")
        got = _drive_omp({d.OMP_ENV_URL: masked}, kind)
        check(
            len(got) == 1 and f"`{fn}`" in got[0],
            f"{fn} surviving only in a comment BELOW the quote detector must "
            f"still fire, got {got}",
        )

    # The stays-silent half: the same names reached through a reshaped import
    # and an aliased call are the SAME code, and a gate that reds on a reformat
    # sends the maintainer to repin for nothing.
    reshaped = live.replace(
        'import { getAgentDir, getConfigRootDir, refreshDirsFromEnv } from "./dirs";',
        'import {\n\tgetAgentDir,\n\tgetConfigRootDir,\n\trefreshDirsFromEnv as rebuild,\n} from "./dirs";',
    ).replace("refreshDirsFromEnv();", "rebuild(); // refreshDirsFromEnv")
    for kind, _ in roles:
        got = _drive_omp({d.OMP_ENV_URL: reshaped}, kind)
        check(not got, f"{kind}: a reshaped import is not a rename, got {got}")

    # Every Rust symbol these messages name must be a REAL fn.
    omp_rs = (d.REPO / "crates/pixtuoid-core/src/source/omp.rs").read_text()
    named = set(d.OMP_ENV_LOCATORS.values()) | set(d.OMP_ENV_PARSERS.values())
    for sym in sorted(named | {"parse_omp_env_file"}):
        check(
            f"fn {sym}(" in omp_rs,
            f"drift messages name `{sym}` — it must exist in source/omp.rs",
        )


def test_no_ledger_silently_excuses_a_hook_our_decoder_can_read() -> None:
    """The shape that shipped the PermissionRequest bug, and the one the FIRST
    version of this gate missed: it checked the ledgers against what we REGISTER,
    which is empty by construction, while the incident was a ledgered name our
    DECODER had an arm for and we never received."""
    # EVERY source file, not just the shared arms, and a name shape that admits
    # snake_case: `[A-Za-z]+` over `decoder.rs` alone saw only the 11 CamelCase
    # CC-shaped arms, so this gate was structurally blind to the six sources
    # whose events are snake_case — `pre_approval_request` in a ledger passed it.
    arms: set[str] = set()
    src_dir = REPO_ROOT / "crates/pixtuoid-core/src/source"
    for f in sorted(src_dir.rglob("*.rs")):
        arms |= set(
            re.findall(r'^\s*"([A-Za-z][A-Za-z0-9_.]*)"\s*(?:\||=>)', f.read_text(), re.M)
        )
    check(len(arms) > 30, f"the decoder-arm reader found {len(arms)} arms; it went stale")
    for shape in ("PreToolUse", "pre_tool_call", "session.start"):
        check(shape in arms, f"the arm reader must see the {shape!r} shape; got {len(arms)} arms")

    ledgered = set()
    for name in dir(d):
        if name.endswith("_KNOWN_OMITTED"):
            ledgered |= getattr(d, name)
    live = arms & ledgered
    check(
        live <= d.LEDGERED_BUT_DECODABLE,
        f"{sorted(live - d.LEDGERED_BUT_DECODABLE)} sit in a *_KNOWN_OMITTED ledger "
        f"AND have a decoder arm — the sweep subtracts the ledger, so an event we "
        f"can read but never registered raises nothing. Register it, or add it to "
        f"LEDGERED_BUT_DECODABLE with the reason NOT registering is right.",
    )
    dead = sorted(d.LEDGERED_BUT_DECODABLE - live)
    check(
        not dead,
        f"LEDGERED_BUT_DECODABLE lists {dead}, which no longer sits in a ledger "
        f"with a decoder arm — drop the entry, the list is shrink-only.",
    )


def test_no_ledger_excuses_a_hook_we_actually_register() -> None:
    """A name in both is inert today and fail-open tomorrow: unregister it and
    the sweep stays silent, because the ledger still excuses it."""
    ours = d.read_our_names(d.Report())
    # Enumerated, not hand-listed: the sibling test was made exhaustive and this
    # one was left behind, so five ledgers added later had no teeth here at all.
    # `cc` is the odd field name — every other ledger's prefix IS its field.
    fields = {"CC": "cc"}
    ledgers = sorted(n for n in dir(d) if n.endswith("_KNOWN_OMITTED"))
    check(len(ledgers) >= 10, f"expected every source's ledger, found {ledgers}")
    for ledger in ledgers:
        stem = ledger[: -len("_KNOWN_OMITTED")]
        field = fields.get(stem, stem.lower())
        check(
            hasattr(ours, field),
            f"{ledger} names no OurNames field ({field!r}) — add the mapping, or the "
            f"ledger is silently unchecked",
        )
        if not hasattr(ours, field):
            continue
        registered = getattr(ours, field)
        check(registered is not None, f"the `{field}` reader must still parse")
        both = sorted((registered or set()) & getattr(d, ledger))
        check(
            not both,
            f"{ledger} lists {both}, which our own `{field}` install target "
            f"also REGISTERS. "
            f"Drop it from the ledger (and its rationale) — leaving it there "
            f"means dropping the registration later raises nothing.",
        )


def main() -> int:
    for t in (
        test_try_fetch_classifies_permanent_vs_transient,
        test_source_parsers_find_nonempty_well_shaped_sets,
        test_upstream_parsers_extract_from_a_snippet,
        test_cc_doc_marker_detection_fires_both_directions,
        test_const_array_parser_ignores_words_quoted_inside_comments,
        test_parser_never_drops_a_real_event,
        test_block_scrape_is_bounded_to_the_decoder,
        test_every_const_array_reader_uses_the_shared_parser,
        test_anchor_gate_fires_in_both_directions,
        test_report_is_the_only_way_to_file_a_finding,
        test_one_stale_reader_does_not_blind_the_sources_after_it,
        test_every_reader_row_matches_a_field_on_our_names,
        test_a_stale_flood_guard_skips_its_check_instead_of_flooding,
        test_no_reader_is_called_outside_the_readers_table,
        test_793_stale_pin_reads_as_probe_health_not_three_renames,
        test_every_swept_url_declares_an_anchor,
        test_report_separates_verified_change_from_probe_health,
        test_enum_body_survives_struct_variants_and_indentation,
        test_report_h1_is_the_issue_title_and_carries_the_disposition,
        test_ts_comment_strip_survives_a_quote_detector_line,
        test_rust_comment_strip_is_not_confused_by_lifetimes,
        test_omp_env_sweeps_fire_and_stay_silent,
        test_the_hermes_approval_gate_stays_observer_only,
        test_no_ledger_excuses_a_hook_we_actually_register,
        test_no_ledger_silently_excuses_a_hook_our_decoder_can_read,
    ):
        t()
    if FAILS:
        print("DRIFT SELFTEST FAILED:")
        for f in FAILS:
            print(f"  - {f}")
        return 1
    print("drift selftest: all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
