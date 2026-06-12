# Phase 3 — onboarding-proxy A/B (with/without KB, 2026-06-12)

The pilot's onboarding question, measured: does the knowledge base improve a
fresh agent's first-pass code quality? Two standardized tasks × two arms at
`a40977c`, implementations in throwaway worktrees (workflow
`wf_5509ed8a-913`; all code discarded, reference patch preserved on #262):

- **KB arm**: full repo (CLAUDE.md tree, AGENTS.md, CONTRIBUTING pitfalls,
  REVIEW-LEDGER, prompts) + plans against
  [`impl-plan.prompt.md`](../../.github/prompts/impl-plan.prompt.md) first.
- **bare arm**: KB files physically removed from the worktree, instructed to
  derive conventions from code + README only, freestyle planning.

Task A: per-pet `mood` config attribute end-to-end (designed trap: the
`[pet-names]` parallel-map temptation). Task B: diagnostic breadcrumb for the
CONN_TIMEOUT mid-payload event loss, #262 item 2 (designed trap: fixing one
platform sibling). Per-implementation two-lens reviews, identical briefs
across arms; a blind classifier counted findings, pitfall classes, and
plan-misses.

## Results

| metric | A-kb | A-bare | B-kb | B-bare |
|---|---|---|---|---|
| first `just preflight` exit | **0** | **0** | 1 | 1 |
| preflight attempts to green | 1 | 1 | 2 | 2 |
| distinct review findings (deduped\*) | 1 | 1 (+1 artifact†) | 2 | 2 |
| worst severity | nit | low | low | low |
| designed trap | avoided | avoided | avoided | avoided |
| implementation output tokens | 34,776 | 29,975 | 33,775 | 26,705 |

\* dual-lens duplicates and "verified clean" record entries excluded.
† "nested CLAUDE.md not updated" — the bare worktree had no CLAUDE.md files
to update; experiment scaffolding, not an agent failure (though it is what a
genuinely KB-less repo looks like to a reviewer: docs-currency violations by
construction).

**On every quality metric the arms are indistinguishable.** Both arms put
`mood` ON the existing `PetEntry`/`Pet` entities with a raw-string-then-
validate field (no parallel map, config-reset-safe); both arms independently
invented the same task-B design — a Drop-guard in the SHARED `hook/mod.rs`
(one seam covers both platform listeners, defused on shutdown) — defeating
the sibling trap architecturally. Task B's symmetric first-pass failures were
both fixed in one retry. KB-arm cost: **+19% output tokens** (135k vs 113k
across impl+review), buying the plan, the ledger consult, and the docs
updates only it could make.

## The central finding: knowledge redundancy

The bare arm reproduced KB-grade decisions because the load-bearing lessons
were not only in the KB files — they are embedded **in the code at the
hazard seams**: `config.rs`'s field docs carry the entire
raw-string-vs-serde-enum footgun story (the `[pet-names]`/all-or-nothing
lesson), and the hook module's structure makes the shared-seam fix the
natural one. The executability ladder predicts exactly this: knowledge that
reached the code (WHY comments, types, tests) survives context stripping;
prose-only knowledge would have differentiated. Both tasks landed on seams
that June's arcs had already pushed down the ladder — a selection effect
that under-measures KB value on less-documented seams.

What the KB measurably added was **process-level**: B-kb consulted the
ledger unprompted, cited R0609-15 (bounded drop = documented design) and
correctly scoped the change as trace-only; A-kb updated the nested CLAUDE.md
files in the same commit (docs currency); and the plan made review cheaper —
lens 1 verified claims instead of re-deriving them.

## Plan-miss rate (first measurement)

KB-arm plans named everything reviews later found except: A-kb 1 miss (warn
message detail), B-kb 1 distinct miss (the breadcrumb hard-attributes a
cause that a second drop path — runtime shutdown — shares). Both misses are
the same species: **diagnostic-message precision**, a class the plan brief's
sections don't currently probe. n=2; recorded per the `plan-miss:` protocol,
not yet a rate.

## Caveats

1. **Contamination upper bound**: bare-arm agents may carry harness-injected
   context from the main checkout; physical file removal + explicit
   ignore-instruction bounds but cannot eliminate it. The plan-brief effect
   is clean (strictly present/absent); the context-file effect is an upper
   bound.
2. **n=2 tasks**, both on well-self-documented seams; directional only.
3. Implementer self-reported first-pass exits (spot-checked by lens 1
   against committed state, journal-verifiable).

## Implications

- For #265's CLAUDE.md slim: this LOWERS the risk — code-embedded knowledge
  carried both tasks, so a map-not-manual context file is safer than feared.
- For the KB roadmap: the highest-leverage promotion target is always the
  code itself; KB files earn their keep on seams the code doesn't yet
  self-document, and on process duties (docs currency, ledger discipline,
  plan auditability) — which is where the arms actually diverged.
- Total experiment cost: 248k output tokens / 13 agents — cheap enough to
  re-run with harder tasks on undocumented seams, which is the natural
  follow-up if a sharper signal is wanted.
