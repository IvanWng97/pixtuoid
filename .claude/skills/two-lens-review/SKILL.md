---
name: two-lens-review
version: 1.0.0
description: "Run pixtuoid's mandatory pre-merge two-lens PR review — 2+ parallel agents on the diff with differentiated lenses (correctness/grounding + design/blast-radius), then drive every finding to a terminal state. Use before merging ANY PR or when the user says 'review this PR/branch', 'two-lens review', or 'is this ready to merge'. Encodes the five hard requirements, the escalation triggers (shim / motion / reducer / generated art / public-facing), and the disposition sweep that the repo learned the hard way."
metadata:
  scope: "pixtuoid repo only"
---

# two-lens-review (v1)

The repo's **mandatory** merge gate: "Don't merge a PR without the two-lens
review" (workspace `CLAUDE.md`, "Things NOT to do" — PR #23 merged unreviewed
with a critical path-traversal). This skill is the *orchestration* of that gate.
The canonical fill-in-the-slots **brief templates live in
[`.github/prompts/pr-review.prompt.md`](../../../.github/prompts/pr-review.prompt.md)** —
read it; this skill tells you how to RUN it and not skip the parts that bite.

## When to use

- Before merging any PR (no exceptions — it's the gate, not a nicety).
- User says "review this branch/PR", "two-lens review", "is this ready to merge".
- After a fix round, to re-review the new head before merge.

## The briefs are canonical — copy them, don't restate them

The **five hard requirements**, the **two lens briefs** (correctness/grounding +
design/blast-radius, with their `<...>` slots), and the **escalation triggers**
(generated art → film-critic, state-machine/concurrency → lifecycle, public-facing
→ editorial, `pixtuoid-hook` → whole-shim never-panic, motion → render-and-watch)
all live in [`.github/prompts/pr-review.prompt.md`](../../../.github/prompts/pr-review.prompt.md).
**Fill the lens briefs from THAT file, never from memory or a paraphrase here** —
duplicating them into this skill is the exact two-copies-drift class lens 2 hunts
(when the prompt gains a sixth requirement or a new trigger, a copy here silently
lags). This skill owns only the parts the prompt covers as loose prose: *when* to
invoke, *how* to orchestrate, and the red-flag self-checks below.

Two agents MINIMUM, lenses **differentiated** (a shared lens makes their misses
re-correlate); lens count scales with blast radius (two is the floor, not the law).

## How to run it (orchestration)

1. **Isolate**: the reviewed branch in a worktree (never the shared checkout —
   two sessions on one tree race on HEAD). Note `path`, `branch`, `base` sha.
2. **Dispatch both lenses in parallel, in the background**, each a subagent with
   its brief from `pr-review.prompt.md`, `<...>` slots FILLED with this change's
   specific claims (a lazily-filled slot turns both reviewers generic). Give each
   the worktree path + `git -C <path> diff <base>..HEAD`.
3. **Collect + verify**: for every MEDIUM+ finding, **verify the premise yourself
   before coding a fix** — reviewers have incomplete design context; check the
   crate's sharp edges first, and if a finding is deliberate design, REFUTE it by
   citing (or ADDING) the relevant `CLAUDE.md` sharp edge.
4. **Fold** accepted findings into ONE review-round commit; record any
   reviewer-flagged plan-misses as `plan-miss:` lines in its message.
5. **Disposition sweep — drive every reviewer/bot finding to exactly one terminal
   state in the PR thread**: FIXED · REFUTED-with-trace (cite/add the sharp edge)
   · ISSUE-FILED (no-deferral rule: only big/refactor defers). "Acknowledged, no
   action" is NOT a state — #40's ignored finding became a 0.4.1 blocker (#46).
6. **After a fix round**, re-run the gates and watch the NEW head's CI.

## Red flags (you're about to skip the gate)

| Thought | Reality |
|---------|---------|
| "It's a tiny/doc-only PR" | The gate has no size exemption; run it (lens count can shrink, the gate can't). |
| "CI is green, that's enough" | CI can't see design, blast radius, or a deliberate-looking real bug. |
| "The reviewer said X, so fix X" | Verify the premise first — check sharp edges; a wrong fix contradicts a design decision. |
| "One thorough agent is fine" | Two differentiated lenses is the floor; one lens's blind spots go uncaught. |
| "I'll note the finding and move on" | Every finding needs a terminal state — dropped findings become release blockers. |
