#!/usr/bin/env python3
"""Risk radar — deterministic, advisory blast-radius surfacer for PR review.

Reads a list of changed file paths (one per line on stdin, repo-relative) and
prints a Markdown checklist of the review escalations that apply to the
high-risk seams the diff touches. It is a *lever, not a gate*: pure path
matching (NO LLM, NO network, NO model judgement), never blocks merge, and
emits no attestation artifact. It only SURFACES the audits the review prompts
(`.github/prompts/pr_review_rules.md` "Escalate by what the diff touches" +
`.github/prompts/pr-review.prompt.md` "When two lenses aren't enough") already
mandate — converting prose-only escalation (which slipped both the bot and
local review in #198) into something the PR states outright.

Future-proofing: adding a seam is ONE `Seam(...)` row below + one assertion in
`_selftest`. The `_selftest` is run in CI before the radar so the map can't
silently rot.

Usage:
  git diff --name-only BASE HEAD | python3 scripts/risk-radar.py   # -> radar.md on stdout (empty if no seam)
  python3 scripts/risk-radar.py --selftest                         # exit 0 = matcher healthy
"""

from __future__ import annotations

import sys
from dataclasses import dataclass
from typing import Callable

MARKER = "<!-- risk-radar -->"


@dataclass(frozen=True)
class Seam:
    key: str
    title: str
    # True iff this repo-relative path belongs to the seam.
    match: Callable[[str], bool]
    # Checklist lines (the escalation), rendered as GitHub task items.
    audit: tuple[str, ...]


# --- The seam map (single source of truth for path-based escalation) ---------
# Grounded in the documented escalation triggers; each predicate is a plain,
# obvious path rule (prefix / substring / suffix) — no glob semantics to be
# surprised by. ADD A SEAM HERE (and a _selftest case) when a new high-risk
# surface appears.
SEAMS: tuple[Seam, ...] = (
    Seam(
        key="hook-shim",
        title="🛑 Hook shim (`crates/pixtuoid-hook/`) — invariant #5",
        match=lambda p: p.startswith("crates/pixtuoid-hook/"),
        audit=(
            "Whole-shim **never-panic** audit (the WHOLE shim, not just the diff): "
            "`args_os()` not `args()` (non-UTF-8 argv panics → non-zero exit, visible to CC), "
            "no slicing/indexing on untrusted bytes, every read bounded, every error path a silent `exit(0)`.",
            "The 200 ms send bound stays (watchdog on both platforms). #198 added a prod `env::args()` and slipped BOTH the bot and local review.",
        ),
    ),
    Seam(
        key="motion-pose",
        title="🎞️ Motion / pose / walk-leg (not diff-readable)",
        match=lambda p: "/motion/" in p or "/pose/" in p,
        audit=(
            "**Render and WATCH it** before approving: a gif via the snapshot example, and/or `scripts/replay-fixture.sh` for resume/lifecycle motion.",
            "Add or update a frame-by-frame continuity guard — the flash/teleport/replay regressions all came back as failing tests first (#61 shipped five walk regressions behind an unchecked 'live run').",
        ),
    ),
    Seam(
        key="reducer-liveness",
        title="🧠 Reducer / liveness ladder / scope (state machine + concurrency)",
        match=lambda p: p.endswith(
            (
                "state/reducer.rs",
                "state/fsm.rs",
                "state/scope.rs",
                "state/correlation.rs",
                "source/jsonl/liveness.rs",
                "source/jsonl/unclaim.rs",
                "source/jsonl/walk.rs",
                "source/exit_watch.rs",
            )
        ),
        audit=(
            "Trace the **downstream interaction graph** (rebind, TTLs, cascade, dedup, sweeps), not just the changed lines — the bug is usually in an interaction the diff doesn't show.",
            "Check the negative branches are pinned (a test that survives deleting the guarded constant pins nothing).",
        ),
    ),
    Seam(
        key="visual",
        title="🎨 Sprite / pixel painter (visual)",
        match=lambda p: p.endswith(".sprite") or "/pixel_painter/" in p,
        audit=(
            "**Visual-verify at half-block scale**: render → `scripts/crop-snapshot.py` → read the PNG → self-critique.",
            "If the office's committed look changed, run `just gen` and commit the regenerated `docs/images/` + `site/public/demos/` in the SAME change (else `just gen-check` reds).",
        ),
    ),
    Seam(
        key="install",
        title="🔧 Install / config-write (`crates/pixtuoid/src/install/`)",
        match=lambda p: "crates/pixtuoid/src/install/" in p,
        audit=(
            "Writes to `settings.json` go through `install/io.rs` (`write_config_atomic` / `lock_config` + `ConfigLock::write_atomic`) — never a direct write; symlink resolution preserved (invariant #4).",
            "Any new/changed `install/` Target supplies a `verify_schema` (the install-soundness health check) mirroring the target's real config format.",
        ),
    ),
    Seam(
        key="json-contract",
        title="🔌 `--json` / Source contract surface",
        match=lambda p: p.endswith(
            ("source/registry.rs", "source/mod.rs")
        )
        or p.endswith("site/src/sources.json"),
        audit=(
            "Touched the `--json` / `SourceStatus` / `REGISTERED_SOURCES` shape? Run `just gen-contract` (else the Raycast `gen:contract` diff + `tsc` go red).",
            "The registry↔`REGISTERED_SOURCES`↔`sources.json` bridges are test-pinned — keep them in lockstep.",
        ),
    ),
    Seam(
        key="ci-gates",
        title="⚙️ CI / gate machinery (you're editing the safety net)",
        match=lambda p: p.startswith(".github/workflows/")
        or p == "justfile"
        or p.startswith(".githooks/"),
        audit=(
            "Confirm you did NOT weaken a gate: no removed required check, no `--no-verify`/hook-skip flag, no relaxed `-D warnings`.",
            "A workflow change can't be proven by local preflight — reason about what runs on push vs PR, and whether secrets/permissions widened.",
        ),
    ),
)


def match_seams(changed_files: list[str]) -> list[Seam]:
    """Return the seams (in declaration order, deduped) any changed file hits."""
    norm = [f.strip().replace("\\", "/") for f in changed_files if f.strip()]
    return [s for s in SEAMS if any(s.match(p) for p in norm)]


def render(seams: list[Seam]) -> str:
    """Markdown checklist for the matched seams, or '' when none match."""
    if not seams:
        return ""
    out = [
        MARKER,
        "## ⚠️ Risk radar — this PR touches high-blast-radius seam(s)",
        "",
        "Deterministic path check (**advisory, non-blocking** — no LLM, no merge gate). "
        "Each seam below carries a documented review escalation; make sure it's done before merge.",
        "",
    ]
    for s in seams:
        out.append(f"### {s.title}")
        out.extend(f"- [ ] {line}" for line in s.audit)
        out.append("")
    out.append(
        "_Generated by `scripts/risk-radar.py` · advisory only · "
        "see `.github/prompts/pr_review_rules.md` for the full escalation rules._"
    )
    return "\n".join(out) + "\n"


def _selftest() -> int:
    """Encode the spec; CI runs this before the radar so the map can't rot."""
    keys = lambda files: [s.key for s in match_seams(files)]

    # Each seam fires for a representative path.
    assert keys(["crates/pixtuoid-hook/src/main.rs"]) == ["hook-shim"]
    assert keys(["crates/pixtuoid-scene/src/motion/mod.rs"]) == ["motion-pose"]
    assert keys(["crates/pixtuoid-core/src/pose/tests.rs"]) == ["motion-pose"]
    assert keys(["crates/pixtuoid-core/src/state/reducer.rs"]) == ["reducer-liveness"]
    assert keys(["crates/pixtuoid-core/src/source/jsonl/liveness.rs"]) == ["reducer-liveness"]
    assert keys(["crates/pixtuoid-scene/sprites/default/robot.sprite"]) == ["visual"]
    assert keys(["crates/pixtuoid-scene/src/pixel_painter/palette.rs"]) == ["visual"]
    assert keys(["crates/pixtuoid/src/install/io.rs"]) == ["install"]
    assert keys(["crates/pixtuoid-core/src/source/registry.rs"]) == ["json-contract"]
    assert keys(["site/src/sources.json"]) == ["json-contract"]
    assert keys([".github/workflows/ci.yml"]) == ["ci-gates"]
    assert keys(["justfile"]) == ["ci-gates"]

    # Non-risk diffs are silent (no false alarms).
    assert keys(["README.md", "docs/ARCHITECTURE.md"]) == []
    assert keys(["site/src/features.json"]) == []
    assert keys([]) == []
    assert render([]) == ""

    # Backslash paths (Windows-style diff) normalize.
    assert keys([r"crates\pixtuoid-hook\src\main.rs"]) == ["hook-shim"]

    # A multi-seam diff lists each seam ONCE, in declaration order.
    multi = keys(
        [
            "crates/pixtuoid/src/install/io.rs",
            "crates/pixtuoid-hook/src/main.rs",
            "crates/pixtuoid-hook/src/transport.rs",  # second shim file -> still one seam
        ]
    )
    assert multi == ["hook-shim", "install"], multi

    # Rendered output carries the marker + a task item per audit line.
    md = render(match_seams(["crates/pixtuoid-hook/src/main.rs"]))
    assert md.startswith(MARKER), md
    assert "- [ ]" in md
    assert "never-panic" in md

    print("risk-radar selftest: OK", file=sys.stderr)
    return 0


def main(argv: list[str]) -> int:
    if "--selftest" in argv:
        return _selftest()
    changed = sys.stdin.read().splitlines()
    sys.stdout.write(render(match_seams(changed)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
