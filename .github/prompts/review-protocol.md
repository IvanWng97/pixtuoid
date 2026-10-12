# Review protocol

Staged by `claude-readonly-review.yml` from its own commit. `REVIEW.md` is the
reviewed repository's own, at its root: it holds the rules, and its Common
section joins this one's.

## Scope

Each reader applies [Common](#common) and its own section, nothing else:

- The automatic review's [orchestrator](#orchestrator) merges duplicate
  candidates and keeps what the [verifier](#verifier) keeps.
- A unit reviewer applies [Unit](#unit) to the files of one unit.
- The correctness reviewer applies `REVIEW.md`'s Correctness lens to the whole
  diff, plus every matching Escalation row, and names the local rows.
- The design reviewer applies `REVIEW.md`'s Design lens to the whole diff.
- A local row lens applies its `REVIEW.md` Escalation row.

## Common

Read the reviewed repository's `AGENTS.md`, and the nested `AGENTS.md` of each
directory the diff touches, first; these rules add to generic defect hunting.

### Evidence

Every finding states its evidence before its claim, cites a `file:line` from a
file actually read, and survives a sharp-edge check: the same seam is not the
same claim. A claim about an external artifact (action tag, crate release,
upstream shape) not fetched this session says "unverified" (#112).

### Severity

[Conventional Comments](https://conventionalcomments.org/) labels, the
`review-schema.json` `severity` enum;
taste with no defect is never posted. Dispositions:
[CONTRIBUTING](https://github.com/IvanWng97/pixtuoid/blob/main/docs/CONTRIBUTING.md#pull-requests).

- `issue (blocking)` — correctness, security, an invariant, or a breach of any
  AGENTS.md rule, its Conventions included. It blocks only once
  the agent that ran the review or a maintainer confirms it against the code;
  the finder's label alone never blocks.
- `issue (non-blocking)` — any other real defect this PR introduced.
- `issue (pre-existing)` — real, not introduced here.

### Findings

Each finding carries:

- `severity`: the [label](#severity)'s decoration.
- `lens`: the `REVIEW.md` lens whose section names the finding's kind; a unit's
  comment, DRY and drift findings are design, its test findings correctness.
- `path`: repository-relative (the `b/` side of `pr.diff`), never absolute.
- `line`: the absolute head-side line, never invented.
- `body`: the verified finding and a concrete failure scenario; a
  maintenance defect's is the edit that would leave a copy or a comment wrong.

## Orchestrator

Return only the structured result
`review-schema.json` defines: a
one-sentence `summary`, the kept findings, every blocking one first and
pre-existing last, and as `overflow` the count of kept findings past the
schema's `maxItems` ceiling, which fails the review's status at that head; the
summary then also names each of those by severity, lens and `path:line`, since
they have no thread.

### Re-review

Never re-flag a finding that already has a thread in
`.claude-review/prior-threads.json` (the review's own threads on the PR),
resolved or not
([re-review convergence](https://code.claude.com/docs/en/code-review#what-you-can-tune));
new findings follow [Severity](#severity), whatever the round. The published
review shows its `Round:`, the count of heads it was published at, where a head
that only merges main in does not advance it; the
[convergence contract](https://github.com/IvanWng97/pixtuoid/blob/main/docs/CONTRIBUTING.md#convergence-contract) says what
each round's fold takes.

## Unit

Read each file of the unit whole at the head (`.claude-review/head/`; a removed
file has only its hunks), against the base tree. A defect anywhere in it is in
scope, `issue (pre-existing)` when the diff did not introduce it.

1. **Comment audit**: for every file, a one-line change included, read its
   entire comment population against the code and AGENTS.md's comment rules
   (accuracy, value, rot, vestigial, self-repetition); a story told twice keeps
   the copy on the narrowest thing it constrains.
2. **DRY** — every new fn/type/helper/const gets a whole-tree search for an
   existing implementation, weighted by divergence risk; every value a new line
   reads is the authority its consumer uses, not a sibling's (#1042, #1155).
   Test fixtures stay inline.
3. **Drift** — docs naming a moved file/fn/flag/count, searched including
   `.github/` and `.claude/`.
4. **Test teeth** for every test the unit holds, as `REVIEW.md`'s Correctness
   defines it.

## Verifier

Refute by default. Keep a candidate only when the code at its `file:line`,
read this time, shows the evidence it states, the claim follows from it, and
its failure scenario is reachable; drop it when `REVIEW.md`'s Do not flag
covers it.
