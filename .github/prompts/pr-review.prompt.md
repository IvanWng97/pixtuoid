# Review briefs — what a reviewer can't derive on its own

Canonical for both scopes of `two-lens-review` (diff gate / whole-codebase
audit). The skill owns *when* to invoke and the convergence contract (round
caps, blocking bar, churn budget).

**One-in-one-out.** A line lives here only if it
changes reviewer BEHAVIOR: a repo-specific trap whose obvious reading is
wrong, a rule about what NOT to flag, or a verification act beyond reading
the diff. Generic defect hunting (logic, races, error handling, unwrap,
perf, design-debt judgment) is assumed — do it as on any codebase.
Architecture invariants and house conventions live in `CLAUDE.md`, which
every reviewer reads first; don't restate them here. An issue# appears only
where the rule would otherwise look arbitrary enough to get "fixed" away.

## Repo-specific traps (the default instinct is wrong here)

- Creation polarity: only a proof-of-LIFE event may create/resurrect an
  entry; a death/exit/TTL signal for an absent id must no-op.
- Lifecycle authority: user/model-controllable CONTENT (transcript text,
  message bodies, tool args) never drives lifecycle/state transitions —
  structural markers + liveness signals only.
- Config writes: atomic AND never destructive on ANY error/skip/default arm —
  existing-but-unparseable is never rewritten, a skip never strips
  pre-existing hooks.
- A SET-but-EMPTY env var reads as unset (`io::nonempty_env`).
- Terminal egress strips Cc controls AND Cf bidi overrides (Trojan-Source).
- IPC endpoints: owner-only at creation (create-restricted-then-rename,
  never a process-global umask); treat a pre-existing endpoint as hostile.
- Per-CLI resolution POLICY is mirrored from that CLI's own resolver — the
  generic dirs/shellexpand answer IS the bug (#343).
- Upgrade path: state written by RELEASED versions survives; a fresh-install
  assumption that wipes an upgrader's config is a HIGH (#457).
- Compare `PathBuf` structurally — a path-string assert with a hardcoded
  separator reds only in windows-test.
- A dead fallback is debt, not safety: an arm whose trigger cannot fire, or
  that duplicates/contradicts an authority, gets flagged — but documented
  load-bearing defense (shim exit-0, config-never-wipe, liveness ladders)
  STAYS.
- Version adjudication: does THIS diff move the public surface or ship a
  feature, and is the 0.x bump right (patch=fix, minor=feature/breaking)?
- Comments follow CLAUDE.md's comment rules verbatim (WHY-only, semantic,
  no length cap for an earned WHY).

## Sweeps that must LEAVE the diff (a diff read can't see these)

- Sibling-set completeness: a guard/cap/validation added to SOME of a
  sibling set (per-source decoders, install targets, platform arms, twin
  call sites) — `rg` the FULL set and verify each member.
- DRY: every NEW fn/type/helper/const gets a whole-tree search for a
  pre-existing implementation; weight by divergence risk.
- Drift: docs naming a moved file/fn/flag/count — sweep `rg --hidden`
  (bare `rg` skips `.github/`/`.claude/`).
- Wire-format/upstream: a decoder or drift-watch row vs the REAL upstream
  shape; whether a source OWES a row is `source/drift.rs`'s header — read it
  before filing "no row" (an audit sweeps EVERY source, not just changed ones).
- Declared-not-wired: every NEW field/flag/check/gate traces to a live
  consumer that reads it.
- Manifest-bridge: `site/src/*.json` / generated schemas vs their Rust
  source of truth.
- Test-teeth: mentally mutate each fix — would its pinning test FAIL? A new
  gate needs fires-on-violation AND stays-silent-on-legitimate, and its
  error message must name the real requirement, not a plausible neighbour.

## Negative space (do NOT flag)

- Behavior documented in the crate's `SHARP-EDGES.md` — open the ENTRY (the
  index line in its `CLAUDE.md` is not the entry).
- Absence of defense-in-depth where a primary defense exists; pure style;
  theoretical risks needing unlikely preconditions.
- Existence/version claims about external artifacts (action tags, crate
  releases, taps) from MEMORY — verify in-session via `gh api`/the registry
  or write "unverified" (#112: a live tap was "nonexistent" for 4 rounds).

## The five hard requirements (every brief carries them)

1. Evidence first, then the claim. 2. The negative space above.
3. Integer confidence 0–100 + `file:line` on every finding.
4. Sharp-edge check on familiar-smelling claims (same seam ≠ same claim).
5. Verdict: exactly one of APPROVE / APPROVE-WITH-NITS / REQUEST-CHANGES.

## Lens 1 — correctness / grounding

```
You are reviewer 1/2 (correctness lens) for <PR/branch> on pixtuoid.
Worktree: <path> (branch <name>, base <sha>). Diff: git -C <path> diff <base>..HEAD.
Read CLAUDE.md first; read the actual code, not just the diff.
1. <change-specific claims to verify, one per line — from the impl-plan
   brief in the PR body when one shipped; a finding the plan never named is
   a plan-stage miss — flag it>
2. The repo-specific traps + the out-of-diff sweeps (the prompt file above).
3. Run the applicable gates yourself; report the EXIT CODE you observed,
   never through a pipe. NAME any CI-only gate this diff can red (semver,
   gen-check, wasm-check, windows-test, insta orphans); `--lib` builds
   neither bin modules nor examples.
[the five hard requirements] Your final message is the report.
```

## Lens 2 — design / blast-radius

```
You are reviewer 2/2 (design lens) for <PR/branch> on pixtuoid.
Worktree: <path>, read-only. Diff: git -C <path> diff <base>..HEAD.
Read CLAUDE.md first. Judge as a demanding critic:
1. <the design questions, one per line>
2. Trace the two nearest consumers of every changed surface for
   contradiction; propose replacement text where you object.
3. New data shapes: name the identity/key-space; consolidate shared
   IDENTITY, not shared topic; verify join keys against REAL production
   constants, never test fixtures (they match by construction).
4. Layering: mechanism calls route through the designated orchestrator; a
   bare `pub` whose only callers are in-crate demotes to `pub(crate)`.
[the five hard requirements] Your final message is the report.
```

## Escalation triggers — verification acts beyond reading

Two lenses are the floor; add one focused lens per matching trigger. The
quality lever is the change-specific `<...>` checklist, never the lens name.

| Diff touches… | The added lens must… |
|---|---|
| Generated art / clips | Extract frames and READ them; census the money shot. |
| Reducer / liveness / motion state machine | Trace the downstream interaction graph (rebind, sweeps, TTLs, polarity) + provenance of every newly-keyed signal. |
| Public-facing rendered artifact | DRIVE the built page and MEASURE: WCAG in every interactive state, mobile pan, no-JS (#455: state-sweep, not spot-check). |
| Substantial new/reshaped comments | Read each comment against the CODE (accuracy/value/rot/vestigial/self-repetition); report N items each WITH a disposition, never "passed". |
| Interactive TUI flow | WALK each user path end-to-end: first run, failure branches, the no-CLI user (#359). |
| `pixtuoid-hook` (the shim) | Audit the WHOLE shim for the never-panic contract: `args_os()`, bounded reads, every error path exit(0) (#198). |
| Motion / pose / walk-leg | Render and WATCH before the verdict (#61: five regressions shipped past code-only review). |
| A string/layout a painter frames | Render the COMPOSED frame; string-equality tests are blind to framing (#308). |
| `install/` or another CLI's config | Enumerate EVERY resolution axis and re-verify each against that CLI's authoritative upstream IN-SESSION; write ⊆ verify (#338). |
| New source / hook integration | LIVE run or hermetic replay WITHOUT capture-rig convenience flags; event shapes from CANONICAL upstream docs, never a fork. |
| Dedup / "behavior-preserving" refactor | Adversarial-toward-revert, per consolidation: one reason-to-change per call site; enumerate which conversions moved semantics — a batch hides exactly one mover (#461). |
| Physical/domain feature, or an arc's LAST PR | Enumerate the domain invariants, re-derive each across the parameter space — per-task diffs compose into violations no diff shows (#471). |

## Orchestrator notes (diff scope)

Dispatch lenses in parallel, in the worktree, in the background. Verify every
MEDIUM+ finding's premise (read the SHARP-EDGES entry) before coding a fix.
ONE fold commit per round (`plan-miss:` lines for plan misses). Every finding
reaches exactly one terminal state in the PR thread (this list is CANONICAL —
other docs point here): FIXED · REFUTED-with-trace (cite or ADD the sharp
edge) · RE-SCOPED (real and INTRODUCED — or first made reachable — by this
change, and bigger than the PR: the PR is wrong-sized, split or redesign it
until the finding is IN scope) · SURFACED (real and PRE-EXISTING — one line
to the owner, who decides, whether or not this change touched its file; a
pre-existing defect never grows the PR). Agents never file issues;
"acknowledged" is not a state. Sweep at the FINAL merge head; check WHICH commit a bot
re-flag was raised against before re-litigating. Gate on the bot's latest
COMMENT verdict + `mergeStateStatus`, never the check table; an
`<!-- absent-… -->` notice is an unreviewed head. Round caps, blocking bar,
churn budget: the skill's convergence contract.

## Whole-codebase scope — orchestration

Same requirements and verify contract; population = the tree, where the
aggregate-only classes live (cross-PR emergent interaction, drift
accumulation, debt accretion, coverage-topology gaps, invariant erosion,
orphaned surface). Scout the work-list → fan out subsystem finders (site +
raycast RENDERED, not source-read) plus per-factor deep sweeps as their OWN
finders (arch-invariants · concurrency/liveness · security threat-model ·
performance · mutation depth on hot logic · silent-failure · drift ·
deep-modules) → adversarial verify, default REFUTE, an independent skeptic
per finding; security/concurrency get 2–3 differentiated skeptics, majority
to confirm, LOOP-UNTIL-DRY (two consecutive empty rounds) → a completeness
critic ("what modality did we NOT run?" — its answer is the next round of
finders) → dedup + rank, KEEPING the refuted-as-deliberate list → the
disposition sweep + a repo-wide stale-phrase sweep == 0 (`rg --hidden`).
Plus a SYSTEM lens (decomposition, dependency directions) and a DRY census —
architecture erodes BETWEEN PRs.
