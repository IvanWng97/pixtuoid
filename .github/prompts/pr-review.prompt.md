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
  no-scan-the-history rule; blocking I/O or lock acquisition on an async worker /
  the render loop — sync flock/fsync/FS probes need `block_in_place` or an
  off-loop task, the Sources-panel ConfigLock stall escaped as a dropped #283
  bot MEDIUM; creation POLARITY — only a proof-of-LIFE event may create/resurrect
  an entry for an unknown id, a death/exit/TTL signal for an absent id must
  no-op, never synthesize: a shared `or_insert` before the match minted a
  phantom mascot from `PidExited` (fbe26049); lifecycle-AUTHORITY —
  user/model-controllable CONTENT (transcript text, message bodies, tool-arg
  fields) must never drive lifecycle/state transitions, structural markers +
  liveness signals only: the `/exit` content-matcher false-positived on messages
  QUOTING it, and a model-authored `subagent_type` spoofed a delegation — both
  read as clever resilience in review); error-handling & silent failure (incl.
  degenerate env/config values: a SET-but-EMPTY env var reads as unset — the
  #172 RUST_LOG policy; route env reads through `io::nonempty_env` — a raw
  `var_os` read of XDG_CONFIG_HOME resolved the sprite-pack dir to CWD,
  18ebf937); security (path traversal; unchecked wire input; config-write
  safety — atomicity AND no destructive fallback: no error/skip/default arm may
  rewrite, wipe, or strip user-owned configs/hooks — existing-but-unparseable is
  NEVER rewritten, a skip must not remove pre-existing hooks, a per-field typo
  must not fail the whole file, a wipe mode that escaped review twice;
  terminal-egress sanitization — strip BOTH Cc controls and Cf bidi overrides,
  `is_control()` covers only Cc and Trojan-Source CVE-2021-42574 rode the gap
  (4bb786fa); local-IPC endpoint security — socket/pipe perms at creation
  (create-restricted-then-rename, never a process-global umask: it races other
  threads), squat/steal arbitration, and predictable rendezvous paths in
  world-writable dirs — prefer XDG_RUNTIME_DIR/0700-dir placement, treat a
  pre-existing endpoint as hostile, the `/tmp/pixtuoid-{uid}.sock` fallback
  class); the architecture invariants (core=no-terminal, scene=no-window, ONE
  Transport-tagged channel, walkable=footprint, no direct settings.json write,
  no prod `println!`, `unreachable_pub`); magic-number / single-source-of-truth;
  cross-platform (path-string separators, Windows parity; resolution-POLICY
  mirroring — each integrated CLI resolves home/config/env its OWN way
  [HOME-first vs USERPROFILE-first, %APPDATA% vs `~/.<cli>`, verbatim vs
  ~-expanded overrides], so the generic dirs/shellexpand answer IS the bug:
  mirror the target CLI's own resolver, install/detect/watch reading ONE
  resolver — #343/#342/#195); sibling-set completeness (the N-1-of-N class: a
  guard/cap/validation added to some but not ALL sibling paths — per-source
  decoders, per-CLI install targets, per-platform arms, twin call sites; the
  most-recurrent escape across all three censuses: #272's decode cap missed 3
  hook-only decoders, then copilot's twice more — ledger R0620-364-04,
  R0620-WCR-01); performance (per-frame allocs, hot-path scans);
  resource/lifecycle (Drop, fd leaks, unbounded growth; error-path ROLLBACK — a
  multi-step setup mutating global/terminal state must unwind the
  already-applied steps when a LATER step Errs, a plain Err bypasses the
  panic-hook restore: the raw-mode strand, a976c604); upgrade-path /
  installed-base (state written by previously-RELEASED versions — settings.json
  hook entries, sentinels/connected flags, installed command strings; a
  fresh-install assumption wipes an upgrader's — #457's HIGH: onboarding SKIP
  stripped pre-0.12 hooks, fixed by freezing to the REAL state; a compat-path
  removal must name its concrete surviving population, an empty set being the
  only free removal — #447's purge rulings); test-teeth (does the pinning test
  FAIL if the behavior breaks — mutate it); declared-not-wired (every NEW
  field/flag/check/gate the diff declares must trace to a live consumer — the
  smells: an `_`-bound capture, a built-but-never-called validator, a gate with
  zero CI reach; #61's `_snap_prev` defeated its own PR's fix AND survived #62's
  dedicated fix-round review; `just arch` had zero CI reach until #273).
- **(B) Design-debt** — duplication/DRY (N implementations of one concept —
  weight by DIVERGENCE risk, not line count); god-object / oversized module with
  a clean split; dead code / legacy remnant; **unnecessary fallback / dead
  default** — a defensive arm (`unwrap_or`/`map_or`/catch-all `_ =>`/`.or_else`/
  retry) whose trigger CANNOT occur, or that DUPLICATES — worse, CONTRADICTS — a
  source of truth, is debt not safety: fallback only when a REAL failure mode
  needs it. The INVERSE of negative-space rule 2 (which forbids demanding
  defense-in-depth where a primary defense exists) — here, don't ADD or LEAVE a
  guard for a state that can't arise. The 0.13 slimming audit found a
  `Furniture::Desk` footprint `unwrap_or(Size{DESK_W,DESK_H})` both dead (the
  const table is always `Some`) AND WRONG (`decor.rs` is `DESK_W+4`, so a
  hypothetical None mis-placed the sprite), plus the `(7,4)` table footprint
  re-hardcoded inline in two more painters — read the authority (`if let Some(..) =
  furniture_def(x).footprint`), never re-copy its fallback. The test is "can the
  trigger fire, and does the arm duplicate/contradict an authority", NOT "does it
  look defensive": load-bearing defense STAYS (documented sharp edges, liveness
  ladders, the shim exit-0 contract, clock-regression `duration_since` folds,
  config-never-wipe); leaky or missing abstraction;
  correlated-state bundling — N fields that ALWAYS change together (a phase + its
  clock + its profile; a liveness axis + its run-set; a decoder + the extractor it
  pairs with) belong in ONE struct/newtype so an illegal combination is
  UNREPRESENTABLE, not merely co-maintained; the recurring win here is exactly
  this (`WanderState` folded 9 flat wander fields; `DaemonPresence` STORES the
  orthogonal axes + PROJECTS `Busy` from the run-set so a 4-site hand-sync can't
  drift; the `Transcript` bundle makes the line-decoder↔cwd-extractor pairing
  structural; the desk-index newtypes) — a manual N-site sync, or a bool/enum pair
  that can contradict, is the smell; inconsistent pattern where one way is clearly
  the house style; misleading
  identifier — a name that lies about what it holds/does (ask: would a human
  reading the logic be misled? one edit from an unrelated sibling in the same
  struct, `meeting_rooms` vs `meeting_room`; a name inverting documented
  vocabulary, `approach_footprint` returning the VISUAL extent vs invariant #6;
  a field named `name` holding an id that invites the wrong join). NOT "pure
  style" (negative-space rule 2 doesn't apply) — the #321 naming pass confirmed
  6 such renames (R0615-NM1..6), and a rename isn't done until the repo-wide
  stale-prose grep is clean.
- **(C) Drift** — doc↔code (a `CLAUDE.md`/README/SKILL/prompt naming a
  file/fn/flag/count that moved — the population INCLUDES the hidden dirs
  `.github/prompts/` and `.claude/skills/`, which bare `rg` skips by default;
  sweep with `rg --hidden` or `grep -rn`, else the class recurs: #448's
  post-merge stale-`line_decoder` MEDIUM and #449's beautify-SKILL recurrence
  were both hidden-dir misses); wire-format / upstream (a decoder or drift-watch
  rule vs the real upstream shape; a new source with no drift-watch row;
  install-path resolvers are an upstream surface too, not just decoders — and an
  AUDIT sweeps EVERY registered source, not just a changed one: each source's
  decoder vs its live upstream wire shape + a PRESENT, ALIVE `check_upstream_drift`
  row, since the watch itself fail-open behind its own self-test false-greened two
  sources through the wrong decoders for weeks, #454);
  version-lockstep (Cargo.toml versions, MSRV, the "N sites must stay in sync"
  invariants; version ADJUDICATION — the contract half semver-checks can't see
  (CONTRIBUTING.md "Releasing": patch=fix/polish, minor=feature OR breaking):
  does the diff move a published crate's public surface or ship a feature, and
  does the number bump in THIS PR, ride the already-open unreleased minor, or
  stay put? over-bumping a break the open minor already covers is the same
  finding — the #471 premature-0.14.0 catch; Lens 2 states this call explicitly
  in its verdict); comment-rot (a comment that now says something FALSE about the
  code) AND comment-value (the house rule "No comments unless WHY": a comment
  restating WHAT the code plainly does, or narrating an obvious step, is noise to
  cut — only a non-obvious WHY earns its place: a workaround, a load-bearing
  constraint, a surprising invariant; a NEW block of `//` narration added by the
  diff that a reader could infer from the code is the smell. Even a LEGITIMATE WHY
  must be right-sized + right-placed: **fn-body comments ≤2 lines**, a longer
  rationale relocated onto the DECLARATION it explains (a `const`/fn/module doc
  can carry a ≈25-word rationale) or a `CLAUDE.md` sharp edge, and any present-tense
  code-state assertion moved into a test — the comment keeps only `pinned by
  <test>`. A ≥3-line `//` run wedged inside a fn body is the flag); manifest-bridge (a
  `site/src/*.json` / generated schema vs its Rust source of truth).
- **(D) Quality + tooling** — test-coverage gaps (changed/existing code with no
  exercising test); mutation-teeth (assertions that survive the mutation — and a
  PROSE CLAIM that a mutant is EQUIVALENT or KILLED is not teeth, it's an
  unverified assertion: pin an equivalent in `.cargo/mutants.toml`'s `exclude_re`
  [mechanically re-checked every run] or kill it with a real test, NEVER trust a
  code comment; arc #2's mutation run found 2 live reducer survivors masked by
  exactly such comments — one citing a "residuals note in tests" that didn't
  exist, one whose "this kills the mutant" assertion couldn't distinguish the
  branches [`duplicate_root_session_start_..._resurrect` + `parent_waiting_..._ends_a_tool` fixed both]);
  isolation & flakiness (real-state writes, wall-clock/order nondeterminism,
  `TEST_ENV_LOCK`, snapshot determinism); CI/build (gate coverage, path-filter
  holes, toolchain skew); gate-teeth & gate liveness (can this check actually
  FAIL, and is it alive: a checker that exits 0 on its own internal error is
  fail-open; an exit code read through a pipe / `; echo $?` is eaten; a check
  never wired into a required workflow gates nothing — the `.harness` fail-open
  + unwired case; a check that passes VACUOUSLY or a scheduled monitor red with
  no consumer — ask of each gate "what makes this pass without checking?", and
  an audit sweeps the run history of every scheduled workflow: the weekly drift
  watcher was dead for weeks behind its own self-test and decoder_fuzz
  false-greened two sources through the wrong decoders, #454; the security cron
  silently red, #440); dependency/supply-chain (unmaintained/droppable deps,
  duplicate versions, feature-flag hygiene; FRESHNESS — a dep meaningfully behind
  its latest release misses upstream bug/security fixes [`cargo outdated`], a
  DISTINCT axis from the daily `cargo deny check advisories` RustSec gate (that
  catches KNOWN vulns, not staleness); and the `deny.toml [advisories]` IGNORE
  list is itself audited — every ignored `RUSTSEC-*` id is re-justified or
  dropped, since a stale ignore silently hiding a now-fixable advisory is the
  fail-open form; an upstream-blocked dedup with no clean bump is TRACKED not
  churned — #486).

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
So a diff lens still runs the DRY / drift / data-shape / sibling-completeness
checks below (they `grep`/`rg` OUTSIDE the diff — the one thing a diff-scoped
read can't do by construction), and the audit adds finders dedicated to the
aggregate-only classes.

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
   `checkout@v6` case). Both existed.
3. **Integer confidence 0–100 + `file:line`** on every finding.
4. **Sharp-edge check** — match familiar-smelling claims against the
   per-crate `CLAUDE.md` "Known sharp edges" (the live, maintained record of
   deliberate-design refutations; premise-anchored: same seam ≠ same claim).
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
3. Sibling-set completeness: for every guard/cap/limit/validation the diff
   adds or extends, enumerate the FULL sibling set (`rg` for the parallel
   decoders / install targets / platform arms / twin call sites — siblings
   live OUTSIDE the diff by construction) and verify each member got the
   same treatment. This sweep must fire at the INTRODUCING PR even when the
   plan enumerated siblings (impl-plan item 3 existed and copilot #292 still
   shipped the N-th uncapped decoder — backstopped at #364, not here).
4. Tests don't lie: for every behavioral claim, check the pinning test would
   FAIL if the behavior broke (mentally mutate the fix; a test that survives
   deletion of the guarded constant pins nothing — the CONN_TIMEOUT lesson,
   ledger R0610-06). Also trace every NEW field/flag/gate the diff declares
   to the consumer that reads it — a declared-but-unwired artifact passes
   every test that ignores it (the compiler won't warn on `_x` bindings or
   `pub` fields).
5. Run the gates yourself: `just <fmt-check|site-check|preflight>` as
   applicable — do not trust the author's claim of green. Include the EXIT
   CODE you observed (never infer it through a pipe). Then NAME any CI-ONLY
   gate this diff can plausibly red — local preflight is blind to all of
   them: semver (an intentional API change needs the 0.x minor bump in THIS
   PR, never a revert), gen-check (a look-changing diff ships regenerated
   docs/images + site demos; a scene change stales the wasm), wasm-check
   (host-green ≠ wasm-green — `just hack` can't see wasm32-only breakage),
   windows-test (path-string asserts), insta orphan snapshots — and remember
   `--lib` builds neither the bin modules nor examples (a bin-module edit
   verified with `--lib` alone is UNVERIFIED).

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
   included.) Same item, the JOIN
   direction: when the diff looks one existing collection's id up in
   another's key-space (registry→install-target, manifest→registry), verify
   the join key against the REAL production constants — never the tests'
   fixtures, whose hand-fed ids match by construction: registry id
   `claude-code` joined against target name `claude` rendered the flagship
   CLI non-actionable while fixtures fed `"claude"` and stayed green (ledger
   R0613-16, CRITICAL — pin the join with an every-id-resolves bridge test).
   Corollary: eviction/reconcile keyed by ITERATING a registry misses
   out-of-registry members — key on the COMPLEMENT of the allowed set
   (R0613-18, CRITICAL).
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
   orchestrator," not "leave it public." The lint ALSO misses a plainer case the
   same demotion fixes: a bare `pub` fn/type whose only callers are IN-CRATE, in
   a `pub` module tree (the path is pub-reachable, so `unreachable_pub` stays
   silent) — demote to `pub(crate)` even with no orchestrator involved (the
   `verify_target`-vs-`has_hooks` least-privilege slip, whole-codebase audit round 2).

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
  graph (rebind, sweeps, TTLs, and every event arm's create-on-unknown-id
  policy vs its polarity) rather than the diff — and audit the PROVENANCE of
  every signal the diff newly keys lifecycle/state on (content-derived = the
  lifecycle-authority class in Family A).
- **Public-facing artifact** (site page, README section, release notes) →
  add an editorial lens reading as an outside engineer, checking every
  number against its source. If the artifact RENDERS (site page/component/
  CSS), the editorial read is not enough — add a rendered-runtime lens:
  build and DRIVE the real page, then MEASURE — WCAG contrast in EVERY
  interactive state (rest/hover/focus/reduced-motion, not just rest), a
  horizontal-pan sweep at mobile viewports, the no-JS strand, and
  @supports/legacy-browser fallback arms. #453's diff-scoped "whole-site
  review" shipped WCAG failures, a mobile pan, and a no-JS blackout — the
  #455 rendered re-audit found them, and its RE-RUN rendered lens caught the
  CTA :hover 4.17:1 AA failure that two static lenses AND a rest-state
  render missed: state-sweep, don't spot-check.
- **Interactive TUI flow changed** (onboarding, panel actions, popup gating,
  keybind dispatch) → add a UX / user-journey lens that WALKS each user path
  end-to-end — first run, every failure branch, the no-CLI user, repeat
  launches — rather than reading the diff. On PR #359 (onboarding) the two
  mandated lenses returned APPROVE with 0 findings while this lens confirmed
  a HIGH — Confirm discarded the `apply_choices` outcomes, silently losing a
  hook-install failure (R0619-02) — plus the version popup muted FOREVER for
  a no-CLI user (R0619-01).
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
- **A string or layout that a PAINTER frames changed** (footer/HUD text, panel
  columns, popup copy — even a text-only diff) → render the COMPOSED frame
  before the verdict (the snapshot example with the relevant flag). A
  string-equality unit test is blind to the painter's framing: #308's
  `footer_warning` embedded its own ⚠ while the footer painter (`footer.rs`) owns the glyph —
  its test asserted the string WITH the glyph, both review rounds passed it, and
  live rendered `⚠ ⚠` (caught post-merge only via `snapshot --drift-warning`,
  R0615-22); #315's new 2-col health flag shifted data rows but not the static
  "Live" header (R0615-32). The doctor→painter boundary is a two-incident escape
  surface; the motion trigger above does NOT fire on static text.
- **Diff touches `install/` or writes another CLI's config** (the surface that
  mutates OTHER CLIs' user-owned files — PR #23's traversal lived here) → add a
  per-axis upstream-mirroring lens: enumerate EVERY resolution axis (home order
  HOME-vs-USERPROFILE; config-dir API — %APPDATA% vs `~/.<cli>`; env overrides
  verbatim-vs-~-expanded; legacy-dir fallbacks; command form round-tripping the
  target shell — cmd.exe quoting/8.3, #195) and re-verify each against that
  CLI's AUTHORITATIVE upstream source IN-SESSION; install/detect/watch must read
  ONE resolver, and write ⊆ verify (#387/#332). Both #338 lenses checked only
  the HOME-vs-USERPROFILE axis and missed the %APPDATA% HIGH (reasonix wrote
  `%USERPROFILE%\.reasonix`, the CLI reads `%APPDATA%\reasonix` →
  installed-but-no-sprite, R0616-338-08); divergence shipped that class twice
  (#343, #342).
- **New source / hook-only integration ships** → demand a LIVE run (or hermetic
  replay: `scripts/replay-fixture.sh`, the live-e2e scripts) WITHOUT the capture
  rig's convenience flags before the verdict — a `-C`/forced-workspace capture
  masks identity-field fallback paths. R0613-05 (env-mode cwd keyed solely on
  `DEEPSEEK_WORKSPACE` → cwd-less envelope, NO sprite) passed the unit tests,
  the 3-lens review, the whole-shim audit AND the bot — caught only by live use.
  And ground event shapes in the CANONICAL upstream docs, never a fork's
  permissive source (R0613-14: the fork's `{plugin,plugins}` glob masked the
  `plugins/`-only canonical path).
- **Dedup / consolidation refactor ships** → add a wrong-abstraction lens
  ADVERSARIAL TOWARD REVERT, one pass per dedup the diff made: do all call sites
  of the new shared abstraction share ONE reason-to-change? Sound shape shares
  only the domain-neutral part — divergent selection/policy stays inline
  ("duplication is cheaper than the wrong abstraction"). Distinct from Lens 2
  item 6, which HUNTS new duplication; this validates consolidations the PR
  SHIPPED, which byte-identity can't judge (it proves behavior, not future
  divergence). #350's per-dedup adversarial pass held all 26 dedups; the two
  borderline calls are adjudicated on record — demote, don't re-litigate
  (R0617-DV-01/02).
- **Diff claims a "behavior-preserving" refactor** (batch dedup, fields→enum
  reshape) → don't accept the aggregate proof: a green suite + gen-check 0-drift
  can't see a decoder-INPUT semantic move. Enumerate PER CALL-SITE which
  conversions are mechanically identical and which semantics MOVED — a batch
  dedup tends to hide exactly one mover (#461: of the six `first_present_str`
  conversions, antigravity's `.or_else` chain had stopped at a
  present-but-non-string key where the shared scan falls through; APPROVE across
  every local lens, caught only by the online review, fixed 69934644). Each
  mover gets its own red-first pin, and a state-shape reshape gets the owning
  crate's continuity guard (scene `CLAUDE.md` "When refactoring").
- **Feature models a physical/domain system, or this is the LAST PR of a
  multi-PR feature arc** (lighting, sky/celestial, weather, walk kinematics —
  anything with real-world invariants: day>night, occlusion, ordering across
  weathers/phases) → add a whole-FEATURE invariant audit: ENUMERATE the domain's
  invariants FIRST, then re-derive each over the FINISHED feature across the
  parameter space (time × weather × phase), not the diff. Per-task diffs each
  read consistent while the composition violates physics — this sits BETWEEN the
  two scopes (too late for any diff lens; the whole-codebase audit doesn't run
  at arc completion). The sun/moon arc's end-of-arc physics audit (#471,
  77efd54f) caught 5 such bugs every per-task two-lens review structurally
  missed (stars gated on darkness instead of `Body::Moon`; Rain disc 0.20 >
  Overcast 0.05; the disc bleeding through the pillar). Distinct from the
  film-critic trigger: committed stills pin ONE instant — these bugs live in
  parameter combinations no baseline renders.

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
became a 0.4.1 release-blocker (#46); two more drop
cases were logged. Commit skew cuts both ways: run the
sweep against the FULL online thread at the FINAL merge head, not the commit
the local lenses ran on — a bot finding that lands on a later commit after
the sweep is the silent-drop class (#283's blocking-executor MEDIUM,
dropped→#330; #383's terminal-by-code-only MEDIUM). Conversely, before acting
on or re-litigating a bot re-flag, check WHICH commit the bot reviewed —
#316 burned ~4 rounds on five already-fixed findings re-raised from an old
commit (R0615-OCP1..5, all REFUTED-STALE). After a fix round, re-run the
gates and watch the NEW head's CI — and gate the merge on the online bot
review's LATEST COMMENT verdict (`Findings: N`) plus `mergeStateStatus`,
never the check table: the claude-review JOB is green even when it posts
findings (#448 merged past a fresh MEDIUM; #449 onward reads the comment
verdict).

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
     tooling; site + integrations/raycast — the non-Rust consumers, with site
     pages RENDERED per the public-facing trigger, not source-read only —
     #453 proved this surface is structurally unaudited otherwise. This finder
     carries its OWN checklist: the `--json` / `SourceStatus` / `OutcomeRow` wire
     contract parity (both consumers vs the Rust source of truth + the committed
     schemas), hand-CSP soundness, a11y, `knip` dead-code, the site/raycast npm
     deps' freshness, and `just site-check` / `site-e2e` gate liveness — the
     cargo-centric sweeps below don't reach a Node project).
   - *Whole-tree specialist DEEP-sweeps*, one FACTOR each across all crates,
     each run at DEPTH as its OWN finder — NOT folded into a subsystem finder.
     (The 2026-07 v1 whole-codebase audit under-ran these by folding
     concurrency/security into the subsystem sweeps and surfaced almost
     nothing on them; a dedicated deep pass per factor is required, because a
     subsystem finder juggling six lenses structurally can't hunt a
     cross-transport race or a threat-model to depth.) The set:
       - **arch-invariants** — the 6 workspace invariants + crate DAG +
         unreachable_pub hygiene.
       - **concurrency & liveness** — races, lock-order, lost wakeups,
         Send/Sync, the multi-transport hook-wins dedup / child-ledger /
         un-claim side-channel, `block_in_place`, the watchdog threads, task
         lifecycle + ordering across the one mpsc seam.
       - **security threat-model** — untrusted input in the jsonl/hook
         decoders + shim stdin, path-traversal (the #23 class), TOCTOU on the
         config lock, command-injection in the OS focus glue (spawns
         swaymsg/hyprctl/wmctrl), env-var + deserialization handling.
       - **performance** — the per-frame paint path (recolor / frame_cache /
         allocation hotspots), reducer per-event cost, A\* pathfind.
       - **test-assurance depth** — actually RUN `cargo-mutants` on the hot
         logic (reducer / decoder / layout) and report survivors; map coverage
         gaps; flag teeth-less assertions (`just mutants` is diff-scoped — the
         audit runs it broader on the hot modules).
       - **error-handling / silent-failure** — swallowed errors + inappropriate
         fallbacks (EXCLUDING documented-deliberate ones: the shim's always-
         exit-0, the watcher's log-and-continue).
       - **drift** — cross-PR docs/comment/wire drift (bare `rg` skips hidden
         dirs — sweep `.github/`/`.claude/` too).
       - **deep-modules** — shallow-module / deepening-opportunity scan.
3. **Verify — adversarially, default REFUTE.** Each finding gets an independent
   skeptic prompted to refute it: read the cited code + callers, check it against
   the per-crate `CLAUDE.md` "Known sharp edges" (a documented sharp edge → REFUTE
   and cite it), and construct a concrete repro (inputs/state → wrong outcome) or
   refute. A slightly-off `file:line` is not grounds to refute a real defect —
   locate the real line. (Unbiased candidate recall first, verification second:
   many finders, then the skeptics — never one pass doing both.)
   **Vote weight scales with blast radius.** A security or concurrency-
   correctness finding gets 2–3 INDEPENDENT skeptics with DIFFERENTIATED lenses
   (does-it-repro / does-a-sharp-edge-excuse-it / is-the-race-actually-
   reachable), confirmed only on a majority — a lone skeptic's miss on a safety
   finding is the expensive class. **The security + concurrency sweeps run
   LOOP-UNTIL-DRY**: re-fan each until two consecutive rounds surface nothing
   new — the tail of a race/vuln hunt is exactly where the real one hides, and
   a single round systematically misses it.
4. **Completeness critic** (before the report): one agent asks "what modality
   did we NOT run — a sweep skipped, a claim unverified, a hot file unread, a
   non-Rust surface un-rendered, a `cargo-mutants` module not run?" Its answer
   is the next round of finders, not a footnote to the report.
5. **Dedup + rank** the survivors (a defect two cells find is one finding); ship a
   report ranked by corrected severity, grouped by factor family, and KEEP the
   refuted-as-deliberate list in it — it proves coverage and keeps the next
   agent's sharp-edge context accurate.
6. **Disposition sweep** — the shared terminal-state rule below (FIXED /
   REFUTED-with-sharp-edge / ISSUE-FILED); apply small fixes in-arc, defer only
   big/refactor; end with the repo-wide stale-phrase sweep == 0 (`rg --hidden`
   or `grep -rn` — bare `rg` silently skips `.github/`/`.claude/`).

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
