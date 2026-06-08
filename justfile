# Project task runner — the single source of truth for build / lint / format /
# test. Every call-site goes through these recipes — the .githooks/ hooks,
# .github/workflows/{ci,release}.yml, and the docs — so there is exactly ONE
# place that defines what each command actually runs (no drift between local,
# CI, and release).
#
# Recipes are grouped by intent (see `just --list`):
#   check    — the dev loop + the pre-push gate (lint / format / test / coverage)
#   build    — compile the workspace + release artifacts
#   release  — cut a new version (one command: `just bump X.Y.Z`)
#   docs     — regenerate the repo's screenshots / demo art
#   site     — the Astro landing page under site/ (npm, its own CI)

# Git Bash is preinstalled on GHA windows runners; keeps every recipe
# single-sourced cross-platform (CI never writes inline commands).
set windows-shell := ["bash", "-cu"]

features := "pixtuoid-core/test-renderer"

# List available recipes.
default:
    @just --list

# ── check ─────────────────────────────────────────────────────────

# Format check only — fast, gates pre-commit.
[group('check')]
fmt-check:
    cargo fmt --all --check

# Apply formatting in place.
[group('check')]
fmt:
    cargo fmt --all

# Clippy across the workspace, warnings denied.
[group('check')]
clippy:
    cargo clippy --workspace --all-targets --features {{ features }} -- -D warnings

# Unused-dependency check.
[group('check')]
machete:
    cargo machete

# License + advisory audit.
[group('check')]
deny:
    cargo deny check

# Fast, independent lint checks in parallel (fmt + machete + deny).
[group('check')]
lint:
    #!/usr/bin/env bash
    set -euo pipefail
    # Per-check logs; dump only the failures so a green run stays quiet.
    tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
    run() { local n="$1"; shift; if "$@" >"$tmp/$n.log" 2>&1; then printf '  \033[32m✓ %s\033[0m\n' "$n"; else printf '  \033[31m✗ %s\033[0m\n' "$n"; cat "$tmp/$n.log"; return 1; fi; }
    pids=(); fail=0
    run fmt     cargo fmt --all --check & pids+=($!)
    run machete cargo machete           & pids+=($!)
    run deny    cargo deny check         & pids+=($!)
    for p in "${pids[@]}"; do wait "$p" || fail=1; done
    [[ $fail -eq 0 ]]

# Workspace tests — nextest if available (parallel + JUnit), else cargo test.
# Extra args are forwarded: `just test reducer::` filters; preflight passes none.
[group('check')]
[doc('Run the workspace tests (nextest if installed); forwards a filter')]
test *args:
    #!/usr/bin/env bash
    set -euo pipefail
    if command -v cargo-nextest &>/dev/null; then
        cargo nextest run --workspace --features {{ features }} {{ args }}
    else
        cargo test --workspace --features {{ features }} {{ args }}
    fi

# Feature-combination check — every feature subset must compile. Catches code
# that silently only builds with `test-renderer` on (CI runs with it always on).
[group('check')]
[doc('Feature-powerset check — every feature subset must compile')]
hack:
    cargo hack --feature-powerset --no-dev-deps check --workspace

# Cross-lint the workspace for Windows (clippy subsumes check; no linking).
[group('check')]
[doc('Cross-lint the workspace for x86_64-pc-windows-msvc via clippy (no linking; ubuntu runner suffices)')]
check-windows:
    cargo clippy --workspace --all-targets --features {{ features }} --target x86_64-pc-windows-msvc -- -D warnings

# SemVer-check the published library against its crates.io baseline. CI-only in
# practice: needs network to fetch the baseline crate. Scoped to pixtuoid-core
# (the headless lib others depend on); the binary crates' libs aren't public API.
[group('check')]
[doc('SemVer-check pixtuoid-core against its crates.io baseline (CI-only)')]
semver:
    cargo semver-checks --package pixtuoid-core

# Coverage + JUnit XML in one run — the exact command ci.yml's coverage job uses.
# CI-only in practice: needs cargo-llvm-cov + cargo-nextest + the `ci` nextest
# profile. Writes lcov.info + target/nextest/ci/junit.xml.
[group('check')]
[doc('Coverage + JUnit XML — the exact command ci.yml runs (needs llvm-cov + nextest)')]
coverage:
    cargo llvm-cov nextest --workspace --features {{ features }} --lcov --output-path lcov.info --profile ci

# Never-panic fuzz the per-source decoders over a JSONL corpus DIR (on-demand;
# not in preflight/CI — points at local or public real sessions, not committed
# data). Auto-routes each line to the CC / Codex / hook decoder by its shape;
# exits non-zero on any panic. Examples:
#   just fuzz ~/.claude/projects     # your CC sessions (newest formats)
#   just fuzz ~/.codex/sessions      # your Codex rollouts
#   git clone https://github.com/daaain/claude-code-log /tmp/cc && just fuzz /tmp/cc/test_data/real_projects
[group('check')]
[doc('Never-panic fuzz the decoders over a JSONL corpus dir: just fuzz ~/.claude/projects')]
fuzz dir:
    cargo build --release --example decoder_fuzz -p pixtuoid-core
    find "{{ dir }}" -name '*.jsonl' -print0 | xargs -0 cat | ./target/release/examples/decoder_fuzz

# Fail if the current release_notes() arm still has the uncurated TODO marker.
# A release-PR guard (#116) — deliberately NOT in preflight, since `just bump`
# leaves the marker for the human to curate after the bump commit.
[group('check')]
[doc('Fail if release_notes() still has the uncurated TODO marker (release-PR guard)')]
notes-curated:
    #!/usr/bin/env bash
    set -euo pipefail
    if grep -q 'TODO: curate' crates/pixtuoid/src/version.rs; then
        echo "error: release_notes() still has the 'TODO: curate' marker — curate the drafted bullets before merge" >&2
        exit 1
    fi
    echo "release notes curated ✓"

# Unit-test the npm package generator (Node, no cargo). The ONLY validation of
# npm/generate.mjs — release.yml runs this as a hard gate right before `npm
# publish`, and ci.yml runs it on every PR so a generator regression is caught
# at review time, not at the irreversible tag-push. NOT in preflight: a Rust
# pre-push shouldn't require a Node toolchain. Needs Node ≥ 22.
[group('check')]
[doc('Test the npm package generator (Node; CI + release call it, not in preflight)')]
npm-check:
    node --test npm/generate.test.mjs

# Install the dev tools every check + recipe relies on (idempotent). Prefers
# cargo-binstall (prebuilt) and falls back to cargo install (compiles).
[group('check')]
[doc('Install the dev tools the checks + recipes need (idempotent)')]
setup-tools:
    #!/usr/bin/env bash
    set -euo pipefail
    tools=(cargo-nextest cargo-machete cargo-deny cargo-hack cargo-semver-checks cargo-edit)
    if command -v cargo-binstall &>/dev/null; then
        cargo binstall -y "${tools[@]}"
    else
        echo "cargo-binstall not found — compiling from source (slow)." >&2
        echo "brew install cargo-binstall (or cargo install cargo-binstall) to grab prebuilt binaries instead." >&2
        cargo install "${tools[@]}"
    fi

# Full pre-push gate: the checks worth running locally before a push.
# (semver, coverage, and smoke are CI-only — network baseline / heavy builds.)
[group('check')]
[doc('Full pre-push gate: lint → clippy → hack → test')]
preflight: lint clippy hack test

# ── build ─────────────────────────────────────────────────────────

# Compile the workspace; extra args are forwarded:
#   just build                                # debug
#   just build --release                      # release
#   just build --release --bins --examples    # what ci.yml's smoke job builds
[group('build')]
[doc('Compile the workspace; forwards args (e.g. --release --bins --examples)')]
build *args:
    cargo build --workspace {{ args }}

# Cross-compile a release build for ONE target triple (release.yml's build
# matrix). Pass `true` for targets that need the Docker-backed `cross` toolchain
# (CI installs it via taiki-e/install-action@cross).
[group('build')]
[doc('Cross-compile a release for ONE target triple (release.yml build matrix)')]
build-target target cross="false":
    #!/usr/bin/env bash
    set -euo pipefail
    use_cross="{{ cross }}"
    if [ "$use_cross" = "true" ]; then
        cross build --release --target "{{ target }}"
    else
        cargo build --release --target "{{ target }}"
    fi

# Package the .deb for ONE already-built target (release.yml's deb job, hence
# --no-build). Needs cargo-deb (CI installs it via taiki-e/install-action@cargo-deb).
[group('build')]
[doc('Package the .deb for ONE already-built target (release.yml deb job)')]
deb target:
    cargo deb -p pixtuoid --no-build --no-strip --target {{ target }}
    cargo deb -p pixtuoid-hook --no-build --no-strip --target {{ target }}

# ── release ───────────────────────────────────────────────────────

# Cut a release: bump to a new version on a release branch.
#
# Rewrites EVERY version number in one shot — the workspace version, the
# inter-crate pixtuoid→pixtuoid-core path-dep requirement, and Cargo.lock (via
# `cargo set-version`) — then drafts the in-app `release_notes()` arm from the
# commit log, runs `just preflight`, and commits on `release/vX.Y.Z`. It STOPS
# before the tag: pushing the tag is what triggers the irreversible crates.io
# publish, so that stays a human step. Needs cargo-edit (`just setup-tools`).
# Honors SKIP_PREFLIGHT=1 for iteration.
[group('release')]
[doc('Cut a release: bump every version number + draft notes on a release branch (no tag/push)')]
bump version:
    #!/usr/bin/env bash
    set -euo pipefail
    ver="{{ version }}"

    # 1. shape — a plain release version, no leading v / pre-release suffix
    [[ "$ver" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
        echo "error: '$ver' is not a release version (expected X.Y.Z)" >&2; exit 1; }

    # 2. clean tracked tree (untracked is fine) — a bump must not sweep up edits
    if ! git diff --quiet || ! git diff --cached --quiet; then
        echo "error: uncommitted changes — commit or stash before bumping" >&2; exit 1; fi

    cur="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"

    # 3. must be strictly newer than the current version
    if [[ "$ver" == "$cur" || "$(printf '%s\n%s\n' "$cur" "$ver" | sort -V | tail -1)" != "$ver" ]]; then
        echo "error: $ver is not newer than the current $cur" >&2; exit 1; fi

    branch="release/v$ver"
    if git rev-parse --verify --quiet "$branch" >/dev/null; then
        echo "error: branch $branch already exists" >&2; exit 1; fi

    # a duplicate release_notes arm is an unreachable_patterns error under
    # clippy -D warnings — catch it here with a clear message, not a compile error
    if grep -q "\"$ver\" =>" crates/pixtuoid/src/version.rs; then
        echo "error: version.rs already has a release_notes arm for $ver" >&2; exit 1; fi

    # releases come from main; forking release/v$ver off anything else is usually wrong
    cur_branch="$(git symbolic-ref --short -q HEAD || echo detached)"
    if [ "$cur_branch" != "main" ]; then
        echo "warning: on '$cur_branch', not main — release/v$ver will fork from here" >&2; fi

    echo "▸ bump $cur → $ver"

    # restore everything if anything below fails before the commit lands, so a
    # failed bump (e.g. red preflight) never strands a half-bumped tree or an
    # orphan release branch. `restore --staged --worktree` also clears the index —
    # a plain `checkout --` would leave the bump *staged* if the commit step failed.
    committed=0
    cleanup() {
        if [ "$committed" = 1 ]; then return 0; fi
        git restore --staged --worktree Cargo.toml Cargo.lock crates/*/Cargo.toml crates/pixtuoid/src/version.rs 2>/dev/null || true
        if [ "$(git symbolic-ref --short -q HEAD 2>/dev/null || true)" = "$branch" ]; then
            git switch -q "$cur_branch" 2>/dev/null || true
            git branch -qD "$branch" 2>/dev/null || true
        fi
    }
    trap cleanup EXIT

    # 4. all version numbers + Cargo.lock in one command (incl. the path-dep)
    cargo set-version --workspace "$ver"

    # 5. draft the in-app release notes from the log since the last tag.
    #    git-cliff owns the GitHub-release changelog; this is the curated in-app
    #    popup — drafted here, trimmed to ~6 highlights by a human before merge.
    last_tag="$(git describe --tags --abbrev=0 2>/dev/null || true)"
    range="${last_tag:+$last_tag..}HEAD"
    notes="$(mktemp)"
    {
        echo "        \"$ver\" => Some(&["
        echo "            // TODO: curate into ~6 user-facing highlights (drafted from \`git log ${range}\`)"
        git log --no-merges --pretty=format:'%s' "$range" \
            | sed -E 's/^[a-z]+(\([^)]*\))?!?: //' \
            | sed 's/\\/\\\\/g; s/"/\\"/g; s/^/            "/; s/$/",/'
        printf '\n        ]),\n'
    } > "$notes"
    awk -v f="$notes" '
        /\[bump-inject-here\]/ { print; while ((getline l < f) > 0) print l; next }
        { print }
    ' crates/pixtuoid/src/version.rs > "$notes.rs" && mv "$notes.rs" crates/pixtuoid/src/version.rs
    rm -f "$notes"
    cargo fmt -p pixtuoid

    # 6. green gate before committing (skippable for iteration)
    if [[ "${SKIP_PREFLIGHT:-}" != "1" ]]; then just preflight; fi

    # 7. land it on a release branch — no tag, no push (the irreversible step)
    git switch -c "$branch"
    git add Cargo.toml Cargo.lock crates/*/Cargo.toml crates/pixtuoid/src/version.rs
    git commit -q -m "chore(release): v$ver"
    committed=1

    printf '\n\033[32m✓ v%s committed on %s\033[0m\n\n  next:\n    1. curate the drafted bullets in crates/pixtuoid/src/version.rs (release_notes\n       arm) down to ~6 highlights, then: git commit --amend -a\n    2. open a PR, review, merge to main\n    3. AFTER merge, tag to publish — IRREVERSIBLE (crates.io + homebrew):\n         git tag v%s && git push origin v%s\n' "$ver" "$branch" "$ver" "$ver"

# ── docs ──────────────────────────────────────────────────────────

# Regenerate every docs/images screenshot + demo.gif from a release build.
# Single source of truth for the office images — the render params, crop
# quadrants, and the themes-composite diagonal angle all live in the script, so
# the screenshots never "drift". Run after any change to the office's look.
# Requires the .venv (Pillow): see README "Visual verification".
[group('docs')]
[doc('Regenerate docs/images screenshots + demo.gif from a release build')]
demo:
    .venv/bin/python3 scripts/gen-docs-images.py

# Pixel-diff two deterministic renders against the checked-in CI baselines
# (docs/images/reference-*.png). Run by ci.yml's smoke job; runnable locally
# before pushing a PR that touches the office's look. Render params MUST match
# the reference step in scripts/gen-docs-images.py (TZ=UTC pins the epoch-
# derived weather slot + twinkle phase; --weather makes the scene explicit).
# A red check after an INTENTIONAL visual change means: run `just demo` and
# commit the regenerated docs/images in the same change.
# Requires the .venv (Pillow) and a release build of the snapshot example.
[group('check')]
[doc('Visual regression: diff deterministic renders vs docs/images/reference-*.png')]
visual-check:
    #!/usr/bin/env sh
    # Both comparisons always run (no fail-fast) so a wide visual change is
    # diagnosable from ONE CI run's diff artifacts, not two round-trips.
    set -eu
    test -x .venv/bin/python3 || { echo "needs the venv: python3 -m venv .venv && .venv/bin/pip install -r requirements-dev.txt"; exit 1; }
    TZ=UTC ./target/release/examples/snapshot --theme normal --cols 192 --rows 64 --now-hour 19 --now-day 1 --weather clear /tmp/visual-day.png > /dev/null
    TZ=UTC ./target/release/examples/snapshot --theme cyberpunk --cols 192 --rows 64 --now-hour 3 --now-day 1 --weather clear /tmp/visual-night.png > /dev/null
    rc=0
    .venv/bin/python3 scripts/compare-screenshots.py docs/images/reference-screenshot.png /tmp/visual-day.png /tmp/diff-day.png || rc=1
    .venv/bin/python3 scripts/compare-screenshots.py docs/images/reference-night.png /tmp/visual-night.png /tmp/diff-night.png || rc=1
    exit $rc

# ── site ──────────────────────────────────────────────────────────
# The Astro landing page — a self-contained Node project under site/ with its
# own CI (.github/workflows/site.yml). See site/README.md.

[group('site')]
[doc('Install the site npm deps (run once per clone)')]
site-setup:
    npm --prefix site ci

[group('site')]
[doc('Site dev server with HMR → http://localhost:4321/pixtuoid/')]
site-dev:
    npm --prefix site run dev

[group('site')]
[doc('Full site gate: format-check → lint → astro check → build (mirrors site CI)')]
site-check:
    npm --prefix site run verify

[group('site')]
[doc('Auto-format the site')]
site-fmt:
    npm --prefix site run format

[group('site')]
[doc('Regenerate the site demo art from the pixtuoid binary')]
site-demos:
    ./site/scripts/gen-demos.sh

[group('site')]
[doc('Sync the README from site data: regen Features table (features.json) + check install commands (install.json)')]
gen-readme:
    node site/scripts/gen-readme.mjs

# Verify the committed README still matches site/src/{features,sources,install}.json
# (exits non-zero on drift; `just gen-readme` regenerates). Pure node:builtins —
# no npm ci. ci.yml runs this on every PR so README↔manifest drift is a hard gate
# at REVIEW time, not just a (path-filtered, advisory) site-CI failure.
[group('site')]
[doc('Fail if the committed README drifted from site data (features/sources/install.json)')]
gen-readme-check:
    node site/scripts/gen-readme.mjs --check
