# Review-economics baseline — May 29 → Jun 11, 2026 (pre-ledger)

The "before" snapshot for the knowledge-base experiments: every multi-agent
review-class Workflow run (>20 agents) found in this machine's workflow
journals, measured by `scripts/review-metrics.py` BEFORE any ledger/KB
mechanism existed. Raw per-run numbers: [`baseline-2026-06.json`](baseline-2026-06.json).

## Headline numbers

| metric | value |
|---|---|
| review-class workflow runs (14 days) | 17 |
| agents dispatched | 840 |
| output tokens | 5,611,665 |
| cache-write tokens | 145,585,099 |
| cache-read tokens | 1,030,017,538 |
| **verifier share of output tokens** | **71.1%** |

The verify stage — adversarial verification of finder candidates — is the
dominant cost center. That is the stage the REVIEW-LEDGER targets: a third
of verification effort in the two whole-codebase reviews re-adjudicated
findings that a previous review had already refuted (see funnel below).

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
