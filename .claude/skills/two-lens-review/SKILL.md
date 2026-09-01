---
name: two-lens-review
version: 1.2.0
description: "Run pixtuoid's review protocol at either scope — the mandatory pre-merge DIFF gate (2+ differentiated-lens agents on the diff) or a whole-codebase AUDIT (subsystem × factor fan-out over the whole tree). Both draw ONE shared factor taxonomy + verify contract + disposition; they differ only in population and orchestration. Use before merging ANY PR, on 'review this PR/branch' / 'is this ready to merge' (diff scope), or on 'whole-codebase review' / pre-release / periodic audit (whole-codebase scope). Encodes the convergence contract (churn budget, two-fix-round cap, HIGH-only blocking), the five hard requirements, the escalation triggers, the adversarial finder→verify fan-out, and the disposition sweep the repo learned the hard way."
metadata:
  scope: "pixtuoid repo only"
---

# two-lens-review (v1.2) — the review gate + the whole-codebase audit

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

## Convergence contract (diff scope)

From the measured review history (derivation in this section's introducing
commit): under ~1500 lines of churn, PRs converge in 0–1 fix rounds; above
it, rounds 2+ are dominated by defects the PREVIOUS round's fixes introduced
and by reversals of already-settled calls — the loop generates its own work.
Hence:

- **Churn budget** — a diff whose ADDED + MODIFIED lines exceed ~1500 does
  not enter review; split it first (stacked PRs). Pure deletions are exempt
  from the count — a removed line ships no behavior for a lens to verify —
  but only when the census rule below is satisfied. A change that both adds
  and deletes at scale is two PRs.
- **Two fix rounds, hard cap.** Round 1: full review, all lenses, folded into
  ONE commit. Round 2: verify the dispositions + review ONLY the delta since
  round 1's head — no full re-sweep (full re-sweeps are where settled calls
  get re-litigated). If round 2 confirms a HIGH in round 1's fixes, STOP:
  revert the fold and re-land smaller, or re-scope the PR. There is no round
  3 of patching patches.
- **Round 2's fold is the last commit that may change behavior, and it is
  verified, not re-reviewed.** Every round-2 fix must be one of three shapes:
  a revert, a deletion, or a change that ships a test FAILING without it.
  Anything else is not a fix — revert the fold and re-land smaller. (Rounds
  2+ are dominated by fix-introduced defects; this keeps the one unreviewed
  commit from carrying one.)
- **Blocking bar** — only a HIGH (correctness / security / invariant)
  CONFIRMED BY THE ORCHESTRATOR against the code — never by the finder's own
  severity label — blocks merge. A MEDIUM this change INTRODUCED is fixed in
  the fold or forces a re-scope; taste findings are optional by default —
  drop them; a pre-existing find is SURFACED to the owner in one line. Nothing
  spawns another round, and agents never file issues.
- **No new gates in a fix round.** A fix may not introduce a new bespoke
  checker/lint/census — gate-shaped fixes routinely arrive fail-open and feed
  the next round. Prefer making the failure IMPOSSIBLE (derive from the one
  source of truth) over DETECTED (police two copies); a genuinely wanted new
  check becomes its own small PR through the design gate. A check
  asserts facts in its own layer — a Rust fact is checked from Rust, never a
  Python regex over `.rs` files.
- **Deletion-shaped PRs enumerate first.** Before deleting N members of a
  class, the population census (full list + criterion) lands in the FIRST
  commit or the PR body, before review starts — reviewers check the census
  once instead of restoring survivors one per round (#943).
- **The bot's `Findings: 0` is evidence, not the gate** — it can be vacuous
  (an errored run wearing a clean badge). The gate: every finding (local
  lenses + both bots) dispositioned, and zero OPEN confirmed HIGH at the
  final head.

## Diff scope — how to run (orchestration)

1. **Isolate**: the reviewed branch in a worktree (never the shared checkout —
   two sessions on one tree race on HEAD). Note `path`, `branch`, `base` sha.
2. **Dispatch both lenses in parallel, in the background**, each a subagent with
   its brief from `pr-review.prompt.md`, `<...>` slots FILLED with this change's
   specific claims (a lazily-filled slot turns both reviewers generic). Give each
   the worktree path + `git -C <path> diff <base>..HEAD`. Then add an escalation
   lens for EVERY trigger the prompt's "Escalation triggers" section
   matches on this change — that trigger→lens list is canonical THERE; don't
   restate it here (a copy would be the two-copies-drift class the header names —
   a new trigger added to the prompt must reach reviews without a manual mirror).
3. **Collect + verify**: first read each lens's ACTUAL return before counting it
   toward the lens floor — a one-word summary or "test"/placeholder findings is a
   STUB (a dispatch, not a review); re-run that lens as a single focused agent
   (PR #455's a11y lens stubbed under an APPROVE-WITH-NITS aggregate; its re-run
   caught a real AA failure). Then for every MEDIUM+ finding, **verify the
   premise yourself before coding a fix** — reviewers have incomplete design
   context; read the doc comment on the declaration the finding names first,
   and if a finding is deliberate design, REFUTE it with the MECHANISM that
   makes it so — a test, a compile-time constraint, a CI gate.
4. **Fold** accepted findings into ONE review-round commit; record any
   reviewer-flagged plan-misses as `plan-miss:` lines in its message.
5. **Disposition sweep** (shared, below).
6. **After a fix round**, re-run the gates and watch the NEW head's CI; before
   merging, read the online bot review's LATEST COMMENT verdict (`Findings: N`)
   + `mergeStateStatus` — the review JOB passes even when it posts findings, so
   the check table alone can't gate (#448). Judge the verdict against the
   convergence contract above: dispositioned findings + zero open confirmed
   HIGH, within the two-fix-round cap. If the bot ERRORED or left no
   findings comment at HEAD (it can fail on a very large diff — `error_max_turns`
   with no comment — or on a spent quota, which the workflow now states itself in
   an `<!-- absent-<marker>:<sha> -->` comment; do NOT read that as a review),
   the gate is unsatisfiable as written: split the PR smaller,
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
3. **Verify** each finding adversarially (default REFUTE; read the doc comment
   on the declaration it names; construct a repro or refute) — a separate skeptic per finding, never the
   finder self-certifying.
4. **Dedup + rank** survivors; ship a report ranked by corrected severity,
   grouped by factor family, KEEPING the refuted-as-deliberate list and the
   MECHANISM each was refuted by — that is the coverage proof.
5. **Disposition sweep** (shared, below); end with the repo-wide stale-phrase
   `grep` == 0.

Scale to the ask: "any bugs?" → a few finders, single-vote verify; "thoroughly
audit / be comprehensive" → larger finder pool, multi-vote adversarial verify,
synthesis. Do the involved/cross-crate refactors it surfaces IN-ARC (design-debt
lens); anything bigger is SURFACED in the ranked report for the owner to pick.

## Disposition sweep (both scopes)

Drive every reviewer/finder/bot finding to **exactly one terminal state**:
FIXED · REFUTED-with-trace (cite the MECHANISM that refutes it — a test, a
compile-time constraint, a CI gate; ADD one where none exists, never prose) ·
RE-SCOPED · SURFACED — the four states are defined ONCE in
`pr-review.prompt.md`'s orchestrator notes; fill from there, never a
paraphrase here. Agents never file issues. "Acknowledged, no action" is NOT
a state — #40's ignored finding became a 0.4.1 blocker (#46). Diff scope: in the PR
thread. Whole-codebase scope: in the ranked report. Sweep at the FINAL merge
head — a finding that lands after the local lenses ran is the #283/#383 drop
class; and check WHICH commit a bot re-flag was raised against before
re-litigating (#316's were stale).

## Red flags (you're about to skip the gate / short the audit)

| Thought | Reality |
|---------|---------|
| "It's a tiny/doc-only PR" | The gate has no size exemption; run it (lens count can shrink, the gate can't). |
| "CI is green, that's enough" | CI can't see design, blast radius, drift, or a deliberate-looking real bug. |
| "The reviewer said X, so fix X" | Verify the premise first — read the declaration's doc comment; a wrong fix contradicts a design decision. |
| "One thorough agent is fine" | Two differentiated lenses is the floor; one lens's blind spots go uncaught. |
| "I'll note the finding and move on" | Every finding needs a terminal state — dropped findings become release blockers. |
| "The diff looks clean, we're done" (audit) | The diff scope can't see drift accumulation / design-debt accretion / arch erosion — those need the whole-codebase pass. |
| "The verdict row shows N lenses ran" | Count REAL returns, not dispatches — a stubbed lens under a clean aggregate hid a real AA failure (#455). |
| "The bot says it's still broken" | Check WHICH commit it reviewed — #316's re-flags were raised against an old commit; five were already fixed (REFUTED-STALE). |
| "The finder found it, report it" (audit) | Findings self-certify nothing — a separate skeptic must try to REFUTE each survivor first. |
| "One more fix round will converge" | Measured: rounds ≥2 mostly find defects the fixes introduced + re-litigate settled calls. Two fix rounds is the cap — revert or re-scope. |
| "This fix needs its own new checker" | Gate-shaped fixes arrive fail-open and feed the next round. Derive from the source of truth, or file the checker as its own PR. |
| "Just unify the duplication" | Some duplication is documented deliberate separation (per-source decoders, per-CLI targets); read the declaration's doc comment before proposing a merge. |
| "The fold is committed — the bots will catch the rest" | Bots review the PR head, not the dispositions; a fold above ~100 behavioral lines is the one unreviewed commit — dispatch the round-2 delta verify the moment it lands, in parallel with the push. |
