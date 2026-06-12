# Review-economics baseline — May 29 → Jun 11, 2026 (pre-ledger)

The "before" snapshot for the knowledge-base experiments: every multi-agent
review-class Workflow run (>20 agents) found under `~/.claude/projects`
(all project dirs, worktree sessions included; duplicate journals of one run
deduped to the fuller copy), measured by `scripts/review-metrics.py` BEFORE
any ledger/KB mechanism existed. An earlier draft scanned only the main
project dir and missed 4 runs — including the June-9 review itself; the
review process caught it, which is the methodology working as intended. Raw per-run numbers: [`baseline-2026-06.json`](baseline-2026-06.json).

## Headline numbers

| metric | value |
|---|---|
| review-class workflow runs (14 days) | 21 |
| agents dispatched | 1,177 |
| output tokens | 7,149,485 |
| cache-write tokens | 197,773,929 |
| cache-read tokens | 1,398,988,529 |
| **verifier share of output tokens** | **75.3%** |

The verify stage — adversarial verification of finder candidates — is the
dominant cost center (the role split is keyword-classified from each agent's
brief: crude but stable for our workflows, so treat 75.3% as a sound estimate,
not exact accounting). That is the stage the REVIEW-LEDGER targets: in the
June-10 review a third of distinct candidates (12 of 37) ended refuted,
several re-deriving verdicts the June-9 run had already adjudicated (see
funnel below).

## The June-9 whole-codebase review @ 151e38d (`wf_bb515859-ce0`)

115 agents — **110 of them verifiers** (3 finders, 1 implementer, 1 other),
457,981 output tokens. The purest illustration of where review money goes:
the find side was 3 agents; adjudicating what they found took 110.

## Flagship run: whole-codebase review @ 7bc2777 (2026-06-10/11)

`wf_cf9c00c3-dc2` — 16 finders (10 subsystem + 6 lens) → dedup → design-intent
skeptic + code-trace verifier pairs; includes the usage-limit stall + resume
(`resumeFromRunId`), so totals are what the review actually COST, not the
idealized single pass. The journal also contains the follow-up fix
implementers dispatched from the same workflow.

| role | agents | output tokens |
|---|---|---|
| verifier | 82 | 551,631 |
| implementer | 70 | 454,017 |
| finder | 8\* | 250,387 |
| dedup | 5 | 57,731 |
| **total** | **165** | **1,313,766** |

\* finder count under-reads: resumed finders were cache-replayed, not re-run —
their original cost is inside the pre-resume agents.

## Adjudication funnel (from the review records)

| review | candidates | confirmed | refuted | refuted % |
|---|---|---|---|---|
| 2026-06-09 @ 151e38d | 49 | 23 | 26 | 53% |
| 2026-06-10 @ 7bc2777 | 42 (37 distinct) | 25 | 12 | 29% |

Both reviews re-refuted overlapping findings (the `/tmp` socket "vulnerability",
the EMFILE hot-spin, Storm>Rain inversion…) — re-paid adjudication that a
ledger should eliminate. The counter-case that shapes the ledger's design:
on the SAME seam, June-9 correctly refuted a socket-steal claim while June-10
confirmed a *different* socket-steal claim as a real MEDIUM (ECONNREFUSED on a
backlog-saturated live daemon → PR #235's flock arbitration). A naive
suppression list would have killed the real finding — hence the
premise-anchored, demote-don't-kill protocol in
[`docs/REVIEW-LEDGER.md`](../REVIEW-LEDGER.md).

## Measurement protocol for the after-side

Run any future review workflow, then:

```
python3 scripts/review-metrics.py <wf-dir> --label "<review name>" --json out.json
```

Compare against this file on: total/verifier output tokens, agents per stage,
and (from the review's own report) the repeat-refutation count — candidates
matched against ledger entries — plus confirmed-findings count as the quality
guard (cost must drop with findings held, or the saving is fake).
