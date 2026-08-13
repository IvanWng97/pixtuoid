# Contributing to pixtuoid

Thanks for your interest! PRs are welcome — especially **new themes**, sprite and
decoration polish, and **`Source` adapters** for agent CLIs we don't support yet
(the ten agent CLIs plus the OpenClaw gateway already wired up are listed in the README).

Before you start, read [`CLAUDE.md`](../CLAUDE.md) at the repo root (and the nested
`crates/*/CLAUDE.md` for the crate you touch). It holds the architecture
invariants and conventions that are load-bearing here, and indexes each crate's
"known sharp edges" — whose full text lives in that crate's `SHARP-EDGES.md`.
Many things that look like bugs are documented, intentional design, so read the
entry and not just its index line.

## Build & test

Requires a recent stable Rust toolchain and [`just`](https://github.com/casey/just)
(`brew install just`). On Linux you also need `lld` (`apt install lld`) —
`.cargo/config.toml` links x86_64-linux builds with it, matching CI. The
`justfile` is the single source of truth for what each check runs — CI and the
git hooks call the same recipes.

```bash
just              # list recipes
just preflight    # full pre-push gate: lint (fmt + machete + deny + arch + shfmt + shellcheck + actionlint + actionlint-composites + zizmor + ci-observability + json-schemas + links + drift-selftest + gen-guides-check + gitenv-selftest + env-paths + prose) → NOTE: `prose` fetches `origin/main`, so preflight now needs network → clippy → hack → test
just fmt          # auto-format
just test         # the whole suite (cargo-nextest if installed, else cargo test)
```

While iterating on one crate, scope it for a much faster loop (seconds vs a full
workspace run):

```bash
cargo nextest run -p pixtuoid <filter>      # or: cargo test -p pixtuoid --lib <filter>
```

> **Don't chain `cargo clippy && cargo test`** — clippy and test use *separate*
> build caches, so chaining recompiles the whole workspace twice. Run
> `just preflight` (the exact CI order), or one check at a time.

### Git hooks

Activate once per clone:

```bash
git config core.hooksPath .githooks
```

`pre-commit` runs `just fmt-check` (sub-second); `pre-push` runs `just preflight`.
Run `just preflight` locally first to avoid the push → CI-red → fix round-trip.

## CI gates

`just preflight` is the local gate; these run only in CI, so a green preflight
does not mean a green PR.

**semver** (pixtuoid-core + pixtuoid-scene — the binary's lib target is not a
semver surface), api-surface (`just api-surface-check` — a committed `cargo
public-api` golden per published crate at `api/<crate>.txt`; the reviewable-diff
twin of the semver gate: semver says "major/minor?", the golden says *what*
changed — regenerate with `just api-surface` + commit when the public surface
shifts), docs (`just doc-check` — `cargo doc` with `-D warnings` over the
`[workspace.lints.rustdoc]` broken/private-intra-doc-link deny + the doctests
`cargo nextest` skips), coverage/smoke, gen-check, gen-readme-check, npm-check,
check-windows (cross-lint for msvc on every PR), snapshots (`cargo insta` —
fails on a pending OR orphan `.snap`, the rot plain `cargo test` can't see).
Cross-file report/upload semantics that actionlint cannot express are pinned by
the yq + Conftest/OPA policy and real action/workflow behavior tests under
`policy/ci-observability/`; `just ci-observability` runs them inside both
`just lint` and the CI hygiene job.
`just zizmor` adds the upstream workflow/action/Dependabot security analyzer:
the repository deliberately requires a symbolic ref or SHA (not SHA-only),
every checkout drops persisted credentials, and accepted analyzer findings use
exact inline suppressions with their reason instead of disabled audit classes.
Dependabot applies a seven-day update cooldown across every configured
ecosystem, and its `github-actions` entry lists `/.github/actions/*` beside `/`
— `directory: /` searches only `.github/workflows` plus a root `action.yml`, so
a third-party pin extracted into a composite would leave update coverage
entirely; the policy denies a composite pin no declared directory covers.
The two automatic Claude reviewers are thin trigger policies over
`claude-readonly-review.yml`: the model job checks out only the trusted default
branch, receives the exact PR diff as inert data, has read-only GitHub/tools,
and emits schema-bound JSON; a separate no-checkout publisher revalidates the PR
head before writing the review comment. A third job comments when the model job
FAILS, because absence otherwise renders as a pass — the publisher skips, no
`Findings:` comment lands, and the PR reads merely `UNSTABLE`, which is how #809
and #815–#818 merged with only a red job to say so. It covers both shapes — the
failure and the decline (`reviewable=false` exits 0, so that arm has no red job
at all). Rare, so it stays thin: one comment, and one rule pinning that the job
exists with both arms and a status function, without which an implicit
`success()` would skip it exactly when it is needed (#819). The human-triggered `@claude` workflow
(`claude.yml`, the only `contents: write` Claude job) checks out without a ref,
so its two `pull_request_review*` arms — the events whose `GITHUB_REF` is the PR
merge ref — additionally require the head to live in this repository, and the
policy keys that requirement off the workflow's `on:` triggers rather than its
surviving `if:` arms (a condition that never names an event SKIPS the job; a
missing condition gates nothing). The `issues`/`issue_comment` arms carry no
`pull_request` object, so the same guard is not expressible there — and
claude-code-action stages the fork tree itself (tag mode's `setupBranch` checks
the PR head out for every open PR), so `issue_comment` needs a job STEP instead:
`Refuse fork pull requests` resolves the PR and exits first, and the policy pins
its existence, that it is scoped to `issue_comment`, and that it precedes the
action, keyed on the API field it reads rather than its name (#799). `issues` is unaffected — the action hardcodes
`isPR` false there. Anthropic WIF is preferred when its
repository variables are configured, with the existing OAuth secret as a
compatibility fallback. Codecov uploads likewise use job-scoped GitHub OIDC
(fork PRs remain Codecov's tokenless path), never a repository upload token.
That gate also pins the advanced CodeQL workflow: all four repository
languages stay explicit, Rust stays on its only supported `none` build mode,
and the no-build extractor receives `rust-src` plus the proc-macro server from
the workspace's declared MSRV (not the runner's rolling stable toolchain).
After analysis, CodeQL's own SARIF metrics fail the Rust job if extraction
diagnostics affect at least as many files as were extracted cleanly, and the
quantified counts are written to the job summary. This is why CodeQL lives in
[`.github/workflows/codeql.yml`](../.github/workflows/codeql.yml) instead of GitHub
default setup — default setup cannot prepare these semantic inputs or enforce
database health.

## Releasing

### Versioning

Pre-1.0, we read SemVer onto `0.y.z` like this:

- **patch (`0.y.Z`)** — bug fixes and minor polish only: no new public API, and nothing breaks.
- **minor (`0.Y.z`)** — everything else: new user-facing features (a source, a theme, a CLI flag) **and** any breaking change to `pixtuoid-core` / `pixtuoid-scene`'s public API.

**What the `semver` gate enforces vs. what's on you.** `cargo semver-checks` (the CI `semver` job, over those two crates) is a *compatibility* gate: it fails a **breaking** change that isn't paired with a minor bump — the "nothing breaks on a patch" half, machine-enforced. It does **not** flag a purely *additive* change shipped as a patch: new public API is backward-compatible, so the tool stays green. The "features also bump minor" half is therefore our **convention**, upheld in review, not by the gate. When a breaking change reddens `semver`, bump the minor **in the same PR** — never weaken the lint to ship a patch. At `1.0` this splits the usual way: additive → minor, breaking → major.

### Cutting the release

Recipes are grouped by intent — run `just --list` to see them:

| To… | Run | What it touches |
| --- | --- | --- |
| **cut a release** | `just bump X.Y.Z` | every version number (workspace + the inter-crate path-deps — `pixtuoid`/`pixtuoid-web` → `pixtuoid-scene` → `pixtuoid-core` — + `Cargo.lock`) · drafts the in-app release notes · `just preflight` · commits on `release/vX.Y.Z` |
| **regenerate doc art** | `just gen` (or `just gen-media` for images only) | `docs/images/*` + `site/public/demos/*` (screenshots + `demo.gif`) from a release build, driven by `scripts/media.json` |

`just bump` rewrites every version number in one shot via `cargo set-version`
(so the path-dep requirement can't drift — the classic missed edit), drafts the
`release_notes()` arm from the commit log since the last tag, runs the full
gate, and lands it on a release branch. It **stops before the tag** — pushing
the tag is what fires the *irreversible* publish (crates.io + npm, and a
homebrew-core autobump), so a human owns that:

```bash
just setup-tools                            # once per clone — installs cargo-edit (+ the rest)
just bump 0.5.1                             # bump + draft notes + preflight → branch release/v0.5.1
# curate the drafted release_notes() bullets to ~6 highlights, then `just gen`
# (the office HUD bakes CARGO_PKG_VERSION, so a bump drifts every committed still)
# and commit docs/images + site/public/demos — else CI's smoke gen-check reds the PR.
# then PR → review → merge, then:
git tag v0.5.1 && git push origin v0.5.1    # fires release.yml → build + crates.io + npm
```

The tag also publishes **outside** this repo: `pixtuoid` is in homebrew-core,
whose formula builds from the tag TARBALL and is `autobump: true`, so
BrewTestBot opens a version-bump PR on its own — and the tarball is fetchable
the instant the tag lands, before `release.yml` has finished. A tag we can't
un-publish is therefore also a homebrew-core build we can't un-trigger. Two
consequences worth internalizing:

- **A from-source build break lands in Homebrew's CI, not ours.** Their formula
  builds the workspace with DEFAULT features (`cargo install --locked`) on
  macOS *and* Linux — the one configuration our own release never builds
  (`release.yml` ships Linux artifacts `--no-default-features`). Anything that
  adds a system-library dependency needs a matching `depends_on` in the core
  formula, landed in the same PR as the version bump. **One is outstanding
  right now**: the default-on `audio` feature (#633) landed after v0.15.0 and
  pulls rodio/cpal, which need ALSA on Linux; the formula declares no runtime
  deps yet, so the first bump that ships `audio` must add
  `depends_on "alsa-lib"` — see [#731](https://github.com/IvanWng97/pixtuoid/issues/731).
- **Their `test do` block is a public contract** — see the "homebrew-core
  contract" comments at `crates/pixtuoid/src/validate.rs`,
  `crates/pixtuoid/src/sources_cli.rs` and
  `crates/pixtuoid-core/src/source/claude_code.rs`.

Preempt the bot: submit the bump PR yourself right after tagging, so the
version bump and any new `depends_on` ship together.

Publishing to crates.io + npm uses **OIDC trusted publishing** — CI carries no
standing registry tokens. The per-crate (crates.io) and per-package (npm)
Trusted Publishers, scoped to the `release.yml` workflow, must already be
configured before the tag is pushed, or that target's publish step fails. See
[#216](https://github.com/IvanWng97/pixtuoid/issues/216).

## The arc loop

Non-trivial work runs as an **arc**: design → build → gate → wrap. The root
[`CLAUDE.md`](../CLAUDE.md) carries the nine-step summary every agent session
loads; this is the per-step detail.

1. **Pick** — an issue (GitHub is the tracker; `gh issue list`) or backlog item.
2. **Grill the design** — decide the open questions ONE at a time, each with a
   recommended answer, before writing code. (A big arc introducing new
   seams/vocabulary grills against the domain docs first.)
3. **Design gate (before build; NOT the step-8 merge review)** — the grilled
   approach clears three design-time lenses so slop dies in design, not review:
   **best-practice search** (confirm the *idiomatic* way against real
   docs/source online — never memory: the dep's own API/features, the standard
   pattern); **adversarial design review** (red-team the design itself — simplest
   shape? failure mode? — BEFORE code exists); **deepening lens** (the deletion
   test — would deleting this concentrate complexity or just move it? — plus the
   deep-vs-shallow check: does the change *deepen* a module or add another
   shallow one = AI-slop?). Cut slop in small, verified steps; a big-radius
   refactor is fine when the deepening earns it. (`codebase-design` /
   `improve-codebase-architecture` drive the deepening lens; this repo keeps its
   domain record + decisions-not-to-relitigate in nested `CLAUDE.md` +
   `SHARP-EDGES.md`, NOT a `CONTEXT.md`/`docs/adr/` — map onto those, don't scaffold
   competing docs.)
4. **Spec** — synthesize the grilled decisions into `docs/superpowers/specs/`
   (LOCAL, git-ignored — the working design record, not the tracker). Also
   plan against [`.github/prompts/impl-plan.prompt.md`](../.github/prompts/impl-plan.prompt.md).
5. **Mock gate (taste/visual work only)** — ratify the AFTER visual BEFORE any
   code (the `beautify-decoration` skill's "The visual-iteration loop").
6. **Build** — TDD (see Conventions): failing test → minimal impl → commit.
7. **Self-review** — a standards+spec pass before pushing. Not the merge gate.
8. **Merge gate (non-negotiable)** — the **two-lens review** (2+ differentiated
   lenses on the diff) + green CI + the online review bot's `Findings: 0` at
   HEAD, checked atomically. (If the bot errors or posts no findings comment at
   HEAD — it can fail on a very large diff — the gate is unsatisfiable as
   written; the `two-lens-review` skill's step 6 owns the fallback.)
   See [Pull requests](#pull-requests) and [the running
   order](#the-running-order). **A human merges.**
9. **Wrap** — retro; record durable lessons.

**Skills.** Repo skills live in [`.claude/skills/`](../.claude/skills/)
(committed, so they travel with the repo). On symlink-capable checkouts,
[`.agents/skills/`](../.agents/skills/) aliases the same directories for Codex.
The skills are `two-lens-review` (the merge gate), `beautify-decoration` (the
visual mock loop), `add-source` / `add-theme` (scaffold + test-teeth for a new
CLI / palette), `procedural-lofi` (synthesize a new ambient sound). Claude Code
auto-surfaces them by description; Codex does the same through the aliases.
Other tools read `AGENTS.md` and run the loop above as prose.

**Bootstrap on a fresh machine / other tool.** `git clone` gives you the repo
skills + all `just` gates immediately. The day-to-day *loop* skills
(`grilling`, `to-spec`, `tdd`, `code-review`, `diagnosing-bugs`, plus
`research`/`grill-with-docs` and `improve-codebase-architecture`/`codebase-design`
for the step-3 design gate) are a
PERSONAL, non-committed layer — install [mattpocock/skills](https://github.com/mattpocock/skills)
if you want the Claude Code implementations; otherwise this section IS the loop.
Do NOT run its `setup-matt-pocock-skills` here — it scaffolds a `CONTEXT.md` +
`docs/adr/` doc convention that would compete with our richer nested `CLAUDE.md`
+ sharp-edges system (neither exists in this repo, and we don't want a second,
rotting one), plus a fixed triage-label vocabulary separate from our existing
issue labels (e.g. `bug` / `enhancement` / `upstream-drift` / `needs-human-verify`).

### The running order

What to run and when, for an agent-driven change:

| when | run | authority |
|---|---|---|
| before code, if non-trivial (new seam / ≥3 files) | plan against [`impl-plan.prompt.md`](../.github/prompts/impl-plan.prompt.md) | — |
| touched the `--json` / `SourceStatus` / `OutcomeRow` shape | `just gen-contract` | [`CLAUDE.md`](../CLAUDE.md) "Build & test" |
| before push | `just preflight` | same — including why never to pipe it |
| before merge | the two-lens review | [Pull requests](#pull-requests) |
| a source/lifecycle change | dogfood against live CC, or replay hermetically | the three tiers below |

The three OpenClaw e2e tiers, cheapest first — none runs in CI:

- `just openclaw-e2e` — hermetic, crafted envelopes on an isolated socket. Free, no gateway needed.
- `just openclaw-multi-e2e` — N REAL gateways, free, needs `openclaw` on PATH. The tier that catches multi-instance render/crowding.
- `just openclaw-backend-e2e` — a real gateway AND one BILLED model turn. Run deliberately.

Their `expect_line` pollers are deliberately not shared; the WHY lives at the
definition in `scripts/openclaw-live-e2e.sh`, where someone about to hoist them
is already looking.

Advisory backstops that surface risk but NEVER gate: `scripts/check_upstream_drift.py`
(wire-format drift); `just bench` (criterion render-path benchmarks — local numbers are the
authoritative ones, recorded in commit messages; the on-demand `bench.yml` mirrors
`mutants.yml`'s advisory shape because shared-runner wall-clock is noise per criterion's own
FAQ, while `codspeed.yml` runs the same benches instrumented per PR — instruction-count
simulation, so runner noise doesn't apply — and posts trends via the CodSpeed app, still
gating nothing); the `risk radar` PR workflow (`scripts/risk-radar.py`) — deterministic
path matching that posts the documented blast-radius escalations as a sticky PR comment so
prose-only escalation can't be silently skipped (#198); and `just comment-lint`'s ast-grep
arm, whose npm install keeps it in advisory `ci-supplemental` where a registry outage cannot
become a required check. A proposal to make that arm BLOCK is priced by `just
comment-lint-replay N` (#907). Merged is not adjudicated — read the flagged lines before
calling them false positives.

Which `comment-lint` arms BLOCK is stated once, in `gate_fails`' docstring in
`scripts/comment-lint.py` — pinned in both directions by that script's selftest.
It is not restated here; this file said "three arms" for one commit after the
code said two, and the online bot is what caught it.

## Conventions (the short version — see [`CLAUDE.md`](../CLAUDE.md) for the full set)

- **TDD first** — failing test → minimal impl. Don't add code without a test that exercises it.
- **DRY, YAGNI** — no features beyond what the current scope specifies.
- **No `unwrap()` in non-test code.** Errors propagate via `anyhow::Result` (app code) / `thiserror` (core). The hook listener and JSONL watcher log-and-continue on malformed input — they never panic.
- **Comments explain WHY, not what** — only where a future reader can't tell from the code.
- **Keep docs current** — a change to module structure, the public API, or developer workflow updates the relevant `CLAUDE.md` / `README.md` in the **same commit**.
- **macOS-first** — BSD-flavored CLI; `shellcheck` any `.sh` you touch.
- **Sprite changes need visual verification** — see `.claude/skills/beautify-decoration/SKILL.md`.
  CI's smoke job also pixel-diffs deterministic renders against `docs/images/reference-*.png`
  (`just gen-check` runs the same gate locally) — an intentional visual change
  must commit the references regenerated by `just gen` in the same change.

## Architecture invariants (don't break these)

1. `pixtuoid-core` and `pixtuoid-scene` (the render+sim engine crate) have **no terminal or window dependencies** (no `ratatui`/`crossterm`/`winit`/`softbuffer`/`stdout` — `just arch` enforces both; terminal/window code lives in the binary's `tui/` and `floating/` painters).
2. Events flow through **one** channel typed `mpsc::Sender<(Transport, AgentEvent)>`; the `Transport` tag is load-bearing (hook-wins dedup).
3. The **`Source` trait** is the only seam for adding a transcript-bearing agent CLI (hook-only CLIs like Reasonix instead ship a hook decoder + an install `Target` — see `crates/pixtuoid-core/CLAUDE.md`).
4. Hook install (`install::install_target`) writes through symlinks (`resolve_symlink`) — don't replace with `fs::rename`.
5. The hook shim must **never block CC** — always exit 0 silently; the 200 ms send bound (watchdog-enforced on both platforms) is non-negotiable.
6. Walkable mask = **ground footprint only** (top-down view); visual sprites may be wider/taller.

## Pull requests

- Every PR is reviewed by **2+ agents** (explorer / reviewer / architect) before merge — no exceptions. The teeth here are the `claude-review` + `claude-security-review` CI workflows plus your own local pass: both Claude model jobs run read-only against a trusted default-branch checkout and inert exact-head diff, then a separate least-privilege job publishes their validated result. The lens-labelled write-up is a practice, not a parsed gate.
- AI-authored PRs get the `needs-human-verify` label and a human visual check before merge.
- Track every consciously-deferred finding as a GitHub issue (`gh issue create`) before moving on.

### Recurring pitfalls (this codebase's review history, distilled)

The mistake families this repo's reviews keep catching — check your diff
against them before opening the PR:

1. **Byte-vs-char slicing.** Anything that truncates or indexes user-visible
   text must slice on `char`/grapheme boundaries, never bytes (`.chars().take(n)`,
   not `&s[..n]`) — labels, tooltips, HUD strings all carry non-ASCII.
2. **Parallel-implementation drift.** If a value/behavior exists in two places
   (Unix + Windows arms, core + tui twins, manifest + enum), either single-source
   it or add a bridge test pinning them equal. Two copies of anything drift apart.
   The in-diff form bites hardest: a guard or fix added to ONE of two sibling
   paths in the same diff (the empty-`RUST_LOG` guard shipped at one call site
   but not its sibling — #159, caught in #172) — when your
   diff guards one path, grep for its siblings before opening the PR.
3. **Sanitize at the decode boundary.** Untrusted input (transcripts, hook
   payloads, file paths) is cleaned where it ENTERS (`decoder.rs` / first-sight),
   not at each use site — a use-site you forget is an injection.
4. **Negative-branch test gaps.** A guard without a test asserting the REFUSAL
   path (wrong input → no-op/warn) will be silently broken by a future refactor.
   Pin the "must not happen" side, not just the happy path.
   When a comment names a hazard with a window/threshold, pin BOTH sides of
   it — the Waiting-clobber comment named the exact out-of-window harm while
   the pin covered only the in-window path (escaped the #150 dedup arc, fixed
   in #232). Derive test offsets from the constant under test
   (`HOOK_WINS_WINDOW / 10`, the #142 pattern), never hardcoded ms — retuning
   the constant silently makes a hardcoded pin vacuous.
5. **Unwired additions.** Every new field, parameter, or asset needs a
   consumer the same diff wires up — the compiler won't always warn (`_x`
   bindings and `pub` fields evade dead-code lints). The smells: a capture
   bound as `_x`, a parameter every call site passes as a literal default, an
   asset or enum variant nothing constructs. (PR #61 shipped `snap_prev`
   bound as `_snap_prev`, silently defeating the very origin-freeze it was
   added for — then survived #62's dedicated fix-round review too; wired
   in #66.)
6. **Denylist completeness.** A denylist/strip-set is only as strong as its
   enumeration: diff it against the platform's *documented* set, never
   memory, and prefer an allowlist where possible — an allowlist can't miss
   a character (PR #206). (`CMD_UNSAFE` shipped missing cmd.exe's
   first-token delimiters — tab, `;`, `,`, `=` — through two dedicated
   security reviews, #198/#201.)

### Handy `gh` commands

```bash
gh pr checks --watch                         # live CI status (vs. polling)
gh pr merge --auto --squash --delete-branch  # auto-merge once checks pass
gh issue develop <number> --checkout         # a branch linked to an issue (auto-closes on merge)
gh run rerun --failed                        # rerun only the failed CI jobs
```

Useful extensions: `gh-poi` (prune merged local branches), `gh-dash` (PR/issue
TUI), `gh skill` (install Agent Skills, incl. into `.claude/skills/`).

## Adding a new agent CLI

Step by step. The registration steps (4–7 and 9) are test-forced — skipping
one fails `just test` (the runtime wiring by
`build_source_set_wires_every_transcript_bearing_source_plus_the_hook_router`
in `runtime/driver.rs`; the manifest row by `supported_sources_manifest.rs`).
Step 8 is forced only for hook-only sources
(`every_hook_only_source_has_an_install_target`) — a transcript-bearing CLI
that ALSO has hooks still needs you to remember its install target. Step 10
(the badge hue) is forced by the theme guards. Steps 1–3 and 11 (docs) are on
you:

1. **Verify the wire format against the CLI's actual source/releases first.**
   Where does it write transcripts, what does a line look like, does it have
   hooks, what identifies a session? Pin every fact to an upstream file/version
   in your comments — wire formats change without notice (`Task` → `Agent` did),
   and a guessed format decodes nothing (see the "Keeping the decode mapping
   current" section in `crates/pixtuoid-core/CLAUDE.md`).

   **Audit its HOME RESOLVER in the same pass, per axis** — home order,
   config-dir API, env-override semantics (verbatim vs `~`-expanded; is
   empty/whitespace unset?), profile subtrees, XDG, legacy-dir fallbacks.
   `$SOME_ENV else ~/.<cli>` is correct only if you READ their resolver and
   found it generic; assumed, every unmirrored axis is fail-silent — the watcher
   polls a directory the CLI never writes to and the office is empty, with no
   error (#880). PROBE the installed artifact rather than reading docs: `strings`
   a bun binary, `require()` a napi `.node`, run the bundled interpreter — a case
   matrix settles empty/whitespace/`~`/relative, the cases nobody writes down.
   Record each verdict, and add a `check_upstream_drift.py` row when the resolver
   is fetchable source.
2. **Write the source module** — `crates/pixtuoid-core/src/source/<name>.rs`
   with a `SOURCE_NAME` const, a `LineDecoder` fn (one JSONL line → `Vec<AgentEvent>`),
   a label deriver, and unit tests for every event mapping. Per-source format
   knowledge lives HERE, not in shared code.
3. **Implement the `Source` trait** (the watcher lifecycle). Your impl is a
   plain `async fn`:

   ```rust
   impl Source for MyCliSource {
       fn name(&self) -> &str { "my-cli" }
       async fn run(self: Box<Self>, tx: TaggedSender) -> anyhow::Result<()> {
           // watch + decode + tx.send(...) until the session universe ends
       }
   }
   ```

   (The trait itself declares `run` as `-> impl Future<Output = …> + Send` —
   the explicit form is what carries the `Send` bound `tokio::spawn` needs,
   so a non-`Send` future in your impl is a compile error, not a runtime
   surprise. `SourceManager` boxes sources via the object-safe `DynSource`
   twin; the blanket impl means you never name it.)

   **Hook-only CLI** (no watchable transcript — e.g. one that full-rewrites
   its session file per turn)? Skip the `LineDecoder`, the `Source` trait, and
   step 7: set `transcript: None` in the registry row, put the format
   knowledge in a `hook.custom` decoder (it must claim EVERY event — see the
   contract on `HookDecoding::custom`), and do step 8 (install target) instead
   — its hooks ride the shared socket.

4. **Add ONE `SourceDescriptor` row** in `crates/pixtuoid-core/src/source/registry.rs`
   — label prefix (2 chars), the line decoder, hook keying (`IdKey` + an
   optional custom hook decoder), truthful capability flags (`has_exit_signal`,
   `resurrects_on_prompt`, `delegations_are_hook_silent`), plus
   `verified_version` ("unknown" until a byte-real capture anchors it — pinned
   non-empty by `every_descriptor_has_a_verified_version`) and `version_probe`
   (the `<cli> --version` argv for `pixtuoid doctor`, or `None`). Lifecycle
   policy derives from the flags; you do **not** edit the reducer.
5. **The descriptor's `name` field IS the roster** — `registered_source_names()`
   projects `REGISTRY` (uniqueness pinned by `registered_source_names_are_unique`),
   and the conformance suite then REQUIRES a fixture for it.
6. **Drop a sanitized real-capture fixture** under
   `crates/pixtuoid-core/tests/sources/fixtures/<name>/<scenario>/`
   (transcript + hook payloads as applicable — see the fixtures README for the
   provenance/sanitization rules), then `cargo insta review` to accept the
   golden snapshot. The harness (`tests/sources/conformance.rs`) asserts all of
   a session's events coalesce to ONE `AgentId` — the duplicate-sprite bug
   class. A CLI with unique lifecycle behavior (subagent hooks, custom exit)
   also gets a dedicated `tests/sources/<cli>.rs` module — the test-layout map
   and the full add-a-CLI test steps are in
   [`crates/pixtuoid-core/tests/CLAUDE.md`](../crates/pixtuoid-core/tests/CLAUDE.md).
7. **Wire it into `runtime/driver.rs::run_async`** (`crates/pixtuoid/src/runtime/driver.rs`) —
   the runtime spawns sources by hand (the registry drives the guard test, not the spawning).
8. **If the CLI has hooks**, add an `install/` target (a `Target` registry row +
   a `merge_install`/`merge_uninstall` pair + a `verify_schema` fn mirroring
   the target's own config format + a registered-events↔decoder-arms guard
   test; `verify_target_is_sound_after_a_real_install_for_every_target` pins
   the schema fn) so connecting `<name>` in the in-TUI Sources panel (`s`) wires
   the shim.
9. **Add a row to [`site/src/sources.json`](../site/src/sources.json)** — the
   single source of truth for the README "Supported Tools" glimpse AND the
   site's full tool × OS support matrix. Set `status`, `featured` (shown in the
   README glimpse), and per-OS `platforms`; then `just gen-readme` to regenerate
   the README. The `supported` set is pinned to `registered_source_names()` by
   `crates/pixtuoid-core/tests/supported_sources_manifest.rs`, so a newly
   registered source FAILS that test until its manifest row exists.
10. **Add the per-source badge hue** — a new field on `SourceColors` in
    `crates/pixtuoid-scene/src/theme/mod.rs` (wired into `SourceColors::all()`
    and the `by_prefix` match) plus its value in EVERY theme file under
    `crates/pixtuoid-scene/src/theme/`, and a `badge_color` in the
    `sources.json` row. `source_colors_cover_every_registered_source`,
    the per-theme legibility/distinctness guards, and the site bridge test
    (`pixtuoid-scene/tests/site_badge_colors.rs`) all fail until it exists.
11. **Other docs in the same PR**: the nested `crates/pixtuoid-core/CLAUDE.md`
    entry, and — if the upstream is open source — a
    `scripts/check_upstream_drift.py` check so a silent rename pages us weekly.

See "Adding a new agent CLI" in [`CLAUDE.md`](../CLAUDE.md) and
`crates/pixtuoid-core/CLAUDE.md` for the deeper wiring detail (and the four
test files that must be updated together if you touch the shared contracts).

## License

By contributing, you agree your contributions are licensed under the same terms
as the project (see the **License** section of the [README](../README.md)).