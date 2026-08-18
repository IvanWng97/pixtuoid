# Review briefs — the factor taxonomy + the two-scope protocol

Canonical for BOTH scopes of `two-lens-review` (diff gate / whole-codebase
audit): the factors, the five hard requirements, the lens briefs, the
escalation triggers, the audit fan-out. The skill owns *when* to invoke and
the convergence contract (round caps, blocking bar, churn budget).

**Size budget: ≤250 lines.** An incident earns AT MOST a one-line factor with
an issue/sha pointer — the narrative lives in the issue, the PR thread, and
git history. Over budget → one-in-one-out, never append-only.

## The factor taxonomy (shared by both scopes)

Four families. A diff-scope lens bundles several per agent; the audit gives
each family/factor its own finder. Neither scope may silently DROP a family —
if a factor doesn't apply, say so.

**(A) Correctness + architecture**
- Logic: off-by-one, inverted condition, wrong boundary.
- Concurrency/liveness: races, lock-order, lost wakeups, stale state, the
  no-scan-the-history rule; blocking I/O on an async worker → `spawn_blocking`
  (`block_in_place` is inert on the block_on root future, #603).
- Creation polarity: only a proof-of-LIFE event may create/resurrect an entry;
  a death/exit/TTL signal for an absent id must no-op (fbe26049).
- Lifecycle authority: user/model-controllable CONTENT never drives
  lifecycle/state transitions — structural markers + liveness signals only.
- Error handling & silent failure; a SET-but-EMPTY env var reads as unset
  (`io::nonempty_env`, #172).
- Security: path traversal (#23); unchecked wire input; config-write safety —
  atomic AND no destructive fallback (existing-but-unparseable is never
  rewritten); terminal-egress strips Cc AND Cf bidi (CVE-2021-42574); IPC
  endpoints created owner-only (restricted-then-rename, never umask), squat
  arbitration, no predictable paths in world-writable dirs.
- The 6 architecture invariants; no direct settings.json write; no prod
  `println!`; `unreachable_pub`.
- Magic numbers: reuse the authority or ONE named const; two copies = drift.
- Cross-platform: no path-STRING asserts (compare `PathBuf`); Windows parity;
  resolution-POLICY mirroring — mirror the target CLI's own resolver, the
  generic dirs/shellexpand answer IS the bug (#343/#342/#195).
- Sibling-set completeness: a guard/cap/validation added to SOME but not ALL
  sibling paths (decoders, install targets, platform arms, twin call sites) —
  the most-recurrent escape class (#272).
- Performance: per-frame allocs, hot-path scans.
- Resource lifecycle: Drop, fd leaks, unbounded growth; error-path ROLLBACK —
  multi-step setup must unwind applied steps on a later Err (a976c604).
- Upgrade path / installed base: state written by RELEASED versions survives;
  a fresh-install assumption that wipes an upgrader's state (#457); a
  compat-path removal names its concrete surviving population (#447).
- Test-teeth: would the pinning test FAIL if the behavior broke (mentally
  mutate it)?
- Declared-not-wired: every NEW field/flag/check/gate traces to a live
  consumer (`_`-bound captures, never-called validators, zero-CI-reach gates —
  #61, #273).

**(B) Design-debt**
- Duplication/DRY — weight by DIVERGENCE risk, not line count.
- God object / oversized module with a clean split; dead code.
- Unnecessary fallback / dead default: an arm whose trigger cannot fire, or
  that duplicates/contradicts an authority, is debt not safety. Load-bearing
  documented defense STAYS (shim exit-0, config-never-wipe, liveness ladders).
- Leaky or missing abstraction.
- Correlated-state bundling: N fields that always change together belong in
  ONE struct/newtype so illegal combinations are unrepresentable.
- Inconsistent pattern where one way is clearly the house style.
- Misleading identifier: a name that lies about what it holds/does.

**(C) Drift**
- Doc↔code: a doc naming a file/fn/flag/count that moved. The population
  includes hidden dirs — sweep `rg --hidden` or `grep -rn` (#448/#449).
- Wire-format/upstream: decoder vs the REAL upstream shape; every new source
  has a drift-watch row; install-path resolvers are upstream surface too; an
  AUDIT sweeps EVERY registered source, and checks the watch itself is alive
  (#454 — fail-open behind its own self-test).
- Version lockstep + adjudication: does THIS diff move the public surface or
  ship a feature, and is the bump right (patch=fix, minor=feature/breaking;
  over-bumping the open minor is the same finding, #471)?
- Comment-rot (now FALSE about the code) + comment-value (WHY only, no
  narration; fn-body comments ≤2 lines, longer rationale onto the declaration
  or a SHARP-EDGES entry; apply CLAUDE.md's comment rules verbatim).
- Manifest-bridge: `site/src/*.json` / generated schemas vs the Rust source
  of truth.

**(D) Quality + tooling**
- Test-coverage gaps on changed code.
- Mutation-teeth: a PROSE claim that a mutant is equivalent/killed is not
  teeth — pin in `.cargo/mutants.toml` or kill with a real test.
- Isolation & flakiness: real-state writes, wall-clock/order nondeterminism,
  `TEST_ENV_LOCK`, snapshot determinism.
- CI/build: gate coverage, path-filter holes, toolchain skew.
- Gate rules need BOTH directions — fires-on-violation AND
  stays-silent-on-legitimate (#788); the deny/error MESSAGE is half the rule —
  it must name the real requirement and remedy (#788/#789).
- Gate-teeth & liveness: what makes this pass WITHOUT checking? Fail-open on
  internal error; exit code eaten by a pipe; never wired into a required
  workflow; scheduled monitors dead/red with no consumer (#454, #440).
- Dependency/supply-chain: freshness (`cargo outdated`) is distinct from the
  advisories gate; every ignored `RUSTSEC-*` id re-justified or dropped (#486).

## The two populations (why scope matters)

The diff sees fix-introduced issues in one change; the audit sees the tree.
Aggregate-only classes (cross-PR emergent interaction, doc-drift accumulation,
design-debt accretion, coverage-topology gaps, arch-invariant erosion,
orphaned surface) are structurally invisible to a diff read — they are the
audit's job. A diff lens still runs the DRY / drift / sibling-completeness
sweeps that `grep` OUTSIDE the diff, the one thing a diff read can't do by
construction.

## The five hard requirements (every brief carries them)

1. **Reasoning before verdict** — trace/evidence first, then the claim.
2. **Negative space** — do NOT flag: behavior documented in the crate's
   `SHARP-EDGES.md` (open the ENTRY, the index line is not it); theoretical
   risks needing unlikely preconditions; absent defense-in-depth where a
   primary defense exists; pure style; existence/version claims about
   external artifacts from MEMORY — verify via `gh api`/the registry in this
   session or write "unverified" (#112: a live tap was "nonexistent" 4 rounds).
3. **Integer confidence 0–100 + `file:line`** on every finding.
4. **Sharp-edge check** — match familiar-smelling claims against the crate's
   `SHARP-EDGES.md` (premise-anchored: same seam ≠ same claim).
5. **Verdict** — exactly one of APPROVE / APPROVE-WITH-NITS / REQUEST-CHANGES.

---

## Lens 1 — correctness / grounding

```
You are reviewer 1/2 (correctness lens) for <PR/branch> on pixtuoid.
Worktree: <path> (branch <name>, base <sha>). Diff: git -C <path> diff <base>..HEAD.

Verify rigorously (read the actual code, not just the diff):
1. <the change-specific claims to check, one per line — from the impl-plan
   brief in the PR body when one shipped. A finding the plan never named is
   a plan-stage miss — flag it.>
2. House rules on touched code: no unwrap() outside tests, tracing not
   println, comments WHY-only, docs-currency.
3. Sibling-set completeness: for every guard/cap/validation the diff adds,
   enumerate the FULL sibling set (`rg` — siblings live OUTSIDE the diff)
   and verify each member got the same treatment.
4. Tests don't lie: mentally mutate each fix — would its pinning test FAIL?
   Trace every NEW field/flag/gate to the consumer that reads it.
5. Run the gates yourself (`just <fmt-check|site-check|preflight>` as
   applicable); report the EXIT CODE you observed, never through a pipe.
   NAME any CI-only gate this diff can red: semver, gen-check, wasm-check,
   windows-test, insta orphans — and `--lib` builds neither bin modules nor
   examples.

[the five hard requirements]
Your final message is the report.
```

## Lens 2 — design / blast-radius

```
You are reviewer 2/2 (design lens) for <PR/branch> on pixtuoid.
Worktree: <path>, read-only. Diff: git -C <path> diff <base>..HEAD.

Judge as a demanding critic:
1. <the design questions, one per line>
2. Downstream interactions: trace at least the two nearest consumers of the
   changed surface for contradiction.
3. Copy/docs sweep of everything new; propose replacement text where you
   object — a finding without a suggested fix is half a finding.
4. Data-shape check on every NEW field/key/map: name its identity/key-space;
   consolidate shared IDENTITY, not shared topic. Verify join keys against
   REAL production constants, never test fixtures (R0613-16/18: fixture ids
   match by construction).
5. Duplication sweep on every NEW fn/type/helper/const: `rg` the whole tree
   for a pre-existing implementation; flag "delegate, don't re-implement",
   weighted by divergence risk.
6. Layering: a new call into a mechanism layer routes through its designated
   orchestrator; a bare `pub` whose only callers are in-crate demotes to
   `pub(crate)` even where `unreachable_pub` is silent.

[the five hard requirements]
Your final message is the report.
```

---

## When two lenses aren't enough (escalation triggers)

Two is the floor; lens count scales with blast radius. The quality lever is
the change-specific `<...>` checklist, not the lens name. Add one focused
lens per matching trigger:

| Diff touches… | Add this lens |
|---|---|
| Generated art / clips | Film-critic: extract frames (1 fps + key moments), READ them, census the money shot. |
| Reducer / liveness / motion state machine | Lifecycle: trace the downstream interaction graph (rebind, sweeps, TTLs, create-on-unknown-id polarity) + provenance of every signal newly keyed on. |
| Public-facing artifact (site/README/notes) | Editorial outside-engineer read; if it RENDERS: drive the real page and MEASURE — WCAG in EVERY interactive state, mobile pan, no-JS (#453/#455: state-sweep, don't spot-check). |
| Substantial new/reshaped comments | Comment lens: each comment vs the CODE on accuracy/value/rot/vestigial/redundancy-within; `just comment-lint` output is the candidate list — report N items each WITH a disposition, never "passed" (#904). |
| Interactive TUI flow | UX walk: each user path end-to-end — first run, failure branches, no-CLI user (#359: two mandated lenses approved past a HIGH). |
| `pixtuoid-hook` (the shim) | Whole-shim never-panic audit: `args_os()`, bounded reads, every error path exit(0) (#198 slipped both bot and local review). |
| Motion / pose / walk-leg | Render and WATCH before the verdict (snapshot gif / `tier-replay.sh`) — #61 shipped five walk regressions past code-only review. |
| A string/layout a PAINTER frames | Render the COMPOSED frame; string-equality tests are blind to framing (#308 `⚠ ⚠`, #315). |
| `install/` or another CLI's config | Per-axis upstream-mirroring: enumerate EVERY resolution axis and re-verify each against that CLI's authoritative source IN-SESSION; write ⊆ verify (#338 missed the %APPDATA% axis). |
| New source / hook-only integration | LIVE run or hermetic replay WITHOUT capture-rig convenience flags (R0613-05 passed everything but live use); event shapes from CANONICAL upstream docs, never a fork. |
| Dedup / consolidation refactor | Wrong-abstraction lens ADVERSARIAL TOWARD REVERT, one pass per dedup: do all call sites share ONE reason-to-change? (#350) |
| "Behavior-preserving" batch refactor | Enumerate PER CALL-SITE which conversions are identical and which semantics MOVED — a batch dedup hides exactly one mover (#461). |
| Physical/domain feature, or LAST PR of an arc | Whole-FEATURE invariant audit: enumerate the domain invariants, re-derive each across the parameter space — per-task diffs compose into physics violations no diff lens sees (#471). |

## Orchestrator process notes (diff scope)

Dispatch lenses in parallel, in the worktree, in the background. Verify every
MEDIUM+ finding's premise yourself (read the SHARP-EDGES entry) before coding
a fix. Fold accepted findings into ONE review-round commit (`plan-miss:`
lines for reviewer-flagged plan misses). Drive every finding to exactly one
terminal state in the PR thread — FIXED / REFUTED-with-trace (cite or ADD the
sharp edge) / ISSUE-FILED; "acknowledged" is not a state (#40→#46). Sweep at
the FINAL merge head (#283/#383 drop class), and check WHICH commit a bot
re-flag was raised against before re-litigating (#316: 4 rounds on stale
flags). Gate on the bot's LATEST COMMENT verdict + `mergeStateStatus`, never
the check table (#448); an `<!-- absent-… -->` notice is an unreviewed head,
not a `Findings: 0`. Round caps, the blocking bar, and the churn budget:
the `two-lens-review` skill's convergence contract.

## Whole-codebase scope — orchestration

Same factors, requirements, verify contract, disposition; population = tree.

1. **Scout** (main loop): crates / LOC / churn / hot files → work-list.
2. **Find** — fan out, each finder carrying the full factor checklist:
   subsystem finders (one per crate/module cluster, incl. site + raycast with
   site pages RENDERED, not source-read — #453) + whole-tree specialist
   DEEP-sweeps, one factor each as its OWN finder (folding them into
   subsystem finders surfaces ~nothing): arch-invariants · concurrency &
   liveness · security threat-model · performance · test-assurance depth
   (RUN cargo-mutants on hot logic) · error-handling/silent-failure · drift
   (`rg --hidden`) · deep-modules.
3. **Verify — adversarially, default REFUTE**: an independent skeptic per
   finding (repro or refute; a sharp edge → REFUTE citing it). Vote weight
   scales with blast radius: security/concurrency get 2–3 differentiated
   skeptics, majority to confirm, and those sweeps run LOOP-UNTIL-DRY (two
   consecutive empty rounds).
4. **Completeness critic**: "what modality did we NOT run?" — its answer is
   the next round of finders, not a footnote.
5. **Dedup + rank** survivors; report grouped by family, KEEPING the
   refuted-as-deliberate list (coverage proof + sharp-edge context).
6. **Disposition sweep** + repo-wide stale-phrase sweep == 0 (`rg --hidden`).

The audit additionally carries a SYSTEM lens (decomposition, dependency
directions, cross-PR seams) and a DRY/duplication census (N copies of one
concept, weighted by divergence risk) — architecture erodes BETWEEN PRs.
