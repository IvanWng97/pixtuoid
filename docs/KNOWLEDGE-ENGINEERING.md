# Knowledge engineering

How this repo stops paying for the same lesson twice — and what we measured
when we tested whether it works.

pixtuoid is built almost entirely by AI coding agents, reviewed by fleets of
AI reviewers, and maintained across hundreds of agent sessions. That makes it
a live laboratory for a question every team adopting agents hits eventually:
**where does the knowledge live, and who pays when it doesn't?** This page is
the system we ran, the experiments we ran against it, and the honest results
— including the null ones.

> **Editor's note (2026-06).** The heavy *process* machinery described below —
> the review-ledger suppression protocol, the disposition sweep's mechanized
> attestation, and the
> `census-reminder` / `review-metrics` / `sharp-edge-inventory` automation —
> has since been **removed** from the repo as a deliberate simplification (the
> disposition sweep itself survives as a practice in `pr-review.prompt.md`'s
> process notes). This
> page is kept as a record of what we built and, more to the point, what we
> *measured*: the null onboarding result and the code-first storage ranking are
> exactly *why* the prose-and-process layers were cut back to the parts that act
> on a change while it is still being written — context files, the in-session
> build gate (`just preflight`), lints and tests, and human-plus-AI review. Read
> the storage layers and "the conveyor" below as the system as it stood during
> the experiments, not as current tooling.

## The numbers that started it

Between May 29 and June 11, 2026, this repo ran 21 review-class multi-agent
workflows: **1,177 agents, 7.1M output tokens — 75.3% of which were spent in
the Verify stage**, adversarially adjudicating candidate findings (one
whole-codebase review dispatched 110 verifiers against 3 finders).
Consecutive reviews re-adjudicated overlapping claims — re-refuting the same
`/tmp`-socket and EMFILE candidates two days apart. Paying repeatedly for
knowledge the project already had is the cost this whole system attacks.

## The model: storage and the conveyor

Knowledge can be **stored** at three altitudes; we rank them by durability —
a ranking the industry evidence supports and our own experiments back
directionally (two tasks; caveats in the results table below):

1. **In the code** — WHY comments at the hazard seams, types that make
   invalid states unrepresentable, tests and lints that fail the build.
   Strongest form: our with/without-KB experiment removed every knowledge
   file from the worktree and the code-embedded lessons still carried both
   tasks (contamination bounded, not eliminated — report below).
2. **In process artifacts** — review briefs, plan templates, PR checklists,
   routing protocols. These don't wait to be read: they are filled into
   prompts and forced into workflows at specific moments.
3. **In prose** — context files, wikis, design docs. Weakest form: the
   industry evidence (below) and our own null result both say marginal
   prose has marginal value. Prose is a map and a waystation, not a
   destination.

What turns storage into a system is the **conveyor**: the process layer
moves lessons down the ladder — an incident becomes a review finding,
a finding becomes a checklist item, a recurring item becomes a comment, a
type, or a CI gate. Documentation can be ignored; the build cannot.

## The storage layers in practice

(The linked reports call these Layers 1–4, this page's former numbering.)

### Context files (prose that loads automatically)

`CLAUDE.md` at the workspace root plus nested per-crate files, with
[`AGENTS.md`](https://agents.md/) symlinked to the root so every agent CLI
reads the same source. The load-bearing content is the **"Known sharp
edges"** sections: things that look like bugs but are deliberate, each with
its WHY — in review after review, these kill false positives (a
design-intent skeptic armed with them refuted 26 candidates in one run).

**Failure mode:** monotonic bloat. An ETH Zurich study (arXiv:2602.11988)
found LLM-generated context files *reduced* agent success in 5 of 8 settings
while raising cost ~20%; the converged practice is a ~100–300 line **map,
not a manual**. **Counter:** size budgets, citation tracking (a sharp edge
no review has cited across **two consecutive censuses** is a demotion
candidate — counted against the **full** `CLAUDE.md` sharp-edge set so a
topic-concentrated review window can't manufacture false staleness; each edge
had a stable slug the ledger cited as `[edge:<slug>]`, kept in lockstep with
the guides mechanically so the census leg was a script run, not a hand-count (#386)), periodic
audits.

### Retrieval (knowledge computed on demand, never stored)

Agentic search over the live tree plus build-system dependency graphs
computed at review time and discarded — persistent indexes drift the moment
they're built. **Failure mode:** dependency graphs are blind to non-code
coupling (this repo's `media.json` ↔ `showcase.json` ↔ README triangle has
zero imports between its corners). **Counter:** if a cross-boundary coupling
matters, a **bridge test** pins it (`supported_sources_manifest` fails the
build if a JSON manifest and a Rust const list diverge); if no test enforces
it, reviews assume it will be missed.

### Memory (episodic, not yet distilled)

Agent session memory captures raw lessons with zero friction; a periodic
distillation pass promotes the keepers into the repo. The pipeline is
one-directional: **capture → distill → promote → expire the raw entry.**

**Failure mode:** both directions die. Unmanaged automatic capture rots — a
publicly audited self-hosted memory deployment hit **97.8% junk in 32 days**
(mem0ai/mem0#4573). Gated capture starves — put an MR in front of writing a
memory and people stop writing. **Counter:** friction goes on the distiller,
never the capturer; recurrence is the promotion trigger (twice = it leaves
prose and becomes a rule). The distillation pass has no calendar of its own —
it rides the periodic context-file audit (the graveyard rule, below): every
audit also sweeps recent session memories for promote-to-repo candidates.

### The review ledger (adjudications as institutional memory)

The review ledger recorded every adjudicated review
finding: seam, claim, verdict, and — critically — the **anchor**: the
mechanism (file + sharp edge + HEAD) that justified it. Future reviews match
candidates against it before spending verifier tokens.

**Failure mode:** a naive suppression list hides real bugs, and this repo has
the proof — one review correctly refuted a socket-steal claim; the next
found a *different* socket-steal on the **same seam** that was real (a
backlog-saturated daemon returns `ECONNREFUSED` on macOS, so a second
instance reclaims a live socket — fixed by flock arbitration in PR #235).
**Counter — the protocol:** a match **demotes, never kills** (one cheap
checker instead of a full panel); anchors expire when the anchoring code
changes; only sharp-edge/PR-cited verdicts get the fast path; periodic
ledger-blind calibration measures the false-suppression rate; append-only —
flips supersede, and the flip itself is knowledge.

## The conveyor: process as the change lifecycle

The process layer's collective form is **a path every change must walk**,
each gate a versioned file with an automatic reader:

| gate | artifact | automatic reader |
|---|---|---|
| plan | [`impl-plan.prompt.md`](../.github/prompts/impl-plan.prompt.md) — 8 sections every non-trivial plan must answer (data-shape identity, named consumers, sibling paths, untrusted-input boundaries, tests-first + negative branches, sharp-edge sweep, blocking verification, layering/orchestration boundary) | routed from the workspace context file; the plan lands in the PR body |
| implement | the 6 recurring pitfalls + the PR template checkbox pointing at them | the template is forced on every author |
| review | [`pr-review.prompt.md`](../.github/prompts/pr-review.prompt.md) — two differentiated lenses, five hard requirements, escalation triggers, sharp-edge routing | copied verbatim into reviewer prompts; the bot loads its own rules file |
| merge | the disposition sweep — every finding ends FIXED / REFUTED-with-trace / ISSUE-FILED / ACCEPTED-residual; plan-misses become `plan-miss:` commit lines | the orchestrator's process notes; commit messages become the data channel |
| periodic | the history census — each census run files its own successor as a `census` issue at +~50 merged PRs (the #388 pattern; the cadence rides the issue backlog, not a scheduled workflow), ledger-blind calibration + a frozen report archive | the issue backlog |

Three properties make this a system rather than a document set:

1. **On the path, not on a shelf.** Knowledge here is not a library someone
   might consult; the templates, the briefs, and the disposition sweep sit on
   the road itself. The PR #86 backtest shows what putting a question on the
   path is worth: 0/3 reviewers flagged a parallel-structure smell under the
   old brief, 3/3 once the data-shape question became a standing item, 0/4
   over-fires on controls. The plan gate is the softest link — its trigger
   is a context-file rule — so its misses are measured (`plan-miss:` commit
   lines) rather than prevented.
2. **One path for every member.** Humans get the PR template, agents get the
   filled briefs, the bot gets its rules file — the platform remembers so no
   individual has to. Onboarding is not training; it is being walked through
   the gates by your first PR.
3. **Closed loop.** The last gate's output rewrites the earlier gates: the
   history census found 7 escape classes, each became a rule, and every rule
   was adversarially verified before landing. Revising the path is itself a
   PR that walks the path.

## What we measured (the honest results)

| experiment | result |
|---|---|
| Ledger A/B — routed vs full verification, same candidate set | **0 false suppressions**; 61% saving per routed candidate; ±0 overall at a 10% route rate — the payoff scales with re-tread density, and the demote path is not yet exercised |
| Review-history census — 185 merged PRs + 50 post-merge fixes | 7 adjudicated escape classes (~4% of PRs) → 7 verified guideline changes; the design-class lens **backtested 0/3 → 3/3** on the original missed diff, 0/4 over-fires on controls |
| Review-history census #2 — 45 merged PRs #262–#328 | escape leg **FAILS** zero-recurrence: 3 of the 7 named classes recurred (rate ~4.4% by escape-classified fix commits ≈ the prior 3.78%, so severity held — rules aimed right, adherence at review- not author-stage); stale-external-fact rounds **0**; 3 disposition drops (vs 4); arch-drift **PASS** (6 invariants verified at HEAD, 4 cross-PR seams logged); sharp-edge citation baseline 3/27 |
| Review-history census #3 — 42 merged PRs #329–#383 | escape leg **PASSES** at 0/42 (cleanest of three — the 3 census-#2 recurrences all back to zero); stale-external rounds **0**; **0 disposition drops** (vs 3) though the #335 channel was built-but-unadopted (marker used 1/42 PRs; #384 tracks flipping the CI to a live advisory audit); arch-drift **PASS** (6 invariants hold through the 2→4-crate split + the compiler-enforced `pixtuoid-scene` boundary); sharp-edge citation 1/27 vs the frozen subset → **inventory widened, 0 demotions fired** (#386) |
| Onboarding proxy — with/without KB, 2 tasks × 2 arms | **Null on first-pass quality** — arms indistinguishable, consistent with the load-bearing lessons already being in the code; the KB's prescribed process duties executed at +19% token overhead (n=2, contamination caveats in the report) |

The null is the most instructive row: it is what the conveyor *succeeding*
looks like. Once a lesson reaches the code, the prose that carried it becomes
scaffolding — which is exactly why the investment ranking puts code first
and prose last.

## The two design principles

**The graveyard rule.** This repo once had a beautifully written agent guide
that nothing loaded; it went stale within weeks and was deleted, its one
not-already-covered rule salvaged. **Knowledge files live or die by their
load path, not their quality.** Design the automatic reader first; every
layer's maintenance must ride an existing habit — the review workflow writes
the ledger, CI runs the lints, the census files its own successor issue —
because knowledge that needs someone to *remember* to maintain it is already
dead.

**The executability ladder.** A lesson that recurs stops being text: this
repo's MSRV lesson is a build gate, its shell-injection lesson is a
character-set guard, its socket-path lesson is a parity test. Prose is the
weakest storage format for knowledge; the terminal rung is code — and the
with/without experiment showed the terminal rung carrying tasks on its own.

## Steal this (adoption order for another team)

1. **Mine your own review history first** (zero new infrastructure): your
   merged MRs and post-merge fixes already contain your escape taxonomy and
   your false-positive classes. Ours took one afternoon and produced seven
   verified guideline changes — including the standing failure classes in
   step 2 and the disposition rule in step 3.
2. **Review briefs**: two differentiated lenses, reasoning-before-verdict,
   negative-space lists, and your mined failure classes as standing items.
3. **A disposition rule**: every review finding reaches a terminal state —
   our census caught an ignored finding becoming a release blocker.
4. **Promote on recurrence**: the second occurrence of a lesson becomes a
   linter rule or a test, not another paragraph (for mobile teams,
   SwiftLint/detekt are excellent terminal rungs).
5. **A ledger only if you run repeated large reviews** — its payoff scales
   with re-adjudication density: measure it, don't assume it.

Efficiency metrics are only reported **paired with their quality guard** —
a review that got cheaper by finding less is not a saving. Every claim on
this page links to the report that grounds it.
