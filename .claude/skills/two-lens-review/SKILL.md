---
name: two-lens-review
version: 2.0.0
description: "Run pixtuoid's review locally at either scope — a DIFF review (the correctness + design lenses as parallel agents, optional pre-flight before the bots, mandatory for REVIEW.md's local escalation rows: render-and-watch motion, drive a page, live CLI runs) or a whole-codebase AUDIT (subsystem × factor fan-out). Every rule lives in REVIEW.md; this skill only orchestrates: dispatch, premise verify, the fold, the disposition threads. Use on 'review this PR/branch', 'is this ready to merge', a diff touching a local-only row, or 'whole-codebase review' / pre-release / periodic audit."
metadata:
  scope: "pixtuoid repo only"
---

# two-lens-review — local orchestration of REVIEW.md

Every rule — checks, lenses, escalation rows, severity, dispositions, the
convergence contract, the output shape — is [`REVIEW.md`](../../../REVIEW.md)'s.
Fill briefs from THAT file, never a paraphrase here.

## When to run

- **Diff scope**, optional pre-flight: before marking a PR ready, to spend the
  bots' round on what a local pass can't catch.
- **Diff scope**, mandatory: the diff matches a REVIEW.md escalation row
  marked **local**.
- **Whole-codebase scope**: "audit the repo", a pre-release or periodic sweep.
  A diff review and an audit scan different populations, so the audit is its
  own pass.

## Diff scope

1. **Isolate** the branch in a worktree (two sessions on one tree race on
   HEAD). Note `path`, `branch`, `base` sha.
2. **Dispatch** both lenses in parallel, in the background, plus one lens per
   matching escalation row, each briefed:

   ```
   You are the <correctness|design|row> lens for <PR/branch> on pixtuoid.
   Worktree: <path> (branch <name>, base <sha>). Diff: git -C <path> diff <base>..HEAD.
   Read AGENTS.md, then apply REVIEW.md's <lens or row>.
   <change-specific claims or design questions, one per line>
   Your final message is the report.
   ```

   The filled slots are the quality lever; a lazily filled one turns every
   lens generic.
3. **Collect and verify.** A one-word or placeholder return is a STUB, not a
   review: re-run that lens alone (#455). Verify every finding's premise
   yourself before coding a fix — read the comments on the item it names
   first — and REFUTE deliberate design with its mechanism.
4. **Fold** accepted findings into ONE commit, with a `plan-miss:` line per
   finding the plan never named.
5. **Disposition** each finding in a review thread. The bots open theirs; open
   each local finding the same way, then reply and resolve:

   ```sh
   pr=<N>; head=$(gh pr view $pr --json headRefOid -q .headRefOid)
   # Off the diff's lines: `-f subject_type=file` instead of line + side; a path
   # outside the diff (or removed by it) goes on the first surviving changed file,
   # its location in the body.
   gh api repos/{owner}/{repo}/pulls/$pr/comments -f commit_id=$head \
     -f path=<path> -F line=<n> -f side=RIGHT -F body=@<finding file>
   threads='query($owner:String!,$name:String!,$n:Int!){repository(owner:$owner,name:$name){pullRequest(number:$n){reviewThreads(first:100){totalCount nodes{id isResolved isOutdated path line comments(first:50){nodes{body}}}}}}}'
   gh api graphql -F owner='{owner}' -F name='{repo}' -F n=$pr -f query="$threads"
   gh api graphql -f t=<thread id> -F b=@<disposition file> \
     -f query='mutation($t:ID!,$b:String!){addPullRequestReviewThreadReply(input:{pullRequestReviewThreadId:$t,body:$b}){comment{id}}}'
   gh api graphql -f t=<thread id> \
     -f query='mutation($t:ID!){resolveReviewThread(input:{threadId:$t}){thread{isResolved}}}'
   ```

6. **Before merge**, read each bot's LATEST comment at HEAD (`Findings: N`) and
   `mergeStateStatus`: the review job passes even when it posts findings, so
   the check table can't gate. Judge against REVIEW.md's gate.

## Whole-codebase scope

Population = the tree, where the aggregate-only classes live (cross-PR
interaction, drift, debt accretion, coverage-topology gaps, invariant erosion,
orphaned surface). Prefer a `Workflow`, else parallel `Agent`s; scale to the
ask. Scout the work-list → subsystem finders (site and Raycast RENDERED) plus
per-factor finders (arch invariants · concurrency/liveness · security threat
model · performance · mutation depth on hot logic · silent failure · drift ·
deep modules) plus a SYSTEM lens (decomposition, dependency directions) and a
DRY census → adversarial verify, default REFUTE, one skeptic per finding and
2–3 differentiated ones for security/concurrency, majority confirms →
loop until two consecutive rounds come back empty → a completeness critic
("what modality did we NOT run?" seeds the next round) → dedup and rank,
keeping the refuted-as-deliberate list → dispositions plus a stale-phrase
sweep (`rg --hidden`) at 0. Involved refactors land in-arc; each bigger item
the owner keeps becomes a FOLLOW-UP PR.

## Red flags

| Thought | Reality |
|---------|---------|
| "CI is green, that's enough" | CI can't see design, blast radius, drift, or a deliberate-looking real bug. |
| "The bots will render it" | A bot reads the diff and the base tree; a local row's run is yours. |
| "I'll note the finding and move on" | Every finding needs a terminal state. |
| "The diff looks clean, we're done" (audit) | Drift, debt accretion and erosion accumulate between PRs; only the audit sees them. |
| "The finder found it, report it" (audit) | A separate skeptic tries to REFUTE each survivor first. |
| "Just unify the duplication" | Some duplication is documented deliberate separation; read the item's comments first. |
| "The fold is committed — the bots will catch the rest" | A fold above ~100 behavioral lines is the one unreviewed commit: dispatch the round-2 delta verify when it lands. |
