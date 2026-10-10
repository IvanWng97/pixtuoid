# Contributing to pixtuoid

Thanks for your interest! PRs are welcome — especially **new themes**, sprite and
decoration polish, and **`Source` adapters** for agent CLIs we don't support yet
(the agent CLIs plus the OpenClaw gateway already wired up are listed in the README).

Before you start, read [`AGENTS.md`](../AGENTS.md) at the repo root (and the
nested `crates/*/AGENTS.md` for the crate you touch). It holds the load-bearing
architecture invariants and conventions. Many things that look like bugs are
documented, intentional design: read the whole item, its doc comment and the
comments on the lines it governs, before changing it.

## Build & test

Requires a recent stable Rust toolchain and [`just`](https://github.com/casey/just)
(`brew install just`). On Linux you also need `lld`, `pkg-config` and the ALSA
headers (`apt install lld pkg-config libasound2-dev`), and, with no GPU driver,
Mesa's software Vulkan for the GPU tests (`apt install mesa-vulkan-drivers`).
The git hooks and most CI jobs call `justfile` recipes.

```bash
just              # list recipes
just preflight    # pre-push gate: lint → clippy; `just preflight full` adds hack → test (CI's Rust recipes)
just fmt          # auto-format
just test         # the whole suite, under cargo-nextest (`just setup-tools`)
just test -p <crate> <filter>   # fast loop while iterating on one crate
```

> **Don't expect clippy to warm `test`'s build** — its check-mode (rmeta)
> builds carry over only build scripts and proc-macros, so iterate with one of
> them.

Activate the git hooks once per clone: `git config core.hooksPath .githooks`
(`pre-commit` = `just fmt-check`; `pre-push` = `just preflight`, lint + clippy;
the tests are CI's).

## CI gates

CI is the gate. Beyond the tests and the feature powerset (`just preflight full`
runs those locally), it runs the jobs below. Preflight's `just lint` covers
only **hygiene**, **zizmor**, cargo-deny and the sprite and banner half of
**generated drift**, so a green preflight does not mean a green PR.

A PR's pushes run only the **light tier** (every job without
`if: inputs.full`: linters, formatters, unit tests on every platform and
compile checks), and its `ci-gate` judges that tier. Both tiers run on the
merge queue's draft PR (`mergify/merge-queue/…`), whose `ci-gate` the queue
merges on, batching up to `.mergify.yml`'s `batch_size` PRs in one run, and on
a push to `main` or a manual dispatch
([two-step CI](https://docs.mergify.com/merge-queue/two-step/)). CodeQL and
CodSpeed skip drafts. The jobs:

- **advisories** — `just deny-advisories` when a PR changes `Cargo.lock` or
  `deny.toml`, and site's and Raycast's `npm run audit` when their
  `package.json` or lockfile changes; `audit.yml` runs all three daily.
- **api-surface** — committed `cargo public-api` goldens at `api/<crate>.txt`;
  regenerate with `just api-surface` + commit when the public surface moves.
- **docs** (`just doc-check`) — rustdoc with `-D warnings` over private items,
  the bins, the examples and each `DOC_TARGETS` triple, plus the doctests
  nextest skips.
- **generated drift** (`just gen-readme-check gen-art-check gen-banner-check
  gen-icons-check compare-selftest`) — generated sprites, icons, the README
  banner and README freshness, and the image comparator.
- **smoke** — the release binaries and the hook shim's silent exit. The
  README's media drift (`just gen-media-check`) is reported in it as evidence,
  not a gate.
- **npm package generator** (`just npm-check`) — the npm package generator +
  OpenClaw plugin contract.
- **msrv** — the workspace compiles on its declared `rust-version`.
- **packaging-build** (full tier) — a clean `cargo install --locked` on both
  Homebrew bottle platforms.
- **coverage** — every tier runs the suite instrumented; the full tier
  uploads it to Codecov, whose statuses are informational.
- **GitGuardian Security Checks** — the GitGuardian app's secret scan, a
  required status.
- **windows-check / windows-test** — msvc cross-lint on every PR, and the
  full suite on a real Windows runner.
- **other-unix-check** (`just check-other-unix`) — FreeBSD cross-lint for
  the other-unix arms.
- **wasm-check** — builds the site's wasm (`just gen-wasm`) and caps its
  gzipped size (`just gen-wasm-check`).
- **site** — `site.yml`: format, lint, types, knip and unit tests on every push; the demo-reading test, e2e and
  Lighthouse on a build with freshly built wasm, so a Rust change that breaks
  a wasm export the page calls fails before it deploys.
- **raycast** — `raycast.yml`: contract-types freshness, `ray build` and `ray lint`, `tsc`, `npm test` and
  `eslint`, run by `ci.yml` only on a PR that changes `integrations/raycast`, the workflow, `setup-npm` or
  `site/package.json`; any other push skips them and `ci-gate` still passes.
- **snapshots** — `cargo insta`; fails on a pending OR orphan `.snap`, the rot
  `just test` can't see.
- **hygiene** — the same `just lint` recipes preflight runs (its CI job exists
  so a skipped local preflight can't land a lint break), including `just ci-observability`
  (`policy/ci-observability/`: contracts for the silent, costly workflow
  failures actionlint and zizmor can't see, and behavior tests of the
  workflows' own shell) and
  `just fixture-pii` (gitleaks over the committed capture tree); the
  capture-tree rules ride `just test` instead.
- **zizmor** — workflow/action security: SHA pins (`actions/*` may float on a major),
  credential-dropping checkouts, exact inline suppressions.
- **The automatic Claude review** rides `claude-readonly-review.yml`: on the
  trusted default branch, a read-only model reads the PR diff, its head files,
  title, body and prior threads as inert data, and spawns the
  [`REVIEW.md`](../REVIEW.md#scope) readers: repeated passes per planned unit,
  one per lens over the whole diff, a verifier per candidate. A unit it skips
  fails the run; a separate least-privilege publisher opens a review thread
  per finding and sets the one required `claude-review` status, with each
  lens's finding count in its description.
- **CodeQL**, advisory (not a required check), stays the advanced workflow (`codeql.yml`): explicit languages,
  a SARIF health gate on Rust's `none`-mode extraction, and an inline query
  filter dropping `rust/cleartext-logging` (WHY on the init step).

## Releasing

### Versioning

Pre-1.0: **patch (`0.y.Z`)** = bug fixes and polish only — no new public API,
nothing breaks. **minor (`0.Y.z`)** = everything else: new user-facing features
AND any breaking change to the published crates' API. Both halves are machine-
applied on the release PR, not per-PR: release-plz derives the level from the
commit log (`features_always_increment_minor` in `release-plz.toml` is the
"features also bump minor" half), and release-plz's own `cargo-semver-checks`
run is the "nothing breaks on a patch" half: a detected break raises the bump
to the next minor on its own, and the release PR's body reports it. Never weaken
a lint to dodge the bump.

### Cutting the release

[release-plz](https://release-plz.dev) owns every version number and the tag;
`release.yml` still owns every publish. Three steps, all human-initiated:

1. **Dispatch** `release-plz.yml` from Actions, on `main`. It opens
   `chore(release): vX.Y.Z` from a `release-plz-*` branch, with the workspace
   version, every path-dep requirement, `Cargo.lock` and `CHANGELOG.md`
   rewritten.
2. **Review it like any PR, but merge it by hand**: the merge queue refuses it,
   since it would squash it onto whatever `main` has become by then, which
   `release-merge` refuses after the merge. If `main` moved, **re-dispatch —
   never "Update branch"**: only a dispatch recomputes `CHANGELOG.md` for the
   new commits, and the merge commit "Update branch" adds counts as a human's,
   so the next dispatch closes this PR and opens a new number. Merged behind
   `main`, or with `main` merged or rebased in, it fails `release-plz.yml`'s
   `release-merge` and publishes nothing. Raise the bump with
   `cargo set-version --workspace X.Y.Z` (cargo-edit) and push only for a break
   `cargo-semver-checks` cannot see. A user-facing change that touched no packaged file
   (`npm/`, `release.yml` packaging) is not in the generated notes — add its line
   to `CHANGELOG.md` by hand as the last commit before merging.
3. **Merge it** (squash). That merge is the *irreversible* step: the `release` job
   publishes every crate to crates.io over OIDC, creates `vX.Y.Z` and a DRAFT
   GitHub release carrying the changelog; the tag then fires `release.yml`,
   which builds every target in its build matrix and the debs, attaches them,
   publishes the draft, and publishes the npm packages. The crates.io and npm
   jobs each wait for an approval from the `release` environment's reviewer,
   crates.io first. The tag also starts a homebrew-core autobump.

The crates.io upload happens in the `release` job on the merge push, which
first waits for that commit's own `ci-gate`: a failure or timeout publishes
nothing, and re-running the workflow after a passing `ci.yml` re-run resumes it.
The wait is in the workflow because the queue can land other PRs while a
release PR is open, so no merge-time check proves the tree it publishes.

`cargo-semver-checks` runs inside release-plz on the release PR, not as a CI
job; `just semver` reproduces its verdict locally.

Both jobs authenticate with `RELEASE_PLZ_TOKEN`, a fine-grained PAT scoped to
this repository with Contents and Pull requests read/write; `release-plz.yml`'s
header says why it cannot be the automatic token.

A release PR that release-plz closes and re-opens (it does that when the branch
carries non-bot commits) leaves a commit you pushed to it — a raised bump —
behind: `git cherry-pick` it onto the new branch. No committed frame carries the
version (`BOARD_BRAND`), so a release PR needs no `just gen`.

The tag also publishes **outside** this repo:
homebrew-core's formula is `autobump: true` and builds from the tag tarball,
instantly, with DEFAULT features on macOS *and* Linux — the one configuration
our release never builds. Two consequences:

- **A from-source build break lands in Homebrew's CI, not ours.** Anything
  adding a system-library dependency needs a matching `depends_on` in the core
  formula, in the same bump PR.
- **Their `test do` block is a public contract** — see the "homebrew-core
  contract" comments at `crates/pixtuoid/src/app/sources_cli.rs`,
  `crates/pixtuoid-core/src/source/codex.rs`. Change homebrew-core's `test do`
  first, against the released version, so the next autobump stays green; the
  packaging-build action replays the block, so it changes in the same PR. A
  `release-plz.yml` dispatch opens no release PR while their block still calls
  a subcommand this tree dropped.

Do not try to preempt BrewTestBot: the formula is on homebrew-core's
autobump list, so `brew bump-formula-pr pixtuoid` refuses by policy and the
bot opens the PR itself within ~3 hours of the tag. Watch THAT PR's CI and
intervene only if it reds.

Publishing uses **OIDC trusted publishing** — CI carries no registry tokens;
the per-crate/per-package Trusted Publishers, each naming environment
`release`, must exist before the tag
([#216](https://github.com/IvanWng97/pixtuoid/issues/216)).

## The arc loop

Non-trivial work runs as an **arc**: design → build → gate → wrap.

1. **Pick** — an issue (`gh issue list`) or backlog item.
2. **Grill the design** — decide the open questions one at a time, each with a
   recommended answer, before writing code.
3. **Design gate** (before build) — three lenses so slop dies in design:
   best-practice search (confirm the idiomatic way against real docs online,
   never memory) · adversarial design review (red-team the design before code
   exists) · deepening lens (would deleting this concentrate complexity or
   just move it? does the change deepen a module or add a shallow one?).
4. **Spec** — synthesize into `docs/superpowers/specs/` (LOCAL, git-ignored)
   and plan against [`impl-plan.prompt.md`](../.github/prompts/impl-plan.prompt.md).
5. **Mock gate** (taste/visual work only) — ratify the AFTER visual before code
   (`beautify-decoration` skill).
6. **Build** — TDD: failing test → minimal impl → commit.
7. **Self-review** — a standards+spec pass before pushing, INCLUDING the
   whole-file comment audit: every file the PR touches — even by one line —
   gets its entire comment population re-read against `AGENTS.md`'s comment
   rules, and the cleanup rides the same PR (population and dispositions:
   [`REVIEW.md`](../REVIEW.md#unit)'s comment audit). Not the merge gate.
8. **Merge gate** — [the gate](#the-merge-gate); the `local-review` skill
   runs its local rows; merging is `@mergifyio queue`, a release PR by hand.
9. **Wrap** — retro; a durable lesson becomes a mechanism (a test, a gate) or
   a line on the narrowest rule it amends — never an agent's private memory,
   which nobody reviews and nothing executes.

**Skills.** Repo skills live in [`.claude/skills/`](../.claude/skills/)
(committed; `.agents/skills/` aliases them for Codex).
On a fresh machine or a non-Claude tool, `git clone` gives you the repo skills
and every `just` gate; this section IS the loop for tools without skills. Do
not scaffold a `CONTEXT.md`/`docs/adr/` convention here — a declaration's own
doc comment is the design record, and the nested `AGENTS.md` says only what its
crate IS.

### The running order

| when | run |
|---|---|
| before code, if non-trivial (new seam / ≥3 files) | plan against [`impl-plan.prompt.md`](../.github/prompts/impl-plan.prompt.md) |
| touched the `--json` / `SourceStatus` / `OutcomeRow` shape | `just gen-contract` |
| before push | the [self-review](#the-arc-loop) (step 7); the pre-push hook runs `just preflight` (never pipe it: a pipe eats the exit code) |
| while the work is in progress | push the branch with no PR: no workflow runs on a push to a branch other than `main`, so a PR-less branch costs the shared runners nothing |
| once the branch is ready to merge and [a PR slot](../AGENTS.md#workflow) is free | open the PR ready: the light tier and the review bots run; a failure only the full tier catches surfaces in the queue, which dequeues the PR |
| once the PR is open, when a REVIEW.md local row matches | the `local-review` skill, its record on the PR |
| once [the merge gate](#the-merge-gate) holds | the session comments `@mergifyio queue` |
| a source/lifecycle change | dogfood against live CC, or replay hermetically (tiers below) |

The e2e tiers live under `scripts/lib/`; none runs in CI. Cheapest first:
`just openclaw-e2e` (hermetic envelopes, free) · `just replay <fixture>` (a
captured rollout through the full headless path) · `just openclaw-multi-e2e`
(N real gateways, free) · `just openclaw-backend-e2e` (one BILLED turn) ·
`just live-sources [id ...]` (one BILLED turn per installed CLI; the only tier
proving a real CLI's output becomes a sprite — sources with no invocation
entry are listed `NOT COVERED`, never skipped silently).

Advisory backstops that surface risk but never gate:
`scripts/check_upstream_drift.py` (wire-format drift) · `just fixture-age`
(which recorded fixtures a local CLI has moved past; LOCAL-only) ·
`just bench` / `just bench-pacing` / CodSpeed (local numbers authoritative; CI benches advisory).

### Parallel sessions

- **One `git worktree` and one cargo target per branch** — a target shared
  across branches swaps uplifted examples and builds one branch's types into
  another. Targets run to several GB each: check `df -h /` before parallel
  builds, and remove a PR's worktree and local branch once it merges.
- **Fold before opening** — a change to a surface an open PR already touches
  folds into it.
- **The queue never idles** — it checks one batch at a time (`.mergify.yml`'s
  `max_parallel_checks`), so queue every PR that holds the gate, in priority
  order, at once; dequeue one only when it would jump a priority PR that is
  already green.

## Conventions and architecture invariants

Both live in [`AGENTS.md`](../AGENTS.md) ("Conventions", "Architecture
invariants"), which every contributor and agent reads first.

## Pull requests

- Review rules: [`REVIEW.md`](../REVIEW.md).

### The merge gate

Green `ci-gate`; the required `claude-review` status
`success` at the final head, from a published review or Dependabot's policy
exemption; every finding's review thread resolved by its
disposition; zero open confirmed `issue (blocking)`; each matching
[local row](../REVIEW.md#escalation)'s run recorded as a PR comment starting
`<!-- local-row:<row>:<head sha> -->`, where `<row>` is the row's first column
up to any colon or parenthesis, lowercased, each run of non-alphanumerics one
`-`, leading and trailing `-` dropped, and the sha is the head the run judged;
an update that only merges `main` in, or whose every changed line is a comment
or prose, leaves the record standing. The
[`local-review`](../.claude/skills/local-review/SKILL.md) skill runs those
rows. A published review passes whatever it found; a failed or missing status
is no review: comment `/claude-review`, else split the PR smaller.

Once the gate holds, comment `@mergifyio queue` ([`.mergify.yml`](../.mergify.yml)):
entry is a command because no queue condition can confirm a finding or match a
local row. The queue tests up to `batch_size` PRs together on a draft PR
(`mergify/merge-queue/…`) running the full tier, then merges the PRs
themselves; it never updates a PR's own branch
([batches](https://docs.mergify.com/merge-queue/batches/): "the original PRs
are the ones merged"), so the bots' statuses on the PR's head are the ones its
`queue_conditions` read. [`media-regen.yml`](../.github/workflows/media-regen.yml)'s
bot PR (`bot/media-regen`, `docs/images/` only) queues itself and needs no
generated-art record: it renders main's merged code, which each look PR's lens
already read as evidence.

The bots never review a fork PR on their own: a maintainer approves its CI
run, then comments `/claude-review`, again after every push. Its author can
resolve their own threads, so before merging read each thread's `resolvedBy`
and its reply. Its bot verdict is advisory, since the
author can steer it through the diff, so the maintainer reads the diff too.
A Dependabot PR is not reviewed: [`claude-review.yml`](../.github/workflows/claude-review.yml)'s
`exempt` job posts the `claude-review` status when every commit is
Dependabot's and the PR only moves versions in its manifests or an action's
`uses:` pin, with no
package, install script or source new to it. Anything else gets `pending`, and
a maintainer comments `/claude-review`.

### Dispositions

Every finding reaches exactly one terminal state in its review thread: FIXED ·
REFUTED (cite the mechanism, per AGENTS.md; add one where none exists. Before
adding code for a finding, establish its case is reachable: when a test or
sweep shows it isn't, that test is the mechanism and no defensive code lands) ·
RE-SCOPED → #N (real and INTRODUCED — or first made reachable — by this
change, and bigger than the PR: split it off into #N; a redesign that brings
the finding into scope ends FIXED) · FOLLOW-UP → #N (real and PRE-EXISTING,
or introduced, non-blocking, no bigger than the PR and found after round 2
per the [convergence contract](#convergence-contract), and not FIXED in place
— in place fits a small defect inside code this change already touches,
adding no local row — so it is fixed in #N; a defect in another session's
tree cites that session's PR). A
disposition is the reply that resolves the thread, STARTING with its state:
`FIXED: …` · `REFUTED: … — <mechanism>` · `RE-SCOPED → #N: …` ·
`FOLLOW-UP → #N: …`, where #N is a PR other than this one: open, merged, or
closed under [the open-PR cap](../AGENTS.md#workflow) with the fix on its branch. A
re-flag of an already-dispositioned finding replies with the original's
disposition (link it). "Acknowledged" and "surfaced" are not states. Sweep at
the FINAL merge head; check WHICH commit a bot re-flag was raised against
before re-litigating.

### Convergence contract

- **Churn budget** — a diff whose added + modified lines exceed ~1500 is split
  (stacked PRs) before review. Pure deletions are exempt once censused; a
  change that both adds and deletes at scale is two PRs.
- **Deletion census** — before deleting N members of a class, the full list
  and its criterion land in the first commit or the PR body (#943).
- **Fixes are never put off; severity ends the loop.** Rounds 1 and 2 each
  fold every accepted finding, whatever code it names, into ONE commit: a
  defect this change introduced is FIXED in it, since a clean-up left for
  later rarely lands ([Google](https://google.github.io/eng-practices/review/reviewer/pushback.html#cleaning-it-up-later)).
  After round 2 a fold takes only what [the merge gate](#the-merge-gate)
  confirms blocks the merge, and a text-only fix (a comment, a doc or a
  user-visible string, no behavior) to a line this change introduced; any
  other non-blocking finding is a FOLLOW-UP: a
  review moves on once only non-blocking suggestions remain
  ([GitLab](https://docs.gitlab.com/development/code_review/)). A blocking
  issue confirmed in a fold STOPS the loop: revert the fold and re-land
  smaller, or re-scope.
- **A fold's behavior change** ships a test that fails without it; the rest
  of a fold is a revert, a deletion, a comment or doc change, or a refactor
  the existing tests cover. Anything else reverts the fold.
- **A fix round adds no new gate** — a wanted check is its own PR, asserting
  facts in its own layer (a Rust fact from Rust, never a Python regex over
  `.rs`).

### Handy `gh` commands

```bash
gh pr checks --watch                         # live CI status
gh issue develop <number> --checkout         # branch linked to an issue
gh run rerun --failed                        # rerun only failed CI jobs
```

## Adding a new agent CLI

The registration steps (4–7, 9) and step 12's roster literals are test-forced —
skipping one fails `just test`. Step 8 is forced only for hook-only sources;
step 10 by the theme guards; steps 1–3, 11 and step 12's `#[test]` are on you.

1. **Verify the wire format against the CLI's actual source/releases first** —
   transcript location, line shape, hooks, session identity; pin every fact
   to an upstream file/version. **Audit its HOME RESOLVER per axis in the
   same pass** — PROBE the installed artifact rather than trusting docs; an
   unmirrored axis is fail-silent: the watcher polls a directory the CLI
   never writes and the office stays empty (#880). Resolver axes are
   deliberately NOT drift-watched — re-run the probe matrix when the CLI majors.
   A custom root gets ONE `pub fn <cli>_home()`, called by both the watcher's
   `default_paths()` and the installer's `default_config_path()` so they
   can't disagree.
2. **Write the source module** — `crates/pixtuoid-core/src/source/<name>.rs`:
   `SOURCE_NAME`, a `LineDecoder` fn (one JSONL line → `Vec<AgentEvent>`), a
   label deriver, unit tests per event mapping. Format knowledge lives HERE.
3. **Implement the `Source` trait** (an async `run(self, tx)` watching +
   decoding until the session universe ends). **Hook-only CLI?** Skip the
   decoder, trait, and step 7: `transcript: None` in the registry row, format
   knowledge in a `hook.custom` decoder (it must claim EVERY event), and do
   step 8 instead.
4. **Add ONE `SourceDescriptor` row** in `source/registry.rs` — label prefix,
   decoder, hook keying, `tool_id_key` (verify against a CAPTURED tool call,
   not a neighbour — kimi's `ToolCall` cost a source its tool ids), truthful
   capability flags, `verified_version` + `version_probe`. Lifecycle policy
   derives from the flags; you do **not** edit the reducer.
5. The descriptor's `name` **is the roster** — `registered_source_names()`
   projects `REGISTRY`, and the conformance suite then requires a fixture. The
   `sources --json` golden (`crates/pixtuoid/tests/snapshots/cli/sources.json`)
   must list it: `SNAPSHOTS=overwrite just test -p pixtuoid --test cli_json`.
6. **Record the fixture** — the test steps in
   [`crates/pixtuoid-core/tests/AGENTS.md`](../crates/pixtuoid-core/tests/AGENTS.md)
   (a RECORDED SessionStart scenario via `just capture-fixture`), then
   `cargo insta review`.
7. **Wire it into `runtime/driver.rs::build_source_set`** (the one
   construction site; the registry drives the guard test, not the spawning).
8. **If the CLI has hooks**, add an `install/` target (a `Target` row +
   `merge_install`/`merge_uninstall` + a `verify_schema` fn mirroring the
   target's own config format + the registered-events↔decoder-arms guard).
9. **Add a row to `site/src/sources.json`** (`status`, per-OS `platforms`), then `just gen-readme`. Pinned to `registered_source_names()`
   by `supported_sources_manifest.rs`.
10. **Add the per-source badge hue** — a `SourceColors` field + value in EVERY
    theme file + `badge_color` in the manifest row; the coverage, legibility
    and site-bridge tests fail until it exists.
11. **Drift-watch in the same PR**: a `check_upstream_drift.py` row where one
    is owed — which surfaces owe
    one is `source/drift.rs`'s header, read it there. A row is four steps: the
    const, the `insert` in that crate's `src/drift_surface.rs`,
    `just gen-drift-surface` (commit both fragments), and the `SURFACE_ROWS`
    row plus its selftest case (the case census fails without it).
12. **Three roster literals in three test binaries** (a scoped run misses
    them): the row-by-row byte pin in `corpus_check.rs`; `TOOL_ID_KEY_UNPROVEN`
    in `tests/sources/captures.rs`; a case row + `#[test]` in
    `crates/pixtuoid/tests/wire_to_pixels.rs`.

## License

By contributing, you agree your contributions are licensed under the same terms
as the project.
