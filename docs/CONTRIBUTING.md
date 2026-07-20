# Contributing to pixtuoid

Thanks for your interest! PRs are welcome — especially **new themes**, sprite and
decoration polish, and **`Source` adapters** for agent CLIs we don't support yet
(the ten agent CLIs plus the OpenClaw gateway already wired up are listed in the README).

Before you start, read [`CLAUDE.md`](../CLAUDE.md) at the repo root (and the nested
`crates/*/CLAUDE.md` for the crate you touch). It holds the architecture
invariants, "known sharp edges", and conventions that are load-bearing here —
many things that look like bugs are documented, intentional design.

## Build & test

Requires a recent stable Rust toolchain and [`just`](https://github.com/casey/just)
(`brew install just`). On Linux you also need `lld` (`apt install lld`) —
`.cargo/config.toml` links x86_64-linux builds with it, matching CI. The
`justfile` is the single source of truth for what each check runs — CI and the
git hooks call the same recipes.

```bash
just              # list recipes
just preflight    # full pre-push gate: lint (fmt + machete + deny + arch + shfmt + actionlint + links) → clippy → hack → test
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
the tag is what fires the *irreversible* crates.io publish, so a human owns that:

```bash
just setup-tools                            # once per clone — installs cargo-edit (+ the rest)
just bump 0.5.1                             # bump + draft notes + preflight → branch release/v0.5.1
# curate the drafted release_notes() bullets to ~6 highlights, then `just gen`
# (the office HUD bakes CARGO_PKG_VERSION, so a bump drifts every committed still)
# and commit docs/images + site/public/demos — else CI's smoke gen-check reds the PR.
# then PR → review → merge, then:
git tag v0.5.1 && git push origin v0.5.1    # fires release.yml → build + crates.io + homebrew
```

Publishing to crates.io + npm uses **OIDC trusted publishing** — CI carries no
standing registry tokens. The per-crate (crates.io) and per-package (npm)
Trusted Publishers, scoped to the `release.yml` workflow, must already be
configured before the tag is pushed, or that target's publish step fails. See
[#216](https://github.com/IvanWng97/pixtuoid/issues/216).

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

- Every PR is reviewed by **2+ agents** (explorer / reviewer / architect) before merge — no exceptions. The teeth here are the `claude-review` + `claude-security-review` CI workflows plus your own local pass; the lens-labelled write-up is a practice, not a parsed gate.
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
