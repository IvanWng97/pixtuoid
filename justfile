# Project task runner. The git hooks, ci*.yml and release.yml call these recipes
# rather than restating their commands, so a command changes in one place;
# `workspace-version` names the one exception.
#
# Recipes are grouped by intent (see `just --list`):
#   rust     — build, test and lint the repo (Rust, shell, workflows), plus the
#              release builds and the on-demand e2e / capture / fixture recipes
#   site     — the Astro landing page under site/ (npm, its own CI)
#   gen      — regenerate + check committed artifacts
#   release  — npm-check, the Node gate release.yml runs before npm publish (and
#              ci-lint.yml's `npm-gen` job)
#   meta     — tooling setup, the local gate (preflight), the fixture
#              gates, and the gates' selftests

# Git Bash is preinstalled on GHA windows runners; keeps every recipe
# single-sourced cross-platform (ci-tests.yml's windows jobs call recipes, never
# inline commands).
set windows-shell := ["bash", "-cu"]

# ── variables ─────────────────────────────────────────────────────
# just evaluates these globally regardless of position; kept at the top (the
# idiom) so the file's config lives in one place.

# The published API surface: the ONLY two crates whose public API is a contract
# (the binary lib target is not). Single-sourced here so both gates over it —
# api-surface / api-surface-check — can't drift; a newly-published crate is
# added in ONE place. release-plz semver-checks every crate with a library, so
# release-plz.toml switches the check off for the binary's lib target.
PUBLISHED_CRATES := "pixtuoid-core pixtuoid-scene"

# Standalone shell FILES share one authority so formatting and lint coverage
# cannot drift. Shell embedded in YAML is a second population this cannot cover:
# workflow `run:` blocks go to actionlint, composite-action ones to
# `actionlint-composites`. Both are shellcheck-only — shfmt cannot rewrite a
# scalar in place — so adding a file here is not enough for embedded shell.
SHELL_SOURCES := "scripts/lib/*.sh .githooks/* policy/ci-observability/*.sh"

# The nightly the api-surface goldens are pinned to (rustdoc JSON is
# nightly-only). Provisioned by `_api-toolchain`.
API_NIGHTLY := "nightly-2026-07-22"

# The cargo-public-api the api/ goldens are reproducible against — tool-exact:
# a different version rewrites them all. ci-lint.yml installs its own copy;
# `_api-toolchain` asserts the pair, so divergence fails loud instead of
# churning goldens.
API_PUBLIC_API := "0.52.0"

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
# Pairs with the shellcheck house rule: shellcheck lints, shfmt formats. `-i 4`
# (4-space) matches the prevailing style; no `-ci` so case bodies stay
# un-indented as written.
[group('rust')]
[doc('Shell-format check over repository shell sources')]
shfmt-check:
    shfmt -i 4 -d {{ SHELL_SOURCES }}

# Apply shell formatting in place (the `.sh` analog of `fmt`).
[group('rust')]
[doc('Apply shfmt formatting in place over repository shell sources')]
shfmt-fix:
    shfmt -i 4 -w {{ SHELL_SOURCES }}

[group('rust')]
[doc('Run shellcheck over repository shell sources')]
shellcheck:
    shellcheck {{ SHELL_SOURCES }}

# Lint the GitHub Actions workflows (actionlint): YAML schema, expression types,
# action input/output names, runner labels, AND shellcheck over every `run:`
# block (so a shell bug inside a workflow is caught at author time, not on a red
# main). Gated via `lint`; the CI `hygiene` job runs it too. Needs shellcheck on
# PATH for the run-block checks (the house-rule tool — already required).
[group('rust')]
[doc('Lint the GitHub Actions workflows (actionlint + shellcheck over run: blocks)')]
actionlint:
    actionlint

# The blind spot the recipe above cannot cover: actionlint models WORKFLOWS, so
# it discovers only .github/workflows and rejects an action.yml outright
# ("jobs section is missing"). Shell that moves from a workflow into a composite
# action therefore loses its shellcheck coverage silently. Pull each `run:` out
# ourselves and check it with the same linter.
[group('rust')]
[doc('Shellcheck every run: block inside the composite actions (actionlint cannot parse action.yml)')]
actionlint-composites:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s nullglob
    actions=(.github/actions/*/action.y*ml) # GitHub accepts action.yaml too
    ((${#actions[@]})) || { echo "error: no composite actions found" >&2; exit 1; }
    work="$(mktemp -d)"
    trap 'rm -rf "$work"' EXIT
    checked=0
    skipped=()
    for action in "${actions[@]}"; do
        count="$(yq '[.runs.steps[] | select(has("run"))] | length' "$action")"
        ((count)) || continue # a pure `uses:` composite has no shell to check
        for i in $(seq 0 $((count - 1))); do
            # The default should never fire — a composite run step must name a
            # shell — but bash is what actionlint assumes for a workflow step.
            shell="$(yq -r ".runs.steps | map(select(has(\"run\"))) | .[$i].shell // \"bash\"" "$action")"
            case "$shell" in
            bash | sh) ;;
            # pwsh/python are not shellcheck's to judge, but a bounded gate that
            # does not name what it dropped reads as full coverage.
            *)
                skipped+=("$action step $i ($shell)")
                continue
                ;;
            esac
            script="$work/$(echo "$action" | tr /. __)-$i.$shell"
            { echo "#!/usr/bin/env $shell"; yq -r ".runs.steps | map(select(has(\"run\"))) | .[$i].run" "$action"; } >"$script"
            shellcheck -s "$shell" "$script" || { echo "  ^ from $action step $i" >&2; exit 1; }
            checked=$((checked + 1))
        done
    done
    ((checked > 0)) || { echo "error: no composite run: blocks were checked" >&2; exit 1; }
    echo "$checked composite run: blocks shellchecked"
    ((${#skipped[@]})) && printf '  skipped (not a shellcheck dialect): %s\n' "${skipped[@]}"
    exit 0

# Security audit for workflows/actions/Dependabot. zizmor owns the parser and
# audit catalog; .github/zizmor.yml records the repository's deliberate
# ref-or-SHA pin policy and every accepted finding is suppressed at its exact
# source location with a WHY.
# The operating MODE is env-derived, not chosen here, and the asymmetry is
# deliberate: tokenless it runs OFFLINE (it says so on stderr) and skips the
# four audits that need the GitHub API — impostor-commit,
# known-vulnerable-actions, ref-confusion, stale-action-refs (typosquat-uses
# still runs, at reduced confidence). ci-lint.yml's hygiene job passes
# GH_TOKEN, so those DO gate in CI: there the recipe refuses to run tokenless.
# Same call as `links` (--offline) and `deny` (advisories deferred to audit.yml): a
# check whose verdict depends on the network and an upstream feed must not
# redden a push of unchanged code. Do NOT auto-export `gh auth token` to close
# the gap — it puts a real token on the wire on every pre-push run and makes the
# local gate depend on gh auth + API rate limits, the exact flakiness those two
# siblings were written to avoid.
[group('rust')]
[doc('Audit GitHub automation security with zizmor')]
zizmor:
    @if [ -n "${GITHUB_ACTIONS:-}" ] && [ -z "${GH_TOKEN:-}" ]; then \
        echo "error: zizmor would run offline in CI and skip its four online audits; give this step a GH_TOKEN" >&2; \
        exit 1; \
    fi
    zizmor --strict-collection .

# action_behavior_test.sh runs the workflows' own shell against stubs, which no
# static contract can do.
[group('rust')]
[doc('Check the CI contracts actionlint and zizmor cannot see')]
ci-observability:
    bash policy/ci-observability/check.sh
    bash policy/ci-observability/action_behavior_test.sh

# Every committed JSON Schema, held to the metaschema. These are contracts a
# consumer reads at runtime — the review schema reaches the Claude CLI, the
# raycast ones pin the `--json` shape — and nothing else parses them: a broken
# one is invisible until the consumer refuses to start.
[group('rust')]
[doc('Validate every committed JSON Schema against the metaschema (check-jsonschema)')]
json-schemas:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s nullglob
    schemas=(.github/prompts/review-schema.json integrations/raycast/contract/*.schema.json)
    ((${#schemas[@]})) || { echo "error: no committed JSON Schemas found" >&2; exit 1; }
    check-jsonschema --check-metaschema "${schemas[@]}"
    echo "${#schemas[@]} JSON Schemas validated"

# Offline link + anchor check (lychee) over the repo's OWN markdown: every
# relative cross-link between the nested CLAUDE.md/AGENTS.md guides + docs/ must
# resolve, and `#anchor` fragments must exist. Directory-walk mode respects
# .gitignore (vendored node_modules etc. auto-skipped); `--offline` = no network,
# so it's deterministic + flake-free. External-URL decay is deliberately NOT
# gated here (it's flaky on the PR path). Gated via `lint`; CI `hygiene` runs it.
[group('rust')]
[doc('Offline link + anchor check (lychee) over the repo markdown — no network, .gitignore-aware')]
links:
    # Source CSS uses Vite package specifiers; its module graph belongs to the
    # site build, while this gate owns documentation links and anchors.
    lychee --offline --include-fragments --extensions md .

# Clippy across the workspace, warnings denied.
[group('rust')]
clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# Unused-dependency check.
[group('rust')]
machete:
    cargo machete

# License + supply-chain gate (bans/licenses/sources). Advisories are NOT here:
# they're owned by the daily audit.yml (`check advisories`) so an overnight
# RustSec advisory can't block a push of unchanged code.
[group('rust')]
deny:
    cargo deny check bans licenses sources

# A PATH-valued env var read with `env::var` DROPS a non-UTF-8 value — a legal
# path — and falls back to a different directory, silently. `--selftest` proves
# the checker can FAIL.
[group('rust')]
[doc('Gate: PATH-valued env vars must be read as bytes, never via env::var')]
env-paths:
    python3 scripts/check-env-paths.py --selftest
    python3 scripts/check-env-paths.py

# Architecture invariant #1, mechanized: pixtuoid-core + pixtuoid-scene stay
# terminal/window/audio-device-free.
[group('rust')]
arch:
    #!/usr/bin/env bash
    set -euo pipefail
    # The backend-agnostic layers — neither may pull a terminal (ratatui/crossterm),
    # window (winit/softbuffer), OR audio-device (rodio/cpal) crate; the binary's
    # painters + audio gateway own those. The
    # crate boundary already makes this a COMPILER fact; this pins it at the dep-tree
    # level too (a transitive pull-in via a feature would slip past the boundary).
    # `--target all` + `--all-features` are LOAD-BEARING, not thoroughness: cargo
    # tree defaults to the runner's own triple under default features, so a
    # `[target.'cfg(windows)'.dependencies] crossterm` in pixtuoid-core resolved
    # green on macOS AND on the ubuntu CI runner — invariant #1 broken on Windows
    # behind a passing gate, and `just check-windows` compiles it happily because
    # the dep is legitimate for that target. `--target all` is metadata-only (it
    # installs nothing), and features are additive, so `--all-features` holds
    # every dep any feature combination can pull.
    for crate in pixtuoid-core pixtuoid-scene; do
        # Capture first so a cargo-tree ERROR (e.g. a crate rename) kills the
        # recipe via set -e, instead of reading as "no match" inside the if —
        # which would print the green line without having checked anything.
        deps="$(cargo tree -p "$crate" --edges normal --prefix none --target all --all-features)"
        if grep -qE '^(ratatui|crossterm|winit|softbuffer|rodio|cpal)' <<<"$deps"; then
            echo "ARCH VIOLATION: $crate depends on a terminal/window/audio-device crate (CLAUDE.md invariant #1)"; exit 1
        fi
    done
    echo "arch: pixtuoid-core + pixtuoid-scene are terminal/window/audio-device-free"

# Fast, independent lint checks in parallel.
[group('rust')]
lint:
    #!/usr/bin/env bash
    set -euo pipefail
    # Fail fast with an actionable message when a lint tool is missing, instead
    # of a bare `command not found` (exit 127) buried in a parallel job's log.
    missing=()
    for t in shfmt shellcheck actionlint zizmor check-jsonschema yq jq cargo-machete cargo-deny lychee gitleaks; do
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
    run env-paths just env-paths        & pids+=($!)
    run genart  just gen-art-check       & pids+=($!)
    run machete cargo machete           & pids+=($!)
    run deny    just deny                & pids+=($!)
    run arch    just arch                & pids+=($!)
    run shfmt   just shfmt-check         & pids+=($!)
    run shell   just shellcheck           & pids+=($!)
    run actions just actionlint          & pids+=($!)
    run composites just actionlint-composites & pids+=($!)
    run zizmor  just zizmor              & pids+=($!)
    run ci-obs  just ci-observability     & pids+=($!)
    run schemas just json-schemas         & pids+=($!)
    run links   just links               & pids+=($!)
    run drift   just drift-selftest       & pids+=($!)
    run tuidrive just tuidrive-selftest   & pids+=($!)
    run e2escrub just e2e-scrub-selftest  & pids+=($!)
    run starhist just star-history-selftest & pids+=($!)
    run fixpii  just fixture-pii          & pids+=($!)
    run piiself just fixture-pii-selftest & pids+=($!)
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
        cargo nextest run --workspace {{ args }}
    else
        cargo test --workspace {{ args }}
    fi

# Frame + wire benchmarks — LOCAL statistical numbers (criterion). CI's
# bench.yml runs the same recipe on-demand, advisory-only: shared-runner
# wall-clock is noise (criterion's own FAQ), so no benchmark ever gates.
# `render_frame` costs a FRAME, `decode_reduce` costs an EVENT; codspeed.yml
# instruments both. Filter forwards to both targets, and a filter matching
# nothing in one of them is not an error: `just bench 360` runs every 360x240
# case, `just bench hook` only the hook-transport fold.
[group('rust')]
[doc('Render-path + wire-path criterion benchmarks; forwards a filter')]
bench *args:
    cargo bench -p pixtuoid-scene --bench render_frame -- {{ args }}
    cargo bench -p pixtuoid-core --bench decode_reduce -- {{ args }}

# Feature-combination check — every feature subset must compile. Catches code
# that silently only builds with `native` on (the wasm core builds without it).
[group('rust')]
[doc('Feature-powerset check — every feature subset must compile')]
hack:
    #!/usr/bin/env bash
    set -euo pipefail
    command -v cargo-hack &>/dev/null || { echo "error: cargo-hack not found — run \`just setup-tools\`" >&2; exit 1; }
    cargo hack --feature-powerset --no-dev-deps check --workspace

# Cross-lint the workspace for Windows (clippy subsumes check; no linking).
# Same toolchain gotcha as `api-surface` and `wasm-build`, and it bites HARDER
# here because the compiler's own advice is wrong: a Homebrew cargo ahead of the
# rustup proxy on PATH ships only the host std, so the cross-lint dies on E0463
# "can't find crate for `core`" while suggesting `rustup target add
# x86_64-pc-windows-msvc` for a target rustup already has. Prepending the proxy
# (a no-op on CI, where it is already first) fixes it; the explicit preflight
# then owns the genuinely-missing case with an accurate message. This is the
# documented way to pre-verify a path-string change against `windows-test`,
# which local preflight is otherwise blind to — so it has to actually run.
[group('rust')]
[doc('Cross-lint the workspace for x86_64-pc-windows-msvc via clippy (no linking; ubuntu runner suffices)')]
check-windows:
    #!/usr/bin/env bash
    set -euo pipefail
    export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
    rustup target list --toolchain stable --installed | grep -q x86_64-pc-windows-msvc \
        || { echo "needs the target: rustup target add x86_64-pc-windows-msvc" >&2; exit 1; }
    cargo clippy --workspace --all-targets --target x86_64-pc-windows-msvc -- -D warnings

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

# Reproduce release-plz's semver verdict LOCALLY. Not a gate and not in CI:
# release-plz runs cargo-semver-checks itself on the release PR and RAISES the
# bump when it finds a break, so this exists only so a human can see the same
# answer before dispatching. Needs network for the baseline crates, and
# cargo-semver-checks on PATH (`cargo binstall cargo-semver-checks`).
[group('rust')]
[doc("Reproduce release-plz's semver verdict for the published crates (local, not a gate)")]
semver:
    cargo semver-checks $(printf -- '--package %s ' {{PUBLISHED_CRATES}})

# Public-API surface snapshot for the PUBLISHED libraries. COMPLEMENTS
# release-plz's own semver check: that answers "is the release bump enough?" on
# the release PR, this shows *what* changed as a reviewable golden diff at
# review time. Goldens live in `api/<crate>.txt` —
# `cargo public-api -s` output (`-s` omits blanket-impl noise like
# `Into`/`Receiver`; auto-derived `Clone`/`Serialize`/… STAY, since
# adding/removing a derive IS a public-API change). cargo-public-api takes one
# crate per call, so a golden file is regenerated per crate. rustdoc JSON is
# nightly-only, so both recipes PIN {{API_NIGHTLY}}. CI runs `api-surface-check`.
[group('rust')]
[doc('Regenerate the api/<crate>.txt public-API goldens (cargo-public-api + pinned nightly)')]
api-surface: _api-toolchain
    #!/usr/bin/env bash
    set -euo pipefail
    # cargo-public-api only honors RUSTUP_TOOLCHAIN when the invoked `cargo` is
    # the rustup PROXY. A Homebrew/system cargo ahead of it on PATH ignores the
    # env, so cargo-public-api falls back to rust-toolchain.toml's STABLE pin and
    # dies on `-Z` (nightly-only). Prepend the rustup bin so the proxy wins (a
    # no-op on CI, where it's already first).
    export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
    for crate in {{PUBLISHED_CRATES}}; do
        RUSTUP_TOOLCHAIN={{API_NIGHTLY}} cargo public-api -p "$crate" -s > "api/$crate.txt"
    done

[group('rust')]
[doc("Fail if a published crate's public API drifted from the api/ goldens (CI-only)")]
api-surface-check: _api-toolchain
    #!/usr/bin/env bash
    set -euo pipefail
    # See `api-surface`: force the rustup proxy cargo so RUSTUP_TOOLCHAIN is honored.
    export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    fail=0
    for crate in {{PUBLISHED_CRATES}}; do
        RUSTUP_TOOLCHAIN={{API_NIGHTLY}} cargo public-api -p "$crate" -s > "$tmp/$crate.txt"
        if ! diff -u "api/$crate.txt" "$tmp/$crate.txt"; then
            echo "error: public API of $crate drifted from api/$crate.txt — run 'just api-surface' and commit the update" >&2
            fail=1
        fi
    done
    exit "$fail"

# Both halves of the goldens' reproducibility contract: refuse a mismatched
# cargo-public-api, then self-provision the pinned nightly (rustdoc JSON is
# nightly-only) if it isn't already installed. The minimal profile carries
# rustdoc (bundled with rustc), all cargo-public-api needs.
[private]
_api-toolchain:
    #!/usr/bin/env bash
    set -euo pipefail
    have="$(cargo public-api --version 2>/dev/null | awk '{print $NF}' || true)"
    if [ -z "$have" ]; then
        echo "error: cargo-public-api not installed — goldens need {{ API_PUBLIC_API }} (just setup-tools)" >&2
        exit 1
    fi
    if [ "$have" != "{{ API_PUBLIC_API }}" ]; then
        echo "error: cargo-public-api $have installed, goldens need {{ API_PUBLIC_API }} (just setup-tools)" >&2
        exit 1
    fi
    command -v rustup >/dev/null || { echo "rustup not found — install {{API_NIGHTLY}} manually for api-surface" >&2; exit 1; }
    rustup toolchain list | grep -q '{{API_NIGHTLY}}' && exit 0
    echo "installing {{API_NIGHTLY}} (api-surface needs nightly rustdoc JSON)…" >&2
    rustup toolchain install {{API_NIGHTLY}} --profile minimal

# Doc-rendering gate. Two things `cargo build`/`clippy`/`nextest` can't see:
# (1) build every item's docs, private ones included, with EVERY rustdoc
# warning as an error — rustdoc resolves links only on the items it renders, so
# a public-only build lets a private item's links rot unseen, while
# `private_intra_doc_links` still fires on a public doc naming a private item
# (the link docs.rs would render broken). The broken/private intra-doc-link
# classes are already `deny` in `[workspace.lints.rustdoc]`; `-D warnings` adds
# bare URLs, invalid HTML, redundant links, and any future rustdoc lint; (2) RUN
# the doctests — `cargo nextest` does NOT execute doctests, so the crate-root
# examples would otherwise go ungated. CI-only in practice (a doc build + a doctest run).
[group('rust')]
[doc('Doc gate: cargo doc (incl. private items) with -D warnings + run the doctests nextest skips (CI-only)')]
doc-check:
    #!/usr/bin/env bash
    set -euo pipefail
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace --document-private-items
    cargo test --doc --workspace

# Coverage + JUnit XML in one run — CI's only test run, on every platform
# (ci-tests.yml's coverage, windows-test and macos-test).
# CI-only in practice: needs cargo-llvm-cov + cargo-nextest + the `ci` nextest
# profile. Writes lcov.info + target/nextest/ci/junit.xml.
[group('rust')]
[doc('Coverage + JUnit XML — the exact command ci-tests.yml runs (needs llvm-cov + nextest)')]
coverage:
    cargo llvm-cov nextest --workspace --lcov --output-path lcov.info --profile ci

# Snapshot hygiene (cargo-insta): runs the suite under nextest and FAILS on a
# pending (un-accepted `.snap.new`) OR unreferenced (orphan `.snap` — e.g. a
# deleted test's leftover) snapshot. This is the gap plain `cargo test` misses:
# a CHANGED snapshot already fails its own assertion, but an ORPHAN one rots
# silently. CI-only in practice (a second full test run, like coverage) —
# NOT in preflight; run it after adding/removing an insta-snapshot test. Needs
# cargo-insta + cargo-nextest.
[group('rust')]
[doc('Snapshot hygiene (cargo-insta): fail on pending OR orphan snapshots — CI-only')]
snapshots:
    cargo insta test --check --unreferenced=reject --test-runner nextest --workspace

# Mutation testing (cargo-mutants): inject bugs into the CHANGED lines and check
# the tests catch them — the "do your assertions have TEETH?" dimension that
# line/region coverage can't see (a covered-but-toothless assertion). DIFF-scoped
# (`--in-diff` vs `$MUTANTS_BASE`, default origin/main) so cost scales with the
# change, not the whole tree; reads `.cargo/mutants.toml` (nextest + the
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
    # Gate on the MUTANT COUNT, not on `.rs` changes. cargo-mutants has no
    # --error-on-zero and exits 0 having tested nothing when the diff yields no
    # mutants — a vacuous green reading as "teeth verified". A `.rs`-changes
    # check is NOT sufficient: a diff of test files plus `exclude_globs` entries
    # yields zero mutants and passes it. `--list` enumerates without running
    # (sub-second), so the pre-check is cheap.
    #
    # A FAILING tool and an empty result are reported separately. Folding them
    # sends the reader to inspect their diff when the real cause is a missing
    # cargo-mutants or an unparseable .cargo/mutants.toml — misdirection is the
    # failure class this gate exists to remove, so it must not commit it.
    if ! listed=$(cargo mutants --in-diff target/mutants.diff --list 2>/dev/null); then
        echo "error: \`cargo mutants --list\` failed — the mutant count is unknown." >&2
        echo "  Usually a missing cargo-mutants (\`just setup-tools\`) or an" >&2
        echo "  unparseable .cargo/mutants.toml. Rerunning with stderr shown:" >&2
        cargo mutants --in-diff target/mutants.diff --list >/dev/null || true
        exit 1
    fi
    if [ -z "$listed" ]; then
        echo "error: the diff vs $base yields ZERO mutants — nothing would be tested." >&2
        echo "  Either there are no .rs changes, or every changed .rs is test code" >&2
        echo "  or excluded by .cargo/mutants.toml (exclude_globs OR exclude_re —" >&2
        echo "  the latter can empty a file's mutants function by function)." >&2
        echo "  Run from a branch touching mutable production Rust, or set MUTANTS_BASE." >&2
        exit 1
    fi
    cargo mutants --in-diff target/mutants.diff {{ args }}

# Record a conformance fixture from bytes a real CLI actually sent. Hook-only
# sources have no persistent corpus — hook events are transient — so their
# fixtures are the ONLY wire evidence they have, and the ones this tree has not
# re-recorded yet were composed by hand. One BILLED model turn per run.
# `{prompt}` expands to the shared scenario prompt; a custom one has to go
# through the script directly, since just joins variadic args and loses quoting.
#   just capture-fixture cursor tool-run cursor-agent -p --trust '{prompt}'
#   just capture-fixture kimi permission-flow "$SHELL"   # drive the TUI yourself
[group('rust')]
[doc('Record a conformance fixture from a real CLI run (BILLED — one model turn)')]
capture-fixture source scenario *cmd:
    cargo run --release -q -p pixtuoid-core --example capture_fixture -- \
        {{ source }} {{ scenario }} {{ cmd }}

[group('rust')]
[doc('Strip every committed fixture of what no decoder reads, in place (local, unbilled)')]
restrip-fixtures:
    cargo run --release -q -p pixtuoid-core --example capture_fixture -- --strip-corpus

# The corpus census, every transcript-bearing source in one pass — the drift half
# of the pair: fixtures catch a decode regression, real bytes catch the wire
# changing under us. Roster and roots both come from the registry.
[group('rust')]
[doc('Census every transcript-bearing source against its real local corpus')]
corpus-all:
    #!/usr/bin/env bash
    set -uo pipefail
    cc=target/release/examples/corpus_check
    [ -x "$cc" ] || { echo "run: just build --release --examples" >&2; exit 2; }
    # Read the roster BEFORE the loop: as a process substitution its exit status
    # is unobservable, so a roster that dies would feed an empty loop and the
    # census would report "everything clean" having censused nothing.
    roster="$("$cc" --roster)" || { echo "corpus_check --roster failed" >&2; exit 2; }
    [ -n "$roster" ] || { echo "corpus_check --roster returned no rows" >&2; exit 2; }
    rc=0
    n=0
    uncovered=()
    while IFS=$'\t' read -r id _ kind _; do
        [ "$kind" = transcript ] || continue
        n=$((n + 1))
        echo "── $id"
        "$cc" "$id"
        # 3 is "no corpus on this host" — never ran that CLI here. It is NOT a
        # defect, and it must not read as covered either, so it is reported apart
        # from both.
        case $? in
        0) ;;
        3) uncovered+=("$id") ;;
        *) rc=1 ;;
        esac
    done <<<"$roster"
    # `-n "$roster"` checked the TEXT; this checks the PARSE. A roster that is
    # non-empty but yields zero transcript rows — a column insertion, a rename of
    # the `transcript` literal — reproduces the exact silent-empty census the
    # guard above was added for.
    [ "$n" -gt 0 ] || { echo "roster parsed 0 transcript rows — column layout changed?" >&2; exit 2; }
    if [ ${#uncovered[@]} -gt 0 ]; then
        echo "NOT COVERED (no local corpus): ${uncovered[*]}"
    fi
    exit "$rc"

# Never-panic fuzz ONE source's transcript decoder over a JSONL corpus DIR
# (on-demand; not in preflight/CI — points at local or public real sessions, not
# committed data). SOURCE is a registered source name (see `registered_source_names`):
# every line is routed through THAT source's registry line_decoder — no shape
# guessing, so a newer source can't be silently misrouted to decode_cc_line.
# Exits non-zero on any panic. Examples:
#   just fuzz claude-code ~/.claude/projects   # your CC sessions (newest formats)
#   just fuzz codex ~/.codex/sessions          # your Codex rollouts
#   just fuzz grok ~/.grok/sessions            # grok ACP transcripts
#   just fuzz omp ~/.omp/agent/sessions        # omp sessions
#   # a PUBLIC real-session corpus, so drift shows up without waiting for your own
#   # sessions to hit the shape. Its codex samples are single .json objects,
#   # which this recipe's *.jsonl glob does not admit.
#   git clone --depth 1 https://github.com/daaain/claude-code-log /tmp/ccl && just fuzz claude-code /tmp/ccl/dev-docs/messages
[group('rust')]
[doc('Never-panic fuzz a source decoder over a JSONL corpus dir: just fuzz claude-code ~/.claude/projects')]
fuzz source dir:
    #!/usr/bin/env bash
    set -euo pipefail
    source="{{ source }}"
    dir="{{ dir }}"
    [ -d "$dir" ] || { echo "error: corpus dir '$dir' does not exist" >&2; exit 1; }
    # Guard the corpus BEFORE fuzzing: a dir with no .jsonl feeds the fuzzer
    # zero lines, and it exits 0 — reporting the never-panic contract verified
    # having tested nothing.
    [ -n "$(find "$dir" -name '*.jsonl' -print -quit)" ] || { echo "error: no .jsonl files under '$dir' — nothing to fuzz" >&2; exit 1; }
    cargo build --release --example decoder_fuzz -p pixtuoid-core
    find "$dir" -name '*.jsonl' -print0 | xargs -0 cat | ./target/release/examples/decoder_fuzz "$source"

# Hermetic OpenClaw daemon live-e2e: drives the REAL shim with crafted gateway
# envelopes on an isolated socket and asserts the lobster's
# idle/busy/degraded/down via the headless `daemons=` line. Zero gateway, zero
# model calls. Same on-demand local tier as `fuzz` — it needs a release build
# and an ExitWatch backend (macOS kqueue / Linux pidfd), so it is not a CI gate.
[group('rust')]
[doc('Hermetic OpenClaw daemon live-e2e (needs `just build --release`)')]
openclaw-e2e:
    scripts/lib/tier-openclaw-hermetic.sh

# N REAL `openclaw gateway run` processes, each in its own throwaway
# OPENCLAW_HOME on its own port, feeding one headless pixtuoid: one
# `openclaw@<port>` row per gateway, instance-local death, and OpenClaw's OWN
# `plugins list` confirming our plugin loads. Zero model calls, zero account
# footprint, but it needs a real `openclaw` on PATH — same on-demand local tier
# as `openclaw-e2e`. Ports are forwarded (default: four consecutive ones).
[group('rust')]
[doc('Multi-gateway live-e2e against the REAL openclaw CLI (needs `just build --release`)')]
openclaw-multi-e2e *ports:
    scripts/lib/tier-openclaw-multi.sh {{ ports }}

# The EXPENSIVE one: a real `openclaw gateway run` PLUS one real model turn on
# the claude-cli backend, proving the gateway's lobster and its backend's `cc·`
# desk sprite coexist live. Real account footprint (your gateway's channels
# connect) and it bills a turn — recipe exists so the script has an invocation
# site and cannot silently rot on a summary-format change, NOT because it should
# be run casually.
[group('rust')]
[doc('OpenClaw + claude-cli backend live-e2e — REAL gateway AND one BILLED model turn')]
openclaw-backend-e2e:
    scripts/lib/tier-openclaw-backend.sh

# The broadest tier: launches each installed agent CLI non-interactively and
# asserts ITS badge renders. One real model turn PER CLI, on each provider's own
# account — the only proof a real CLI's real output reaches a real sprite.
[group('rust')]
[doc('Live multi-source e2e — every installed agent CLI, one BILLED turn each')]
live-sources *ids:
    scripts/lib/tier-live-sources.sh {{ ids }}

# Replays a captured rollout through the FULL headless path — real watcher, real
# socket, only the input is fixed.
[group('rust')]
[doc('Replay a captured rollout fixture through a hermetic headless run')]
replay fixture delay="3":
    scripts/lib/tier-replay.sh {{ fixture }} {{ delay }}

# Compile the workspace; extra args are forwarded:
#   just build                                # debug
#   just build --release                      # release
#   just build --release --bins --examples    # what ci-tests.yml's smoke job builds
[group('rust')]
[doc('Compile the workspace; forwards args (e.g. --release --bins --examples)')]
build *args:
    cargo build --workspace {{ args }}

# packaging-build/action.yml keeps its own just-free parse of the same line —
# that composite deliberately never installs just (see ci-builds.yml).
[group('rust')]
[doc("Print the workspace version — release.yml's tag check and release-plz.yml's tag assertion read it")]
workspace-version:
    @grep -m1 '^version' Cargo.toml | cut -d'"' -f2

# Cross-compile a release build for ONE target triple (release.yml's build
# matrix). Pass `true` for targets that need the Docker-backed `cross` toolchain
# (CI installs it via taiki-e/install-action@cross); anything but true/false
# fails loudly (the case below).
[group('rust')]
[doc('Cross-compile a release for ONE target triple (release.yml build matrix)')]
build-target target cross="false":
    #!/usr/bin/env bash
    set -euo pipefail
    use_cross="{{ cross }}"
    # Anything but the two legal words means the caller's positional args
    # shifted, so fail loudly rather than infer "not true, so cargo".
    case "$use_cross" in
    true | false) ;;
    *)
        echo "error: cross must be 'true' or 'false', got '$use_cross' (positional args shifted?)" >&2
        exit 1
        ;;
    esac
    # Every LINUX artifact drops `audio` (musl can't link ALSA statically; the
    # aarch64 cross image has no ALSA headers), so prebuilt Linux binaries ship
    # SILENT and Linux audio is a from-source feature (#633; see
    # docs/CONFIGURATION.md). Every other default feature rides `portable`
    # (pixtuoid's Cargo.toml). Derived here, not passed: the flags are a
    # property of the target. $flags stays UNQUOTED below — quoting the empty
    # non-Linux case would pass cargo an empty positional arg.
    flags=""
    case "{{ target }}" in
    *linux*) flags="--no-default-features --features portable" ;;
    esac
    if [ "$use_cross" = "true" ]; then
        cross build --release --target "{{ target }}" $flags
    else
        cargo build --release --target "{{ target }}" $flags
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
    npx --prefix site playwright install chromium chromium-headless-shell

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
[doc('Site static tier: format-check → lint → astro check → knip → unit tests → build → check:docs → audit (site CI runs e2e + lighthouse before the audit)')]
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
# Regenerate the committed artifacts that derive from a single source of truth,
# and check the committed copies (each `*-check` header says against what).

[group('gen')]
[doc('Regenerate the committed art (generated sprites + icons + README sections + docs images + site demos)')]
gen: gen-art gen-icons gen-media gen-readme

[group('gen')]
[doc("Regenerate the bundled pack's generated sprites (every @Nx variant + the 1x pieces it owns) from scripts/gen-art.py")]
gen-art:
    python3 scripts/gen-art.py crates/pixtuoid-scene/sprites/default

# Stdlib-only, so `lint` runs it without the venv `gen-check` needs.
[group('gen')]
[doc('Fail if a committed generated sprite differs from what scripts/gen-art.py draws')]
gen-art-check:
    python3 scripts/gen-art.py --selftest
    python3 scripts/gen-art.py --check crates/pixtuoid-scene/sprites/default

# Sync the README's install/features/tools sections from site/src/*.json.
[group('gen')]
[doc('Sync README install/features/tools sections from site/src/*.json')]
gen-readme:
    node scripts/gen-readme.mjs

# Regenerate the --json contract chain after changing `SourceStatus` or
# `OutcomeRow`: re-emit their JSON Schemas from the Rust serde types, then
# regenerate the Raycast TS types from them. The freshness gates (the
# `*_schema_matches_the_committed_contract` golden tests in `just test`, and
# the raycast CI's `gen:contract` diff) FAIL until you run this — so the Rust
# producer and the TS consumer can't hand-drift. Needs raycast deps installed
# (`npm --prefix integrations/raycast ci`).
[group('gen')]
[doc('Regenerate the --json contract: the SourceStatus + OutcomeRow JSON Schemas (Rust) + the Raycast TS types')]
gen-contract:
    UPDATE_CONTRACT_SCHEMA=1 cargo test -p pixtuoid --lib schema_matches_the_committed_contract
    npm --prefix integrations/raycast run gen:contract

# Fail if the committed README drifted from site/src/{features,sources,install}.json.
# Pure node:builtins — no npm ci. ci-lint.yml's `readme` job runs this,
# and gen-check composes it.
[group('gen')]
[doc('Fail if the committed README drifted from site data (features/sources/install.json)')]
gen-readme-check:
    node scripts/gen-readme.mjs --check

# Args are forwarded; scripts/gen-media.py's docstring owns the jobs, flags and
# toolchain.
[group('gen')]
[doc('Regenerate docs/images/ + site/public/demos/ from scripts/media.json')]
gen-media *args:
    .venv/bin/python3 scripts/gen-media.py {{ args }}

[group('gen')]
[doc('Regenerate site/src/assets/pix-icons/ from the embedded sprite-pack palette')]
gen-icons:
    .venv/bin/python3 scripts/gen-pix-icons.py

# The ONE wasm compile step — gen-wasm (below) and ci-builds.yml's wasm-check job both
# call this, so the package/target/profile CI checks can't drift from what
# gen-wasm ships. Toolchain gotcha: the PATH cargo/rustc may be Homebrew's, which
# has NO wasm32 std — and even `rustup run stable cargo` fails because cargo
# resolves `rustc` via PATH. So the recipe prepends the RUSTUP toolchain bin (via
# `rustup which`) and invokes that cargo explicitly.
[group('gen')]
[doc('Compile pixtuoid-web for wasm32 (the size-tuned wasm-release profile) — shared by gen-wasm + CI wasm-check')]
wasm-build:
    #!/usr/bin/env sh
    set -eu
    command -v rustup >/dev/null || { echo "needs rustup (Homebrew rust has no wasm std)"; exit 1; }
    rustup target list --toolchain stable --installed | grep -q wasm32-unknown-unknown \
        || { echo "needs the wasm target: rustup target add wasm32-unknown-unknown"; exit 1; }
    TB="$(dirname "$(rustup which --toolchain stable rustc)")"
    PATH="$TB:$PATH" "$TB/cargo" build -p pixtuoid-web --target wasm32-unknown-unknown --profile wasm-release

# The gen-only tool preflight — a SEPARATE recipe so it runs BEFORE the wasm-build
# dependency, failing fast if wasm-bindgen/wasm-opt are missing instead of after a
# minutes-long `wasm-build` compile. wasm-bindgen-cli must match the crate's pinned
# wasm-bindgen (see crates/pixtuoid-web/Cargo.toml); wasm-opt (binaryen) shrinks
# the blob. (ci-builds.yml's wasm-check calls `wasm-build` directly, then checks
# the committed pair — neither step needs these.)
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
        target/wasm32-unknown-unknown/wasm-release/pixtuoid_web.wasm
    wasm-opt -Oz -o site/public/wasm/pixtuoid_web_bg.wasm site/public/wasm/pixtuoid_web_bg.wasm
    # Stamp the wasm/glue PAIR (#424): every emitted file's sha256 lands in one
    # manifest, which gen-wasm-check verifies (its header says why a stamp and
    # not a rebuild comparison).
    # `! -name '.*'` keeps dotfiles out: a Finder-dropped .DS_Store is gitignored,
    # so stamping it would verify locally and fail CI (missing file) — local-green/CI-red.
    (cd site/public/wasm && find . -maxdepth 1 -type f ! -name manifest.sha256 ! -name '.*' | LC_ALL=C sort | xargs shasum -a 256 > manifest.sha256)
    ls -la site/public/wasm/

# Bloat + PAIR gate for the committed wasm artifact. Size: the hero must stay
# a lazy-load behind the poster, so a silent size regression (a dep pulling in
# formatting machinery, an accidental debug build) fails loudly. The cap is on
# the GZIPPED size, because the wire cost is what the poster is hiding.
# Raw is REPORTED, never gated as wire: the runner prices the wasm gzipped, as
# GitHub Pages ships it (`startPagesLikeProxy`), so site/lighthouserc.json sees raw
# growth only as parse/compile cost (total-blocking-time and
# user-timings:pixtuoid-revealed, `error`-level; site.yml fires on site/**, where
# the wasm lives). WIRE cost is gated twice on purpose — here, naming the wasm,
# and there via `interactive`/`largest-contentful-paint`, byte budgets under
# simulated throttling sized to admit a wasm AT this cap. The cap is growth
# budget for the scene the hero runs, not a margin over today's payload, so a
# regression shows as the printed gap shrinking, not as a red. Pair (#424): the
# wasm-bindgen JS glue's ABI must match the exact .wasm it was generated with;
# a one-sided merge resolution or partial regen ships a silent runtime throw,
# so every committed file must match gen-wasm's sha256 manifest AND every file
# must be covered by it. Byte-exact rebuild-match is deliberately NOT checked
# in CI — wasm output drifts across rustc versions, and CI installs latest
# stable, so local `just gen-wasm` + review is the freshness authority. Nothing
# here reads a scene/core/web source, so a merge that skips `just gen-wasm`
# ships a stale hero with every gate green; the compensating control is root
# CLAUDE.md's "Build & test" gen-wasm note, not this recipe.
# No input-hash stamp: pixtuoid-core's `native` source runtime is code the wasm
# never links, so a stamp would demand a wasm regen on changes that cannot alter
# it. That reason does not cover scene or web, where any change can move the
# wasm (panic locations carry line numbers); gating those is an open owner call.
[group('gen')]
[doc('Fail if the committed wasm pair is missing, over the size cap, or hash-mismatched')]
gen-wasm-check:
    #!/usr/bin/env sh
    set -eu
    W=site/public/wasm/pixtuoid_web_bg.wasm
    M=site/public/wasm/manifest.sha256
    # -s, not -f: an EMPTY committed wasm passes -f, and the ratio below divides
    # by its size. Failing here says what is wrong; failing there says "division
    # by 0".
    test -s "$W" || { echo "missing or empty $W — run 'just gen-wasm'"; exit 1; }
    # Not tuned to the last KB: this measures gzip locally while the CDN does its
    # own.
    CAP=524288
    # Compress to a FILE, not through a pipe: POSIX sh has no `pipefail`, so
    # `gzip … | wc -c` reports wc's status and a broken gzip would measure zero
    # bytes and pass the cap unconditionally — the gate would go green exactly
    # when it stopped working.
    GZ=$(mktemp)
    trap 'rm -f "$GZ"' EXIT
    gzip -9 -c "$W" > "$GZ"
    WIRE=$(wc -c < "$GZ" | tr -d ' ')
    RAW=$(wc -c < "$W" | tr -d ' ')
    test "$WIRE" -le "$CAP" || { echo "$W gzips to $WIRE bytes (> $CAP cap) — investigate the bloat"; exit 1; }
    # Report the headroom, don't just pass silently. A ratchet you can only read
    # at the moment it breaks gives no warning that it is about to — and a prose
    # estimate of the size drifts unnoticed precisely because every run is green.
    # Raw rides along with its RATIO, not bare: a bare byte count has nothing to
    # be read against. The ratio does — it is gzipped-over-raw, so RISING means
    # new poorly-compressible code and falling means new sprite text.
    echo "wasm $WIRE / $CAP bytes gzipped ($((WIRE * 100 / CAP))% of cap, $(((CAP - WIRE) / 1024)) KB headroom; $RAW raw, compressing to $((WIRE * 100 / RAW))%)"
    test -f "$M" || { echo "missing $M — run 'just gen-wasm' (the wasm/glue pair manifest)"; exit 1; }
    (cd site/public/wasm && shasum -a 256 --strict -c manifest.sha256 >/dev/null) \
        || { echo "wasm/glue pair MISMATCH vs $M — a partial regen or one-sided merge; run 'just gen-wasm' and commit all of site/public/wasm/"; exit 1; }
    for f in site/public/wasm/*; do
        b=$(basename "$f")
        [ "$b" = manifest.sha256 ] && continue
        awk -v want="./$b" '$2 == want { found = 1 } END { exit !found }' "$M" \
            || { echo "$f is not covered by $M — run 'just gen-wasm'"; exit 1; }
    done
    echo "gen-wasm-check OK: $W ($WIRE bytes gzipped <= $CAP), pair manifest verified"

# Drift gate: fail if anything `just gen` writes is stale, or the committed wasm
# pair is broken (`gen-wasm-check`).
# Pixel-diffs every PNG (threshold 0); video clips + demo.gif are presence-only
# (ffmpeg/gifsicle bytes aren't stable cross-version, but the renders feeding
# them ARE pixel-deterministic). Run by ci-tests.yml's smoke job; runnable locally
# before pushing a visual change. A red check after an INTENTIONAL office change
# means: run `just gen` and commit everything it rewrote in the same change.
# Requires the .venv + ffmpeg + node; it builds the examples it renders with.
[group('gen')]
[doc('Fail if anything `just gen` writes has drifted, or the wasm pair is broken')]
gen-check: compare-selftest wasm-check-selftest gen-readme-check gen-wasm-check gen-art-check
    #!/usr/bin/env sh
    set -eu
    test -x .venv/bin/python3 || { echo "needs the venv: python3 -m venv .venv && .venv/bin/pip install -r requirements-dev.txt"; exit 1; }
    .venv/bin/python3 scripts/gen-media.py --check
    .venv/bin/python3 scripts/gen-pix-icons.py --check

# ── release ───────────────────────────────────────────────────────

# The repo's NODE-side gate (no cargo): the npm package generator AND the bundled
# OpenClaw plugin contract.
#   - npm/generate.test.mjs — the ONLY validation of npm/generate.mjs. release.yml
#     runs it as a hard gate right before `npm publish`, and ci-lint.yml's `npm-gen` job
#     so a generator regression is caught at review time, not at the tag-push.
#   - scripts/openclaw-plugin.test.mjs — drives the RENDERED openclaw_plugin.js the
#     way OpenClaw's loader does. The Rust side can only grep that template as a
#     string, so this is the only place its runtime contract (never block the
#     gateway / never forward content / always stamp the gateway identity) is
#     actually EXECUTED.
# NOT in preflight: a Rust pre-push shouldn't require a Node toolchain. Needs Node ≥ 22.
[group('release')]
[doc('Node gates: the npm package generator + the OpenClaw plugin contract (CI + release; not in preflight)')]
npm-check:
    node --test npm/generate.test.mjs scripts/openclaw-plugin.test.mjs

# ── meta ──────────────────────────────────────────────────────────

# The local gate `.githooks/pre-push` runs: lint + clippy, fast over warm build
# caches. Tests are CI's: in the hook, every worktree's push re-ran the whole
# suite on one shared machine. `full` adds the feature powerset and the tests —
# the Rust recipes CI's lint/clippy/hack/test jobs run; what it still can't see
# is in CONTRIBUTING.md#ci-gates.
[group('meta')]
[doc('Local gate: lint → clippy; `full` = lint → clippy → hack → test')]
preflight mode="":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{ mode }}" in
    "") just lint && just clippy ;;
    full) just lint && just clippy && just hack && just test ;;
    *) echo "usage: just preflight [full]" >&2; exit 2 ;;
    esac

# Install the dev tools every check + recipe relies on (idempotent). Prefers
# cargo-binstall (prebuilt) and falls back to cargo install (compiles).
[group('meta')]
[doc('Install the dev tools the checks + recipes need (idempotent)')]
setup-tools:
    #!/usr/bin/env bash
    set -euo pipefail
    # cargo-public-api rides API_PUBLIC_API — the tool-exact story lives there.
    # cargo-edit: `cargo set-version --workspace` raises a release PR's version
    # by hand for a break cargo-semver-checks cannot see
    # (docs/CONTRIBUTING.md#releasing).
    tools=(cargo-nextest cargo-machete cargo-deny cargo-hack cargo-edit cargo-insta lychee cargo-public-api@{{ API_PUBLIC_API }})
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
    # Non-cargo lint tools `just lint` refuses to start without: shfmt/shellcheck
    # cover the shell sources, actionlint/zizmor the workflows, yq + jq the CI
    # contracts, check-jsonschema the schemas. brew on macOS. gitleaks backs
    # `just fixture-pii`, a REQUIRED gate: without it on PATH the recipe cannot
    # run at all (it does not degrade to a weaker scan, because a weaker scan is
    # what it replaced).
    for t in shfmt actionlint shellcheck zizmor yq jq check-jsonschema gitleaks; do
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
    for t in shfmt actionlint shellcheck zizmor yq jq check-jsonschema gitleaks; do
        command -v "$t" &>/dev/null || missing+=("$t")
    done
    if (( ${#missing[@]} )); then
        echo "error: ${missing[*]} still missing after setup — install via your package manager (e.g. brew install ${missing[*]}); \`just lint\` needs it." >&2
        exit 1
    fi
    # Activate the local pre-push gate (dormant by default in a fresh clone, so CI
    # would otherwise be the only gate). Idempotent. CI runs every gate itself,
    # so a skipped local hook still meets them at merge.
    git config core.hooksPath .githooks

# The size gate's own negative control, because nothing else can be one: the
# justfile is outside SHELL_SOURCES, so shellcheck never reads a recipe body, and
# a size cap that stops measuring reports success for any artifact at all. This
# pins the FAIL-OPEN class specifically — the pipe hazard gen-wasm-check's gzip
# step is written around: driving the real recipe with a gzip that exits 1 must
# red it.
# Not covered, deliberately: the over-cap and empty-artifact arms, which would
# have to mutate the committed wasm to exercise. Their failures are loud; the
# fail-open one is the silent class worth a test.
[group('meta')]
[doc('Self-test the wasm size gate: prove it still reds when its measurement breaks')]
wasm-check-selftest:
    #!/usr/bin/env sh
    set -eu
    stub=$(mktemp -d)
    trap 'rm -rf "$stub"' EXIT
    printf '#!/bin/sh\nexit 1\n' > "$stub/gzip"
    chmod +x "$stub/gzip"
    if PATH="$stub:$PATH" just gen-wasm-check >/dev/null 2>&1; then
        echo "wasm-check-selftest: FAIL — gen-wasm-check passed with a broken gzip;"
        echo "  the size measurement is fail-OPEN. Did the gzip call become a pipe?"
        exit 1
    fi
    just gen-wasm-check >/dev/null
    echo "wasm-check-selftest: OK (reds on a broken measurement, greens on a real one)"

# The pixel comparator is the primitive under `gen-check` and the smoke job; an
# always-green comparator reports success for any render at all. Its own recipe
# because it needs only Pillow, while `gen-check` needs the toolchain its header
# lists: a developer who cannot run that gate should still be able to run this.
[group('meta')]
[doc('Self-test the pixel comparator that gen-check and smoke ride on')]
compare-selftest:
    #!/usr/bin/env sh
    set -eu
    # Prefer the venv (the same Pillow gen-media.py uses), but do not require it:
    # any python3 with Pillow answers the question this recipe asks.
    if [ -x .venv/bin/python3 ]; then py=.venv/bin/python3; else py=python3; fi
    "$py" scripts/compare-screenshots.py --selftest

# Regenerate the committed drift-surface fragments — what each crate declares it
# READS (pixtuoid-core) and REGISTERS (pixtuoid). `check_upstream_drift.py` reads
# these instead of parsing our Rust, so a rename must be re-emitted or the watch
# narrows. The gate is the crates' own tests, which fail on a stale file; this is
# just the writer.
[group('gen')]
[doc('Regenerate crates/*/drift-surface.json after changing a decoded/registered name')]
gen-drift-surface:
    UPDATE_DRIFT_SURFACE=1 cargo test -p pixtuoid-core --lib drift_surface
    UPDATE_DRIFT_SURFACE=1 cargo test -p pixtuoid --lib drift_surface

# Self-test the upstream-drift watcher — its ONLY test. A regex-parser regression
# is a silent monitor death (the script returns empty / raises, the weekly job
# alarms on junk or watches nothing); this pins the parsers + the fetch
# classifier. Pure Python, no deps, no network.
[group('meta')]
[doc('Self-test the upstream-drift watcher (parsers + fetch classifier)')]
drift-selftest:
    python3 scripts/check_upstream_drift_selftest.py

# The pty driver's pure halves — the ANSI stripper, the composer comparison, the
# gate/menu wording. Each fails silently, at the price of a BILLED turn: a broken
# stripper just stops matching, and the capture comes back empty blaming the
# CLI. Runs in `lint`; CI's hygiene job enumerates it separately.
[group('meta')]
[doc("Self-test the TUI capture driver's pure logic")]
tuidrive-selftest:
    python3 scripts/lib/tuidrive.py --selftest

# The git env scrub, both copies: `e2e_init_repo` and the pre-push hook. A git
# hook exports GIT_DIR/GIT_INDEX_FILE into every child and those OUTRANK
# `git -C <dir>`, so an unscrubbed helper COMMITS to the developer's real repo
# while printing nothing. The suite runs the unscrubbed form first and proves
# it leaks, so a scrub that stopped scrubbing cannot pass. Hermetic: one
# mktemp -d, no network, no real repo.
[group('meta')]
[doc("Self-test the git env scrub — the e2e helper and the pre-push hook")]
e2e-scrub-selftest:
    bash scripts/lib/e2e-common-selftest.sh

# The README star chart's renderer — bitmap font, axes, paging. (Its copied
# office colours are pinned from Rust: scene's `tests/readme_chart_palette.rs`.)
# Runs in `lint`; CI's hygiene job enumerates it separately; the star-history
# workflow runs it before each render.
[group('meta')]
[doc("Self-test the README star-chart renderer")]
star-history-selftest:
    python3 scripts/star-history.py --selftest


# The capture-tree rules — every scenario declares what
# fixtures/provenance.schema.json requires, and a recorded one's claims are
# falsified by its own bytes. They are RUST TESTS (`tests/sources/captures.rs`)
# and so already ride `just test` on all three platforms; this recipe is the
# named entry point for a human who wants only this answer.
#
# They stay Rust: a Python rule would need its own copy of the capture walk kept
# in step with `harness::captures`, and GitHub's Windows images ship `python.exe`
# with no `python3` while no test job installs Python, so windows-test would
# red.
[group('meta')]
[doc("Assert every capture declares its provenance and its claims are falsifiable")]
fixture-metadata:
    #!/usr/bin/env bash
    set -euo pipefail
    # A filter that matches NOTHING exits 0 having run no rule, so rename the
    # module and this named gate passes vacuously — the class the file's own
    # floors exist to prevent. Count what ran and require it.
    out=$(cargo test -p pixtuoid-core --test sources captures:: -- --nocapture 2>&1) || { echo "$out"; exit 1; }
    echo "$out"
    ran=$(echo "$out" | sed -n 's/^test result: ok\. \([0-9]*\) passed.*/\1/p' | head -1)
    [ "${ran:-0}" -gt 0 ] || { echo "fixture-metadata: the \`captures::\` filter matched no test — the module moved" >&2; exit 1; }

# The recorder refuses a capture carrying its own identity, but that check runs
# ONCE, on the capturer's terminal. This re-scans what is actually COMMITTED, so
# a fixture added by hand, edited later, or captured before the check existed is
# covered too. gitleaks, not a hand-rolled scanner: `.gitleaks.toml` says why.
# Runs in `lint`; CI's hygiene job enumerates it separately.
[group('meta')]
[doc("Scan the committed fixture tree for secrets and the recorder's identity")]
fixture-pii:
    #!/usr/bin/env bash
    set -euo pipefail
    # TWO passes, and the split is load-bearing: the credential sweep wants
    # gitleaks' default allowlist (which waives filesystem-shaped strings), and
    # the identity rules are destroyed by it — a global allowlist outranks a
    # rule-scoped one. `.gitleaks-identity.toml` carries the proof.
    tree=crates/pixtuoid-core/tests/sources
    gitleaks dir "$tree" -c .gitleaks.toml          --no-banner --redact
    gitleaks dir "$tree" -c .gitleaks-identity.toml --no-banner --redact

# Prove both configs can FAIL, and that neither fires on the paths a fixture or a
# CI-path assertion legitimately carries. The direction that matters is "must
# fire": an identity rule moved into `.gitleaks.toml` is silently waived there by
# the default global allowlist, and `fixture-pii` would still exit 0 — the exact
# half-dead state this pair was split to prevent. Credential probes are assembled
# at RUNTIME so no token-shaped literal is ever committed.
[group('meta')]
[doc('Prove the fixture-pii configs red on a leak and green on a legitimate path')]
fixture-pii-selftest:
    #!/usr/bin/env bash
    set -euo pipefail
    d=$(mktemp -d); trap 'rm -rf "$d"' EXIT
    mkdir -p "$d/probe" "$d/quiet"
    # ONE probe per file, and the assertion is the exact SET of files each config
    # reports — not "did it find something". A config whose rules are half dead
    # still fires on its surviving rule, so an aggregate check greens on the very
    # state this exists to catch.
    printf '/home/alice\n'          > "$d/probe/identity-home.txt"
    printf '/Users/bob/notes.txt\n' > "$d/probe/identity-users.txt"
    # A username that STARTS with a placeholder token, and the Windows separator:
    # the two shapes a looser rule admits.
    printf '/Users/dev-ops\n'  > "$d/probe/identity-prefix.txt"
    printf 'C:\\Users\\bob\n'  > "$d/probe/identity-win.txt"
    printf -- '--Users-carol-Desktop-proj--\n' > "$d/probe/identity-dashed.txt"
    printf 'mcp__internal_tracker\n' > "$d/probe/identity-mcp.txt"
    printf '{"user_email":"a.person@gmail.com"}\n' > "$d/probe/identity-email.txt"
    # The one identity class with no shape of its own — reachable only under the
    # label a CLI renders it beneath.
    printf 'Git user: Ada Lovelace\n' > "$d/probe/identity-gituser.txt"
    printf '{"authorization":"Bearer %s"}\n' "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9x" \
        > "$d/probe/identity-bearer.txt"
    # Assembled, and deliberately NOT `AKIAIOSFODNN7EXAMPLE` — gitleaks' default
    # allowlist waives AWS's own documentation key, so that one proves nothing.
    printf 'aws_key = "AKIA%s"\n' "QYZ3K7RFVD2NMXWB" > "$d/probe/cred-aws.txt"
    # A real secret wearing a wire identifier's PREFIX: an unanchored wire-id
    # allowlist waives it by substring.
    printf '{"api_key":"msg_%s"}\n' "Xq7RvN2bK9wLpT4mZs8cHf1jY6dQ3aGe0uVi5nBr" > "$d/probe/cred-disguised.txt"
    # The RESIDUAL, pinned so it is visible rather than assumed closed: a secret
    # that lands INSIDE the wire id's own 20-32 length window still rides the
    # allowlist. Any shape-based waiver admits a secret wearing that shape; what
    # stops this class is `no_identity_key_holds_a_value_outside_a_pinned_exemption`,
    # which reads the committed bytes — and an exempt capture, being outside it, is
    # the one place a human read is still the only check.
    printf '{"api_key":"msg_%s"}\n' "Kd8sQm2zXv6bTn4wRj9c" > "$d/quiet/cred-residual.txt"
    printf '/Users/dev/x\n/home/runner/work\n/home/ubuntu\n/home/linuxbrew\n/Users/Shared\nmcp__exampleThing\nC:\\Users\\Me\n/Users/dev.\n' \
        > "$d/quiet/identity.txt"
    printf -- '--private-tmp-pixtuoid-capture-proj--\n--Users-dev-proj--\n-home-runner-work\n' \
        > "$d/quiet/identity-dashed.txt"
    printf 'dev@example.com\nbot@users.noreply.github.com\nx@localhost\n' > "$d/quiet/email.txt"
    printf 'Git user: dev\nGit user: runner\nGit user: ubuntu\n' > "$d/quiet/identity-gituser.txt"
    printf 'Bearer short\n' > "$d/quiet/bearer.txt"
    printf 'msg_%s\n%s\n' "0123456789abcdefghij" "20260815_120000_a1b2c3" > "$d/quiet/cred.txt"
    fired() {
        gitleaks dir "$2" -c "$1" --no-banner --redact --report-format json \
            --report-path "$d/out.json" >/dev/null 2>&1 || true
        jq -r '[.[].File | split("/") | last] | unique | join(",")' "$d/out.json"
    }
    fail=0
    # The credential config does NOT own the identity class — its default global
    # allowlist waives filesystem-shaped strings, which is why the pair is split.
    for spec in ".gitleaks.toml=cred-aws.txt,cred-disguised.txt" \
                ".gitleaks-identity.toml=identity-bearer.txt,identity-dashed.txt,identity-email.txt,identity-gituser.txt,identity-home.txt,identity-mcp.txt,identity-prefix.txt,identity-users.txt,identity-win.txt"; do
        cfg=${spec%%=*}; want=${spec#*=}
        got=$(fired "$cfg" "$d/probe")
        if [ "$got" != "$want" ]; then
            echo "fixture-pii-selftest: FAIL — $cfg reported [$got], want [$want]" >&2
            fail=1
        fi
        got=$(fired "$cfg" "$d/quiet")
        if [ -n "$got" ]; then
            echo "fixture-pii-selftest: FAIL — $cfg fired on legitimate paths [$got]" >&2
            fail=1
        fi
    done
    [ "$fail" -eq 0 ] || exit 1
    echo "fixture-pii-selftest: OK (each rule reds on its own probe, green on legitimate paths)"

# Which recorded fixtures have drifted from the CLI that produced them — version
# first (the sharp signal), age second. LOCAL and advisory: CI has none of these
# CLIs to compare against, and a stale fixture is a re-capture candidate, not a
# defect. Exit 3 = candidates found (the `corpus-all` convention).
[group('rust')]
[doc('Report recorded fixtures whose CLI has moved on (advisory, exit 3 = stale)')]
fixture-age *args:
    python3 scripts/fixture-age.py {{ args }}
