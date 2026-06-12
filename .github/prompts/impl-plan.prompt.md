# Implementation-plan brief — the review, shifted left

The upstream twin of [`pr-review.prompt.md`](pr-review.prompt.md): the review
protocol catches the repo's known failure classes after the code exists; this
brief front-loads the same classes while each costs one plan line instead of
a finding plus a fix round. Evidence from the review-history census
(`docs/review-metrics/mining-2026-06.md`): at least 4 of the 7 post-merge
escapes were plan-preventable (the unwired field, the denylist gap, the
sibling path, the half-pinned window), and the one design-class miss — PR
#86's parallel config structure — was caught by a design-stage architect
pass, not by review.

**When to use:** new feature, new config key / CLI flag, new seam or module,
or any change touching a documented sharp edge or ≥3 source files.
**Skip for:** typo/docs fixes, mechanical renames, single-file bugfixes with
an obvious pinning test — right-size the process; don't 700-line-plan a
300-line change.

## The plan must answer (the mined failure classes, moved left)

Every section gets an answer; "n/a" counts only with a reason.

1. **Data shapes** — for every NEW field, config key, map, or collection:
   name its identity/key-space. If it overlaps an existing structure's
   identity (two collections keyed by the same id; an attribute map
   shadowing an entity list), the plan consolidates into one entity type or
   justifies why not. Shared IDENTITY consolidates; shared TOPIC stays
   separate (the `[pet-names]` lesson).
2. **Consumers** — every new field, parameter, or asset names the consumer
   this same change wires up. A plan line "add X" without "Y reads X at Z"
   is the unwired-addition smell (CONTRIBUTING pitfall 5) at its cheapest
   fix point — `_snap_prev` shipped unconsumed and defeated its own PR.
3. **Siblings** — every guard or fix enumerates its sibling paths up front
   (Unix/Windows arms, twin call sites, parallel manifests) and says which
   get the same treatment in this change (pitfall 2's in-diff form).
4. **Untrusted input** — if the change touches transcript/hook/file/config
   input, name the decode boundary where it is sanitized (pitfall 3), and
   whether any user-visible truncation is char-safe (pitfall 1). A
   denylist's enumeration cites the platform's DOCUMENTED set, never memory
   (pitfall 6).
5. **Negative branches** — list the refusal paths the tests will pin, BOTH
   sides of every window/threshold, with offsets derived from the constant
   under test (pitfall 4).
6. **Sharp edges + ledger** — read the nested `CLAUDE.md` for every crate
   touched and list the sharp edges that constrain this design. Check
   `docs/REVIEW-LEDGER.md` for CONFIRMED rows on the touched seams — those
   are known hazards exactly where you are about to work. (Plan time is the
   ledger's second consumption path; verification routing is its first.)
7. **Verification plan** — the gates to run, and any watch-it requirement:
   motion/pose changes render an animation and WATCH it; sprite changes run
   the `beautify-decoration` loop. Verification steps are blocking plan
   items, not checkboxes — PR #61 shipped five walk regressions behind an
   unchecked "live run" checkbox.

## The contract with review

The plan's answers BECOME the review's change-specific checklist: lens 1's
slot ("the change-specific claims to check") is filled from the plan's
claims. A review finding the plan never named is a measured failure of the
plan stage, not just a bug in the code — record it when it happens (the
knowledge-base pilot's Phase-3 experiment measures exactly this rate; see
`docs/KNOWLEDGE-BASE.md` "Measuring it").
