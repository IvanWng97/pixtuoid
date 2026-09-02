# Contributing to pixtuoid

Thanks for your interest! PRs are welcome — especially **new themes**, sprite and
decoration polish, and **`Source` adapters** for agent CLIs we don't support yet
(the agent CLIs plus the OpenClaw gateway already wired up are listed in the README).

Before you start, read [`CLAUDE.md`](../CLAUDE.md) at the repo root (and the
nested `crates/*/CLAUDE.md` for the crate you touch). It holds the load-bearing
architecture invariants and conventions. Many things that look like bugs are
documented, intentional design: read the item's doc comment before changing it.

## Build & test

Requires a recent stable Rust toolchain and [`just`](https://github.com/casey/just)
(`brew install just`). On Linux you also need `lld` (`apt install lld`). The
`justfile` is the single source of truth for every check — CI and the git hooks
call the same recipes.

```bash
just              # list recipes
just preflight    # full pre-push gate: lint → clippy → hack → test (the exact CI order)
just fmt          # auto-format
just test         # the whole suite (cargo-nextest if installed, else cargo test)
cargo nextest run -p <crate> <filter>   # fast loop while iterating on one crate
```

> **Don't chain `cargo clippy && cargo test`** — clippy and test use *separate*
> build caches, so chaining recompiles the whole workspace twice. Run
> `just preflight` (the exact CI order), or one check at a time.

Activate the git hooks once per clone: `git config core.hooksPath .githooks`
(`pre-commit` = `just fmt-check`; `pre-push` = `just preflight`).

## CI gates

`just preflight` is the local gate; these run only in CI, so a green preflight
does not mean a green PR:

- **semver** — a breaking change to `pixtuoid-core`/`pixtuoid-scene` without a
  minor bump fails (the binary's lib target is not a semver surface).
- **api-surface** — committed `cargo public-api` goldens at `api/<crate>.txt`;
  regenerate with `just api-surface` + commit when the public surface moves.
- **docs** — `cargo doc` with `-D warnings` (broken/private intra-doc links
  deny) plus the doctests nextest skips.
- **coverage/smoke · gen-check · gen-readme-check · npm-check** — committed
  media, README and npm manifest freshness.
- **check-windows** — msvc cross-lint on every PR.
- **snapshots** — `cargo insta`; fails on a pending OR orphan `.snap`, the rot
  plain `cargo test` can't see.
- **hygiene** — the same `just lint` recipes preflight runs (its CI job exists
  so a skipped local preflight can't land a lint break), including `just ci-observability`
  (the yq + Conftest/OPA policy tests under `policy/ci-observability/` pinning
  cross-file workflow semantics actionlint can't express) and
  `just fixture-pii` (gitleaks over the committed capture tree). The
  capture-tree RULES gate harder: they are Rust tests
  (`tests/sources/captures.rs`, entry `just fixture-metadata`) and ride
  `just test` on all three platforms.
- **zizmor** — workflow/action security: symbolic-or-SHA pins,
  credential-dropping checkouts, exact inline suppressions. Dependabot's
  `github-actions` entry lists `/.github/actions/*` beside `/` — `directory:
  /` alone leaves a composite's pin uncovered (policy-enforced).
- **The two automatic Claude reviewers** ride `claude-readonly-review.yml`: a
  read-only model job on the trusted default branch, the PR diff as inert
  data, a separate least-privilege publisher — and a third job that comments
  when the model job fails or declines, because absence otherwise renders as
  a pass (#809). `claude.yml` refuses fork PR heads.
- **CodeQL** stays the advanced workflow (`codeql.yml`): explicit languages,
  Rust's `none` build mode fed the MSRV toolchain, a SARIF health gate, and an
  inline query filter dropping `rust/cleartext-logging` (WHY on the init step).

## Releasing

### Versioning

Pre-1.0: **patch (`0.y.Z`)** = bug fixes and polish only — no new public API,
nothing breaks. **minor (`0.Y.z`)** = everything else: new user-facing features
AND any breaking change to the published crates' API. `cargo semver-checks`
machine-enforces only the "nothing breaks on a patch" half; "features also bump
minor" is convention, upheld in review. When a breaking change reddens
`semver`, bump the minor **in the same PR** — never weaken the lint.

### Cutting the release

```bash
just setup-tools    # once per clone
just bump 0.5.1     # rewrites EVERY version number (workspace + path-deps + lockfile),
                    # drafts release_notes(), runs preflight → branch release/v0.5.1
# curate the notes to ~6 highlights, then `just gen` (the HUD bakes
# CARGO_PKG_VERSION, so a bump drifts every committed still) and commit
# docs/images + site/public/demos — else smoke's gen-check reds the PR.
# PR → review → merge, then:
git tag v0.5.1 && git push origin v0.5.1   # fires release.yml → build + crates.io + npm
```

`just bump` **stops before the tag** — pushing the tag is the *irreversible*
publish, so a human owns it. The tag also publishes **outside** this repo:
homebrew-core's formula is `autobump: true` and builds from the tag tarball,
instantly, with DEFAULT features on macOS *and* Linux — the one configuration
our release never builds. Two consequences:

- **A from-source build break lands in Homebrew's CI, not ours.** Anything
  adding a system-library dependency needs a matching `depends_on` in the core
  formula, in the same bump PR. Outstanding now: the default-on `audio`
  feature needs `depends_on "alsa-lib"` — [#731](https://github.com/IvanWng97/pixtuoid/issues/731).
- **Their `test do` block is a public contract** — see the "homebrew-core
  contract" comments at `crates/pixtuoid/src/validate.rs`,
  `crates/pixtuoid/src/sources_cli.rs`,
  `crates/pixtuoid-core/src/source/claude_code.rs`.

Do not try to preempt BrewTestBot: the formula is on homebrew-core's
autobump list, so `brew bump-formula-pr pixtuoid` refuses by policy and the
bot opens the PR itself within ~3 hours of the tag. Watch THAT PR's CI and
intervene only if it reds.

Publishing uses **OIDC trusted publishing** — CI carries no registry tokens;
the per-crate/per-package Trusted Publishers must exist before the tag
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
   gets its entire comment population re-read against `CLAUDE.md`'s comment
   rules, and the cleanup rides the same PR (population and dispositions:
   [`pr-review.prompt.md`](../.github/prompts/pr-review.prompt.md)'s
   always-on comment row). Not the merge gate.
8. **Merge gate (non-negotiable)** — the **two-lens review** (2+ differentiated
   lenses on the diff) + green CI + every online-bot finding dispositioned,
   judged under the `two-lens-review` skill's **convergence contract**: churn
   budget before review, a two-fix-round hard cap, only a confirmed HIGH
   blocks, and a bot `Findings: 0` is evidence, not the gate. (Bot errored or
   absent at HEAD → the skill's step 6 owns the fallback.) **A human merges.**
9. **Wrap** — retro; durable lessons go to the agent's own memory layer, not
   new repo docs.

**Skills.** Repo skills live in [`.claude/skills/`](../.claude/skills/)
(committed; `.agents/skills/` aliases them for Codex): `two-lens-review`,
`beautify-decoration`, `add-source`, `add-theme`, `procedural-lofi`.
On a fresh machine or a non-Claude tool, `git clone` gives you the repo skills
and every `just` gate; this section IS the loop for tools without skills. Do
not scaffold a `CONTEXT.md`/`docs/adr/` convention here — a declaration's own
doc comment is the design record, and the nested `CLAUDE.md` says only what its
crate IS.

### The running order

| when | run |
|---|---|
| before code, if non-trivial (new seam / ≥3 files) | plan against [`impl-plan.prompt.md`](../.github/prompts/impl-plan.prompt.md) |
| touched the `--json` / `SourceStatus` / `OutcomeRow` shape | `just gen-contract` |
| before push | `just preflight` (never piped — a pipe eats the exit code) |
| before merge | the two-lens review |
| a source/lifecycle change | dogfood against live CC, or replay hermetically (tiers below) |

One change spanning the Rust lib + the site + the Raycast extension:
[`PARALLEL-DELIVERY.md`](PARALLEL-DELIVERY.md).

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
`just bench` / CodSpeed (local numbers authoritative; CI benches advisory).

## Conventions (the short version — see [`CLAUDE.md`](../CLAUDE.md) for the full set)

- **TDD first** — failing test → minimal impl. No code without a test.
- **DRY, YAGNI** — nothing beyond the current scope.
- **No `unwrap()` in non-test code**; `anyhow` (app) / `thiserror` (core); the
  hook listener and JSONL watcher log-and-continue, never panic.
- **Comments explain WHY, not what.**
- **Keep docs current** — structure/API/workflow changes update the relevant
  `CLAUDE.md`/`README.md` in the same commit.
- **macOS-first** — BSD CLI; `shellcheck` any `.sh` you touch.
- **Sprite changes need visual verification** — `beautify-decoration` skill;
  an intentional visual change commits the `just gen`-regenerated references
  in the same change (CI pixel-diffs against `docs/images/reference-*.png`).

## Architecture invariants (don't break these)

1. `pixtuoid-core` and `pixtuoid-scene` have **no terminal or window
   dependencies** (`just arch` + the crate boundary enforce it); terminal/
   window code lives in the binary's `tui/` and `floating/` painters.
2. Events flow through **one** channel typed `mpsc::Sender<(Transport,
   AgentEvent)>`; the `Transport` tag is load-bearing (hook-wins dedup).
3. The **`Source` trait** is the only seam for a transcript-bearing agent CLI
   (hook-only CLIs ship a hook decoder + an install `Target` instead).
4. Hook install writes **through symlinks** (`resolve_symlink`).
5. The hook shim **never blocks CC** — always exit 0; the 200 ms send bound is
   watchdog-enforced on both platforms.
6. Walkable mask = **ground footprint only**; sprites may be visually larger.

## Pull requests

- Every PR is reviewed by **2+ agents with differentiated lenses** before
  merge — no exceptions. The mechanical teeth are the `claude-review` +
  `claude-security-review` workflows plus your local two-lens pass.
- AI-authored PRs get the `needs-human-verify` label and a human visual check.
- **Every reviewer/bot finding reaches exactly one terminal state in the PR
  thread** — FIXED · REFUTED-with-trace · RE-SCOPED · SURFACED, defined ONCE
  in [`pr-review.prompt.md`](../.github/prompts/pr-review.prompt.md). Agents
  never file issues, and "acknowledged, no action" is not a state.

### Recurring pitfalls (this codebase's review history, distilled)

1. **Byte-vs-char slicing** — user-visible text truncates on `char`/grapheme
   boundaries, never bytes.
2. **Parallel-implementation drift** — a value in two places (platform arms,
   core+tui twins, manifest+enum) gets single-sourced or a bridge test; when
   your diff guards one path, grep for its siblings (#159→#172).
3. **Sanitize at the decode boundary** — untrusted input is cleaned where it
   enters, not at each use site.
4. **Negative-branch test gaps** — pin the REFUSAL path, both sides of any
   window/threshold, with offsets derived from the constant under test.
5. **Unwired additions** — every new field/parameter/asset needs a consumer
   wired in the same diff (`_x` bindings and `pub` fields evade the lints; #61).
6. **Denylist completeness** — diff any strip-set against the platform's
   documented set; prefer an allowlist (#198/#201/#206).

### Handy `gh` commands

```bash
gh pr checks --watch                         # live CI status
gh pr merge --auto --squash --delete-branch  # auto-merge once checks pass
gh issue develop <number> --checkout         # branch linked to an issue
gh run rerun --failed                        # rerun only failed CI jobs
```

## Adding a new agent CLI

The registration steps (4–7, 9) are test-forced — skipping one fails
`just test`. Step 8 is forced only for hook-only sources; step 10 by the theme
guards; steps 1–3, 11 and 12 are on you.

1. **Verify the wire format against the CLI's actual source/releases first** —
   transcript location, line shape, hooks, session identity; pin every fact
   to an upstream file/version. **Audit its HOME RESOLVER per axis in the
   same pass** — PROBE the installed artifact rather than trusting docs; an
   unmirrored axis is fail-silent: the watcher polls a directory the CLI
   never writes and the office stays empty (#880). Resolver axes are
   deliberately NOT drift-watched — re-run the probe matrix when the CLI majors.
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
   projects `REGISTRY`, and the conformance suite then requires a fixture.
6. **Drop a sanitized real-capture fixture** under
   `tests/sources/fixtures/<name>/<scenario>/` (see the fixtures README for
   provenance rules), then `cargo insta review`. The conformance harness
   asserts all of a session's events coalesce to ONE `AgentId`. Test-layout
   map: [`crates/pixtuoid-core/tests/CLAUDE.md`](../crates/pixtuoid-core/tests/CLAUDE.md).
7. **Wire it into `runtime/driver.rs::run_async`** (the registry drives the
   guard test, not the spawning).
8. **If the CLI has hooks**, add an `install/` target (a `Target` row +
   `merge_install`/`merge_uninstall` + a `verify_schema` fn mirroring the
   target's own config format + the registered-events↔decoder-arms guard).
9. **Add a row to `site/src/sources.json`** (`status`, `featured`, per-OS
   `platforms`), then `just gen-readme`. Pinned to `registered_source_names()`
   by `supported_sources_manifest.rs`.
10. **Add the per-source badge hue** — a `SourceColors` field + value in EVERY
    theme file + `badge_color` in the manifest row; the coverage, legibility
    and site-bridge tests fail until it exists.
11. **Docs in the same PR**: the nested `crates/pixtuoid-core/CLAUDE.md` entry,
    and a `check_upstream_drift.py` row where one is owed — which surfaces owe
    one is `source/drift.rs`'s header, read it there. A row is four steps: the
    const, the `insert` in that crate's `src/drift_surface.rs`,
    `just gen-drift-surface` (commit both fragments), and the `SURFACE_ROWS`
    row plus its selftest case (the case census fails without it).
12. **Three roster literals no failure message spells out**: the row-by-row
    byte pin in `corpus_check.rs`; `TOOL_ID_KEY_UNPROVEN` in
    `tests/sources/captures.rs`; a case row + `#[test]` in
    `crates/pixtuoid/tests/wire_to_pixels.rs`.

## License

By contributing, you agree your contributions are licensed under the same terms
as the project.
