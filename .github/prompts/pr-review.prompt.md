# Review briefs — the factor taxonomy + the two-scope protocol

Canonical for BOTH review scopes — they share ONE set of factors and differ only
in POPULATION + orchestration:

- **Diff scope** — the mandatory pre-merge gate (workspace `CLAUDE.md`, "Don't
  merge a PR without the two-lens review"): 2+ differentiated-lens agents on the
  diff, disposition in the PR thread.
- **Whole-codebase scope** — the periodic / pre-release AUDIT: subsystem × factor
  fan-out over the WHOLE tree, ranked report.

The FACTORS below are the shared "points" — every one matters to both scopes
(a diff review checks each on the changed lines; an audit fans out over all of
them × subsystems). Adding a factor here upgrades BOTH flows at once; that shared
source is the whole reason these live in one file (a per-scope copy would drift —
the exact two-copies class Lens 2 hunts). Fill the `<...>` slots; keep the five
hard requirements — each is there because its absence measurably hurt
(false-positive rates, a 0–1 confidence-scale incident, re-litigated verdicts).

## The factor taxonomy (shared by both scopes)

Four families. A diff-scope lens bundles several of these per agent (scaled to
blast radius); a whole-codebase run gives each family/factor its own finder or
sweep. Neither scope may silently DROP a family — if a factor doesn't apply to a
given change/tree, say so, don't skip it.

- **(A) Correctness + architecture** — logic/off-by-one/inverted-condition;
  concurrency & liveness (races, lock-order, lost wakeups, stale state, the
  no-scan-the-history rule); error-handling & silent failure; security (path
  traversal, unchecked wire input, config-write safety, terminal-egress control
  chars); the architecture invariants (core=no-terminal, scene=no-window, ONE
  Transport-tagged channel, walkable=footprint, no direct settings.json write,
  no prod `println!`, `unreachable_pub`); magic-number / single-source-of-truth;
  cross-platform (path-string separators, Windows parity); performance (per-frame
  allocs, hot-path scans); resource/lifecycle (Drop, fd leaks, unbounded growth);
  test-teeth (does the pinning test FAIL if the behavior breaks — mutate it).
- **(B) Design-debt** — duplication/DRY (N implementations of one concept —
  weight by DIVERGENCE risk, not line count); god-object / oversized module with
  a clean split; dead code / legacy remnant; leaky or missing abstraction;
  inconsistent pattern where one way is clearly the house style.
- **(C) Drift** — doc↔code (a `CLAUDE.md`/README/SKILL naming a file/fn/flag/count
  that moved); wire-format / upstream (a decoder or drift-watch rule vs the real
  upstream shape; a new source with no drift-watch row); version-lockstep
  (Cargo.toml versions, MSRV, the "N sites must stay in sync" invariants);
  comment-rot; manifest-bridge (a `site/src/*.json` / generated schema vs its
  Rust source of truth).
- **(D) Quality + tooling** — test-coverage gaps (changed/existing code with no
  exercising test); mutation-teeth (assertions that survive the mutation);
  isolation & flakiness (real-state writes, wall-clock/order nondeterminism,
  `TEST_ENV_LOCK`, snapshot determinism); CI/build (gate coverage, path-filter
  holes, toolchain skew); dependency/supply-chain (unmaintained/droppable deps,
  duplicate versions, feature-flag hygiene).

## The two populations (why scope matters)

A diff review and a whole-codebase audit scan DIFFERENT populations: the diff
sees *fix-introduced* issues inside one change; the audit sees *existing* code as
a whole. Every factor applies to both — but a handful exist ONLY at whole-tree
scale and are structurally invisible to a diff-scoped read: cross-PR emergent
(A×B interacting, neither diff wrong alone), doc-drift ACCUMULATION (no single PR
owns the stale line), design-debt ACCRETION (each PR adds one parallel copy),
coverage-topology gaps (each PR tests its own lines; the seams go untested),
arch-invariant EROSION (every diff conforms; the boundary weakens in aggregate),
orphaned surface (the diff that removed the last caller looked clean). The diff
scope catches these per-CHANGE; only the whole-codebase scope sees the AGGREGATE.
So a diff lens still runs the DRY / drift / data-shape checks below (they
`grep`/`rg` OUTSIDE the diff — the one thing a diff-scoped read can't do by
construction), and the audit adds finders dedicated to the aggregate-only classes.

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
4. **Sharp-edge check** — match familiar-smelling claims against the
   per-crate `CLAUDE.md` "Known sharp edges" (the live, maintained record of
   deliberate-design refutations; premise-anchored: same seam ≠ same claim).
   `docs/REVIEW-LEDGER.md` is a frozen archive you may skim for older
   adjudications, but it is no longer required reading.
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
7. Layering / orchestration boundary: when the diff adds a call into a
   lower/mechanism layer (config-write, install, FS, a foundation helper) or
   newly exposes one, check it routes THROUGH the layer's designated
   orchestrator, not around it. Flag (a) a NEW `pub` item that exposes a
   foundation/underlayer seam the orchestrator should own, and (b) a NEW call
   site that reaches the mechanism directly instead of the facade. The
   single-gateway rule: install/uninstall are `pub(crate)`, `crate::sources`
   is the SOLE caller — a second direct caller (or a `pub` that invites one)
   is the finding, even when it compiles cleanly. `unreachable_pub` (CI
   `-D warnings`) is the mechanical half — a `pub` in a PRIVATE module tree;
   this lens owns the half the lint can't see: a reachable-but-should-be-
   funnelled API, where the right fix is "demote to `pub(crate)` and call the
   orchestrator," not "leave it public."

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
reviewer-flagged plan-misses as `plan-miss:` lines in that commit's message.
Before merging, drive every reviewer/bot finding to exactly one terminal
state IN THE PR THREAD — FIXED, REFUTED-with-trace (if it's deliberate
design, cite or ADD the relevant per-crate `CLAUDE.md` sharp edge), or
ISSUE-FILED (no-deferral rule applies: only big/refactor work defers).
"Acknowledged, no action" is not a state: #40's ignored migration finding
became a 0.4.1 release-blocker (#46); two more drop cases:
docs/review-metrics/mining-2026-06.md. After a fix round, re-run the gates
and watch the NEW head's CI.

---

## Whole-codebase scope — orchestration

The audit applies the SAME factor taxonomy + five hard requirements + verify
contract + disposition as the diff scope; only the population (the whole tree)
and the fan-out change. Shape (prescribed; degradable to sequential/parallel
`Agent` fan-out when `Workflow` is unavailable):

1. **Scout** (main loop): map crates / LOC / churn / hot files → the work-list.
2. **Find** — fan out, each finder carrying the full factor checklist:
   - *Subsystem finders*, one per crate/module cluster (core: decoders /
     liveness+watcher / reducer+state; scene: painter / motion+layout /
     theme+misc; binary: install / runtime+tui / widgets+floating; hook+web+
     tooling).
   - *Whole-tree specialist sweeps*, one FACTOR each across all crates —
     arch-invariants, concurrency/liveness seams, security, drift — the
     aggregate-only lenses a per-subsystem finder structurally can't run.
3. **Verify — adversarially, default REFUTE.** Each finding gets an independent
   skeptic prompted to refute it: read the cited code + callers, check it against
   the per-crate `CLAUDE.md` "Known sharp edges" (a documented sharp edge → REFUTE
   and cite it), and construct a concrete repro (inputs/state → wrong outcome) or
   refute. A slightly-off `file:line` is not grounds to refute a real defect —
   locate the real line. (Unbiased candidate recall first, verification second:
   many finders, then the skeptics — never one pass doing both.)
4. **Dedup + rank** the survivors (a defect two cells find is one finding); ship a
   report ranked by corrected severity, grouped by factor family, and KEEP the
   refuted-as-deliberate list in it — it proves coverage and keeps the next
   agent's sharp-edge context accurate.
5. **Disposition sweep** — the shared terminal-state rule below (FIXED /
   REFUTED-with-sharp-edge / ISSUE-FILED); apply small fixes in-arc, defer only
   big/refactor; end with the repo-wide stale-phrase `grep` == 0.

The audit additionally carries a **SYSTEM lens** (module decomposition still
right, dependency directions clean, cross-PR composition seams) **and a
DRY/duplication census** — because architecture (and duplication) erodes BETWEEN
PRs, not within them (no per-PR lens can see it; the census's "emergent cross-PR
composition" bucket — a NON-escape class precisely because no per-PR review could
have caught it — is its bug-shaped form). The duplication census greps for N
implementations of ONE concept (a helper, type-shape, or constant re-hand-rolled
in K places) — each PR was locally clean, so only the whole-tree pass sees the K
copies; weight each cluster by divergence risk (silently-drifted copies are the
bug-shaped form, like the two `expand_tilde`s that split into a Windows bug). Live
cases a year of per-PR + whole-codebase reviews missed until a burden-flipped
smell pass found them: `Frame`/`RgbBuffer` vs `Grid<T>`, the two `expand_tilde`s,
`lerp_rgb` fronting `mix_lab`.
