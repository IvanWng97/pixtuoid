#!/usr/bin/env python3
"""Risk radar — deterministic, advisory blast-radius surfacer for PR review.

Reads a list of changed file paths (one per line on stdin, repo-relative) and
prints a Markdown checklist of the review escalations that apply to the
high-risk seams the diff touches. A *lever, not a gate*: pure path matching (no
LLM, no network, no model judgement), never blocks merge, emits no attestation
artifact.

Scope is deliberately **blast-radius / invariant** seams, not prose-quality
escalations. Committed-art review is keyed on the SOURCE change that alters the
render (layout/theme/painter/…), not on the regenerated artifacts.

Anti-rot: each `Seam` names the doc anchor it mirrors (`source=`), and
`_selftest` asserts that anchor substring still exists in the referenced file.

Usage:
  git diff --name-only BASE HEAD | python3 scripts/risk-radar.py   # -> radar.md on stdout (empty if no seam)
  python3 scripts/risk-radar.py --selftest                         # exit 0 = matcher healthy
"""

from __future__ import annotations

import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

MARKER = "<!-- risk-radar -->"

_ROOT = Path(__file__).resolve().parent.parent


@dataclass(frozen=True)
class Seam:
    key: str
    title: str
    match: Callable[[str], bool]
    audit: tuple[str, ...]
    # (doc file, distinctive substring it must still contain) — bridge-tested by
    # `_selftest` so prose drift goes red.
    source: tuple[str, str]


# ADD A SEAM HERE (plus a _selftest case and the `source` anchor) when a new
# high-risk surface appears. Each predicate is a plain path rule — no glob
# semantics to be surprised by.
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
        source=(".github/prompts/pr_review_rules.md", "pixtuoid-hook"),
    ),
    Seam(
        key="motion-pose",
        title="🎞️ Motion / pose / walk-leg (not diff-readable)",
        match=lambda p: "/motion/" in p or "/pose/" in p,
        audit=(
            "**Render and WATCH it** before approving: a gif via the snapshot example, and/or `scripts/lib/tier-replay.sh` for resume/lifecycle motion.",
            "Add or update a frame-by-frame continuity guard — the flash/teleport/replay regressions all came back as failing tests first (#61 shipped five walk regressions behind an unchecked 'live run').",
        ),
        source=(".github/prompts/pr_review_rules.md", "walk-leg"),
    ),
    # Matches by DIRECTORY so a NEW file in either is covered automatically; the
    # source/-root probe rungs aren't in a dir of their own, so they stay
    # slash-anchored file matches.
    Seam(
        key="reducer-liveness",
        title="🧠 Reducer / liveness ladder / scope (state machine + concurrency)",
        match=lambda p: "/state/" in p
        or "/source/jsonl/" in p
        or p.endswith(("/exit_watch.rs", "/cc_probe.rs", "/fd_probe.rs")),
        audit=(
            "Trace the **downstream interaction graph** (rebind, TTLs, cascade, dedup, sweeps), not just the changed lines — the bug is usually in an interaction the diff doesn't show.",
            "Check the negative branches are pinned (a test that survives deleting the guarded constant pins nothing).",
        ),
        source=(".github/prompts/pr_review_rules.md", "liveness ladder"),
    ),
    # Deliberately its own seam rather than a branch of `hook-shim` (which audits
    # the shim's never-panic contract) or `reducer-liveness` (state-machine
    # reasoning): this is endpoint creation and arbitration, a security surface
    # with its own questions.
    Seam(
        key="hook-endpoint",
        title="🔌 Hook endpoint (daemon side) — socket/pipe creation + arbitration",
        match=lambda p: "/source/hook/" in p,
        audit=(
            "The endpoint must never be reachable with looser-than-owner-only modes: create-restricted-then-rename, **never** a process-global umask (it races every other task's file creation).",
            "Liveness arbitration must not be able to steal a LIVE owner's socket, and a hostile pre-squat must fail the bind LOUDLY rather than silently degrade.",
            "Check the guard is actually PINNED, not merely present — the whole #485 dir guard was once deletable (`-> Ok(())`) with a green suite. Mutate it; don't read it.",
            "`unix.rs` / `windows.rs`: the Windows arm is excluded from mutation testing and only ever runs in CI, so changes there need explicit reasoning.",
        ),
        source=(".github/prompts/pr_review_rules.md", "the daemon side"),
    ),
    Seam(
        key="visual",
        title="🎨 Sprite / painter / scene-look (visual — changes the rendered office)",
        match=lambda p: p.endswith(".sprite")
        or "/pixel_painter/" in p
        or "/theme/" in p
        or "/layout/" in p
        or p.endswith(("/pet.rs", "/chitchat.rs")),
        audit=(
            "**Visual-verify at half-block scale**: render → `scripts/crop-snapshot.py` → read the PNG → self-critique.",
            "If the office's committed look changed, run `just gen` and commit the regenerated `docs/images/` + `site/public/demos/` in the SAME change (else `just gen-check` reds).",
        ),
        source=("CLAUDE.md", "Sprite changes require visual verification"),
    ),
    Seam(
        key="install",
        title="🔧 Install / config-write (`crates/pixtuoid/src/install/`)",
        match=lambda p: "crates/pixtuoid/src/install/" in p,
        audit=(
            "Writes to `settings.json` go through `install/io.rs` (`write_config_atomic` / `lock_config` + `ConfigLock::write_atomic`) — never a direct write; symlink resolution preserved (invariant #4).",
            "Any new/changed `install/` Target supplies a `verify_schema` (the install-soundness health check) mirroring the target's real config format.",
        ),
        source=("CLAUDE.md", "write_config_atomic"),
    ),
    # Fires on ANY source/registry.rs + source/mod.rs edit, not only
    # contract-shape changes: over-firing is the safe side for a wire contract.
    Seam(
        key="json-contract",
        title="🔌 `--json` / Source contract surface",
        match=lambda p: p.endswith(("source/registry.rs", "source/mod.rs"))
        or p.endswith("crates/pixtuoid/src/sources.rs")
        or p.startswith("integrations/raycast/contract/")
        or p.endswith("site/src/sources.json"),
        audit=(
            "Touched the `--json` / `SourceStatus` / `OutcomeRow` / `WireOutcome` shape (their home is `crates/pixtuoid/src/sources.rs`) or the source roster (`registered_source_names()`)? Run `just gen-contract` (else the Raycast `gen:contract` diff + `tsc` go red).",
            "The registry (`registered_source_names()`)↔`sources.json` bridge + the committed `integrations/raycast/contract/*.schema.json` goldens are test-pinned — keep them in lockstep.",
        ),
        source=("CLAUDE.md", "gen-contract"),
    ),
    Seam(
        key="ci-gates",
        title="⚙️ CI / gate machinery (you're editing the safety net)",
        match=lambda p: p.startswith(".github/workflows/")
        or p.startswith(".github/actions/")
        or p == "justfile"
        or p.startswith("policy/ci-observability/")
        or p.startswith(".githooks/")
        or p
        in (
            "scripts/compare-screenshots.py",
            "scripts/gen-media.py",
            "scripts/gen-readme.mjs",
        ),
        audit=(
            "Confirm you did NOT weaken a gate: no removed required check, no `--no-verify`/hook-skip flag, no relaxed `-D warnings`.",
            "A workflow change can't be proven by local preflight — reason about what runs on push vs PR, and whether secrets/permissions widened.",
            "Editing the pixel/README drift-gate LOGIC (`compare-screenshots.py` threshold / `getbbox` trap, `gen-media.py --check`, `gen-readme.mjs`)? A silent weakening there has the same blast radius as a workflow edit.",
        ),
        source=("CLAUDE.md", "hook-skipping flags"),
    ),
)


def match_seams(changed_files: list[str]) -> list[Seam]:
    norm = [f.strip().replace("\\", "/") for f in changed_files if f.strip()]
    return [s for s in SEAMS if any(s.match(p) for p in norm)]


def render(seams: list[Seam]) -> str:
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

    assert keys(["crates/pixtuoid-hook/src/main.rs"]) == ["hook-shim"]
    assert keys(["crates/pixtuoid-scene/src/motion/mod.rs"]) == ["motion-pose"]
    assert keys(["crates/pixtuoid-scene/src/pose/tests.rs"]) == ["motion-pose"]
    assert keys(["crates/pixtuoid-scene/sprites/default/robot.sprite"]) == ["visual"]
    assert keys(["crates/pixtuoid-scene/src/pixel_painter/palette.rs"]) == ["visual"]
    assert keys(["crates/pixtuoid/src/install/io.rs"]) == ["install"]
    assert keys(["site/src/sources.json"]) == ["json-contract"]
    assert keys([".github/workflows/ci.yml"]) == ["ci-gates"]
    assert keys([".github/actions/setup-cargo-just/action.yml"]) == ["ci-gates"]
    assert keys(["justfile"]) == ["ci-gates"]

    for p in (
        "crates/pixtuoid-core/src/state/mod.rs",
        "crates/pixtuoid-core/src/state/reducer/mod.rs",
        "crates/pixtuoid-core/src/state/fsm.rs",
        "crates/pixtuoid-core/src/state/scope.rs",
        "crates/pixtuoid-core/src/state/correlation.rs",
        "crates/pixtuoid-core/src/source/jsonl/mod.rs",
        "crates/pixtuoid-core/src/source/jsonl/health.rs",
        "crates/pixtuoid-core/src/source/jsonl/liveness.rs",
        "crates/pixtuoid-core/src/source/jsonl/unclaim.rs",
        "crates/pixtuoid-core/src/source/jsonl/walk.rs",
        "crates/pixtuoid-core/src/source/exit_watch.rs",
        "crates/pixtuoid-core/src/source/cc_probe.rs",
        "crates/pixtuoid-core/src/source/fd_probe.rs",
    ):
        assert keys([p]) == ["reducer-liveness"], p
    # The dir match is slash-anchored — a per-source decoder or a "state"-ish
    # name that is NOT under state/ or jsonl/ must NOT fire reducer-liveness.
    assert keys(["crates/pixtuoid-core/src/source/copilot.rs"]) == []
    assert keys(["crates/x/src/reinstate.rs"]) == []

    for p in (
        "crates/pixtuoid-core/src/source/hook/mod.rs",
        "crates/pixtuoid-core/src/source/hook/unix.rs",
        "crates/pixtuoid-core/src/source/hook/windows.rs",
        "crates/pixtuoid-core/src/source/hook/router.rs",
        "crates/pixtuoid-core/src/source/hook/pid_watch.rs",
    ):
        assert keys([p]) == ["hook-endpoint"], p
    # Slash-anchored: a same-named sibling file must not be swallowed.
    assert keys(["crates/pixtuoid-hook/src/main.rs"]) == ["hook-shim"]
    assert keys(["crates/pixtuoid-core/src/source/hook.rs"]) == []
    for p in (
        "crates/pixtuoid-scene/src/theme/cyberpunk.rs",
        "crates/pixtuoid-scene/src/layout/compute.rs",
        "crates/pixtuoid-scene/src/pet.rs",
        "crates/pixtuoid-scene/src/chitchat.rs",
    ):
        assert keys([p]) == ["visual"], p
    assert keys(["crates/pixtuoid-core/src/source/mod.rs"]) == ["json-contract"]
    assert keys(["crates/pixtuoid-core/src/source/registry.rs"]) == ["json-contract"]
    assert keys(["crates/pixtuoid/src/sources.rs"]) == ["json-contract"]
    assert keys(["integrations/raycast/contract/outcome-row.schema.json"]) == ["json-contract"]
    assert keys([".githooks/pre-push"]) == ["ci-gates"]
    assert keys(["policy/ci-observability/main.rego"]) == ["ci-gates"]
    assert keys(["scripts/compare-screenshots.py"]) == ["ci-gates"]
    assert keys(["scripts/gen-media.py"]) == ["ci-gates"]
    assert keys(["scripts/gen-readme.mjs"]) == ["ci-gates"]
    # A non-gate script stays silent (scripts/ is NOT a blanket match).
    assert keys(["scripts/crop-snapshot.py"]) == []

    # Slash-anchored: a hypothetical `carpet.rs` must NOT fire `pet.rs`.
    assert keys(["crates/x/src/carpet.rs"]) == []

    assert keys(["README.md", "docs/ARCHITECTURE.md"]) == []
    assert keys(["site/src/features.json"]) == []
    assert keys([]) == []
    assert render([]) == ""

    assert keys([r"crates\pixtuoid-hook\src\main.rs"]) == ["hook-shim"]

    multi = keys(
        [
            "crates/pixtuoid/src/install/io.rs",
            "crates/pixtuoid-hook/src/main.rs",
            "crates/pixtuoid-hook/src/transport.rs",  # second shim file -> still one seam
        ]
    )
    assert multi == ["hook-shim", "install"], multi

    md = render(match_seams(["crates/pixtuoid-hook/src/main.rs"]))
    assert md.startswith(MARKER), md
    assert "- [ ]" in md
    assert "never-panic" in md

    for s in SEAMS:
        doc, anchor = s.source
        text = (_ROOT / doc).read_text(encoding="utf-8")
        assert anchor in text, f"{s.key}: anchor «{anchor}» missing from {doc}"

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
