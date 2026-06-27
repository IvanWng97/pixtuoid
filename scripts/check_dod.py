#!/usr/bin/env python3
"""Definition-of-Done gate — mechanize the non-code-shaped lifecycle discipline.

The repo has two binding layers and a gap between them: prose (CLAUDE.md / agent
MEMORY.md) binds the agent's *intention* (unreliable), and the ~25 ci.yml jobs +
`just preflight` bind *code shape*. The uncovered middle is the lifecycle
discipline — design → impl-plan → TDD → self-review → two-lens review → online
review → traceability → hygiene — which is prose-only, so an agent does the
satisfying core work and PUNTS the closing discipline as an optional offer. PR
#23 merged unreviewed with a path-traversal vuln because the two-lens mandate has
no mechanical floor.

This is the floor. ONE source of truth (pure parsers + thin git/gh glue, the
exact idiom of check_review_disposition.py / sharp_edge_inventory.py), called
from BOTH layers so there is no local-vs-CI drift:

  --stop            agent layer: Stop hook. Block turn-end on an ahead-of-main
                    code commit whose attestation/blocking checks are unmet.
  --gate-from-stdin agent layer: PreToolUse(Bash) hook. Deny `gh pr merge` /
                    `git push` / `--no-verify` git ops unless the DoD passes.
  --local           change layer: pre-push hook. Blocking checks on local
                    ahead-of-main commits.
  --pr <n>          change layer: CI `definition-of-done` job. The AUTHORITATIVE
                    gate — it IGNORES DOD_BYPASS, so a bypassed local push still
                    faces a check no env var can relax.
  --judge-prompt    emit the LLM-judge prompt (the substance-over-existence check)
                    to stdout; CI pipes it to the model. Keeps the model call out
                    of the pure Python.
  --selftest        pure-function tests, no network.

Design property (the gate-gaming defense — the agent writing the artifacts is the
agent being gated): only the CHANGE layer is authoritative. DOD_BYPASS relaxes
ONLY the agent-layer hooks. Existence checks are the floor; the CHANGE layer +
diff-FACT cross-checks + the advisory LLM-judge are the substance.

    check_dod.py --pr 401 [--repo OWNER/REPO]
    check_dod.py --local
    check_dod.py --stop                 # reads git state of the cwd worktree
    check_dod.py --gate-from-stdin      # reads the PreToolUse JSON on stdin
    check_dod.py --selftest
"""

import argparse
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

from _gov import _strip_control  # shared pure helper (see docs/governance-scripts.md)

DEFAULT_REPO = "IvanWng97/pixtuoid"

# The ONE place install writes ~/.claude/settings.json (CLAUDE.md invariant #4).
SETTINGS_WRITE_ALLOWED = "crates/pixtuoid/src/install/io.rs"

# Docs-ish paths: a tree touching ONLY these runs zero code checks.
# A doc-name is the BASENAME (optionally one extension), not a prefix of a longer
# code filename — `crates/x/src/readme_gen.rs` is code, not docs (anchor to `$`).
_DOC_RE = re.compile(
    r"(^|/)(README|CONTRIBUTING|CHANGELOG)(\.[A-Za-z]+)?$|\.md$|^docs/|^site/|\.(png|webm|gif|jpe?g|svg)$",
    re.IGNORECASE,
)
_RUST_SRC_RE = re.compile(r"^crates/[^/]+/src/.*\.rs$")
_RUST_TEST_RE = re.compile(r"(^crates/[^/]+/tests/)|(_tests?\.rs$)|(/tests?\.rs$)")
_SPRITE_RE = re.compile(r"(^|/)sprites/|\.sprite$")


# --------------------------------------------------------------------------- #
# Data shapes                                                                 #
# --------------------------------------------------------------------------- #


@dataclass(frozen=True)
class Violation:
    check: str          # the sub-check name
    severity: str       # "blocking" | "warn"
    where: str          # file:line or artifact name
    detail: str


@dataclass(frozen=True)
class ChangeClass:
    files: tuple        # every changed path
    code_touched: bool  # any crates/**/*.rs (prod or test)
    docs_only: bool     # every changed path is docs-ish
    public_surface: bool  # cli.rs / config.rs / source/registry.rs touched
    sprite: bool        # any sprite asset touched
    span_files: int
    feature_shaped: bool  # >=3 files OR a newly-added crates src module
    shim_touched: bool


# --------------------------------------------------------------------------- #
# Pure parsers (each pinned by check_dod_selftest.py)                         #
# --------------------------------------------------------------------------- #


def classify_changed_files(name_status: str) -> ChangeClass:
    """Parse `git diff --name-status <base>...HEAD` into a ChangeClass. Handles
    rename/copy rows (`R100\\told\\tnew`) by taking the destination path. The
    classifier is the false-positive killer: a docs_only/clean tree => zero code
    checks."""
    files: list[str] = []
    added_module = False
    for raw in name_status.splitlines():
        raw = raw.strip()
        if not raw:
            continue
        parts = raw.split("\t")
        status = parts[0]
        path = parts[-1]  # dest for R/C rows, the path otherwise
        files.append(path)
        if status.startswith("A") and _RUST_SRC_RE.match(path):
            added_module = True
    code = [f for f in files if f.endswith(".rs") and f.startswith("crates/")]
    docs_only = bool(files) and all(_DOC_RE.search(f) for f in files)
    public = any(
        f.endswith(("cli.rs", "config.rs"))
        or f.endswith("source/registry.rs")
        for f in files
    )
    return ChangeClass(
        files=tuple(files),
        code_touched=bool(code),
        docs_only=docs_only,
        public_surface=public,
        sprite=any(_SPRITE_RE.search(f) for f in files),
        span_files=len(files),
        feature_shaped=(len(files) >= 3) or added_module,
        shim_touched=any(f.startswith("crates/pixtuoid-hook/") for f in files),
    )


def added_lines(unified_diff: str) -> dict:
    """Parse a unified `git diff` into {path: [(new_lineno, added_text), ...]}.
    The `+++ b/<path>` header is honored ONLY in the file-header position (before
    the first `@@` of a file, anchored by `diff --git`), so a hunk CONTENT line
    that itself begins with `+++ ` (source line `++ ...`) is read as an addition,
    not mistaken for a new file header. Removed lines are ignored."""
    out: dict = {}
    path = None
    new_ln = 0
    in_hunk = False
    for line in unified_diff.splitlines():
        if line.startswith("diff --git "):
            in_hunk = False
            path = None
            continue
        if line.startswith("+++ ") and not in_hunk:
            p = line[4:].strip()
            path = None if p == "/dev/null" else p[2:] if p[:2] in ("a/", "b/") else p
            continue
        if line.startswith("@@"):
            m = re.search(r"\+(\d+)", line)
            new_ln = int(m.group(1)) if m else 0
            in_hunk = True
            continue
        if path is None or not in_hunk:
            continue
        if line.startswith("+"):
            out.setdefault(path, []).append((new_ln, line[1:]))
            new_ln += 1
        elif line.startswith("-"):
            continue
        else:
            new_ln += 1
    return out


def name_status_from_diff(unified_diff: str) -> str:
    """Reconstruct `git diff --name-status` (A/M/D/R + path) from a unified diff, so
    CI's `--pr` path can feed the SAME classifier as local without a second gh call
    that synthesizes every file as `M` (which blinds the added-module/feature_shaped
    classifier in CI). Status from the `new file` / `deleted file` / `rename to`
    markers; default `M`."""
    rows: list = []
    cur = None
    status = "M"

    def flush():
        if cur:
            rows.append(f"{status}\t{cur}")

    for line in unified_diff.splitlines():
        if line.startswith("diff --git "):
            flush()
            m = re.search(r" b/(.+)$", line)
            cur = m.group(1) if m else None
            status = "M"
        elif line.startswith("new file mode"):
            status = "A"
        elif line.startswith("deleted file mode"):
            status = "D"
        elif line.startswith("rename to "):
            cur = line[len("rename to "):].strip()
            status = "R"
    flush()
    return "\n".join(rows)


def _is_rust_prod(path: str) -> bool:
    return bool(
        _RUST_SRC_RE.match(path)
        and not _RUST_TEST_RE.search(path)
        and not path.endswith("build.rs")
        and "/examples/" not in path
    )


# Best-effort "is this a live token, not prose/data": drop `"..."` string literals
# and `// ...` line comments before matching, so a token MENTIONED in a comment or
# string ("// replace println! with tracing", `let s = "...settings.json..."`) is
# not read as a call. Not a full Rust parser (block comments / multi-line strings
# slip through), but it kills the common FP class without widening the blocking net.
_RUST_STRLIT = re.compile(r'"(?:[^"\\]|\\.)*"')
_RUST_LINE_COMMENT = re.compile(r"//.*$")


def _rust_code_only(text: str) -> str:
    return _RUST_LINE_COMMENT.sub("", _RUST_STRLIT.sub('""', text))


_PRINT_RE = re.compile(r"\b(println|eprintln)\s*!")
_PRINT_ALLOW = re.compile(r"//\s*dod:allow\(println\)")
# Files where stdout/stderr printing IS the contract — the CLAUDE.md exemption
# ("headless summary and explicit CLI output excepted"). Editing a line in one of
# these de-grandfathers it under diff-scoping, so exempt by path, not by pragma.
_CLI_OUTPUT_RE = re.compile(r"/(main|cli|validate|init_pack|doctor)\.rs$|/runtime/mod\.rs$")


def prod_print_violations(added: dict) -> list:
    """[BLOCKING] A newly-added println!/eprintln! in a Rust production path
    (excluding the CLI-presenter files) without an explicit
    `// dod:allow(println) — <why>` pragma. Diff-scoped, so the ~38 grandfathered
    sites and all current code are exempt by construction (a hand-asserted count
    is itself the silent-death class)."""
    out: list = []
    for path, lines in added.items():
        if not _is_rust_prod(path) or _CLI_OUTPUT_RE.search(path):
            continue
        for ln, text in lines:
            # Match on code-only text; the pragma lives IN a comment, so test it
            # against the ORIGINAL line.
            if _PRINT_RE.search(_rust_code_only(text)) and not _PRINT_ALLOW.search(text):
                out.append(
                    Violation(
                        "prod-print", "blocking", f"{path}:{ln}",
                        "new println!/eprintln! in a production path — use tracing,"
                        " or annotate `// dod:allow(println) — <why>` "
                        f"(CLAUDE.md): {_strip_control(text.strip())[:80]}",
                    )
                )
    return out


_WRITE_RE = re.compile(r"fs::write|File::create|OpenOptions|\.write_all\(|\.write\(")


def settings_write_violations(added: dict) -> list:
    """[BLOCKING] A newly-added filesystem write touching `settings.json` outside
    install/io.rs (CLAUDE.md: go through write_config_atomic). Test files and
    tempdir-scoped lines are exempt. Matches on code-only text (a rule-documenting
    comment is not a write). KNOWN LIMITATION: line-scoped, so a write split across
    two lines (`let p = dir.join("settings.json");\\n fs::write(p, ..)`) is missed
    — this is a defense-in-depth backstop; the architectural invariant (the
    `pub(crate)` install/io.rs boundary + `just arch`) is the primary enforcement."""
    out: list = []
    for path, lines in added.items():
        if not path.endswith(".rs") or path == SETTINGS_WRITE_ALLOWED:
            continue
        if _RUST_TEST_RE.search(path):
            continue
        for ln, text in lines:
            code = _rust_code_only(text)
            if "settings.json" in text and _WRITE_RE.search(code):
                if "tmp" in text.lower() or "tempdir" in text.lower():
                    continue
                out.append(
                    Violation(
                        "settings-write", "blocking", f"{path}:{ln}",
                        "writes settings.json outside install/io.rs — go through"
                        " write_config_atomic (CLAUDE.md invariant #4)",
                    )
                )
    return out


def _strip_shell_quotes(s: str) -> str:
    """Drop '...'/"..." quoted spans so a flag MENTIONED in a commit message or an
    echo'd string isn't read as a live flag. Heuristic (no nested/escaped-quote
    handling) — shell quoting is undecidable and this is an advisory deny."""
    return re.sub(r"'[^']*'|\"[^\"]*\"", "", s)


# `--no-verify` on any git command, or `SKIP_PREFLIGHT=1` at a command/segment
# boundary — both LINEAR regexes (lazy `*?` + literal, no nested quantifier).
_GIT_NOVERIFY_LONG = re.compile(r"\bgit\b[^|&;]*?--no-verify\b")
_SKIP_ENV = re.compile(r"(?:^|[|&;]\s*|\b(?:env|export)\s+)SKIP_PREFLIGHT=1\b")

# Git global options that consume the NEXT token as a value (`git -c k=v commit`,
# `git -C dir push`), so the subcommand scanner skips that value token too.
_GIT_VALUE_OPTS = frozenset(
    {"-c", "-C", "--git-dir", "--work-tree", "--namespace", "--exec-path", "--super-prefix"}
)


# Shell command/segment separators — split on them so a chained `git add && git
# push` is seen as TWO invocations (matching the old `re.search`-anywhere behavior;
# scanning only the first `git` was a regression). Quote-strip BEFORE splitting so a
# separator inside a message (`commit -m "a; b"`) isn't a real boundary.
_SHELL_SEP = re.compile(r"[;&|\n]+")


def _git_invocations(cmd: str):
    """Every (subcommand, tokens-after-it) for EACH `git` across the shell-separated
    segments of `cmd`, skipping leading git GLOBAL options and their space-separated
    values. TOKENIZED, not regex — ReDoS-immune by construction (the prior
    `(?:\\s+--?[\\w-]+(?:[= ]\\S+)?)*` form had catastrophic backtracking on
    `-- -- --` runs: py/redos)."""
    out: list = []
    for seg in _SHELL_SEP.split(_strip_shell_quotes(cmd or "")):
        toks = seg.split()
        i = 0
        while i < len(toks):
            if toks[i] != "git":
                i += 1
                continue
            j = i + 1
            while j < len(toks) and toks[j].startswith("-"):
                opt = toks[j]
                j += 1
                if opt in _GIT_VALUE_OPTS and j < len(toks):
                    j += 1  # consume the option's value token
            if j < len(toks):
                out.append((toks[j], toks[j + 1:]))
            i = j + 1
    return out


def _has_push(cmd: str) -> bool:
    """Whether ANY git invocation in `cmd` is a `push` (incl. chained `git x && git
    push`)."""
    return any(sub == "push" for sub, _ in _git_invocations(cmd))


def _commit_has_short_no_verify(cmd: str) -> bool:
    """ANY `git commit` carrying a combinable short `-n` (commit -n == --no-verify),
    e.g. `-n`/`-nm`/`-mn` — even behind a global option or chained after another git
    (`git add . && git commit -nm x`)."""
    return any(
        sub == "commit" and any(t.startswith("-") and not t.startswith("--") and "n" in t[1:] for t in rest)
        for sub, rest in _git_invocations(cmd)
    )


def command_has_no_verify(cmd: str) -> bool:
    """Whether a shell command bypasses the git hooks (the binding PreToolUse half —
    a transient command leaves no diff trace). Quote-stripped first so a `-n` /
    `--no-verify` inside a message or an echo'd string is not a false deny; the
    `-n` short flag is matched only on `git commit` (where it means --no-verify)."""
    c = _strip_shell_quotes(cmd or "")
    return bool(_GIT_NOVERIFY_LONG.search(c)) or _commit_has_short_no_verify(c) or bool(_SKIP_ENV.search(c))


# The committed-diff backstop fires ONLY on paths where a hook-skipping flag would
# be a LIVE git op — shell scripts, git hooks, CI workflows, the justfile. Source
# files (.rs/.py — including THIS gate's own regexes/tests/string literals) are NOT
# scanned: the token there is data, not an invocation (else the gate fails its own
# PR). The PreToolUse deny covers the transient-command case.
_GITOP_PATH_RE = re.compile(r"\.sh$|(^|/)\.githooks/|(^|/)\.github/workflows/|(^|/)[Jj]ustfile$")
# A line that merely DOCUMENTS the ban (a shell/hook comment, or ban/forbid prose).
_NOVERIFY_DOC = re.compile(r"^\s*#|\bban\b|\bforbid", re.IGNORECASE)


def no_verify_violations(added: dict) -> list:
    """[BLOCKING] A committed hook-skipping git op in a script/hook/CI/justfile path
    (backstop to the PreToolUse deny). Comment lines documenting the ban are
    allowlisted; source-file paths are out of scope (the token there is data)."""
    out: list = []
    for path, lines in added.items():
        if not _GITOP_PATH_RE.search(path):
            continue
        for ln, text in lines:
            if command_has_no_verify(text) and not _NOVERIFY_DOC.search(text):
                out.append(
                    Violation(
                        "no-verify", "blocking", f"{path}:{ln}",
                        "adds a hook-skipping git flag (--no-verify / -n / SKIP_PREFLIGHT)"
                        " in a script/hook/CI path — banned (CLAUDE.md)",
                    )
                )
    return out


# An enum-variant decl, EXCLUDING common non-variant capitalized idents (match
# arms / constructors) that would otherwise read as new subcommands (warn noise).
_NEW_CMD_RE = re.compile(
    r"^\s*(?!Some\b|Ok\b|Err\b|None\b|Self\b|Box\b|Vec\b|Arc\b|Rc\b|String\b|Result\b|Option\b)"
    r"[A-Z]\w+\s*(\{|\(|,|$)"
)


def public_surface_violations(class_: ChangeClass, added: dict) -> list:
    """[WARN] A newly-added `Cmd::`/subcommand variant or config field with no
    README + nearest-CLAUDE.md update in the same diff (docs-currency). Keys off
    ADDED hunks, never a file match, so a rename/move doesn't misfire. WARN, not
    blocking: a clap rename and the variant heuristic are too noisy to fail a PR."""
    touched = set(added.keys())
    readme = any(f.endswith("README.md") for f in touched)
    claude_md = any(f.endswith("CLAUDE.md") for f in touched)
    docs_ok = readme and claude_md
    out: list = []
    for path, lines in added.items():
        is_cli = path.endswith("cli.rs")
        is_cfg = path.endswith("config.rs")
        if not (is_cli or is_cfg):
            continue
        for ln, text in lines:
            stripped = text.strip()
            looks_new_arm = is_cli and _NEW_CMD_RE.match(stripped) and "//" not in stripped[:2]
            looks_new_key = is_cfg and re.match(r"^\s*pub\s+\w+\s*:", text)
            if (looks_new_arm or looks_new_key) and not docs_ok:
                out.append(
                    Violation(
                        "docs-currency", "warn", f"{path}:{ln}",
                        "adds a public surface (subcommand/config key) without"
                        " updating README + the nearest CLAUDE.md in the same"
                        f" change (CLAUDE.md): {_strip_control(stripped)[:60]}",
                    )
                )
                break  # one report per file is enough
    return out


_PATHSEP_RE = re.compile(r"(to_string_lossy|\.display\(\)\.to_string)\(\)")


def path_sep_warnings(added: dict) -> list:
    """[WARN] A path stringified and compared against a `/`-bearing literal — the
    Windows-only mis-compare class. windows-test is the real backstop."""
    out: list = []
    for path, lines in added.items():
        if not path.endswith(".rs"):
            continue
        for ln, text in lines:
            if _PATHSEP_RE.search(text) and re.search(r'"[^"]*/[^"]*"', text):
                out.append(
                    Violation(
                        "path-sep", "warn", f"{path}:{ln}",
                        "asserts a path STRING with a hardcoded '/' — compare"
                        " PathBuf structurally (Windows class, CLAUDE.md)",
                    )
                )
    return out


_TWO_LENS_HDR = re.compile(r"^\s*two-lens-review\s*:", re.IGNORECASE)
# label : VERDICT [ ... conf <N> ]  — the confidence tail is OPTIONAL (requiring an
# inline number on every lens line was needless red-friction; the bar is >=2
# distinct lenses each with a verdict).
_LENS_LINE = re.compile(
    r"^\s*[-*]\s*([A-Za-z][\w/() -]*?)\s*:\s*([A-Za-z][A-Za-z-]*)\b"
    r"(?:.*?\bconf(?:idence)?\s*[:=]?\s*(\d+)\b)?",
    re.IGNORECASE,
)


def parse_two_lens(pr_body: str) -> list:
    """The (lens-label, verdict, confidence-or-None) rows under a `Two-lens-review:`
    block in the PR body. Confidence is optional. Format (CONTRIBUTING.md):
        Two-lens-review:
        - correctness: APPROVE (conf 85)
        - design/blast-radius: REQUEST-CHANGES"""
    rows: list = []
    in_block = False
    for raw in (pr_body or "").splitlines():
        if _TWO_LENS_HDR.match(raw):
            in_block = True
            continue
        if in_block:
            m = _LENS_LINE.match(raw)
            if m:
                conf = int(m.group(3)) if m.group(3) else None
                rows.append((m.group(1).strip().lower(), m.group(2).upper(), conf))
            elif raw.strip() == "":
                continue
            elif not raw.strip().startswith(("-", "*")):
                in_block = False
    return rows


def two_lens_ok(pr_body: str) -> bool:
    """[BLOCKING at merge] >=2 DISTINCT lens labels, each with a verdict token.
    A confidence, if given, must be a sane 0..100 (a garbage >100 doesn't count)."""
    rows = parse_two_lens(pr_body)
    labels = {label for (label, _v, c) in rows if c is None or 0 <= c <= 100}
    return len(labels) >= 2


def ledger_touched(class_: ChangeClass) -> bool:
    """[WARN] Whether docs/REVIEW-LEDGER.md is in the diff."""
    return any(f.endswith("docs/REVIEW-LEDGER.md") for f in class_.files)


_CLOSES_RE = re.compile(
    r"\b(clos(?:e|es|ed)|fix(?:e[sd])?|resolv(?:e|es|ed))\s+#(\d+)", re.IGNORECASE
)


def closes_keywords(*texts: str) -> list:
    """[WARN] Every issue number a close-keyword fires on (PR body + commits), so
    the human can verify none is stale on a re-scope (GitHub fires from body OR
    commit, and conditional phrasing still fires)."""
    nums: list = []
    for t in texts:
        nums.extend(int(m.group(2)) for m in _CLOSES_RE.finditer(t or ""))
    return sorted(set(nums))


_DEFER_RE = re.compile(r"\b(defer(?:red)?|follow-?up|punt(?:ed)?|later\b|TODO)\b", re.IGNORECASE)
_ISSUE_REF = re.compile(r"#\d+")


def deferrals_have_issue(text: str) -> list:
    """[WARN] Deferral language without a nearby issue ref on the same line — a
    deferred finding with no issue is a silently-dropped finding (CLAUDE.md).
    Heuristic, hence warn."""
    out: list = []
    for raw in (text or "").splitlines():
        if _DEFER_RE.search(raw) and not _ISSUE_REF.search(raw):
            out.append(_strip_control(raw.strip())[:100])
    return out


# Attestation: which checkboxes a change-class must have ticked. Key => label.
_ATTEST_BOXES = {
    "tdd": "TDD",
    "self-review": "Self-review",
    "design": "Design",
    "impl-plan": "Impl-plan",
    "docs-currency": "Docs-currency",
}


def _ticked(md: str, key: str) -> bool:
    pat = re.compile(rf"-\s*\[[xX]\]\s*{re.escape(_ATTEST_BOXES[key])}\b")
    return bool(pat.search(md or ""))


def attestation_gaps(class_: ChangeClass, md: str) -> list:
    """[agent-layer] The required-but-unticked attestation boxes for this class.
    Class-gated to control false positives: a clean/docs-only tree requires none;
    feature-shaped work additionally requires Design + Impl-plan; a public-surface
    change requires Docs-currency."""
    if not class_.code_touched:
        return []
    required = ["tdd", "self-review"]
    if class_.feature_shaped:
        required += ["design", "impl-plan"]
    if class_.public_surface:
        required.append("docs-currency")
    return [
        Violation("attestation", "blocking", f".dod/attestation.md::{_ATTEST_BOXES[k]}",
                  f"unticked DoD box: {_ATTEST_BOXES[k]}")
        for k in required if not _ticked(md, k)
    ]


def bypass_reason(md: str, env: dict) -> str | None:
    """A non-empty bypass reason from DOD_BYPASS env or a `DOD-BYPASS: <reason>`
    line in the attestation. Agent-layer ONLY — the CHANGE layer ignores it."""
    env_val = (env.get("DOD_BYPASS") or "").strip()
    if env_val:
        return env_val
    m = re.search(r"^\s*DOD-BYPASS\s*:\s*(\S.*)$", md or "", re.MULTILINE)
    return m.group(1).strip() if m else None


def blocking_violations(class_: ChangeClass, added: dict) -> list:
    """Every BLOCKING code-shaped violation — the near-zero-FP floor the CI job
    enforces. Each sub-check self-scopes by path, so this is cheap on an empty
    diff. Excludes the heuristic warns and the agent-layer attestation/merge-only
    checks."""
    return (
        prod_print_violations(added)
        + settings_write_violations(added)
        + no_verify_violations(added)
    )


def all_warnings(class_: ChangeClass, added: dict, pr_body: str, *commit_texts) -> list:
    """Heuristic (WARN) findings — surfaced, never failing a build: docs-currency,
    path-separator, and deferral-without-issue."""
    out = path_sep_warnings(added) + public_surface_violations(class_, added)
    for d in deferrals_have_issue("\n".join([pr_body, *commit_texts])):
        out.append(Violation("deferral", "warn", "PR text", f"deferral without an issue ref: {d}"))
    return out


def verdict(violations: list) -> int:
    """Exit code: non-zero iff a BLOCKING violation is present."""
    return 1 if any(v.severity == "blocking" for v in violations) else 0


def judge_prompt(diff: str, pr_body: str) -> str:
    """The advisory LLM-judge prompt (substance over existence): rate whether the
    two-lens block is a real differentiated review and whether TDD/DRY hold. CI
    pipes this to the model; the pure Python never calls a model."""
    return (
        "You are an adversarial reviewer auditing whether a PR's process artifacts "
        "are SUBSTANTIVE, not theater. Given the diff and the PR body, answer as "
        "JSON {two_lens_substantive: bool, tdd_evident: bool, concerns: [..]}.\n"
        "- two_lens_substantive: do the >=2 review lenses show DISTINCT perspectives "
        "(not paraphrases) with specific, diff-grounded observations?\n"
        "- tdd_evident: is there a test hunk that plausibly preceded/accompanies the "
        "implementation (not an after-thought)?\n\n"
        "=== PR BODY ===\n" + (pr_body or "")[:4000] + "\n\n=== DIFF (truncated) ===\n" + (diff or "")[:12000]
    )


def render(violations: list) -> str:
    blocking = [v for v in violations if v.severity == "blocking"]
    warns = [v for v in violations if v.severity == "warn"]
    out = []
    for v in blocking:
        out.append(f"  ✗ [{v.check}] {v.where} — {v.detail}")
    for v in warns:
        out.append(f"  ⚠ [{v.check}] {v.where} — {v.detail}")
    return "\n".join(out)


# --------------------------------------------------------------------------- #
# git / gh glue (untested — the pure functions above carry the logic).        #
# --------------------------------------------------------------------------- #


def _git(*args: str, cwd=None) -> str:
    return subprocess.run(
        ["git", *args], capture_output=True, text=True, cwd=cwd
    ).stdout


def _worktree_root() -> Path:
    out = _git("rev-parse", "--show-toplevel").strip()
    return Path(out) if out else Path.cwd()


def _merge_base(root: Path) -> str:
    return _git("merge-base", "origin/main", "HEAD", cwd=root).strip() or "HEAD~1"


def _local_change(root: Path) -> tuple:
    """(ChangeClass, added) for ahead-of-main COMMITS in this worktree (not the
    working tree — defeats commit-then-stash fakery)."""
    base = _merge_base(root)
    ns = _git("diff", "--name-status", base, "HEAD", cwd=root)
    diff = _git("diff", "--unified=0", base, "HEAD", cwd=root)
    return classify_changed_files(ns), added_lines(diff)


def _attestation(root: Path) -> str:
    p = root / ".dod" / "attestation.md"
    return p.read_text() if p.exists() else ""


def _gh_json(args: list):
    out = subprocess.run(["gh", *args], capture_output=True, text=True).stdout
    return json.loads(out) if out.strip() else None


# --------------------------------------------------------------------------- #
# CLI modes                                                                    #
# --------------------------------------------------------------------------- #


def run_stop() -> int:
    """Stop hook: block turn-end on an ahead-of-main code commit with unmet DoD.
    Debounced by a tree-hash so it nags once per unchanged tree (loop-safe)."""
    root = _worktree_root()
    class_, added = _local_change(root)
    if not class_.code_touched:
        print(json.dumps({}))
        return 0
    reason = bypass_reason(_attestation(root), os.environ)
    if reason:
        print(f"[dod] BYPASS (agent-layer): {_strip_control(reason)[:120]}", file=sys.stderr)
        print(json.dumps({}))
        return 0
    fails = blocking_violations(class_, added) + attestation_gaps(class_, _attestation(root))
    state = root / ".dod" / ".state"
    tree = _git("rev-parse", "HEAD^{tree}", cwd=root).strip()
    if fails and state.exists() and state.read_text().strip() == tree:
        print(json.dumps({}))  # already nagged for this exact tree
        return 0
    if fails:
        state.parent.mkdir(exist_ok=True)
        state.write_text(tree)
        msg = ("Definition-of-Done not met for this code branch — resolve, then "
               "continue (or set DOD_BYPASS=\"<reason>\"):\n" + render(fails))
        # Canonical Stop-block schema; the CI `definition-of-done` job is the
        # authoritative backstop if the harness schema ever drifts.
        print(json.dumps({"decision": "block", "reason": msg}))
        return 0
    print(json.dumps({}))
    return 0


def _deny(reason: str) -> int:
    print(json.dumps({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    }))
    return 0


# Merge via `gh pr merge` / `gh api .../merge` (linear regex). `git push` detection
# tolerating leading global options goes through `_git_subcommand_tokens` (the
# tokenizer) — a regex for it had a py/redos. Agent-layer; CI is the backstop.
_MERGE_RE = re.compile(r"\bgh\s+pr\s+merge\b|\bgh\s+api\b[^|&;]*?/merge\b")
_BYPASS_HINT = ' Override (agent-layer only; CI still gates): DOD_BYPASS="<reason>".'


def _current_pr_body(root: Path) -> str:
    """The open PR's body for the current branch, or '' if none / gh unavailable.
    The two-lens block lives in the PR body, so the merge gate reads it from here,
    NOT the last commit message (which essentially never carries the block)."""
    p = subprocess.run(["gh", "pr", "view", "--json", "body", "-q", ".body"],
                       cwd=root, capture_output=True, text=True)
    return p.stdout if p.returncode == 0 else ""


def run_gate_from_stdin() -> int:
    """PreToolUse(Bash): deny hook-skipping git ops, and gate merge/push on the
    blocking DoD. Unrelated Bash is untouched."""
    try:
        cmd = (json.load(sys.stdin).get("tool_input") or {}).get("command", "")
    except (json.JSONDecodeError, AttributeError):
        return 0
    if command_has_no_verify(cmd):
        return _deny("This repo bans --no-verify / SKIP_PREFLIGHT on git ops "
                     "(CLAUDE.md)." + _BYPASS_HINT)
    root = _worktree_root()
    is_merge = bool(_MERGE_RE.search(_strip_shell_quotes(cmd)))
    is_push = _has_push(cmd)
    if not (is_merge or is_push):
        return 0
    if bypass_reason(_attestation(root), os.environ):
        return 0  # agent-layer bypass; CI still gates
    class_, added = _local_change(root)
    fails = blocking_violations(class_, added)
    if is_merge:
        # the two-lens block lives in the PR body; fall back to the commit message
        # only when there is no open PR yet (best-effort).
        body = _current_pr_body(root) or _git("log", "-1", "--format=%B", cwd=root)
        if not two_lens_ok(body):
            fails = fails + [Violation("two-lens", "blocking", "PR body",
                                       "no Two-lens-review block with >=2 lenses — "
                                       "run the two-lens review before merge (CLAUDE.md)")]
    if fails:
        return _deny("DoD not met for " + ("merge" if is_merge else "push")
                     + ":\n" + render(fails) + _BYPASS_HINT)
    return 0


def run_local() -> int:
    """pre-push hook: blocking checks on ahead-of-main commits. Honors DOD_BYPASS
    (agent-layer); the CI job does not."""
    root = _worktree_root()
    class_, added = _local_change(root)
    reason = bypass_reason(_attestation(root), os.environ)
    if reason:
        print(f"[dod] BYPASS (agent-layer, pre-push): {_strip_control(reason)[:120]}", file=sys.stderr)
        return 0
    fails = blocking_violations(class_, added)
    warns = all_warnings(class_, added, "")
    if warns:
        print("[dod] warnings:\n" + render(warns), file=sys.stderr)
    if fails:
        print("[dod] BLOCKING — fix before push (or DOD_BYPASS=\"<reason>\"):\n"
              + render(fails), file=sys.stderr)
        return 1
    return 0


def run_pr(repo: str, pr: int) -> int:
    """CI `definition-of-done` job — the AUTHORITATIVE gate. Ignores DOD_BYPASS.
    Degrades to advisory (exit 0 + a VISIBLE notice) when gh can't return the PR
    data, so a transient API error never reds a PR — but it NEVER silently passes
    having checked nothing (the monitor-death class): a non-zero gh exit, an empty
    body, or an empty diff routes to the degraded branch with a loud notice, not a
    clean-looking pass."""
    try:
        vp = subprocess.run(["gh", "pr", "view", str(pr), "--repo", repo, "--json",
                             "body,headRefOid,baseRefName,commits"],
                            capture_output=True, text=True)
        dp = subprocess.run(["gh", "pr", "diff", str(pr), "--repo", repo],
                            capture_output=True, text=True)
    except Exception as e:  # noqa: BLE001 — degrade advisory, never red on glue
        print(f"#{pr}: DoD check DEGRADED (gh invocation error: {e}) — advisory pass.")
        return 0
    diff = dp.stdout
    if vp.returncode != 0 or dp.returncode != 0 or not vp.stdout.strip() or not diff.strip():
        print(f"#{pr}: DoD check DEGRADED — gh returned no data "
              f"(rc view={vp.returncode} diff={dp.returncode}: "
              f"{(vp.stderr or dp.stderr).strip()[:140]}) — advisory pass, NOTHING CHECKED.")
        return 0
    try:
        view = json.loads(vp.stdout)
    except json.JSONDecodeError as e:
        print(f"#{pr}: DoD check DEGRADED (unparseable gh JSON: {e}) — advisory pass.")
        return 0
    if not view.get("headRefOid"):
        print(f"#{pr}: DoD check DEGRADED (no headRefOid in gh JSON) — advisory pass.")
        return 0
    body = view.get("body") or ""
    commit_msgs = "\n".join(
        (c.get("messageHeadline", "") + "\n" + c.get("messageBody", ""))
        for c in view.get("commits", [])
    )
    # Derive A/M/D status from the diff itself (one fewer gh call, and the real
    # status feeds the added-module/feature_shaped classifier — synthesizing all-`M`
    # blinded it in CI).
    class_ = classify_changed_files(name_status_from_diff(diff))
    added = added_lines(diff)
    fails = blocking_violations(class_, added)
    # merge-scope: a code PR must carry the two-lens block + (warn) a ledger trace
    if class_.code_touched and not two_lens_ok(body):
        fails.append(Violation("two-lens", "blocking", "PR body",
                               "no Two-lens-review block with >=2 distinct lenses "
                               "+ verdict (CONTRIBUTING.md)"))
    warns = all_warnings(class_, added, body, commit_msgs)
    if class_.code_touched and not ledger_touched(class_):
        warns.append(Violation("ledger", "warn", "docs/REVIEW-LEDGER.md",
                               "no ledger trace touched — every review adjudication "
                               "leaves a trace (CLAUDE.md)"))
    print(f"#{pr}: DoD — {len(fails)} blocking, {len(warns)} warn "
          f"(class: code={class_.code_touched} docs_only={class_.docs_only})")
    if fails or warns:
        print(render(fails + warns))
    return verdict(fails)


def main() -> int:
    ap = argparse.ArgumentParser(description="Definition-of-Done gate.")
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--stop", action="store_true")
    ap.add_argument("--gate-from-stdin", action="store_true")
    ap.add_argument("--local", action="store_true")
    ap.add_argument("--pr", type=int)
    ap.add_argument("--judge-prompt", action="store_true")
    ap.add_argument("--repo", default=DEFAULT_REPO)
    args = ap.parse_args()
    if args.selftest:
        sys.path.insert(0, str(Path(__file__).resolve().parent))
        import check_dod_selftest as st
        return st.run()
    if args.stop:
        return run_stop()
    if args.gate_from_stdin:
        return run_gate_from_stdin()
    if args.local:
        return run_local()
    if args.judge_prompt:
        if args.pr is not None:
            try:
                view = _gh_json(["pr", "view", str(args.pr), "--repo", args.repo,
                                 "--json", "body"]) or {}
                diff = subprocess.run(["gh", "pr", "diff", str(args.pr), "--repo", args.repo],
                                      capture_output=True, text=True).stdout
                print(judge_prompt(diff, view.get("body") or ""))
            except Exception:  # noqa: BLE001 — advisory; never crash the judge step
                print(judge_prompt("", ""))
        else:
            root = _worktree_root()
            base = _merge_base(root)
            print(judge_prompt(_git("diff", base, "HEAD", cwd=root), _attestation(root)))
        return 0
    if args.pr is not None:
        return run_pr(args.repo, args.pr)
    ap.error("pass a mode: --pr N | --local | --stop | --gate-from-stdin | --judge-prompt | --selftest")


if __name__ == "__main__":
    sys.exit(main())
