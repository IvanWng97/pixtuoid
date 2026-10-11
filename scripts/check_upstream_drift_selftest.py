#!/usr/bin/env python3
"""Self-test for check_upstream_drift.py — a regex-parser regression is a SILENT
monitor death: the weekly job either alarms on junk or watches nothing.

Run: `python3 scripts/check_upstream_drift_selftest.py` (exit 0 = pass). No pytest
dependency on purpose — the repo has no Python test harness."""

from __future__ import annotations

import ast
import contextlib
import io
import json
import pathlib
import re
import sys
import traceback
import urllib.error

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
# A `__pycache__` hit made this gate test a DIFFERENT file than the one on disk —
# a same-size restore inside one mtime tick left the .pyc valid. Not a gate, then.
sys.dont_write_bytecode = True
import check_upstream_drift as d  # noqa: E402

FAILS: list[str] = []


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


def test_plugin_doc_markers_fire_on_vanish() -> None:
    for url, entry in d.PLUGIN_DOC_MARKERS.items():
        full = "\n".join(entry.markers)
        check(d.plugin_marker_findings(url, full) == [], f"{url}: quiet doc -> none")
        first = sorted(entry.markers)[0]
        got = d.plugin_marker_findings(url, "\n".join(m for m in entry.markers if m != first))
        installer = d.PLUGIN_HOST_INSTALLERS[entry.cli]
        check(
            len(got) == 1 and first in got[0] and installer in got[0],
            f"{url}: vanish fires: {got!r}",
        )


def test_plugin_contract_findings_reach_review_only_for_registered_hosts() -> None:
    """A host's plugin documents are fetched only while we register its events, and
    a vanished marker files a review naming that host's installer."""
    real = d.fetch
    try:
        url = d.CODEX_PLUGIN_CMD_URL
        entry = d.PLUGIN_DOC_MARKERS[url]
        first = sorted(entry.markers)[0]
        maimed = ANCHOR_SAMPLES[url] + "\n".join(m for m in entry.markers if m != first)
        served: list[str] = []

        def stub(u: str) -> str:
            served.append(u)
            if u == url:
                return maimed
            raise urllib.error.URLError("offline: not this case's document")

        d.fetch = stub
        rep = d.Report()
        d.run_checks(d.OurNames(codex={"SessionStart"}), report=rep)
        check(
            any(first in r and "install/codex.rs" in r for r in rep.review),
            f"a vanished Codex plugin marker files a review: {rep.review!r}",
        )

        served.clear()
        d.run_checks(d.OurNames(cc={"SessionStart"}), report=d.Report())
        codex_docs = {u for u, e in d.PLUGIN_DOC_MARKERS.items() if e.cli == "Codex"}
        check(
            not codex_docs & set(served),
            f"an unregistered host's plugin documents are not fetched: {sorted(served)}",
        )
    finally:
        d.fetch = real


# One snippet per anchored document; a new ANCHORS entry with no sample here
# fails the gate test below.
ANCHOR_SAMPLES: dict[str, str] = {
    d.CC_PLUGIN_COMPONENTS_URL: "# Add components to a plugin\n",
    d.CC_PLUGIN_INSTALL_URL: "# Install and manage plugins\n",
    d.CC_PLUGIN_LOADING_URL: "# Plugin loading reference\n",
    d.CODEX_PLUGIN_CMD_URL: "pub enum PluginSubcommand {\n    Add(AddPluginArgs),\n}\n",
    d.CODEX_MARKETPLACE_CMD_URL: "enum MarketplaceSubcommand {\n    Add(AddMarketplaceArgs),\n}\n",
    d.CODEX_MARKETPLACE_URL: "const MARKETPLACE_MANIFEST_RELATIVE_PATHS: &[&str] = &[];\n",
    d.CODEX_PLUGIN_EDIT_URL: "fn set_plugin_enabled(doc: &mut DocumentMut) -> bool {\n}\n",
    d.CODEX_MARKETPLACE_RECORD_URL: "pub(super) fn record_added_marketplace_entry(\n) {\n}\n",
    d.CODEX_MARKETPLACE_SOURCE_URL: "fn resolve_local_source_path(source: &str) {\n}\n",
    d.REASONIX_PLUGIN_DOC_URL: "\n## CLI Mode\n",
    d.CURSOR_HOOKS_URL: "\n### Hook events\n\n#### preToolUse\n",
    d.KIMI_HOOKS_URL: '"hook_event_name": "PreToolUse"',
    d.CC_HOOKS_URL: "\n# Hooks reference\n",
    d.REASONIX_HOOK_URL: 'const (\n    SessionStart Event = "SessionStart"\n)\n',
    d.CODEWHALE_HOOK_URL: "pub enum HookEvent {\n    SessionStart,\n}\n",
    d.CODEX_PROTOCOL_URL: "pub enum HookEventName {\n    SessionStart,\n}\n",
    d.HERMES_PLUGINS_URL: 'VALID_HOOKS: set[str] = {\n    "on_session_start",\n}\n',
    d.HERMES_SHELL_HOOK_URL: '_BLOCKING_EVENTS = frozenset({"pre_tool_call"})\n',
    d.OMP_EXT_SHARED_EVENTS_URL:
        'export interface SessionShutdownEvent { type: "session_shutdown"; }\n',
    d.OMP_EXT_TYPES_URL:
        'export interface ToolApprovalRequestedEvent { type: "tool_approval_requested"; }\n',
    d.OMP_SESSION_MANAGER_URL:
        'export class SessionManager {\n\tgetSessionFile(): string | undefined {}\n}\n',
    d.DSH_RUNTIME_TYPES_URL:
        "declare module '@deepseek-ai/cordis' {\n  interface Events {\n"
        "    'agent/pre-step'(payload: {}): void\n  }\n}\n",
    d.DSH_SESSION_TYPES_URL: "export interface SessionEventMap {\n  'assistant/message': { text: string }\n}\n",
    d.DSH_APPROVAL_TYPES_URL:
        "declare module '@deepseek-ai/dsh-session/types' {\n  interface SessionEventMap {\n"
        "    'approval/asked': { id: ApprovalRequestId }\n  }\n}\n",
    # The `also` shape: upstream puts `interface Context` between the module
    # header and the `Events` the keys live in, so the sample must too.
    d.DSH_SESSION_INDEX_URL:
        "declare module '@deepseek-ai/cordis' {\n  interface Context {\n    session: Session\n  }\n"
        "  interface Events {\n    'session/created'(session: Session): void\n  }\n}\n",
    d.OMP_EXT_DISCOVERY_URL:
        'export async function discoverExtensionModulePaths() {}\n',
    d.OMP_DIRS_URL:
        'export function getAgentDir(): string {\n\treturn dirs.agentDir;\n}\n',
    d.GROK_HOOK_URL: "pub enum HookEventName {\n    SessionStart,\n}\n",
    d.OPENCLAW_HOOK_TYPES_URL: 'export type PluginHookName =\n  | "agent_end"\n',
    d.OPENCLAW_PATHS_URL: "export const DEFAULT_GATEWAY_PORT = 18789;\n",
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
        check(len(d.ANCHORS) >= 4, f"ANCHORS covers every swept document, got {len(d.ANCHORS)}")
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

            # The sample above satisfies both halves, so only deleting one
            # proves the second is load-bearing rather than decorative.
            if anchor.also is None:
                continue
            for drop, half in (("pattern", anchor.pattern), ("also", anchor.also)):
                maimed = re.sub(half, "", sample, count=1)
                check(
                    maimed != sample,
                    f"{anchor.owns}: the {drop} half must occur in its own sample",
                )
                d.fetch = lambda _u, _s=maimed: _s
                r = d.Report()
                check(
                    d.fetch_anchored(url, "T", r) is None and len(r.blind) == 1,
                    f"{anchor.owns}: {drop} missing -> blind, got blind={r.blind}",
                )
    finally:
        d.fetch = real


def test_one_document_is_fetched_once_per_run() -> None:
    """A document is upstream's answer for the whole run, and several checks read
    the same one — three read codex's `protocol.rs`."""
    real = d.fetch
    try:
        calls: list[str] = []

        def dead(u: str) -> str:
            calls.append(u)
            raise urllib.error.HTTPError(u, 404, "gone", {}, None)  # type: ignore[arg-type]

        d.fetch = dead
        rep = d.Report()
        d.run_checks(d.read_our_names(rep), report=rep)
        # A floor: a run that swept nothing would satisfy every check below.
        check(len(calls) > 20, f"the run actually swept, got {len(calls)} fetches")
        check(
            not (dupes := sorted({u for u in calls if calls.count(u) > 1})),
            f"each document is fetched once per run; repeated: {dupes}",
        )
        check(
            not (twice := sorted({b for b in rep.blind if rep.blind.count(b) > 1})),
            f"a dead pin files ONE blind line, not one per reader; repeated: {twice}",
        )

        # A SUCCESSFUL document is reused as well, not only a failed one.
        served: list[str] = []

        def live(u: str, _s: str = ANCHOR_SAMPLES[d.CODEX_PROTOCOL_URL]) -> str:
            served.append(u)
            if u == d.CODEX_PROTOCOL_URL:
                return _s
            raise urllib.error.URLError("offline: not this case's document")

        d.fetch = live
        rep = d.Report()
        d.run_checks(d.read_our_names(rep), report=rep)
        check(
            served.count(d.CODEX_PROTOCOL_URL) == 1,
            f"a document that ANSWERED is cached too, got "
            f"{served.count(d.CODEX_PROTOCOL_URL)} fetches of protocol.rs",
        )
    finally:
        d.fetch = real


def test_the_omp_extension_router_flags_a_name_it_cannot_place() -> None:
    """The router's documented promise, as a gate: a name it cannot place goes
    blind, and an unverifiable carrier suppresses its fields rather than renaming
    every one of them."""
    shared = "export interface SessionSwitchEvent {\n\tpreviousSessionFile: string;\n}\n"
    ext = (
        "export interface ToolApprovalRequestedEvent {\n\ttoolCallId: string;\n}\n"
        "export interface ToolApprovalResolvedEvent {\n\tapproved: boolean;\n}\n"
        "export interface ExtensionContext {\n\tcwd: string;\n}\n"
    )
    sm = "export class SessionManager {\n\tgetSessionFile(): string {}\n}\n"

    rep = d.Report()
    d.check_omp_extension_reads(shared, ext, sm, {"cwd", "toolCallId"}, rep)
    check(
        not rep.blind and not rep.breaking,
        f"names the router can place stay silent; blind={rep.blind} breaking={rep.breaking}",
    )

    rep = d.Report()
    d.check_omp_extension_reads(shared, ext, sm, {"pxdNewlyRead"}, rep)
    check(
        any("pxdNewlyRead" in b for b in rep.blind),
        f"a read with no declaration site must go BLIND, not green; blind={rep.blind}",
    )
    check(
        not rep.breaking,
        f"an unplaceable name is probe health, never a rename; got {rep.breaking}",
    )

    # A carrier that will not parse must suppress its fields, not rename them —
    # `approved` included, whose OPTIONALITY arm reads the same value and would
    # raise on None, killing every check after this one.
    for fields in ({"toolCallId"}, {"approved", "toolCallId"}):
        rep = d.Report()
        d.check_omp_extension_reads(shared, "", sm, fields, rep)
        check(
            any("carrier" in b for b in rep.blind) and not rep.breaking,
            f"an unreadable carrier is blind and suppresses {sorted(fields)}; "
            f"blind={rep.blind} breaking={rep.breaking}",
        )


def test_a_redirected_omp_agent_dir_is_breaking() -> None:
    """`omp_agent_dir()`'s no-flatten rule rests on this returning the resolver's
    dir verbatim, and the check's own message says no local check can see the
    failure it guards."""
    real = d.fetch
    try:

        def drive(body: str) -> d.Report:
            def stub(u: str, _b: str = body) -> str:
                if u == d.OMP_DIRS_URL:
                    return _b
                raise urllib.error.URLError("offline: not this case's document")

            d.fetch = stub
            rep = d.Report()
            d.run_checks(d.read_our_names(rep), report=rep)
            return rep

        verbatim = drive("export function getAgentDir(): string {\n\treturn dirs.agentDir;\n}\n")
        check(
            not any("getAgentDir" in x for x in verbatim.breaking),
            f"the verbatim resolver is silent; got {verbatim.breaking}",
        )

        redirected = drive(
            "export function getAgentDir(): string {\n"
            "\treturn process.env.XDG_DATA_HOME ?? dirs.agentDir;\n}\n"
        )
        check(
            any("getAgentDir" in x for x in redirected.breaking),
            f"a redirect must be BREAKING, not silence; breaking={redirected.breaking} "
            f"blind={redirected.blind}",
        )
    finally:
        d.fetch = real


def test_two_declarations_are_probe_health_not_a_first_match() -> None:
    """A first-match read picks the decoy the day upstream grows a `#[cfg(test)]`
    twin above the real declaration, and then files breaking drift against a
    healthy upstream."""
    check(d.sole_match(r"P\s*=\s*(\d+)", "P = 7777\n") is not None, "one match -> the match")
    check(d.sole_match(r"P\s*=\s*(\d+)", "nothing here\n") is None, "no match -> None")
    check(d.sole_match(r"P\s*=\s*(\d+)", "P = 1\nP = 7777\n") is None,
          "two matches -> None, never the first")

    real = d.fetch
    try:
        rep0 = d.Report()
        ports = sorted(d.read_our_names(rep0).openclaw_gateway_port or ())
        check(bool(ports), "the fragment supplies the gateway port")

        def drive(doc: str) -> d.Report:
            def stub(u: str, _d: str = doc) -> str:
                if u == d.OPENCLAW_PATHS_URL:
                    return _d
                raise urllib.error.URLError("offline: not this case's document")

            d.fetch = stub
            rep = d.Report()
            d.run_checks(d.read_our_names(rep), report=rep)
            return rep

        sole = drive(f"export const DEFAULT_GATEWAY_PORT = {ports[0]};\n")
        check(not sole.breaking, f"one declaration matching ours is silent; got {sole.breaking}")

        twin = drive(
            "const DEFAULT_GATEWAY_PORT = 1234; // a fixture twin, ABOVE the real one\n"
            f"export const DEFAULT_GATEWAY_PORT = {ports[0]};\n"
        )
        check(
            not twin.breaking and any("DEFAULT_GATEWAY_PORT" in b for b in twin.blind),
            f"ambiguity must abstain, never report the decoy's port as drift; "
            f"breaking={twin.breaking} blind={twin.blind}",
        )
    finally:
        d.fetch = real


def test_the_openclaw_degraded_watch_tells_a_vanished_field_from_a_missing_type() -> None:
    """The hook-name sweep stays green while a field or literal the plugin stamps
    `errored` from vanishes, and Degraded silently becomes unreachable."""
    table = d.OPENCLAW_DEGRADED_FIELDS
    check(bool(table), "the watch pins at least one type")
    plugin = (d.REPO / "crates/pixtuoid/src/install/openclaw_plugin.js").read_text()
    for fields in table.values():
        for f, literal in fields:
            check(re.search(rf"\.{f}\b", plugin) is not None,
                  f"the watch pins `{f}`, which openclaw_plugin.js no longer reads")
            if literal is not None:
                check(re.search(rf"\.{f}\s*[!=]==\s*{re.escape(literal)}", plugin) is not None,
                      f"the watch pins `{f}` {literal}, which openclaw_plugin.js no longer compares")

    real = d.fetch
    try:
        rep0 = d.Report()
        hooks = sorted(d.read_our_names(rep0).openclaw or ())
        check(bool(hooks), "the fragment supplies the openclaw hook set")
        # The anchor and every registered hook, so only the event types vary.
        names = "export type PluginHookName =\n" + "".join(f'  | "{h}"\n' for h in hooks)

        def member(f: str, literal: str | None, drop_literal: bool) -> str:
            """A literal's union wraps one member per line, as upstream's formatter
            does once the line passes its width."""
            if literal is None:
                return f"  {f}?: unknown;\n"
            union = ['"other"'] if drop_literal else [literal, '"other"']
            return f"  {f}?:\n" + "".join(f"    | {u}\n" for u in union[:-1]) + f"    | {union[-1]};\n"

        def document(omit: tuple[str, str] | None = None,
                     drop_literal: tuple[str, str] | None = None,
                     rename: str | None = None) -> str:
            """Every pinned type, each extending a base as upstream's model-call
            events do. An omitted field also sits in a comment, a nested object and
            the next type: the three spellings a whole-document match takes for a
            declaration."""
            out = names
            for ty, fields in table.items():
                decl = f"{ty}V2" if ty == rename else ty
                out += f"export type {decl} = PluginHookContextWindow & {{\n  callId: string;\n"
                for f, literal in fields:
                    if (ty, f) == omit:
                        out += f"  /*\n  {f}?: string;\n  */\n  detail?: {{\n    {f}?: string;\n  }};\n"
                        continue
                    out += member(f, literal, (ty, f) == drop_literal)
                out += "};\n"
                if omit is not None and ty == omit[0]:
                    out += f"export type PluginHookNextEvent = {{\n  {omit[1]}?: string;\n}};\n"
            return out

        def drive(doc: str) -> d.Report:
            def stub(u: str, _d: str = doc) -> str:
                if u == d.OPENCLAW_HOOK_TYPES_URL:
                    return _d
                raise urllib.error.URLError("offline: not this case's document")

            d.fetch = stub
            rep = d.Report()
            d.run_checks(d.read_our_names(rep), report=rep)
            return rep

        def on_table(lines: list[str]) -> list[str]:
            return [x for x in lines if any(ty in x for ty in table)]

        intact = drive(document())
        check(not intact.breaking and not on_table(intact.blind),
              f"every field declared -> silent; breaking={intact.breaking} "
              f"blind={on_table(intact.blind)}")
        for ty, fields in table.items():
            for f, literal in fields:
                gone = drive(document(omit=(ty, f)))
                check(len(gone.breaking) == 1 and f"`{ty}`" in gone.breaking[0]
                      and f"`{f}`" in gone.breaking[0] and not on_table(gone.blind),
                      f"`{ty}.{f}` gone -> one breaking line naming it; "
                      f"breaking={gone.breaking} blind={on_table(gone.blind)}")
                if literal is not None:
                    narrowed = drive(document(drop_literal=(ty, f)))
                    check(len(narrowed.breaking) == 1 and literal in narrowed.breaking[0],
                          f"`{ty}.{f}` without {literal} -> one breaking line naming it; "
                          f"breaking={narrowed.breaking}")
            renamed = drive(document(rename=ty))
            check(not renamed.breaking and len(on_table(renamed.blind)) == 1
                  and f"`{ty}`" in on_table(renamed.blind)[0],
                  f"a missing `{ty}` is probe health, never drift; "
                  f"breaking={renamed.breaking} blind={renamed.blind}")

        one_line = drive(names + "".join(
            f"export type {ty} = {{ "
            + "; ".join(f"readonly {f}?: " + (literal or "unknown") for f, literal in fields)
            + " };\n"
            for ty, fields in table.items()))
        check(not one_line.breaking and not on_table(one_line.blind),
              f"a one-line restyle is not a vanish; breaking={one_line.breaking} "
              f"blind={on_table(one_line.blind)}")
    finally:
        d.fetch = real


def test_the_anchor_requirement_cannot_be_waived_quietly() -> None:
    """A one-line edit reopens the gate with every other test still green:
    dropping `also=` makes the half-anchor check SKIP rather than fail."""
    # Pinned BY URL: with `also` gone the loop in the anchor test just skips, which
    # is indistinguishable from an anchor that never needed a second half.
    # `\s+`, not `\s*`: indentation is the only nesting signal a both-present
    # anchor has, so a flush-left `interface Events` on ANOTHER module must fail.
    a = d.ANCHORS[d.DSH_SESSION_INDEX_URL]
    for indent, want in ((" ", True), ("\t", True), ("    ", True), ("", False)):
        body = f"declare module '@deepseek-ai/cordis' {{\n{indent}interface Events {{\n}}\n}}\n"
        got = bool(re.search(a.pattern, body)) and bool(re.search(a.also or "", body))
        check(got is want, f"indent {indent!r}: anchored={got}, want {want}")

    for url in (d.DSH_SESSION_INDEX_URL,):
        check(
            d.ANCHORS[url].also is not None,
            f"{url}: `interface Context` sits between this module header and the "
            f"`Events` that owns the keys, so one pattern cannot span it — `also` "
            f"is what makes the anchor whole",
        )


def test_no_anchor_is_a_name_the_watcher_checks() -> None:
    """`Anchor`'s docstring bans this in prose, and prose has no failure mode.

    Such an anchor vanishes WITH the names on the very rename it exists to tell
    apart from a stale pin, and the blind line then argues the maintainer out of
    the fix."""
    rep = d.Report()
    ours = d.read_our_names(rep)
    names = {
        n
        for f in ours.__dataclass_fields__
        if isinstance(v := getattr(ours, f), set)
        for n in v
    }
    # An empty vocabulary would make every `re.search` below vacuous.
    check(bool(names), "the fragments supply the swept vocabulary")
    circular = sorted(
        (a.owns, p, n)
        for a in d.ANCHORS.values()
        for p in (a.pattern, a.also)
        if p is not None
        for n in names
        if re.search(p, n)
    )
    check(not circular, f"an anchor must not match a name it guards: {circular}")


def test_a_multi_document_check_is_all_or_nothing() -> None:
    """Where a name is declared in exactly one half, a partial join reads a
    still-present name as VANISHED and files breaking drift against what was only
    a fetch failure."""
    real = d.fetch
    try:
        urls = [(f"https://x.invalid/{i}", f"doc {i}") for i in range(3)]
        bodies = {u: f"pub enum HookEvent {{ Pxv{i} }}\n" for i, (u, _) in enumerate(urls)}
        anchor = d.Anchor(r"pub enum HookEvent\b", "`HookEvent`")
        saved = {u: d.ANCHORS.get(u) for u, _ in urls}
        try:
            for u, _ in urls:
                d.ANCHORS[u] = anchor

            d.fetch = lambda u, _b=bodies: _b[u]
            r = d.Report()
            joined = d.fetch_all_anchored(urls, r)
            check(joined is not None and all(b.strip() in joined for b in bodies.values()),
                  f"every document present -> all of them joined, got {joined!r}")
            check(not r.blind and not r.errors, f"a clean run files nothing, got {r.blind}")

            # Exactly one short, and the whole check must abstain — never join the rest.
            for drop, _ in urls:
                d.fetch = lambda u, _d=drop, _b=bodies: (
                    _UNANCHORED if u == _d else _b[u]
                )
                r = d.Report()
                check(d.fetch_all_anchored(urls, r) is None,
                      f"{drop} unanchored -> the whole check abstains")
                check(len(r.blind) == 1 and not r.errors,
                      f"{drop} unanchored -> one blind line, got {r.blind}")
        finally:
            for u, prev in saved.items():
                if prev is None:
                    d.ANCHORS.pop(u, None)
                else:
                    d.ANCHORS[u] = prev
    finally:
        d.fetch = real


def test_main_prints_exactly_the_four_buckets() -> None:
    """`drift_watch`'s `Buckets` requires these four keys and refuses any other,
    so a rename here fails the run instead of dropping findings."""
    real = d.fetch
    try:
        d.fetch = lambda _u: (_ for _ in ()).throw(urllib.error.URLError("offline"))
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            d.main()
        got = json.loads(out.getvalue())
        check(
            set(got) == {"breaking", "review", "blind", "errors"},
            f"main() prints the four buckets, got {sorted(got)}",
        )
        check(bool(got["errors"]), f"an offline run files its fetches as errors: {got}")
    finally:
        d.fetch = real


def test_report_is_the_only_way_to_file_a_finding() -> None:
    """`Report` owns the buckets and the blind line's wording."""
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


def test_the_hermes_blocking_watch_fires_on_an_appearance_not_a_vanish() -> None:
    """The one APPEARANCE watch among the name checks, so the vanish census cannot
    cover it: exit 0 on a blocking approval hook ANSWERS the prompt (invariant #5).
    """
    real = d.fetch
    try:
        rep0 = d.Report()
        ours = d.read_our_names(rep0)
        unsafe = sorted(d.HERMES_BLOCKING_UNSAFE & (ours.hermes or set()))
        check(bool(unsafe), f"we register something the watch calls unsafe: {unsafe}")

        def drive(members: list[str]) -> d.Report:
            body = "_BLOCKING_EVENTS = frozenset({%s})\n" % ", ".join(
                f'"{m}"' for m in members
            )

            def stub(u: str, _b: str = body) -> str:
                if u == d.HERMES_SHELL_HOOK_URL:
                    return _b
                raise urllib.error.URLError("offline: not this case's document")

            d.fetch = stub
            rep = d.Report()
            d.run_checks(d.read_our_names(rep), report=rep)
            return rep

        quiet = drive(["pre_tool_call"])
        check(not quiet.breaking, f"an observer-only set stays silent: {quiet.breaking}")
        loud = drive(["pre_tool_call", unsafe[0]])
        check(any(unsafe[0] in x for x in loud.breaking),
              f"`{unsafe[0]}` turning blocking must fire; got {loud.breaking}")

        # The OTHER appearance direction on the plugins document: a hook we
        # register being reclassified as unservable by a shell hook.
        registered = sorted(ours.hermes or ())
        check(bool(registered), "the fragment supplies hermes' registration set")

        def plugins(unservable: list[str]) -> d.Report:
            body = (
                "VALID_HOOKS: set[str] = {\n"
                + "".join(f'    "{h}",\n' for h in registered)
                + "}\n"
                + "SHELL_UNSUPPORTED_HOOKS: set[str] = {\n"
                + "".join(f'    "{h}",\n' for h in unservable)
                + "}\n"
            )

            def stub(u: str, _b: str = body) -> str:
                if u == d.HERMES_PLUGINS_URL:
                    return _b
                raise urllib.error.URLError("offline: not this case's document")

            d.fetch = stub
            rep = d.Report()
            d.run_checks(d.read_our_names(rep), report=rep)
            return rep

        ok = plugins(["transform_api_error_classification"])
        check(not ok.breaking, f"a hook we do not register stays silent: {ok.breaking}")
        bad = plugins([registered[0]])
        check(any(registered[0] in x for x in bad.breaking),
              f"`{registered[0]}` turning unservable must fire; got {bad.breaking}")
        # The anchor is the declaration itself, so a moved set is probe health.
        d.fetch = lambda _u: "BLOCK_EXIT_CODE = 2\n"
        gone = d.Report()
        d.run_checks(d.read_our_names(gone), report=gone)
        check(not gone.breaking, f"a moved declaration is not drift: {gone.breaking}")
    finally:
        d.fetch = real


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

    # The OVER-capture direction: a spill into the following `impl` becomes a
    # phantom variant, worse than a miss — it makes a real rename look present.
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


def test_every_block_reader_strips_comments_before_counting_braces() -> None:
    """The rule two of the three readers state in their own docstrings, and the
    third re-broke: a brace inside a comment moves the bounds. The dangerous
    outcome is not the empty parse (loud) but the TRUNCATED one — the size floor
    still passes and the tail of the set silently leaves the sweep."""
    clean = 'VALID_HOOKS: set[str] = {\n    "a_one",\n    "b_two",\n}\n'
    want = {"a_one", "b_two"}
    check(d.python_set_literal(clean, "VALID_HOOKS: set[str] = {") == want, "clean baseline")
    for label, comment in (
        ("unmatched open", '    # returns {"action": "continue"\n'),
        ("stray close", "    # anything else } lets the turn finish\n"),
        ("both", '    # {"a": 1} and a trailing }\n'),
    ):
        poisoned = 'VALID_HOOKS: set[str] = {\n    "a_one",\n' + comment + '    "b_two",\n}\n'
        got = d.python_set_literal(poisoned, "VALID_HOOKS: set[str] = {")
        check(
            got == want,
            f"a comment with an {label} brace must not move the bounds: got {sorted(got)}",
        )


def test_rust_comment_strip_is_not_confused_by_lifetimes() -> None:
    """Tracking `'` as a string opener swallows the comment AFTER an odd number of
    lifetimes into a fake literal, re-admitting every word inside it. An EVEN count
    cannot show this — the fake span closes before the comment — so a two-lifetime
    fixture passes under both implementations and proves nothing."""
    odd = "fn f<'a>(x: &'a str) -> &'static u8 { /* PxGhost */ }"
    check("PxGhost" not in d.strip_rust_comments(odd),
          f"lifetimes must not open a string: {d.strip_rust_comments(odd)}")

    # `//` inside a string literal: a blind `//[^\n]*` eats the rest of the line.
    in_string = 'const U: &str = "https://x/y"; const K: &str = "PxKeep";'
    check(d.strip_rust_comments(in_string) == in_string,
          f"a `//` inside a string is not a comment: {d.strip_rust_comments(in_string)}")

    # NESTED: `/\\*.*?\\*/` stops at the first `*/` and re-admits `PxGhost`.
    nested = 'A, /* outer /* inner */ PxGhost */ B,'
    stripped = d.strip_rust_comments(nested)
    check("PxGhost" not in stripped, f"a nested comment leaks: {stripped}")
    check("A," in stripped and "B," in stripped, f"code around it survives: {stripped}")


def test_the_floor_is_what_we_already_handle() -> None:
    """A parse returning fewer names than we already decode is broken or degraded,
    and must file probe health rather than five ⛔ against working decoders —
    document. Proven by running the sweep, not by reading the comparison."""
    rep = d.Report()
    ours = d.OurNames(cc={f"ev{i}" for i in range(9)})
    check(
        not d.parse_is_believable("cc", {f"ev{i}" for i in range(7)}, ours, rep),
        "a parse below what we handle must not be believed",
    )
    check(
        any("SKIPPED for cc" in f for f in rep.blind),
        f"and must file probe health naming the source: {rep.blind}",
    )
    rep2 = d.Report()
    check(
        d.parse_is_believable("cc", ours.cc | {"brand_new"}, ours, rep2),
        "at/above the floor must be believed",
    )
    check(not rep2.blind, f"and must file nothing: {rep2.blind}")


def test_every_believability_gate_can_name_the_document_it_doubts() -> None:
    """`parse_is_believable` reads `PARSE_SOURCES[source]` to say WHICH pin to
    re-check, so a caller missing from that table raises KeyError — and only on
    the day a parse actually degrades, which is the day the report matters."""
    src = (pathlib.Path(__file__).resolve().parent / "check_upstream_drift.py").read_text()
    callers = set(re.findall(r'parse_is_believable\(\s*"(\w+)"', src))
    check(bool(callers), "the gate still has callers")
    missing = sorted(callers - set(d.PARSE_SOURCES))
    check(
        not missing,
        f"{missing} call parse_is_believable but declare no PARSE_SOURCES row, so a "
        f"degraded parse would raise KeyError instead of filing probe health.",
    )
    stale = sorted(set(d.PARSE_SOURCES) - callers)
    check(not stale, f"PARSE_SOURCES lists {stale}, which no longer gates anything.")
    # The gate must still REFUSE: a floor nothing can fail is not a floor.
    rep = d.Report()
    caller = sorted(callers)[0]
    ours = d.OurNames(**{caller: {f"h{i}" for i in range(4)}})
    check(
        not d.parse_is_believable(caller, {"only_one"}, ours, rep),
        f"a one-name parse must fail {caller}'s floor",
    )
    check(
        any(f"SKIPPED for {caller}" in f for f in rep.blind),
        f"and must name {caller} in the probe-health line: {rep.blind}",
    )


def test_every_swept_url_declares_an_anchor() -> None:
    """Every swept document declares an anchor. The anchor gate is #793's fix: a
    pin that 200s as a facade reads as mass drift without it. A new sweep declaring
    none would skip that gate silently — hence a census over every `*_URL`, not a
    floor on ANCHORS."""
    import re as _re

    src = (pathlib.Path(__file__).parent / "check_upstream_drift.py").read_text()
    urls = _re.findall(r"^([A-Z][A-Z0-9_]*_URLS?)\s*=", src, _re.M)
    check(bool(urls), "the *_URL declaration shape moved — this census reads nothing")
    unclassified = []
    for name in urls:
        value = getattr(d, name)
        for one in value if isinstance(value, tuple) else [value]:
            if one not in d.ANCHORS:
                unclassified.append(name)
    check(
        not unclassified,
        f"{unclassified} declare no anchor, so the #793 stale-pin gate does not "
        f"cover them.",
    )


def test_every_surface_row_names_a_key_the_emitter_actually_writes() -> None:
    """Nothing in either language binds `SURFACE_ROWS` to the keys the RUST side
    writes, so a key renamed in `src/drift_surface.rs` would surface only as a
    probe-health line in the weekly run — late enough to have already skipped a
    sweep. This is that binding, and it runs in `just lint`. The reverse too: a key
    nothing consumes is the "maintaining what we don't use" this file deletes."""
    consumed = set()
    for field, rel, group, key in d.SURFACE_ROWS:
        frag = d.load_fragment(rel)
        check(group in frag, f"{rel} has no `{group}` group for {field}")
        check(key in frag[group], f"{rel} {group} has no `{key}` for {field}")
        check(frag[group].get(key), f"{rel} {group}.{key} is empty")
        consumed.add((rel, group, key))

    emitted = {
        (rel, group, key)
        for rel in (d.CORE_LIB_FRAGMENT, d.CORE_BIN_FRAGMENT)
        for group, rows in d.load_fragment(rel).items()
        for key in rows
    }
    unconsumed = sorted(f"{rel.split('/')[1]} {g}.{k}" for rel, g, k in emitted - consumed)
    check(not unconsumed, f"emitted but read by nobody: {unconsumed}")


def test_one_absent_surface_row_does_not_blind_the_sources_beside_it() -> None:
    """A gap in the emitted surface costs exactly what it feeds. The 16 hand-written
    readers this replaced had per-reader `try` for it; the fragments give per-KEY
    isolation instead, and it must be the same promise or one missing row silently
    darkens the whole watch."""
    real = d.load_fragment
    try:
        full = {rel: real(rel) for rel in (d.CORE_LIB_FRAGMENT, d.CORE_BIN_FRAGMENT)}

        holed = {rel: json.loads(json.dumps(f)) for rel, f in full.items()}
        del holed[d.CORE_BIN_FRAGMENT]["registered"]["claude-code"]
        d.load_fragment = lambda rel: holed[rel]
        rep = d.Report()
        ours = d.read_our_names(rep)
        check(ours.cc is None, f"the holed field is dark, got {ours.cc!r}")
        for other in ("cursor", "kimi", "codex", "omp_extension_subdir"):
            check(getattr(ours, other), f"{other} must survive a sibling's gap")
        named = [b for b in rep.blind if "`cc`" in b]
        check(len(named) == 1, f"exactly one probe-health line names cc: {rep.blind}")

        # A fragment that does not load at all darkens only what IT feeds.
        def half(rel):
            if rel == d.CORE_BIN_FRAGMENT:
                raise OSError("gone")
            return full[rel]

        d.load_fragment = half
        rep2 = d.Report()
        ours2 = d.read_our_names(rep2)
        check(ours2.cc is None, "a registration field is dark when its fragment is gone")
        check(
            bool(ours2.omp_extension_subdir),
            "a field of the other fragment survives it going missing",
        )
    finally:
        d.load_fragment = real


def test_the_cc_hook_table_parser_extracts_from_a_snippet() -> None:
    """The parser, against a hand-made document of the shape it reads. It fails
    SAFE (an empty parse becomes probe health, not a ⛔), but a regex-parser
    regression is a silent monitor death."""
    cc = d.upstream_cc_hook_events(
        "# Hooks reference\n\n"
        "| Event | When it fires |\n|---|---|\n"
        "| `PreToolUse` | before a tool |\n"
        "| `PermissionRequest` | on a gate |\n"
    )
    check(cc is not None and {"PreToolUse", "PermissionRequest"} <= cc, f"cc table: {cc}")
    check(
        d.upstream_cc_hook_events("# Hooks reference\n\nprose with no table\n") in (None, set()),
        "a table-less page must not read as a full parse",
    )


def test_omp_approved_optionality_is_watched() -> None:
    """required->optional on `approved` must fire — the presence matcher alone
    accepts `approved?:` and the decoder reads absent-as-denied."""
    shared = (
        'export interface SessionSwitchEvent {\n'
        '\tpreviousSessionFile: string | undefined;\n}\n'
    )
    sm = "getSessionFile(): x {}\ngetSessionId(): x {}\n"

    def types(approved: str) -> str:
        return (
            "export interface ToolApprovalRequestedEvent {\n"
            "\ttoolCallId: string;\n\ttoolName: string;\n\treason: string;\n}\n"
            "export interface ToolApprovalResolvedEvent {\n"
            f"\ttoolCallId: string;\n\ttoolName: string;\n\treason: string;\n\t{approved}\n"
            "}\n"
            "export interface ExtensionContext {\n\tcwd: string;\n}\n"
        )

    fields = {
        "approved", "cwd", "getSessionFile", "getSessionId",
        "previousSessionFile", "reason", "toolCallId", "toolName",
    }
    r = d.Report()
    d.check_omp_extension_reads(shared, types("approved: boolean;"), sm, fields, r)
    check(not r.breaking, f"required `approved` is clean, got {r.breaking}")
    r = d.Report()
    d.check_omp_extension_reads(shared, types("approved?: boolean;"), sm, fields, r)
    check(
        any("OPTIONAL" in x for x in r.breaking),
        f"`approved?:` must fire the optionality arm, got {r.breaking}",
    )


def test_every_source_check_fires_on_a_vanish_and_stays_silent_otherwise() -> None:
    """The gate #941 asks for: EVERY surface row's check, both directions, offline.

    Each document is BUILT from the set we depend on, so an added event cannot make
    this stale, and in the UPSTREAM spelling — the trap that hid codewhale's arm in
    review (upstream declares `MessageSubmit`, we register `message_submit`)."""
    real = d.fetch
    try:
        rep0 = d.Report()
        ours = d.read_our_names(rep0)
        check(not rep0.blind, f"the fragments load: {rep0.blind}")
        full: dict[str, list[str]] = {
            f: sorted(getattr(ours, f) or ()) for f, *_ in d.SURFACE_ROWS
        }

        def pascal(name: str) -> str:
            return "".join(p.title() for p in name.split("_"))

        def bare_enum(enum: str, names: list[str]) -> str:
            return f"pub enum {enum} {{\n" + "".join(f"    {n},\n" for n in names) + "}\n"

        def omp_extension_docs(reads: list[str]) -> dict[str, str]:
            """The three extension-API docs, shaped like upstream: the anchors'
            interfaces, whose field lines exist only for the names still in the
            mutated set — exactly an upstream rename's silhouette."""
            prev = (
                "\tpreviousSessionFile: string | undefined;\n"
                if "previousSessionFile" in reads
                else ""
            )
            shared = (
                "export interface SessionShutdownEvent {\n}\n"
                f"export interface SessionSwitchEvent {{\n{prev}}}\n"
            )
            approval_fields = {
                "toolCallId": "string",
                "toolName": "string",
                "reason": "string",
                "approved": "boolean",
            }
            ext = ""
            for iface in ("ToolApprovalRequestedEvent", "ToolApprovalResolvedEvent"):
                body = "".join(
                    f"\t{f}: {t};\n" for f, t in approval_fields.items() if f in reads
                )
                ext += f"export interface {iface} {{\n{body}}}\n"
            cwd = '\tcwd: string;\n' if "cwd" in reads else ""
            ext += (
                "export interface ExtensionContext {\n"
                + cwd
                + "\tsessionManager: ReadonlySessionManager;\n}\n"
            )
            sm = "export class SessionManager {\n" + "".join(
                f"\t{g}(): string {{\n\t}}\n"
                for g in ("getSessionFile", "getSessionId")
                if g in reads
            ) + "}\n"
            return {
                d.OMP_EXT_SHARED_EVENTS_URL: shared,
                d.OMP_EXT_TYPES_URL: ext,
                d.OMP_SESSION_MANAGER_URL: sm,
            }

        cases = [
            ("reasonix", str, lambda ns: {
                d.REASONIX_HOOK_URL:
                    "const (\n" + "".join(f'    {n} Event = "{n}"\n' for n in ns) + ")\n"}),
            ("codewhale", pascal, lambda ns: {
                d.CODEWHALE_HOOK_URL: bare_enum("HookEvent", ns)}),
            ("codex", str, lambda ns: {
                d.CODEX_PROTOCOL_URL: bare_enum("HookEventName", ns)}),
            ("hermes", str, lambda ns: {
                d.HERMES_PLUGINS_URL:
                    "VALID_HOOKS: set[str] = {\n" + "".join(f'    "{n}",\n' for n in ns) + "}\n"}),
            ("grok", str, lambda ns: {
                d.GROK_HOOK_URL:
                    "macro_rules! hook_events {\n"
                    "    ($($variant:ident { $($rest:tt)* }),* $(,)?) => {\n"
                    "        pub enum HookEventName {\n"
                    "            $($variant,)*\n"
                    "        }\n"
                    "    };\n"
                    "}\n\n"
                    "hook_events! {\n"
                    + "".join(f'    {n} {{ alias: "x" }},\n' for n in ns) + "}\n"}),
            ("openclaw_gateway_port", str, lambda ns: {
                d.OPENCLAW_PATHS_URL: f"export const DEFAULT_GATEWAY_PORT = {ns[0]};\n"}),
            ("openclaw", str, lambda ns: {
                d.OPENCLAW_HOOK_TYPES_URL:
                    "export type PluginHookName =\n" + "".join(f'  | "{n}"\n' for n in ns)}),
            ("omp_extension_reads", str, omp_extension_docs),
            ("omp_extension_subdir", str, lambda ns: {
                d.OMP_EXT_DISCOVERY_URL:
                    "export async function discoverExtensionModulePaths() {}\n"
                    + "".join(
                        f'discoverExtensionModulePaths(ctx, path.join(dir, "{n}"))\n'
                        for n in ns
                    ),
                d.OMP_DIRS_URL:
                    "export function getAgentDir(): string {\n"
                    "\treturn dirs.agentDir;\n}\n"}),
            ("cursor", str, lambda ns: {
                d.CURSOR_HOOKS_URL: "### Hook events\n\n" + "".join(f"#### {n}\n\n" for n in ns)}),
            ("kimi", str, lambda ns: {
                d.KIMI_HOOKS_URL: "hook_event_name\n\n" + "".join(f"| `{n}` | x |\n" for n in ns)}),
            ("cc", str, lambda ns: {
                d.CC_HOOKS_URL: "# Hooks reference\n\n| Event | When it fires |\n|---|---|\n"
                + "".join(f"| `{n}` | x |\n" for n in ns)}),
            ("dsh_plugin_events", str, lambda ns: {
                d.DSH_RUNTIME_TYPES_URL:
                    "declare module '@deepseek-ai/cordis' {\n  interface Events {\n"
                    + "\n".join(f"'{n}'" for n in ns),
                d.DSH_SESSION_TYPES_URL: "export interface SessionEventMap {\n",
                d.DSH_APPROVAL_TYPES_URL:
                    "declare module '@deepseek-ai/dsh-session/types' {\n"
                    "  interface SessionEventMap {\n",
                d.DSH_SESSION_INDEX_URL:
                    "declare module '@deepseek-ai/cordis' {\n  interface Context {\n  }\n"
                    "  interface Events {\n"}),
        ]
        covered = {c[0] for c in cases}
        rows = {f for f, *_ in d.SURFACE_ROWS}
        check(covered == rows, f"every surface row needs a case; differ: {covered ^ rows}")

        def drive(docs: dict[str, str]) -> d.Report:
            """OFFLINE: every URL this case does not serve raises rather than going
            to the network. `else real(u)` made `just lint` issue 208 live requests
            per run, and `lint` joins its jobs with `wait`, so a blackholing network
            would hang preflight and pre-push at 30s/request."""
            def stub(u: str, _d: dict[str, str] = docs) -> str:
                if u in _d:
                    return _d[u]
                raise urllib.error.URLError("offline: not this case's document")

            d.fetch = stub
            rep = d.Report()
            d.run_checks(d.read_our_names(rep), report=rep)
            return rep

        for field, spell, build in cases:
            names = full[field]
            check(bool(names), f"{field}: the fragment supplies a set")
            victims = names[:1]
            # The vanish arm ADDS as it drops, in the name's own shape — below the
            # floor the check SKIPS. `_` not case is the axis (cursor's lone `stop`).
            # The decoy is built from the VICTIM, so the mutated document carries the
            # dropped name's spelling in a NON-declaration position: that is the only
            # shape that can tell `f'"{name}"' not in text` from `name not in text`,
            # and eight matchers exist for exactly that distinction.
            stem = spell(victims[0])
            if stem.isdigit():
                # A numeric VALUE row: a `Pxd` suffix leaves our digits at the front,
                # where the reader's `(\d+)` finds them and the arm reads unchanged.
                sfx = ("0", "00")
            elif "_" in stem:
                sfx = ("_pxd", "_pxdb")
            else:
                sfx = ("Pxd", "Pxdb")
            kept = [spell(n) for n in names if n not in victims]

            intact = drive(build([spell(n) for n in names]))
            check(not intact.breaking,
                  f"{field}: an intact document must stay silent; got {intact.breaking}")
            rep = drive(build(kept + [stem + s for s in sfx]))
            check(any(victims[0] in x for x in rep.breaking),
                  f"{field}: a vanished `{victims[0]}` must fire; got {rep.breaking} "
                  f"(skipped as probe health? {rep.blind})")
    finally:
        d.fetch = real


def main() -> int:
    # Derived, not hand-listed — a missing test is inert while the suite prints
    # "all checks passed". `ast.walk` over `.body`: a nested/async def shadows too.
    tree = ast.parse(pathlib.Path(__file__).read_text())
    defined: dict[str, list[int]] = {}
    for n in ast.walk(tree):
        name = getattr(n, "name", None) if isinstance(
            n, (ast.FunctionDef, ast.AsyncFunctionDef)
        ) else None
        if name is None and isinstance(n, ast.Assign):
            name = next(
                (t.id for t in n.targets if isinstance(t, ast.Name)),
                None,
            )
        if name and name.startswith("test_"):
            defined.setdefault(name, []).append(n.lineno)
    # Here, not in a test: a test detecting duplicate names is itself shadowable.
    if dupes := sorted(k for k, v in defined.items() if len(v) > 1):
        print(f"DRIFT SELFTEST FAILED:\n  - test name bound twice, shadowing: {dupes}")
        return 1
    tests = tuple(o for n, o in list(globals().items()) if n.startswith("test_"))
    # The binding a name RESOLVES to must be the one the AST saw: a
    # `globals()[...]` rebind leaves the AST untouched, so only this catches it.
    for t in tests:
        lines = defined.get(t.__name__)
        if lines is None or t.__code__.co_firstlineno not in lines:
            print(
                f"DRIFT SELFTEST FAILED:\n  - {t.__name__} resolves to a function "
                f"defined at line {t.__code__.co_firstlineno}, which this file does "
                f"not declare (rebound at runtime?)"
            )
            return 1
    for t in tests:
        # A raise past a recorded `check()` would otherwise replace the whole FAILS
        # list — the line that already explains the failure — with a traceback.
        try:
            t()
        except Exception:  # noqa: BLE001 - a raising test is a failing test
            FAILS.append(f"{t.__name__} raised:\n{traceback.format_exc()}")
    if FAILS:
        print("DRIFT SELFTEST FAILED:")
        for f in FAILS:
            print(f"  - {f}")
        return 1
    print("drift selftest: all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
