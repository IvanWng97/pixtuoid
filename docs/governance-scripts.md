# Governance scripts — the index

The repo enforces the **non-code-shaped** parts of the lifecycle (process, review,
traceability, hygiene) with a small family of Python scripts that the ~25
code-shape CI jobs (clippy, tests, semver, …) can't see. They all share ONE idiom,
and this page is the single place their *cadence and authority* live (the
[`CLAUDE.md` scripts map](../CLAUDE.md) describes what each file *does*; this table
says *when it runs and whether it can fail your PR*).

## The shared idiom

Every script in the family is built the same way, so learning one teaches all:

1. **Pure parsers + thin git/gh glue.** The logic is in side-effect-free functions
   (diff/PR-body/comment → findings); the network/subprocess glue is a thin shell
   that the tests don't touch.
2. **A both-sides `*_selftest.py`.** Each parser is pinned on BOTH sides of its
   threshold, because *a parser that silently finds nothing is a false "all clear"*
   — the #283/#384 silent-monitor-death class. The selftest is itself CI-gated.
3. **A `just` recipe + a CI step.** No local-vs-CI drift: the hook, the recipe, and
   the CI job call the same code.
4. **Advisory by default; blocking only when detection is near-zero-FP.** Severity
   is calibration, not phasing — a heuristic check warns, a mechanical fact blocks.

## The scripts

| Script | What it gates | Trigger | Authority | Bypass | Selftest |
|---|---|---|---|---|---|
| `check_dod.py` | The whole lifecycle (TDD/design/impl-plan attestation, two-lens block, prod-`println!` / `settings.json`-write / `--no-verify` bans, ledger trace, deferred→issue, docs-currency) | `.claude/settings.json` Stop + PreToolUse hooks (agent); `.githooks/pre-push` + CI `definition-of-done` job (change) | Agent layer **advisory**; change layer **BLOCKING** on diff-derived facts + the merge two-lens block | `DOD_BYPASS=…` env or `DOD-BYPASS:` line — **agent layer only**, always logged; CI ignores it | `check_dod_selftest.py` |
| `check_review_disposition.py` | Every `claude[bot]` MEDIUM+ finding reached a terminal disposition (`Bot-findings-adjudicated:` marker / ledger row / issue) | `review-disposition.yml` on every PR | **Advisory** (per-file coarse + the bot re-flags stale rounds, so a hard gate mis-fires) | n/a | `check_review_disposition_selftest.py` |
| `sharp_edge_inventory.py` | `docs/review-metrics/sharp-edge-inventory.md` stays in lockstep with the `CLAUDE.md` sharp-edge bullets; ledger `[edge:<slug>]` citations resolve | CI `hygiene` job (`just sharp-edge-inventory`) | **BLOCKING** (drift gate) | n/a | `sharp_edge_inventory_selftest.py` |
| `check_upstream_drift.py` | The wire formats of the agent CLIs we decode haven't changed under us | `upstream-drift.yml` (weekly) | **Advisory** (alarms; files/annotates) | n/a | `check_upstream_drift_selftest.py` |
| `census_reminder.py` | A review-history census is due vs the ~50-PR window | `census-reminder.yml` (weekly; `just census-reminder` local) | **Advisory** (auto-files a deduped `census` issue) | n/a | `census_reminder_selftest.py` |
| `review-metrics.py` | The review-economics collector (no gate of its own) | CI `hygiene` job pins its selftest (`just review-metrics-selftest`) | **Advisory** (data collector) | n/a | `review-metrics_selftest.py` |

## Shared mechanism: the advisory LLM-judge

Two of these gates split enforcement into *existence is the floor, substance is the
judge*: a mechanical check confirms an artifact is **present** (and blocks if it's
a hard fact), and an advisory model call rates whether it's **substantive**, not
theater. That model call (jq → Anthropic → job-summary) is the shared composite
action [`.github/actions/llm-judge`](../.github/actions/llm-judge/action.yml); the
gate-specific prompt comes from each script's `--judge-prompt` mode:

- `check_dod.py --judge-prompt` — are the two-lens lenses genuinely distinct? is TDD evident?
- `check_review_disposition.py --judge-prompt` — are the dispositions real adjudications, or rubber-stamps?

The judge is always `continue-on-error` and no-ops without `ANTHROPIC_API_KEY`, so
it never reds a PR (and is absent on fork PRs).

## Why these aren't unified into one script

They look similar but have **distinct cadence, authority, and domain** (a weekly
issue-filer vs a per-PR blocking gate); merging the *logic* would erase those
boundaries. `scripts/_gov.py` holds only what is genuinely shared *and*
abstraction-free: the pure `_strip_control` boundary-sanitizer, which was
byte-identical in both scripts — DRYing a verbatim pure helper has no abstraction
cost and removes a real drift risk (it stays pinned through both `*_selftest.py`
import sites). The remaining overlap is ~15 lines of `gh` plumbing (`DEFAULT_REPO`,
`_gh_json`): that is policy-shaped (each gate's repo / auth / error handling) and
has only two consumers, so the rule of three is unmet — it stays inlined per
script. **Revisit moving the `gh` plumbing into `_gov.py` at the third
`gh`-consuming gate.**
