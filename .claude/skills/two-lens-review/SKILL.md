---
name: two-lens-review
version: 1.1.0
description: "Run pixtuoid's review protocol at either scope — the mandatory pre-merge DIFF gate (2+ differentiated-lens agents on the diff) or a whole-codebase AUDIT (subsystem × factor fan-out over the whole tree). Both draw ONE shared factor taxonomy + verify contract + disposition; they differ only in population and orchestration. Use before merging ANY PR, on 'review this PR/branch' / 'is this ready to merge' (diff scope), or on 'whole-codebase review' / pre-release / periodic audit (whole-codebase scope). Encodes the five hard requirements, the escalation triggers, the adversarial finder→verify fan-out, and the disposition sweep the repo learned the hard way."
metadata:
  scope: "pixtuoid repo only"
---

# two-lens-review (v1.1) — the review gate + the whole-codebase audit

ONE protocol, two SCOPES over the SAME factors:

- **Diff scope** — the repo's **mandatory** merge gate ("Don't merge a PR without
  the two-lens review" — workspace `CLAUDE.md`, "Things NOT to do"; PR #23 merged
  unreviewed with a critical path-traversal). 2+ differentiated-lens agents on the
  diff, disposition in the PR thread.
- **Whole-codebase scope** — the periodic / pre-release AUDIT. A diff review and
  an audit scan DIFFERENT populations (fix-introduced-in-one-change vs existing
  code + cross-PR accumulation), so the audit is a SEPARATE pass, not a bigger
  PR review — but it runs the same factors, verify contract, and disposition.

The factors, the fill-in-the-slots lens briefs, the five hard requirements, the
escalation triggers, AND the whole-codebase fan-out orchestration are all
canonical in
[`.github/prompts/pr-review.prompt.md`](../../../.github/prompts/pr-review.prompt.md) —
**read it; fill from THAT file, never a paraphrase here** (a copy here is the exact
two-copies-drift class Lens 2 hunts — when the prompt gains a factor or trigger, a
copy here silently lags). This skill owns only *when* to invoke each scope, *how*
to orchestrate, and the red-flag self-checks.

## When to use

**Diff scope:**
- Before merging any PR (no exceptions — it's the gate, not a nicety; no size
  exemption — lens count can shrink, the gate can't).
- User says "review this branch/PR", "two-lens review", "is this ready to merge".
- After a fix round, to re-review the new head before merge.

**Whole-codebase scope:**
- User says "whole-codebase review" / "audit the repo"; a pre-release or milestone
  sweep; a periodic drift/design-debt pass.
- NOT the per-PR gate — that's the diff scope above.

Two agents MINIMUM (diff scope), lenses **differentiated** (a shared lens makes
their misses re-correlate); lens/finder count scales with blast radius (or tree
size). The quality lever is never the lens NAME — it's the change-specific
checklist filled into the `<...>` slots, and the FACTOR COVERAGE (no family
silently dropped).

## Diff scope — how to run (orchestration)

1. **Isolate**: the reviewed branch in a worktree (never the shared checkout —
   two sessions on one tree race on HEAD). Note `path`, `branch`, `base` sha.
2. **Dispatch both lenses in parallel, in the background**, each a subagent with
   its brief from `pr-review.prompt.md`, `<...>` slots FILLED with this change's
   specific claims (a lazily-filled slot turns both reviewers generic). Give each
   the worktree path + `git -C <path> diff <base>..HEAD`. Then add an escalation
   lens for EVERY trigger the prompt's "When two lenses aren't enough" section
   matches on this change — that trigger→lens list is canonical THERE; don't
   restate it here (a copy would be the two-copies-drift class the header names —
   a new trigger added to the prompt must reach reviews without a manual mirror).
3. **Collect + verify**: first read each lens's ACTUAL return before counting it
   toward the lens floor — a one-word summary or "test"/placeholder findings is a
   STUB (a dispatch, not a review); re-run that lens as a single focused agent
   (PR #455's a11y lens stubbed under an APPROVE-WITH-NITS aggregate; its re-run
   caught a real AA failure). Then for every MEDIUM+ finding, **verify the
   premise yourself before coding a fix** — reviewers have incomplete design
   context; check the crate's sharp edges first, and if a finding is deliberate
   design, REFUTE it by citing (or ADDING) the relevant `CLAUDE.md` sharp edge.
4. **Fold** accepted findings into ONE review-round commit; record any
   reviewer-flagged plan-misses as `plan-miss:` lines in its message.
5. **Disposition sweep** (shared, below).
6. **After a fix round**, re-run the gates and watch the NEW head's CI; before
   merging, read the online bot review's LATEST COMMENT verdict (`Findings: N`)
   + `mergeStateStatus` — the review JOB passes even when it posts findings, so
   the check table alone can't gate (#448). If the bot ERRORED or left no
   findings comment at HEAD (it can fail on a very large diff — `error_max_turns`
   with no comment), the gate is unsatisfiable as written: split the PR smaller,
   else fall back to one extra differentiated lens + owner merge, recorded in the
   PR thread. State the condition behaviorally (errored/absent), never a fixed
   LOC ceiling.

## Whole-codebase scope — how to run (orchestration)

The full fan-out template (subsystem finders + whole-tree specialist sweeps →
adversarial verify → dedup → ranked report) is the "Whole-codebase scope —
orchestration" section of `pr-review.prompt.md`. In brief:

1. **Scout** (main loop): map crates / LOC / churn / hot files → the work-list.
2. **Find**: fan out subsystem finders (per crate/module cluster) + whole-tree
   specialist sweeps (arch-invariants, concurrency/liveness, security, drift —
   the aggregate-only lenses). Each finder carries the FULL factor checklist.
   Prefer a `Workflow` (pipeline per cell); degrade to parallel `Agent` fan-out.
3. **Verify** each finding adversarially (default REFUTE; check sharp edges;
   construct a repro or refute) — a separate skeptic per finding, never the
   finder self-certifying.
4. **Dedup + rank** survivors; ship a report ranked by corrected severity,
   grouped by factor family, KEEPING the refuted-as-deliberate list (coverage
   proof + sharp-edge context for the next agent).
5. **Disposition sweep** (shared, below); end with the repo-wide stale-phrase
   `grep` == 0.

Scale to the ask: "any bugs?" → a few finders, single-vote verify; "thoroughly
audit / be comprehensive" → larger finder pool, multi-vote adversarial verify,
synthesis. Do the involved/cross-crate refactors it surfaces IN-ARC (design-debt
lens); defer only genuinely big/refactor work to issues.

## Disposition sweep (both scopes)

Drive every reviewer/finder/bot finding to **exactly one terminal state**:
**FIXED** · **REFUTED-with-trace** (cite or ADD the relevant per-crate `CLAUDE.md`
sharp edge — that keeps the next agent's context accurate) · **ISSUE-FILED**
(no-deferral rule: only big/refactor defers). "Acknowledged, no action" is NOT a
state — #40's ignored finding became a 0.4.1 blocker (#46). Diff scope: in the PR
thread. Whole-codebase scope: in the ranked report. Sweep at the FINAL merge
head — a finding that lands after the local lenses ran is the #283/#383 drop
class; and check WHICH commit a bot re-flag was raised against before
re-litigating (#316's were stale).

## Red flags (you're about to skip the gate / short the audit)

| Thought | Reality |
|---------|---------|
| "It's a tiny/doc-only PR" | The gate has no size exemption; run it (lens count can shrink, the gate can't). |
| "CI is green, that's enough" | CI can't see design, blast radius, drift, or a deliberate-looking real bug. |
| "The reviewer said X, so fix X" | Verify the premise first — check sharp edges; a wrong fix contradicts a design decision. |
| "One thorough agent is fine" | Two differentiated lenses is the floor; one lens's blind spots go uncaught. |
| "I'll note the finding and move on" | Every finding needs a terminal state — dropped findings become release blockers. |
| "The diff looks clean, we're done" (audit) | The diff scope can't see drift accumulation / design-debt accretion / arch erosion — those need the whole-codebase pass. |
| "The verdict row shows N lenses ran" | Count REAL returns, not dispatches — a stubbed lens under a clean aggregate hid a real AA failure (#455). |
| "The bot says it's still broken" | Check WHICH commit it reviewed — #316's re-flags were raised against an old commit; five were already fixed (REFUTED-STALE). |
| "The finder found it, report it" (audit) | Findings self-certify nothing — a separate skeptic must try to REFUTE each survivor first. |
| "Just unify the duplication" | Some duplication is documented deliberate separation (per-source decoders, per-CLI targets); check the sharp edge before proposing a merge. |
