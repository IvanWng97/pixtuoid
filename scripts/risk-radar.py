#!/usr/bin/env python3
"""Risk radar — deterministic, advisory blast-radius surfacer for PR review.

Reads a list of changed file paths (one per line on stdin, repo-relative) and
prints a Markdown checklist of the review escalations that apply to the
high-risk seams the diff touches. It is a *lever, not a gate*: pure path
matching (NO LLM, NO network, NO model judgement), never blocks merge, and
emits no attestation artifact. It only SURFACES the audits the review prompts
(`.github/prompts/pr_review_rules.md` "Escalate by what the diff touches",
`.github/prompts/pr-review.prompt.md` "When two lenses aren't enough", and the
root `CLAUDE.md` invariants) already mandate — converting prose-only escalation
(which slipped both the bot and local review in #198) into something the PR
states outright.

SCOPE (deliberate): this covers **blast-radius / invariant** seams — code whose
change can break a contract, a platform, the office's rendered look, or a CI
gate. It does NOT cover *prose-quality* escalations (the editorial lens for
README/site/release-notes), which aren't blast-radius. Committed-art review is
keyed on the SOURCE change that alters the render (layout/theme/painter/…), not
on the regenerated `docs/images` / `site/public/demos` artifacts.

Anti-rot: each `Seam` names the doc anchor it mirrors (`source=`), and
`_selftest` asserts that anchor substring still exists in the referenced file —
so if someone removes/renames an escalation in the docs, the seam's grounding
goes red instead of silently drifting (the repo's `expand_tilde`-divergence
bug class). Adding a seam is ONE `Seam(...)` row + the per-path/anchor asserts
in `_selftest`.

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

# Repo root (this file lives at <root>/scripts/risk-radar.py) — used only by the
# selftest's anti-rot anchor check.
_ROOT = Path(__file__).resolve().parent.parent


@dataclass(frozen=True)
class Seam:
    key: str
    title: str
    # True iff this repo-relative path belongs to the seam.
    match: Callable[[str], bool]
    # Checklist lines (the escalation), rendered as GitHub task items.
    audit: tuple[str, ...]
    # (doc file, distinctive substring it must still contain) — the grounding
    # this seam mirrors; bridge-tested by `_selftest` so prose drift goes red.
    source: tuple[str, str]


# --- The seam map (single source of truth for path-based escalation) ---------
# Grounded in the documented escalation triggers; each predicate is a plain,
# obvious path rule (prefix / substring / suffix) — no glob semantics to be
# surprised by. ADD A SEAM HERE (and a _selftest case + the `source` anchor)
# when a new high-risk surface appears.
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
            "**Render and WATCH it** before approving: a gif via the snapshot example, and/or `scripts/replay-fixture.sh` for resume/lifecycle motion.",
            "Add or update a frame-by-frame continuity guard — the flash/teleport/replay regressions all came back as failing tests first (#61 shipped five walk regressions behind an unchecked 'live run').",
        ),
        source=(".github/prompts/pr_review_rules.md", "walk-leg"),
    ),
    # reducer-liveness matches by DIRECTORY (state/ + source/jsonl/) so a NEW
    # file in either is covered automatically — the old explicit file list was
    # the rot-prone outlier (every other seam matches by dir substring) and
    # silently missed jsonl/mod.rs + jsonl/health.rs, the watcher orchestration
    # loop (refresh_probe_snapshot / walk_jsonl / drain_child_end_unclaims /
    # FailureLatch). The source/-root probe rungs aren't in a dir of their own,
    # so they stay slash-anchored file matches.
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
    # json-contract deliberately fires on ANY source/mod.rs edit (not only
    # contract-shape changes): it holds REGISTERED_SOURCES + AgentEvent + the
    # Source trait, the contract-bearing items — over-firing is the safe side
    # for a wire contract (a 5s "shape unchanged" dismissal beats a missed
    # gen-contract). Same rationale for justfile under ci-gates: the justfile
    # IS the single source of truth for every gate, so any edit can weaken one.
    Seam(
        key="json-contract",
        title="🔌 `--json` / Source contract surface",
        match=lambda p: p.endswith(("source/registry.rs", "source/mod.rs"))
        or p.endswith("site/src/sources.json"),
        audit=(
            "Touched the `--json` / `SourceStatus` / `REGISTERED_SOURCES` shape? Run `just gen-contract` (else the Raycast `gen:contract` diff + `tsc` go red).",
            "The registry↔`REGISTERED_SOURCES`↔`sources.json` bridges are test-pinned — keep them in lockstep.",
        ),
        source=("CLAUDE.md", "gen-contract"),
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
        source=("CLAUDE.md", "hook-skipping flags"),
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
    assert keys(["crates/pixtuoid-scene/sprites/default/robot.sprite"]) == ["visual"]
    assert keys(["crates/pixtuoid-scene/src/pixel_painter/palette.rs"]) == ["visual"]
    assert keys(["crates/pixtuoid/src/install/io.rs"]) == ["install"]
    assert keys(["site/src/sources.json"]) == ["json-contract"]
    assert keys([".github/workflows/ci.yml"]) == ["ci-gates"]
    assert keys(["justfile"]) == ["ci-gates"]

    # reducer-liveness matches whole dirs (state/ + source/jsonl/) + the
    # source/-root probe rungs. Includes jsonl/mod.rs + jsonl/health.rs — the
    # watcher orchestration loop the old file-list silently missed.
    for p in (
        "crates/pixtuoid-core/src/state/mod.rs",
        "crates/pixtuoid-core/src/state/reducer.rs",
        "crates/pixtuoid-core/src/state/fsm.rs",
        "crates/pixtuoid-core/src/state/scope.rs",
        "crates/pixtuoid-core/src/state/correlation.rs",
        "crates/pixtuoid-core/src/source/jsonl/mod.rs",  # was uncovered
        "crates/pixtuoid-core/src/source/jsonl/health.rs",  # was uncovered
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
    # visual fires on the office-look SOURCE dirs, not just sprites/painter:
    for p in (
        "crates/pixtuoid-scene/src/theme/cyberpunk.rs",
        "crates/pixtuoid-core/src/layout/compute.rs",
        "crates/pixtuoid-scene/src/pet.rs",
        "crates/pixtuoid-scene/src/chitchat.rs",
    ):
        assert keys([p]) == ["visual"], p
    # json-contract fires on the module root + the registry:
    assert keys(["crates/pixtuoid-core/src/source/mod.rs"]) == ["json-contract"]
    assert keys(["crates/pixtuoid-core/src/source/registry.rs"]) == ["json-contract"]
    # ci-gates fires on the hooks dir too:
    assert keys([".githooks/pre-push"]) == ["ci-gates"]

    # `pet.rs` matching is slash-anchored — a hypothetical `carpet.rs` must NOT fire.
    assert keys(["crates/x/src/carpet.rs"]) == []

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

    # Anti-rot bridge: every seam's grounding anchor must still exist in its doc.
    # If an escalation is removed/renamed upstream, this goes red — forcing a
    # re-sync instead of a silently-stale audit comment.
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
