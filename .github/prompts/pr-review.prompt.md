# PR review briefs — the two-lens protocol

Canonical templates for the mandatory 2-agent review (workspace `CLAUDE.md`,
"Don't merge a PR without the two-lens review"). Fill the `<...>` slots; keep
the five hard requirements — each one is there because its absence measurably
hurt (false-positive rates, a 0–1 confidence-scale incident, re-litigated
verdicts).

Both briefs MUST carry, verbatim or equivalent:

1. **Reasoning before verdict** — for every finding, state the trace/evidence
   FIRST, then the claim.
2. **Negative space** — do NOT flag: behavior documented as a sharp edge in
   any `CLAUDE.md` (read the nested file for the crate under review first),
   theoretical risks requiring unlikely preconditions, absence of
   defense-in-depth where a primary defense exists, pure style.
3. **Integer confidence 0–100 + `file:line`** on every finding.
4. **Ledger check** — match familiar-smelling claims against
   `docs/REVIEW-LEDGER.md` (its header protocol governs; premise-anchored:
   same seam ≠ same claim).
5. **Verdict** — exactly one of APPROVE / APPROVE-WITH-NITS / REQUEST-CHANGES.

---

## Lens 1 — correctness / grounding

```
You are reviewer 1/2 (correctness lens) for <PR/branch> on pixtuoid.
Worktree: <path> (branch <name>, base <sha>). Diff: git -C <path> diff <base>..HEAD.

Verify rigorously (read the actual code, not just the diff):
1. <the change-specific claims to check, one per line — e.g. "the staging math
   vs motion's bootstrap", "byte-identity of the refactor", "every cited
   PR/sharp-edge exists">
2. House rules on touched code: no unwrap() outside tests, tracing not
   println, comments WHY-only, docs-currency (CLAUDE.md/README updated when
   public surface moved).
3. Tests don't lie: for every behavioral claim, check the pinning test would
   FAIL if the behavior broke (mentally mutate the fix; a test that survives
   deletion of the guarded constant pins nothing — the CONN_TIMEOUT lesson,
   ledger R0610-06).
4. Run the gates yourself: `just <fmt-check|site-check|preflight>` as
   applicable — do not trust the author's claim of green. Include the EXIT
   CODE you observed (never infer it through a pipe).

[the five hard requirements]
Your final message is the report.
```

## Lens 2 — design / blast-radius

```
You are reviewer 2/2 (design lens) for <PR/branch> on pixtuoid.
Worktree: <path>, read-only. Diff: git -C <path> diff <base>..HEAD.

Judge as a demanding critic:
1. <the design questions, one per line — e.g. "does the caption oversell the
   still", "is the channel order right", "is the protocol executable by the
   next agent who has only this file">
2. Downstream interactions: who consumes the changed surface; trace at least
   the two nearest consumers (code or docs) for contradiction.
3. Copy/docs sweep of everything new (typos, overclaims, undefined notation).
4. Propose concrete replacement text where you object — a finding without a
   suggested fix is half a finding.

[the five hard requirements]
Your final message is the report.
```

---

## When two lenses aren't enough

Two is the floor, not the law — lens count scales with blast radius. The
quality lever is never the lens NAME; it's the change-specific checklist
filled into the `<...>` slots (a lazily-filled slot turns both reviewers
generic, and their misses re-correlate). Escalation triggers from this repo's
history:

- **Generated art / clips ship** → add a film-critic lens: extract frames
  (1 fps + dense around key moments), READ them, census the money shot
  (the south-seat occlusion and the crop-edge fixture were both frame-census
  catches).
- **State machine / concurrency seam touched** (reducer, liveness ladder,
  motion) → add a lifecycle lens that traces the downstream interaction
  graph (rebind, sweeps, TTLs) rather than the diff.
- **Public-facing artifact** (site page, README section, release notes) →
  add an editorial lens reading as an outside engineer, checking every
  number against its source.

Process notes for the orchestrator: dispatch both in parallel, in the
worktree, background; verify every MEDIUM+ finding's premise yourself before
coding a fix (reviewers have incomplete design context — check sharp edges
first); fold accepted findings into ONE review-round commit; deferred-but-real
findings get a GitHub issue (no-deferral rule applies: only big/refactor work
defers). After a fix round, re-run the gates and watch the NEW head's CI.
