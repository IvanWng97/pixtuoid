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
   defense-in-depth where a primary defense exists, pure style, and
   existence/version claims about external artifacts (GH Action tags, crate
   releases, sibling repos/taps) made from memory — verify via `gh api`/the
   registry IN THIS SESSION, or write "unverified" instead of asserting.
   A registry 404 observed now IS a finding; a recollection is not — reviews
   insisted a 12-day-old tap "doesn't exist" for 4 rounds (#112; the twin
   `checkout@v6` case: docs/review-metrics/mining-2026-06.md). Both existed.
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
1. <the change-specific claims to check, one per line — filled from the
   impl-plan brief's claims in the PR body when the change shipped with one
   (impl-plan.prompt.md) — e.g. "the staging math vs motion's bootstrap",
   "byte-identity of the refactor", "every cited PR/sharp-edge exists">
   For planned changes: a finding the plan never named is a plan-stage
   miss — flag it in your report.
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
5. Data-shape check on every NEW field, config key, map, or collection the
   diff introduces: name its identity/key-space. If it overlaps an existing
   structure's identity (two collections keyed by the same id; an attribute
   map shadowing an entity list), flag consolidation into one entity type —
   two facts about the same thing want one type, and the second attribute is
   the moment to create it. Do NOT demand merging orthogonal state that
   merely concerns the same entity (render caches, interaction state, scalar
   keys with disjoint key-spaces) — consolidate shared IDENTITY, not shared
   TOPIC. (The `[pet-names]` lesson, PR #86 — backtest-validated, controls
   included: docs/review-metrics/mining-2026-06.md.)
6. Duplication / DRY sweep on every NEW fn, type, helper, or const the diff
   introduces: `grep -rn`/`rg` the WHOLE tree for a pre-existing implementation
   of the same behavior. A diff shows only what's ADDED, so this is the one
   check that REQUIRES searching outside the diff — a second copy is invisible
   to a diff-scoped read by construction. Flag a new symbol whose body already
   exists elsewhere as "delegate, don't re-implement", weighting the finding by
   DIVERGENCE RISK (the two copies drifting apart is the real cost, not the
   line count). Distinct from #5: that is data-shape identity; this is
   behavioral/logic duplication. Smell-audit incidents a year of reviews
   missed: two `expand_tilde`s drifted into a Windows `~\` bug; `lerp_rgb` was a
   no-op wrapper renaming `mix_lab` (a cheap-sounding name fronting an expensive
   call — a LYING wrapper is the same finding); `Frame`/`RgbBuffer` each
   re-hand-rolled `Grid<T>`'s row-major buffer.

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
- **Diff touches `pixtuoid-hook` (the shim)** → run a never-panic audit on
  the WHOLE shim, not just the diff: `args_os()` not `args()` (non-UTF-8
  argv panics → exit 101, visible to CC), no slicing/indexing on untrusted
  bytes, every read bounded, every error path a silent `exit(0)`. Invariant
  #5 is the repo's most-documented contract, yet PR #198 added `env::args()`
  and both bot and local rounds missed it (caught post-merge, bae3541).
- **Motion / pose / walk-leg behavior changed** → render and WATCH it before
  the verdict: animated gif via the snapshot example, and/or replay a fixture
  through the binary (`scripts/replay-fixture.sh`) for resume/lifecycle
  motion. PR #61 was approved by per-phase + whole-feature code review (its
  "live run" test-plan checkbox left unchecked) and shipped five walk
  regressions, all visible within minutes of watching (fixed in #62,
  919ea7a). This fires even when no
  committed art changes: the film-critic trigger above covers shipped clips,
  and the lifecycle lens traces state, not pixels in motion.

Process notes for the orchestrator: dispatch both in parallel, in the
worktree, background; verify every MEDIUM+ finding's premise yourself before
coding a fix (reviewers have incomplete design context — check sharp edges
first); fold accepted findings into ONE review-round commit, recording any
reviewer-flagged plan-misses as `plan-miss:` lines in that commit's message
and carrying them into the squash body (the census harvests that channel).
Before merging, sweep
every reviewer/bot finding to exactly one terminal state — FIXED,
REFUTED-with-trace (ledger row if it will recur), ISSUE FILED (no-deferral
rule applies: only big/refactor work defers), or ACCEPTED-residual with its
WHY documented in code or a ledger row (the ledger's verdict vocabulary
governs). "Acknowledged, no action" is not a state: #40's ignored migration
finding became a 0.4.1 release-blocker (#46); two more drop cases:
docs/review-metrics/mining-2026-06.md. After a fix
round, re-run the gates and watch the NEW head's CI.

Whole-codebase reviews (the finder → dedup → ledger-routing → verification
pipeline; worked design: docs/review-metrics/phase2-ab-2026-06.md)
additionally: finder briefs do NOT carry the ledger — unbiased candidate
recall is what keeps the ledger's calibration measurable; the ledger enters
at routing/verification only. Until a run records the demote path's
false-suppression rate (status lives in that report or its successor —
phase 2 never fired a demotion), verify A/B per that design: a control arm
of full pairs on ALL candidates — which doubles the run's verification
spend and doubles as protocol step 8's ledger-blind calibration — alongside
the ledger-routed treatment arm. Once the rate is recorded, delete this
A/B clause: routing alone is the steady state. Whole-codebase reviews also
carry a SYSTEM lens — module decomposition still right, dependency
directions clean, cross-PR composition seams, AND a DRY/duplication census —
because architecture (and duplication) erodes BETWEEN PRs, not within them
(no per-PR lens can see it; the census's "emergent cross-PR composition"
bucket — a NON-escape class precisely because no per-PR review could have
caught it — is its bug-shaped form). The duplication census greps for N
implementations of ONE concept (a helper, type-shape, or constant
re-hand-rolled in K places) — each PR was locally clean, so only the
whole-tree pass sees the K copies; weight each cluster by divergence risk
(silently-drifted copies are the bug-shaped form, like the two `expand_tilde`s
that split into a Windows bug). Live cases a year of per-PR + whole-codebase
reviews missed until a burden-flipped smell pass found them: `Frame`/`RgbBuffer`
vs `Grid<T>`, the two `expand_tilde`s, `lerp_rgb` fronting `mix_lab`.
