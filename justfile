# Project task runner — the single source of truth for build / lint / format /
# test. Every call-site goes through these recipes — the .githooks/ hooks,
# .github/workflows/{ci,release}.yml, and the docs — so there is exactly ONE
# place that defines what each command actually runs (no drift between local,
# CI, and release).
#
# Recipes are grouped by intent (see `just --list`):
#   rust     — compile the workspace + every Rust gate (fmt / clippy / test / …)
#   site     — the Astro landing page under site/ (npm, its own CI)
#   gen      — regenerate committed artifacts (README sections + docs images + site demos)
#   release  — cut a new version (bump) + the distribution gates (npm-check / notes)
#   meta     — tooling setup + the full pre-push / full-stack gates

# Git Bash is preinstalled on GHA windows runners; keeps every recipe
# single-sourced cross-platform (CI never writes inline commands).
set windows-shell := ["bash", "-cu"]

features := "pixtuoid-core/test-renderer"

# List available recipes.
default:
    @just --list

# ── rust ──────────────────────────────────────────────────────────

# Format check only — fast, gates pre-commit.
[group('rust')]
fmt-check:
    cargo fmt --all --check

# Apply formatting in place.
[group('rust')]
fmt:
    cargo fmt --all

# Shell-format check (shfmt) — the `.sh` analog of `fmt-check`, gated via `lint`.
# Pairs with the shellcheck house rule: shellcheck lints, shfmt formats. Covers
# scripts/ + the git hooks. `-i 4` (4-space) matches the prevailing style; no
# `-ci` so case bodies stay un-indented as written.
[group('rust')]
[doc('Shell-format check (shfmt) over scripts/ + .githooks/ — the .sh analog of fmt-check')]
shfmt-check:
    shfmt -i 4 -d scripts/*.sh .githooks/*

# Apply shell formatting in place (the `.sh` analog of `fmt`).
[group('rust')]
[doc('Apply shfmt formatting in place over scripts/ + .githooks/')]
shfmt-fix:
    shfmt -i 4 -w scripts/*.sh .githooks/*

# Lint the GitHub Actions workflows (actionlint): YAML schema, expression types,
# action input/output names, runner labels, AND shellcheck over every `run:`
# block (so a shell bug inside a workflow is caught at author time, not on a red
# main). Gated via `lint`; the CI `hygiene` job runs it too. Needs shellcheck on
# PATH for the run-block checks (the house-rule tool — already required).
[group('rust')]
[doc('Lint the GitHub Actions workflows (actionlint + shellcheck over run: blocks)')]
actionlint:
    actionlint

# Offline link + anchor check (lychee) over the repo's OWN markdown: every
# relative cross-link between the nested CLAUDE.md/AGENTS.md guides + docs/ must
# resolve, and `#anchor` fragments must exist. Directory-walk mode respects
# .gitignore (vendored node_modules etc. auto-skipped); `--offline` = no network,
# so it's deterministic + flake-free. External-URL decay is deliberately NOT
# gated here (it's flaky on the PR path). Gated via `lint`; CI `hygiene` runs it.
[group('rust')]
[doc('Offline link + anchor check (lychee) over the repo markdown — no network, .gitignore-aware')]
links:
    lychee --offline --include-fragments .

# Clippy across the workspace, warnings denied.
[group('rust')]
clippy:
    cargo clippy --workspace --all-targets --features {{ features }} -- -D warnings

# Unused-dependency check.
[group('rust')]
machete:
    cargo machete

# License + supply-chain gate (bans/licenses/sources). Advisories are NOT here:
# they're owned by the daily audit.yml (`check advisories`) so an overnight
# RustSec advisory can't block a push of unchanged code. Keep this list in sync
# with the ci.yml `deny` job's `command:`.
[group('rust')]
deny:
    cargo deny check bans licenses sources

# Architecture invariant #1, mechanized: pixtuoid-core + pixtuoid-scene stay terminal/window-free.
# The other five invariants have test/bridge backstops; this one was
# review-enforced only until the KB pilot's gap-closure audit (2026-06-12,
# follow-on to the #261-#271 arc).
[group('rust')]
arch:
    #!/usr/bin/env bash
    set -euo pipefail
    # The backend-agnostic layers — neither may pull a terminal (ratatui/crossterm)
    # OR window (winit/softbuffer) crate; the tui + floating painters own those. The
    # crate boundary already makes this a COMPILER fact; this pins it at the dep-tree
    # level too (a transitive pull-in via a feature would slip past the boundary).
    for crate in pixtuoid-core pixtuoid-scene; do
        # Capture first so a cargo-tree ERROR (e.g. a crate rename) kills the
        # recipe via set -e, instead of reading as "no match" inside the if —
        # which would print the green line without having checked anything.
        deps="$(cargo tree -p "$crate" --edges normal --prefix none)"
        if grep -qE '^(ratatui|crossterm|winit|softbuffer)' <<<"$deps"; then
            echo "ARCH VIOLATION: $crate depends on a terminal/window crate (CLAUDE.md invariant #1)"; exit 1
        fi
    done
    echo "arch: pixtuoid-core + pixtuoid-scene are terminal/window-free"

# Fast, independent lint checks in parallel (fmt + machete + deny + arch + shfmt + actionlint + links).
[group('rust')]
lint:
    #!/usr/bin/env bash
    set -euo pipefail
    # Fail fast with an actionable message when a lint tool is missing, instead
    # of a bare `command not found` (exit 127) buried in a parallel job's log.
    missing=()
    for t in shfmt actionlint cargo-machete cargo-deny lychee; do
        command -v "$t" &>/dev/null || missing+=("$t")
    done
    if (( ${#missing[@]} )); then
        printf 'error: missing lint tool(s): %s — run `just setup-tools`\n' "${missing[*]}" >&2
        exit 1
    fi
    # Per-check logs; dump only the failures so a green run stays quiet.
    tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
    run() { local n="$1"; shift; if "$@" >"$tmp/$n.log" 2>&1; then printf '  \033[32m✓ %s\033[0m\n' "$n"; else printf '  \033[31m✗ %s\033[0m\n' "$n"; cat "$tmp/$n.log"; return 1; fi; }
    pids=(); fail=0
    run fmt     cargo fmt --all --check & pids+=($!)
    run machete cargo machete           & pids+=($!)
    run deny    just deny                & pids+=($!)
    run arch    just arch                & pids+=($!)
    run shfmt   just shfmt-check         & pids+=($!)
    run actions just actionlint          & pids+=($!)
    run links   just links               & pids+=($!)
    for p in "${pids[@]}"; do wait "$p" || fail=1; done
    [[ $fail -eq 0 ]]

# Workspace tests — nextest if available (parallel + JUnit), else cargo test.
# Extra args are forwarded: `just test reducer::` filters; preflight passes none.
[group('rust')]
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
[group('rust')]
[doc('Feature-powerset check — every feature subset must compile')]
hack:
    #!/usr/bin/env bash
    set -euo pipefail
    command -v cargo-hack &>/dev/null || { echo "error: cargo-hack not found — run \`just setup-tools\`" >&2; exit 1; }
    cargo hack --feature-powerset --no-dev-deps check --workspace

# Cross-lint the workspace for Windows (clippy subsumes check; no linking).
[group('rust')]
[doc('Cross-lint the workspace for x86_64-pc-windows-msvc via clippy (no linking; ubuntu runner suffices)')]
check-windows:
    cargo clippy --workspace --all-targets --features {{ features }} --target x86_64-pc-windows-msvc -- -D warnings

# Verify the workspace builds on the DECLARED MSRV (rust-version in Cargo.toml).
# Catches a dep bump (or newer stdlib use) that silently raises the floor past
# the version we advertise to crates.io consumers of pixtuoid-core. CI-only in
# practice (installs a pinned toolchain + a full check), NOT in preflight.
# Reads the version from Cargo.toml so there's one source of truth.
[group('rust')]
[doc('Check the workspace builds on the declared MSRV (rust-version in Cargo.toml)')]
msrv:
    #!/usr/bin/env bash
    set -euo pipefail
    msrv="$(grep -m1 '^rust-version' Cargo.toml | sed -E 's/.*"([0-9]+\.[0-9]+(\.[0-9]+)?)".*/\1/')"
    echo "declared MSRV: $msrv"
    rustup toolchain install "$msrv" --profile minimal --no-self-update >/dev/null 2>&1 || true
    # Clear RUSTFLAGS so the DEFAULT linker is used. This gate verifies COMPILATION
    # on the floor; the linker is irrelevant to MSRV. `.cargo/config.toml`'s
    # `-fuse-ld=lld` perf flag (x86_64-linux only) needs lld, which a fresh
    # minimal-toolchain build on the CI runner can't resolve — the cached perf
    # jobs never re-link build scripts so they never hit it, but this no-cache
    # gate links them fresh. (RUSTFLAGS env overrides target.*.rustflags wholesale.)
    RUSTFLAGS="" rustup run "$msrv" cargo check --workspace

# SemVer-check the published libraries against their crates.io baselines. CI-only
# in practice: needs network to fetch the baseline crates. Scoped to pixtuoid-core
# (the headless lib) + pixtuoid-scene (the published engine crate); the binary
# crates' libs aren't public API.
[group('rust')]
[doc('SemVer-check pixtuoid-core + pixtuoid-scene against their crates.io baselines (CI-only)')]
semver:
    cargo semver-checks --package pixtuoid-core --package pixtuoid-scene

# Coverage + JUnit XML in one run — the exact command ci.yml's coverage job uses.
# CI-only in practice: needs cargo-llvm-cov + cargo-nextest + the `ci` nextest
# profile. Writes lcov.info + target/nextest/ci/junit.xml.
[group('rust')]
[doc('Coverage + JUnit XML — the exact command ci.yml runs (needs llvm-cov + nextest)')]
coverage:
    cargo llvm-cov nextest --workspace --features {{ features }} --lcov --output-path lcov.info --profile ci

# Snapshot hygiene (cargo-insta): runs the suite under nextest and FAILS on a
# pending (un-accepted `.snap.new`) OR unreferenced (orphan `.snap` — e.g. a
# deleted test's leftover) snapshot. This is the gap plain `cargo test` misses:
# a CHANGED snapshot already fails its own assertion, but an ORPHAN one rots
# silently. CI-only in practice (a second full test run, like coverage/semver) —
# NOT in preflight; run it after adding/removing an insta-snapshot test. Needs
# cargo-insta + cargo-nextest.
[group('rust')]
[doc('Snapshot hygiene (cargo-insta): fail on pending OR orphan snapshots — CI-only')]
snapshots:
    cargo insta test --check --unreferenced=reject --test-runner nextest --workspace --features {{ features }}

# Mutation testing (cargo-mutants): inject bugs into the CHANGED lines and check
# the tests catch them — the "do your assertions have TEETH?" dimension that
# line/region coverage can't see (a covered-but-toothless assertion). DIFF-scoped
# (`--in-diff` vs `$MUTANTS_BASE`, default origin/main) so cost scales with the
# change, not the ~6,900-mutant tree; reads `.cargo/mutants.toml` (nextest + the
# untestable/timing exclusions). ADVISORY — CI runs it NON-blocking; a surviving
# mutant is a hint to strengthen a test, not a merge gate. Run on a
# reducer/decoder/layout PR; forwards args (e.g. `just mutants --list`). Needs
# cargo-mutants + nextest.
[group('rust')]
[doc('Mutation-test the diff vs origin/main (cargo-mutants --in-diff) — advisory')]
mutants *args:
    #!/usr/bin/env bash
    set -euo pipefail
    base="${MUTANTS_BASE:-origin/main}"
    mkdir -p target
    git diff "$base...HEAD" > target/mutants.diff
    # A diff with NO Rust changes (dispatched from `main` where HEAD==origin/main,
    # or a docs/justfile/site-only branch) makes `--in-diff` test ZERO mutants and
    # exit 0 — a vacuous green reading as "teeth verified" having checked nothing
    # (cargo-mutants has no --error-on-zero flag). Gate on actual `.rs` changes.
    if ! git diff --name-only "$base...HEAD" -- '*.rs' | grep -q .; then
        echo "error: no Rust changes vs $base — no mutants to test. Run from a feature branch with .rs changes, or set MUTANTS_BASE." >&2
        exit 1
    fi
    cargo mutants --in-diff target/mutants.diff --features {{ features }} {{ args }}

# Never-panic fuzz the per-source decoders over a JSONL corpus DIR (on-demand;
# not in preflight/CI — points at local or public real sessions, not committed
# data). Auto-routes each line to the CC / Codex / Copilot / Antigravity / hook
# decoder by its shape; exits non-zero on any panic. Examples:
#   just fuzz ~/.claude/projects       # your CC sessions (newest formats)
#   just fuzz ~/.codex/sessions        # your Codex rollouts
#   just fuzz ~/.copilot/session-state # your Copilot CLI sessions
#   git clone https://github.com/daaain/claude-code-log /tmp/cc && just fuzz /tmp/cc/test_data/real_projects
[group('rust')]
[doc('Never-panic fuzz the decoders over a JSONL corpus dir: just fuzz ~/.claude/projects')]
fuzz dir:
    #!/usr/bin/env bash
    set -euo pipefail
    dir="{{ dir }}"
    # Guard the corpus BEFORE fuzzing: under the default no-pipefail shell a
    # typo'd dir made `find` fail while the pipeline status stayed the
    # fuzzer's — which fuzzes zero lines and exits 0, reporting the
    # never-panic contract verified having tested nothing.
    [ -d "$dir" ] || { echo "error: corpus dir '$dir' does not exist" >&2; exit 1; }
    [ -n "$(find "$dir" -name '*.jsonl' -print -quit)" ] || { echo "error: no .jsonl files under '$dir' — nothing to fuzz" >&2; exit 1; }
    cargo build --release --example decoder_fuzz -p pixtuoid-core
    find "$dir" -name '*.jsonl' -print0 | xargs -0 cat | ./target/release/examples/decoder_fuzz

# Compile the workspace; extra args are forwarded:
#   just build                                # debug
#   just build --release                      # release
#   just build --release --bins --examples    # what ci.yml's smoke job builds
[group('rust')]
[doc('Compile the workspace; forwards args (e.g. --release --bins --examples)')]
build *args:
    cargo build --workspace {{ args }}

# Cross-compile a release build for ONE target triple (release.yml's build
# matrix). Pass `true` for targets that need the Docker-backed `cross` toolchain
# (CI installs it via taiki-e/install-action@cross).
[group('rust')]
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
[group('rust')]
[doc('Package the .deb for ONE already-built target (release.yml deb job)')]
deb target:
    cargo deb -p pixtuoid --no-build --no-strip --target {{ target }}
    cargo deb -p pixtuoid-hook --no-build --no-strip --target {{ target }}

# ── site ──────────────────────────────────────────────────────────
# The Astro landing page — a self-contained Node project under site/ with its
# own CI (.github/workflows/site.yml). See site/README.md.

[group('site')]
[doc('Install the site npm deps + the e2e browser (run once per clone)')]
site-setup:
    npm --prefix site ci
    npx --prefix site playwright install chromium

[group('site')]
[doc('Site dev server with HMR → http://localhost:4321/ (foreground; agents: site-dev-bg)')]
site-dev:
    npm --prefix site run dev

# Agent-facing dev-server lifecycle (Astro 7 `--background`): the daemon has no
# stdin/TTY tie, so it survives the launching shell — the foreground `astro dev`
# quits on stdin EOF, which killed agent-driven servers between commands.
# Readiness = the DEV-ONLY /_astro/status health endpoint (preview 404s it);
# the astro bin is called directly like playwright.config.ts does (same cwd, no
# npm wrapper layer). NOTE: dev and preview share port 4321 — stop the daemon
# (site-dev-stop) before `just site-e2e`, or its webServer spawn fails loud.
[group('site')]
[doc('Dev server as a background daemon (survives stdin EOF) — waits on /_astro/status; stop: just site-dev-stop')]
site-dev-bg:
    #!/usr/bin/env sh
    set -eu
    cd site
    node node_modules/astro/bin/astro.mjs dev --background
    # 60 × 0.5s = 30s readiness budget
    for _ in $(seq 1 60); do
        if curl -fsS -m 2 http://localhost:4321/_astro/status >/dev/null 2>&1; then
            echo "ready → http://localhost:4321/  (logs: cd site && npx astro dev logs --follow)"
            exit 0
        fi
        sleep 0.5
    done
    echo "site-dev-bg: daemon started but /_astro/status not ready after 30s" >&2
    exit 1

[group('site')]
[doc('Stop the background dev server (astro dev stop; no-op if none is running)')]
site-dev-stop:
    cd site && node node_modules/astro/bin/astro.mjs dev stop

[group('site')]
[doc('Site static tier: format-check → lint → astro check → knip → unit tests → build (site CI runs e2e + lighthouse after these)')]
site-check:
    npm --prefix site run verify

[group('site')]
[doc('Auto-format the site')]
site-fmt:
    npm --prefix site run format

[group('site')]
[doc('E2E smoke suite vs the PRODUCTION build (astro preview) — the runtime-contract gate')]
site-e2e:
    #!/usr/bin/env sh
    set -eu
    cd site
    # deterministic ★ count for the whole suite (config/gh-stars.mjs GH_STARS_OVERRIDE
    # seam) — an unauthenticated build would otherwise rate-limit to null and hide
    # the star chip, silently no-op-ing its e2e assertion.
    export GH_STARS_OVERRIDE=842
    npm run build
    npx playwright test

# ── gen ───────────────────────────────────────────────────────────
# Regenerate the committed artifacts that derive from a single source of truth:
# README sections from site/src/*.json (gen-readme), and the office images for
# BOTH docs/images/ and site/public/demos/ from scripts/media.json (gen-media).

# Regenerate everything: README sections + docs images + site demos.
[group('gen')]
[doc('Regenerate ALL committed artifacts (README sections + docs images + site demos)')]
gen: gen-readme gen-media gen-icons

# Sync the README's install/features/tools sections from site/src/*.json.
[group('gen')]
[doc('Sync README install/features/tools sections from site/src/*.json')]
gen-readme:
    node scripts/gen-readme.mjs

# Regenerate the --json contract chain after changing `SourceStatus`: re-emit the
# JSON Schema from the Rust serde type, then regenerate the Raycast TS type from
# it. The two freshness gates (the `source_status_schema_matches…` golden test in `just test`, and
# the raycast CI's `gen:contract` diff) FAIL until you run this — so the Rust
# producer and the TS consumer can't hand-drift. Needs raycast deps installed
# (`npm --prefix integrations/raycast ci`).
[group('gen')]
[doc('Regenerate the --json contract: SourceStatus JSON Schema (Rust) + the Raycast TS type')]
gen-contract:
    UPDATE_CONTRACT_SCHEMA=1 cargo test -p pixtuoid --lib schema_matches_the_committed_contract
    npm --prefix integrations/raycast run gen:contract

# Fail if the committed README drifted from site/src/{features,sources,install}.json.
# Pure node:builtins — no npm ci. ci.yml runs this on every PR (the `readme` job),
# and gen-check composes it.
[group('gen')]
[doc('Fail if the committed README drifted from site data (features/sources/install.json)')]
gen-readme-check:
    node scripts/gen-readme.mjs --check

# Regenerate docs/images/ + site/public/demos/ from scripts/media.json — ONE
# manifest-driven driver (replaced gen-docs-images.py + gen-demos.sh). Builds the
# snapshot example once; Pillow for stills/composite/gif, ffmpeg for clips/crops,
# gifsicle for the gif. Forwards args, e.g. `just gen-media --only docs`.
# Requires the .venv (Pillow) + ffmpeg + gifsicle.
[group('gen')]
[doc('Regenerate docs/images/ + site/public/demos/ from scripts/media.json')]
gen-media *args:
    .venv/bin/python3 scripts/gen-media.py {{ args }}

[group('gen')]
[doc('Regenerate site/src/assets/pix-icons/ from the embedded sprite-pack palette')]
gen-icons:
    .venv/bin/python3 scripts/gen-pix-icons.py

# The ONE wasm compile step — gen-wasm (below) and ci.yml's wasm-check job both
# call this, so the package/target/profile CI checks can't drift from what
# gen-wasm ships. Toolchain gotcha (load-bearing, cost 2 debug cycles): the
# PATH cargo/rustc may be Homebrew's, which has NO wasm32 std — and even
# `rustup run stable cargo` fails because cargo resolves `rustc` via PATH. So
# the recipe prepends the RUSTUP toolchain bin (via `rustup which`) and invokes
# that cargo explicitly.
[group('gen')]
[doc('Compile pixtuoid-web for wasm32 (release) — shared by gen-wasm + CI wasm-check')]
wasm-build:
    #!/usr/bin/env sh
    set -eu
    command -v rustup >/dev/null || { echo "needs rustup (Homebrew rust has no wasm std)"; exit 1; }
    rustup target list --toolchain stable --installed | grep -q wasm32-unknown-unknown \
        || { echo "needs the wasm target: rustup target add wasm32-unknown-unknown"; exit 1; }
    TB="$(dirname "$(rustup which --toolchain stable rustc)")"
    PATH="$TB:$PATH" "$TB/cargo" build -p pixtuoid-web --target wasm32-unknown-unknown --release

# The gen-only tool preflight — a SEPARATE recipe so it runs BEFORE the wasm-build
# dependency, failing fast if wasm-bindgen/wasm-opt are missing instead of after a
# minutes-long release compile. wasm-bindgen-cli must match the crate's pinned
# wasm-bindgen (see crates/pixtuoid-web/Cargo.toml); wasm-opt (binaryen) shrinks
# the blob ~10-20%. (ci.yml's wasm-check calls `wasm-build` directly — it only
# compiles, so it needs neither of these.)
[private]
gen-wasm-tools:
    #!/usr/bin/env sh
    set -eu
    command -v wasm-bindgen >/dev/null || { echo "needs wasm-bindgen-cli: cargo install wasm-bindgen-cli --locked"; exit 1; }
    command -v wasm-opt >/dev/null || { echo "needs wasm-opt: brew install binaryen"; exit 1; }

# Build the live-office wasm module (pixtuoid-web) + its JS glue into
# site/public/wasm/ — a COMMITTED artifact (like public/demos/), so the site CI
# stays Node-only. The compile itself is the shared `wasm-build` recipe; the
# gen-only tools are checked first via the gen-wasm-tools pre-dep (fail-fast).
[group('gen')]
[doc('Build pixtuoid-web (wasm) + JS glue into site/public/wasm/')]
gen-wasm: gen-wasm-tools wasm-build
    #!/usr/bin/env sh
    set -eu
    mkdir -p site/public/wasm
    wasm-bindgen --target web --out-dir site/public/wasm \
        target/wasm32-unknown-unknown/release/pixtuoid_web.wasm
    wasm-opt -Oz -o site/public/wasm/pixtuoid_web_bg.wasm site/public/wasm/pixtuoid_web_bg.wasm
    # Stamp the wasm/glue PAIR (#424): the JS glue's ABI must match the exact
    # .wasm it was generated with, so every emitted file's sha256 lands in one
    # manifest, verified by gen-wasm-check. Generation-time stamping keeps CI
    # toolchain-free (byte-exact rebuilds drift across rustc versions — the
    # documented reason rebuild comparison is NOT CI'd).
    # `! -name '.*'` keeps dotfiles out: a Finder-dropped .DS_Store is gitignored,
    # so stamping it would verify locally and fail CI (missing file) — local-green/CI-red.
    (cd site/public/wasm && find . -maxdepth 1 -type f ! -name manifest.sha256 ! -name '.*' | LC_ALL=C sort | xargs shasum -a 256 > manifest.sha256)
    ls -la site/public/wasm/

# Bloat + PAIR gate for the committed wasm artifact. Size: the hero must stay
# a lazy-load behind the poster, so a silent size regression (a dep pulling in
# formatting machinery, an accidental debug build) fails loudly — 1 MiB raw ≈
# ~350-400 KB gzipped; the artifact is ~700 KB today. Pair (#424): the
# wasm-bindgen JS glue's ABI must match the exact .wasm it was generated with;
# a one-sided merge resolution or partial regen ships a silent runtime throw,
# so every committed file must match gen-wasm's sha256 manifest AND every file
# must be covered by it. Byte-exact rebuild-match is deliberately NOT checked
# in CI — wasm output drifts across rustc versions, and CI installs latest
# stable (local `just gen-wasm` + review is the freshness authority, like the
# committed demo media).
[group('gen')]
[doc('Fail if the committed wasm pair is missing, over the size cap, or hash-mismatched')]
gen-wasm-check:
    #!/usr/bin/env sh
    set -eu
    W=site/public/wasm/pixtuoid_web_bg.wasm
    M=site/public/wasm/manifest.sha256
    test -f "$W" || { echo "missing $W — run 'just gen-wasm'"; exit 1; }
    CAP=1048576
    SIZE=$(wc -c < "$W" | tr -d ' ')
    test "$SIZE" -le "$CAP" || { echo "$W is $SIZE bytes (> $CAP cap) — investigate the bloat"; exit 1; }
    test -f "$M" || { echo "missing $M — run 'just gen-wasm' (the wasm/glue pair manifest)"; exit 1; }
    (cd site/public/wasm && shasum -a 256 --strict -c manifest.sha256 >/dev/null) \
        || { echo "wasm/glue pair MISMATCH vs $M — a partial regen or one-sided merge; run 'just gen-wasm' and commit all of site/public/wasm/"; exit 1; }
    for f in site/public/wasm/*; do
        b=$(basename "$f")
        [ "$b" = manifest.sha256 ] && continue
        awk -v want="./$b" '$2 == want { found = 1 } END { exit !found }' "$M" \
            || { echo "$f is not covered by $M — run 'just gen-wasm'"; exit 1; }
    done
    echo "gen-wasm-check OK: $W ($SIZE bytes <= $CAP), pair manifest verified"

# Drift gate: fail if any committed README section OR rendered still is stale.
# Pixel-diffs every PNG (threshold 0); video clips + demo.gif are presence-only
# (ffmpeg/gifsicle bytes aren't stable cross-version, but the renders feeding
# them ARE pixel-deterministic). Run by ci.yml's smoke job; runnable locally
# before pushing a visual change. A red check after an INTENTIONAL office change
# means: run `just gen` and commit the regenerated docs/images/ +
# site/public/demos/ in the same change. Requires the .venv + ffmpeg + gifsicle
# + a release build of the snapshot example.
[group('gen')]
[doc('Fail if any committed README section or rendered image has drifted')]
gen-check: gen-readme-check gen-wasm-check
    #!/usr/bin/env sh
    set -eu
    test -x .venv/bin/python3 || { echo "needs the venv: python3 -m venv .venv && .venv/bin/pip install -r requirements-dev.txt"; exit 1; }
    .venv/bin/python3 scripts/gen-media.py --check
    .venv/bin/python3 scripts/gen-pix-icons.py --check

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

    # the release-notes injection (step 5) is an awk match on this marker; if it's
    # ever removed the awk silently no-ops, leaving version.rs without the new arm
    # (surfacing only later as a cryptic preflight test failure). Fail loud here —
    # the one un-guarded step in an otherwise heavily-guarded recipe.
    if ! grep -q '\[bump-inject-here\]' crates/pixtuoid/src/version.rs; then
        echo "error: version.rs is missing the [bump-inject-here] marker — release-notes injection would silently no-op" >&2; exit 1; fi
    if ! grep -q '\[bump-version-list-here\]' crates/pixtuoid/src/version.rs; then
        echo "error: version.rs is missing the [bump-version-list-here] marker — the SHIPPED_VERSIONS injection would silently no-op" >&2; exit 1; fi

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
    awk -v f="$notes" -v ver="$ver" '
        /\[bump-inject-here\]/ { print; while ((getline l < f) > 0) print l; next }
        /\[bump-version-list-here\]/ { print; printf "        \"%s\",\n", ver; next }
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

    printf '\n\033[32m✓ v%s committed on %s\033[0m\n\n  next:\n    1. curate the drafted bullets in crates/pixtuoid/src/version.rs (release_notes\n       arm) down to ~6 highlights, then: git commit --amend -a\n    2. regenerate committed artifacts — the office HUD bakes CARGO_PKG_VERSION, so a\n       bump drifts every still: just gen, then commit docs/images + site/public/demos\n       (else CI smoke gen-check reds the PR)\n    3. open a PR, review, merge to main\n    4. AFTER merge, tag to publish — IRREVERSIBLE (crates.io + homebrew):\n         git tag v%s && git push origin v%s\n' "$ver" "$branch" "$ver" "$ver"

# Unit-test the npm package generator (Node, no cargo). The ONLY validation of
# npm/generate.mjs — release.yml runs this as a hard gate right before `npm
# publish`, and ci.yml runs it on every PR so a generator regression is caught
# at review time, not at the irreversible tag-push. NOT in preflight: a Rust
# pre-push shouldn't require a Node toolchain. Needs Node ≥ 22.
[group('release')]
[doc('Test the npm package generator (Node; CI + release call it, not in preflight)')]
npm-check:
    node --test npm/generate.test.mjs

# Fail if the current release_notes() arm still has the uncurated TODO marker.
# A release-PR guard (#116) — deliberately NOT in preflight, since `just bump`
# leaves the marker for the human to curate after the bump commit.
[group('release')]
[doc('Fail if release_notes() still has the uncurated TODO marker (release-PR guard)')]
notes-curated:
    #!/usr/bin/env bash
    set -euo pipefail
    if grep -q 'TODO: curate' crates/pixtuoid/src/version.rs; then
        echo "error: release_notes() still has the 'TODO: curate' marker — curate the drafted bullets before merge" >&2
        exit 1
    fi
    echo "release notes curated ✓"

# ── meta ──────────────────────────────────────────────────────────

# Full pre-push gate: the Rust checks worth running locally before a push.
# (semver, coverage, and the gen/smoke gates are CI-only — network baseline /
# heavy builds / venv+ffmpeg.)
[group('meta')]
[doc('Full pre-push gate: lint → clippy → hack → test')]
preflight: lint clippy hack test

# Everything: the Rust pre-push gate + the site gate + the artifact-drift gate.
# Heavier than preflight (needs the site npm deps + the .venv + ffmpeg).
[group('meta')]
[doc('Full-stack gate: preflight + site-check + gen-check')]
verify: preflight site-check gen-check

# Install the dev tools every check + recipe relies on (idempotent). Prefers
# cargo-binstall (prebuilt) and falls back to cargo install (compiles).
[group('meta')]
[doc('Install the dev tools the checks + recipes need (idempotent)')]
setup-tools:
    #!/usr/bin/env bash
    set -euo pipefail
    tools=(cargo-nextest cargo-machete cargo-deny cargo-hack cargo-semver-checks cargo-edit cargo-insta lychee)
    if command -v cargo-binstall &>/dev/null; then
        cargo binstall -y "${tools[@]}"
    else
        echo "cargo-binstall not found — compiling from source (slow)." >&2
        echo "brew install cargo-binstall (or cargo install cargo-binstall) to grab prebuilt binaries instead." >&2
        cargo install "${tools[@]}"
    fi
    # The rust-analyzer component powers the editor / AI-agent LSP (go-to-def,
    # find-references — the tool the "change all N keying sites in lockstep"
    # invariants depend on). rust-toolchain.toml pins only rustfmt+clippy, so
    # without this the `~/.cargo/bin/rust-analyzer` rustup shim errors with
    # "Unknown binary" and the LSP silently degrades to grep. Idempotent; skipped
    # cleanly when rustup is absent (e.g. a distro-packaged toolchain).
    if command -v rustup &>/dev/null; then
        rustup component add rust-analyzer >/dev/null 2>&1 ||
            echo "could not add the rust-analyzer component — install it for LSP support" >&2
    fi
    # Non-cargo lint tools that `just lint` gates on (shfmt formats shell,
    # actionlint lints the workflows, and shellcheck backs actionlint's run-block
    # checks — WITHOUT it on PATH, actionlint silently SKIPS them, so a shell bug
    # in a workflow `run:` block passes `just lint` green locally). brew on macOS;
    # elsewhere point at the install docs rather than silently leaving `just lint`
    # unable to run — or, worse, passing with the shellcheck pass quietly skipped.
    for t in shfmt actionlint shellcheck; do
        command -v "$t" &>/dev/null && continue
        if command -v brew &>/dev/null; then
            brew install "$t" || true
        fi
    done
    # Re-verify AFTER the install attempts: a `brew install` that exits 0 without
    # putting the binary on PATH (transient failure), or no brew at all, must be
    # caught here — not silently pass as a successful setup (the #283-class silent
    # no-op this recipe is meant to prevent).
    missing=()
    for t in shfmt actionlint shellcheck; do
        command -v "$t" &>/dev/null || missing+=("$t")
    done
    if (( ${#missing[@]} )); then
        echo "error: ${missing[*]} still missing after setup — install via your package manager (e.g. brew install ${missing[*]}); \`just lint\` needs it." >&2
        exit 1
    fi
    # Activate the local pre-push gate (dormant by default in a fresh clone, so CI
    # would otherwise be the only gate). Idempotent. CI re-runs `just preflight`
    # regardless, so a skipped local hook still meets the same checks at merge.
    git config core.hooksPath .githooks

# Self-test the upstream-drift watcher — its ONLY test. A regex-parser regression
# is a silent monitor death (the script returns empty / raises, the weekly job
# alarms on junk or watches nothing); this pins the parsers + the fetch
# classifier. Pure Python, no deps, no network.
[group('meta')]
[doc('Self-test the upstream-drift watcher (parsers + fetch classifier)')]
drift-selftest:
    python3 scripts/check_upstream_drift_selftest.py

# Risk radar — show the documented review escalations for the high-risk seams
# THIS branch touches (advisory, deterministic, no LLM). Dogfood before pushing
# so you know what a reviewer must check; the `risk radar` PR workflow posts the
# same checklist as a sticky comment. `base` defaults to the branch point.
[group('meta')]
[doc('Surface review escalations for the high-risk seams this branch touches')]
risk-radar base="origin/main":
    @git diff --name-only {{ base }}...HEAD | python3 scripts/risk-radar.py || true

# Self-test the risk-radar matcher — the gate on its seam map (the `risk radar`
# workflow runs this before every radar). A broken predicate is a silent
# escalation miss. Pure Python, no deps, no network.
[group('meta')]
[doc('Self-test the risk-radar matcher (seam map predicates)')]
risk-radar-test:
    python3 scripts/risk-radar.py --selftest

