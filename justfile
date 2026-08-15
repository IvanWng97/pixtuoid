# Project task runner — the single source of truth for build / lint / format /
# test. Every call-site goes through these recipes — the .githooks/ hooks,
# .github/workflows/ci*.yml, .github/workflows/release.yml, and the docs — so there is exactly ONE
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

# ── variables ─────────────────────────────────────────────────────
# just evaluates these globally regardless of position; kept at the top (the
# idiom) so the file's config lives in one place.

# The published semver surface: the ONLY two crates whose public API is a
# contract (the binary lib target is not). Single-sourced here so the three
# gates over it — semver / api-surface / api-surface-check — can't drift; a
# newly-published crate is added in ONE place.
PUBLISHED_CRATES := "pixtuoid-core pixtuoid-scene"

# Standalone shell FILES share one authority so formatting and lint coverage
# cannot drift. Shell embedded in YAML is a second population this cannot cover:
# workflow `run:` blocks go to actionlint, composite-action ones to
# `actionlint-composites`. Both are shellcheck-only — shfmt cannot rewrite a
# scalar in place — so adding a file here is not enough for embedded shell.
SHELL_SOURCES := "scripts/*.sh scripts/lib/*.sh .githooks/* policy/ci-observability/*.sh"

# The nightly the api-surface goldens are pinned to (rustdoc JSON is
# nightly-only). Self-installed by `_api-nightly`; CI + setup-tools pin
# cargo-public-api 0.52.0 to match. Bump both together or the golden churns.
API_NIGHTLY := "nightly-2026-07-22"

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
# scripts/, git hooks, and CI policy behavior tests. `-i 4` (4-space) matches
# the prevailing style; no `-ci` so case bodies stay un-indented as written.
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
# action therefore loses its shellcheck coverage silently — which is exactly
# what happened to the homebrew-core contract asserts in packaging-build. Pull
# each `run:` out ourselves and check it with the same linter.
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
# GH_TOKEN, so those DO gate in CI, and a ci-observability rule pins that step
# so the online half cannot be dropped silently. Same call as `links`
# (--offline) and `deny` (advisories deferred to audit.yml): a check whose
# verdict depends on the network and an upstream feed must not redden a push of
# unchanged code. Do NOT auto-export `gh auth token` to close the gap — it puts
# a real token on the wire on every pre-push run and makes the local gate
# depend on gh auth + API rate limits, the exact flakiness those two siblings
# were written to avoid.
[group('rust')]
[doc('Audit GitHub automation security with zizmor')]
zizmor:
    zizmor --strict-collection .

# Cross-file CI contracts that actionlint cannot express. yq owns YAML 1.2
# parsing, jq owns SARIF fixtures, and Conftest/OPA owns policy evaluation.
[group('rust')]
[doc('Check repository CI contracts with Conftest/OPA policy-as-code')]
ci-observability:
    #!/usr/bin/env bash
    set -euo pipefail
    files=()
    while IFS= read -r file; do files+=("$file"); done < <(find .github/workflows .github/actions -type f \( -name '*.yml' -o -name '*.yaml' \) -print | sort)
    ((${#files[@]})) || { echo "error: no GitHub Actions YAML files found" >&2; exit 1; }
    [[ -s .github/actionlint.yaml ]] || { echo "error: .github/actionlint.yaml is missing or empty" >&2; exit 1; }
    [[ -s .github/zizmor.yml ]] || { echo "error: .github/zizmor.yml is missing or empty" >&2; exit 1; }
    [[ -s .github/dependabot.yml ]] || { echo "error: .github/dependabot.yml is missing or empty" >&2; exit 1; }
    [[ -s site/package.json ]] || { echo "error: site/package.json is missing or empty" >&2; exit 1; }
    files+=(.github/actionlint.yaml .github/zizmor.yml .github/dependabot.yml site/package.json)
    combined="$(mktemp)"
    policy_test_results="$(mktemp)"
    trap 'rm -f "$combined" "$policy_test_results"' EXIT
    yq eval-all -o=json '[{"path": filename, "contents": .}] | {"documents": .}' "${files[@]}" >"$combined"
    conftest fmt --check policy/ci-observability
    # conftest embeds OPA but exposes neither `check` nor coverage, so the OPA
    # binary owns both. `--strict` catches compile-level slop conftest accepts
    # (an unused argument shipped here undetected); the coverage threshold is a
    # RATCHET on #789 — an uncovered rule head means "the body was never true",
    # i.e. no test makes that rule fire, which is how two vacuous rules reached
    # main. Raise the number as rules gain tests; never lower it. Every deny head
    # now fires in a test, so the uncovered remainder is helper lines — a COUNT
    # here would rot on the next rule, so don't reintroduce one.
    opa check --strict policy/ci-observability
    opa test --coverage --threshold 97 policy/ci-observability >/dev/null
    if ! conftest verify --policy policy/ci-observability --output json >"$policy_test_results"; then
        yq -P '.' "$policy_test_results" >&2
        exit 1
    fi
    policy_test_count="$(yq -e 'length' "$policy_test_results")"
    ((policy_test_count > 0)) || { echo "error: Conftest discovered no Rego unit tests" >&2; exit 1; }
    echo "$policy_test_count Rego unit tests passed"
    conftest test --parser json --policy policy/ci-observability "$combined"
    bash policy/ci-observability/action_behavior_test.sh
    iconv -f US-ASCII -t US-ASCII codecov.yml >/dev/null
    # Regal is the OPA project's own Rego linter; .regal/config.yaml records
    # every deliberate disagreement with a WHY. LAST on purpose: it judges style,
    # and under `set -e` an earlier position would abort the recipe before the
    # correctness checks above ever evaluated the documents.
    regal lint policy/ci-observability

# Every committed JSON Schema, held to the metaschema. These are contracts a
# consumer reads at runtime — the review schema reaches the Claude CLI, the
# raycast ones pin the `--json` shape — and nothing else parses them: a broken
# one is invisible until the consumer refuses to start, which is exactly how the
# review bots died for 31h.
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
# path — and falls back to a different directory, silently (the #880/#343/#342/#195
# shape reached through the encoding). `--selftest` proves the checker can FAIL.
[group('rust')]
[doc('Gate: PATH-valued env vars must be read as bytes, never via env::var')]
env-paths:
    python3 scripts/check-env-paths.py --selftest
    python3 scripts/check-env-paths.py

# Architecture invariant #1, mechanized: pixtuoid-core + pixtuoid-scene stay terminal/window-free.
# The other five invariants have test/bridge backstops; this one was
# review-enforced only until the KB pilot's gap-closure audit (2026-06-12,
# follow-on to the #261-#271 arc).
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
    # installs nothing), and both crates' feature sets are one flag wide.
    for crate in pixtuoid-core pixtuoid-scene; do
        # Capture first so a cargo-tree ERROR (e.g. a crate rename) kills the
        # recipe via set -e, instead of reading as "no match" inside the if —
        # which would print the green line without having checked anything.
        deps="$(cargo tree -p "$crate" --edges normal --prefix none --target all --all-features)"
        if grep -qE '^(ratatui|crossterm|winit|softbuffer|rodio|cpal)' <<<"$deps"; then
            echo "ARCH VIOLATION: $crate depends on a terminal/window crate (CLAUDE.md invariant #1)"; exit 1
        fi
    done
    echo "arch: pixtuoid-core + pixtuoid-scene are terminal/window-free"

# Fast, independent lint checks in parallel.
[group('rust')]
lint:
    #!/usr/bin/env bash
    set -euo pipefail
    # Fail fast with an actionable message when a lint tool is missing, instead
    # of a bare `command not found` (exit 127) buried in a parallel job's log.
    missing=()
    for t in shfmt shellcheck actionlint zizmor conftest opa regal check-jsonschema yq jq iconv cargo-machete cargo-deny lychee; do
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
    run guides  just gen-guides-check     & pids+=($!)
    run prose   just comment-lint-gate    & pids+=($!)
    run gitenv  just gitenv-selftest      & pids+=($!)
    run tuidrive just tuidrive-selftest   & pids+=($!)
    run fixmeta just fixture-metadata     & pids+=($!)
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

# SemVer-check the published libraries against their crates.io baselines. CI-only
# in practice: needs network to fetch the baseline crates. Scoped to pixtuoid-core
# (the headless lib) + pixtuoid-scene (the published engine crate); the binary
# crates' libs aren't public API.
[group('rust')]
[doc('SemVer-check pixtuoid-core + pixtuoid-scene against their crates.io baselines (CI-only)')]
semver:
    cargo semver-checks $(printf -- '--package %s ' {{PUBLISHED_CRATES}})

# Public-API surface snapshot for the PUBLISHED libraries. COMPLEMENTS
# `just semver`: the semver gate answers "major/minor bump?", this shows *what*
# changed as a reviewable golden diff. Goldens live in `api/<crate>.txt` —
# `cargo public-api -s` output (`-s` omits blanket-impl noise like
# `Into`/`Receiver`; auto-derived `Clone`/`Serialize`/… STAY, since
# adding/removing a derive IS a public-API change). cargo-public-api takes one
# crate per call, so a golden file is regenerated per crate. rustdoc JSON is
# nightly-only, so both recipes PIN {{API_NIGHTLY}}. CI-only in practice (like
# semver) — run `just api-surface` + commit the golden whenever either crate's
# public surface changes.
[group('rust')]
[doc('Regenerate the api/<crate>.txt public-API goldens (cargo-public-api + pinned nightly)')]
api-surface: _api-nightly
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
api-surface-check: _api-nightly
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

# Self-provision the api-surface pinned nightly (rustdoc JSON is nightly-only) if
# it isn't already installed — the same idempotent posture as setup-tools. The
# minimal profile carries rustdoc (bundled with rustc), all cargo-public-api
# needs to build the crate's rustdoc JSON.
[private]
_api-nightly:
    #!/usr/bin/env bash
    set -euo pipefail
    command -v rustup >/dev/null || { echo "rustup not found — install {{API_NIGHTLY}} manually for api-surface" >&2; exit 1; }
    rustup toolchain list | grep -q '{{API_NIGHTLY}}' && exit 0
    echo "installing {{API_NIGHTLY}} (api-surface needs nightly rustdoc JSON)…" >&2
    rustup toolchain install {{API_NIGHTLY}} --profile minimal

# Doc-rendering gate. Two things `cargo build`/`clippy`/`nextest` can't see:
# (1) build the rendered docs with EVERY rustdoc warning as an error — the
# broken/private intra-doc-link classes are already `deny` in
# `[workspace.lints.rustdoc]`, and `-D warnings` also catches bare URLs, invalid
# HTML, redundant links, and any future rustdoc lint, so `cargo doc` output stays
# pristine (dead links render as broken anchors on docs.rs); (2) RUN the doctests
# — `cargo nextest` does NOT execute doctests, so the crate-root examples would
# otherwise go ungated. CI-only in practice (a doc build + a doctest run).
[group('rust')]
[doc('Doc gate: cargo doc with -D warnings + run the doctests nextest skips (CI-only)')]
doc-check:
    #!/usr/bin/env bash
    set -euo pipefail
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
    cargo test --doc --workspace

# Coverage + JUnit XML in one run — the exact command ci-tests.yml's coverage job uses.
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
# silently. CI-only in practice (a second full test run, like coverage/semver) —
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

# Comment-slop advisory: flag NEW runs of 3+ consecutive line comments (Rust
# `//`, Python `#`) inside a function body, AND new `///`/`//!` runs past
# `DOC_RUN_MAX` (the ast-grep regexes exclude doc comments, where most bloat lands) (the repo's "fn-body comments ≤2
# lines" convention — pr-review.prompt.md's comment-value factor). DIFF-SCOPED
# like `mutants` (`scripts/comment-lint.py` over the ast-grep rules in
# `.ast-grep/rules/`), so the ~5k pre-existing legitimate WHY comments are
# grandfathered and only new code is checked. ADVISORY by default (prints + exit 0); `--gate` makes
# the RE-PARENT and PROSE arms exit 1 — the ast-grep and doc-run-length arms only ever report,
# `--worktree` lints uncommitted edits, `--github` emits inline PR annotations.
# Needs ast-grep (setup-tools) + python3. Forwards args (e.g. a different base).
[group('meta')]
[doc('Advisory: flag NEW comment runs — fn-body `//` and over-long `///` (diff-scoped)')]
comment-lint *args:
    python3 scripts/comment-lint.py {{ args }}

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
    rc=0
    uncovered=()
    while IFS=$'\t' read -r id _ kind _; do
        [ "$kind" = transcript ] || continue
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
    done < <("$cc" --roster)
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
#   # sessions to hit the shape. That repo moved test_data/ -> dev-docs/messages/
#   # (verified 2026-08-14: 59 lines, 0 decode-err); its codex samples are single
#   # .json objects, which this recipe's *.jsonl glob does not admit.
#   git clone --depth 1 https://github.com/daaain/claude-code-log /tmp/ccl && just fuzz claude-code /tmp/ccl/dev-docs/messages
[group('rust')]
[doc('Never-panic fuzz a source decoder over a JSONL corpus dir: just fuzz claude-code ~/.claude/projects')]
fuzz source dir:
    #!/usr/bin/env bash
    set -euo pipefail
    source="{{ source }}"
    dir="{{ dir }}"
    # Guard the corpus BEFORE fuzzing: under the default no-pipefail shell a
    # typo'd dir made `find` fail while the pipeline status stayed the
    # fuzzer's — which fuzzes zero lines and exits 0, reporting the
    # never-panic contract verified having tested nothing.
    [ -d "$dir" ] || { echo "error: corpus dir '$dir' does not exist" >&2; exit 1; }
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
# site and cannot silently rot on a summary-format change (it shipped broken
# once for exactly that reason), NOT because it should be run casually.
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
# socket, only the input is fixed. Recipe-less until now, for the reason above.
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

# Cross-compile a release build for ONE target triple (release.yml's build
# matrix). Pass `true` for targets that need the Docker-backed `cross` toolchain
# (CI installs it via taiki-e/install-action@cross). `cross` is validated rather
# than defaulted because callers pass it POSITIONALLY: an unquoted, unset
# matrix.cross expands to nothing, and the collapse slid `flags` into this slot
# on both Linux legs that omit the key — pinned by the arg-shift case below.
[group('rust')]
[doc('Cross-compile a release for ONE target triple (release.yml build matrix)')]
build-target target cross="false" flags="":
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
    # flags: extra cargo flags — release.yml passes --no-default-features for
    # every LINUX artifact (musl can't link ALSA statically; the aarch64 cross
    # image has no ALSA headers), so prebuilt Linux binaries ship SILENT and
    # Linux audio is a from-source feature (#633; see docs/CONFIGURATION.md).
    if [ "$use_cross" = "true" ]; then
        cross build --release --target "{{ target }}" {{ flags }}
    else
        cargo build --release --target "{{ target }}" {{ flags }}
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
# Regenerate the committed artifacts that derive from a single source of truth:
# README sections from site/src/*.json (gen-readme), and the office images for
# BOTH docs/images/ and site/public/demos/ from scripts/media.json (gen-media).

# Regenerate everything: README sections + docs images + site demos.
[group('gen')]
[doc('Regenerate ALL committed artifacts (README sections + docs images + site demos)')]
gen: gen-icons gen-media gen-readme

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
# Pure node:builtins — no npm ci. ci-lint.yml runs this on every PR (the `readme` job),
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

# The ONE wasm compile step — gen-wasm (below) and ci-builds.yml's wasm-check job both
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
# the blob ~10-20%. (ci-builds.yml's wasm-check calls `wasm-build` directly — it only
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
# formatting machinery, an accidental debug build) fails loudly. The cap is on
# the GZIPPED size, because the wire cost is what the poster is hiding — gating
# the raw proxy instead is what blocked the density-variant sprite art (#871).
# Raw is REPORTED, not gated — it is parse/compile cost, which the site's own
# Lighthouse budget measures DIRECTLY on the runner (total-blocking-time and
# user-timings:pixtuoid-revealed are `error`-level in site/lighthouserc.json,
# and site.yml fires on site/** which is where the wasm lives), so a byte-count
# proxy for it would be the weaker instrument. Meanwhile the cap is deliberately
# LOOSE — sized for the density-art phase
# rather than today's payload, so its headroom is art budget and NOT regression
# sensitivity; the recipe prints the gap so you can see how much. RETIRE that
# slack once the art phase lands: re-run the recipe and set the cap to the new
# figure plus a margin. Pair (#424): the
# wasm-bindgen JS glue's ABI must match the exact .wasm it was generated with;
# a one-sided merge resolution or partial regen ships a silent runtime throw,
# so every committed file must match gen-wasm's sha256 manifest AND every file
# must be covered by it. Byte-exact rebuild-match is deliberately NOT checked
# in CI — wasm output drifts across rustc versions, and CI installs latest
# stable, so local `just gen-wasm` + review is the freshness authority. Note
# what that does and does NOT resemble in the committed demo media: the
# clips/gif are presence-only for this same non-determinism reason, but the
# STILLS are re-rendered and pixel-diffed at threshold 0 by gen-check, so media
# staleness IS mechanically gated and wasm staleness is not. Nothing here reads
# a scene/core/web source, so a merge that skips `just gen-wasm` ships a stale
# hero with every gate green; the compensating control is the merge-gate brief
# (.github/prompts/pr-review.prompt.md, "a scene change stales the wasm"), not
# this recipe. Input-hash stamping was considered and rejected: most commits
# under crates/pixtuoid-{core,scene}/src are `native`-gated code the wasm never
# links, so the gate would demand a ~1 MB binary regen on changes that provably
# cannot alter it.
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
    # own, and the cap carries deliberate art-phase slack (see the note above).
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

# Drift gate: fail if any committed README section OR rendered still is stale.
# Pixel-diffs every PNG (threshold 0); video clips + demo.gif are presence-only
# (ffmpeg/gifsicle bytes aren't stable cross-version, but the renders feeding
# them ARE pixel-deterministic). Run by ci-tests.yml's smoke job; runnable locally
# before pushing a visual change. A red check after an INTENTIONAL office change
# means: run `just gen` and commit the regenerated docs/images/ +
# site/public/demos/ in the same change. Requires the .venv + ffmpeg + gifsicle
# + a release build of the snapshot example.
[group('gen')]
[doc('Fail if any committed README section or rendered image has drifted')]
gen-check: compare-selftest wasm-check-selftest gen-readme-check gen-wasm-check
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
# before the tag: pushing the tag is what triggers the irreversible publish
# (crates.io + npm, and a homebrew-core autobump), so that stays a human step.
# Needs cargo-edit (`just setup-tools`).
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

    printf '\n\033[32m✓ v%s committed on %s\033[0m\n\n  next:\n    1. curate the drafted bullets in crates/pixtuoid/src/version.rs (release_notes\n       arm) down to ~6 highlights, then: git commit --amend -a\n    2. regenerate committed artifacts — the office HUD bakes CARGO_PKG_VERSION, so a\n       bump drifts every still: just gen, then commit docs/images + site/public/demos\n       (else CI smoke gen-check reds the PR)\n    3. open a PR, review, merge to main\n    4. AFTER merge, tag to publish — IRREVERSIBLE (crates.io + npm, and the tag\n       tarball auto-bumps homebrew-core; see docs/CONTRIBUTING.md#releasing):\n         git tag v%s && git push origin v%s\n' "$ver" "$branch" "$ver" "$ver"

# The repo's NODE-side gate (no cargo): the npm package generator AND the bundled
# OpenClaw plugin contract.
#   - npm/generate.test.mjs — the ONLY validation of npm/generate.mjs. release.yml
#     runs it as a hard gate right before `npm publish`, and ci-lint.yml on every PR
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
    # cargo-public-api is PINNED (the `just api-surface` goldens are reproducible
    # only against an exact tool + nightly pair — see the api-surface recipe).
    tools=(cargo-nextest cargo-machete cargo-deny cargo-hack cargo-semver-checks cargo-edit cargo-insta lychee cargo-public-api@0.52.0)
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
    # ast-grep backs the `comment-lint` advisory (structural Rust + Python rules in
    # .ast-grep/rules/); shfmt/actionlint/shellcheck/zizmor back workflow
    # linting, while yq + jq + Conftest/OPA evaluate repository-specific policy.
    for t in shfmt actionlint shellcheck zizmor ast-grep yq jq conftest opa regal check-jsonschema; do
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
    for t in shfmt actionlint shellcheck zizmor yq jq conftest opa regal check-jsonschema iconv; do
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

# The size gate's own negative control, because nothing else can be one: the
# justfile is outside SHELL_SOURCES, so shellcheck never reads a recipe body, and
# a size cap that stops measuring reports success for any artifact at all. This
# pins the FAIL-OPEN class specifically — the first draft of the gzip gate wrote
# `gzip … | wc -c`, which under POSIX sh (no pipefail) reports wc's status, so a
# broken gzip measured zero and PASSED. Driving the real recipe with a gzip that
# exits 1 is what that form cannot survive.
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

# The pixel comparator is the primitive under `gen-check` and the smoke job, and
# it had no test of its own — an always-green comparator reports success for any
# render at all. Its own recipe, matching the other two selftests, because it
# needs only Pillow while `gen-check` needs the venv plus ffmpeg, node, a release
# snapshot build and the wasm pair: a developer who cannot run that gate should
# still be able to run this.
[group('meta')]
[doc('Self-test the pixel comparator that gen-check and smoke ride on')]
compare-selftest:
    #!/usr/bin/env sh
    set -eu
    # Prefer the venv (the same Pillow gen-media.py uses), but do not require it:
    # any python3 with Pillow answers the question this recipe asks.
    if [ -x .venv/bin/python3 ]; then py=.venv/bin/python3; else py=python3; fi
    "$py" scripts/compare-screenshots.py --selftest

# The guides' index blocks are generated projections of their sibling files —
# the WHY lives in scripts/gen-guides.py's docstring. Edit the SIBLING, run
# `gen-guides`; `-check` gates drift in `lint` + CI's hygiene job.
[group('gen')]
[doc('Regenerate guide index blocks from SHARP-EDGES/LAYOUT/WHERE-TO-LOOK siblings')]
gen-guides:
    python3 scripts/gen-guides.py

[group('gen')]
[doc('Fail if a guide index block drifted from its sibling (runs in lint)')]
gen-guides-check:
    # Negative controls FIRST (the ast-grep-test idiom): a generator whose own
    # fires/does-not-fire contract broke reports a clean pass on garbage.
    python3 scripts/gen-guides.py --selftest
    python3 scripts/gen-guides.py --check

# Self-test the upstream-drift watcher — its ONLY test. A regex-parser regression
# is a silent monitor death (the script returns empty / raises, the weekly job
# alarms on junk or watches nothing); this pins the parsers + the fetch
# classifier. Pure Python, no deps, no network.
[group('meta')]
[doc('Self-test the upstream-drift watcher (parsers + fetch classifier)')]
drift-selftest:
    python3 scripts/check_upstream_drift_selftest.py

# Both-directions pins for the ast-grep rules themselves: `valid:` cases must
# stay silent, `invalid:` must fire. Snapshots skipped — the cases assert
# fires/does-not-fire, which is the contract; snapshots would only add churn.
# Invoked by the `comment-lint` CI job (the one that pins ast-grep), so a broken
# rule contract cannot rot unseen.
[group('meta')]
[doc('Test the ast-grep comment-slop rules (both directions)')]
ast-grep-test:
    ast-grep test --skip-snapshot-tests

# The DRIVER's own half: which files it diffs, and which it scans. The rules
# have `ast-grep-test`; this pins the pathspec + the hidden-dir flag, on a
# throwaway repo. Invoked by the `comment-lint` CI job alongside the rule tests.
[group('meta')]
[doc('Self-test the comment-lint driver (pathspec + hidden-dir scan)')]
comment-lint-selftest:
    python3 scripts/comment-lint.py --selftest

# The comment checks as a GATE. Wired in two places that must stay in step:
# `just lint` (so preflight and pre-push block) and the `comment gate` job in
# ci-lint.yml. The ast-grep arm stays advisory in ci-supplemental — its npm
# install must never gate.
[group('meta')]
[doc('Gate the comment checks: selftest, then --gate against a FRESH origin/main')]
comment-lint-gate:
    #!/usr/bin/env bash
    set -euo pipefail
    # A stale `origin/main` moves the merge-base back: the re-parent arm then
    # blocks on other people's merged commits and the prose RATIO is diluted by
    # them. CI fetches (ci-lint.yml); refusing beats measuring against a stale ref.
    # Bounded by GIT's own stall detector, not `timeout(1)` — that is coreutils,
    # absent on a stock macOS box. `lint` joins its jobs with `wait`, so an
    # unbounded fetch on a captive-portal network hangs preflight and pre-push.
    git -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=20 fetch --quiet origin main \
      || { echo "comment-lint-gate: cannot reach origin — the gate needs a fresh main" >&2; exit 1; }
    python3 scripts/comment-lint.py --selftest
    python3 scripts/comment-lint.py origin/main --gate

# Prices any proposal to make an arm BLOCK: today's rules replayed against
# already-merged commits, so each finding is a block the proposal would have
# imposed on work that shipped. On-demand like `fuzz` — N checkouts, origin/main.
[group('meta')]
[doc('Replay the comment-lint arms against the last N merged commits (default 20)')]
comment-lint-replay n="20":
    #!/usr/bin/env bash
    set -euo pipefail
    # Without it the fn-body count is 0 for every commit and the run still exits
    # 0 — the fail-open `fuzz` guards for the same reason.
    command -v ast-grep >/dev/null \
      || { echo "comment-lint-replay: needs ast-grep — run \`just setup-tools\`" >&2; exit 1; }
    root="$(git rev-parse --show-toplevel)"
    tmp="$(mktemp -d)"
    trap 'git worktree remove --force "$tmp/wt" >/dev/null 2>&1 || true; rm -rf "$tmp"' EXIT
    git worktree add -q --detach "$tmp/wt" HEAD
    git -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=20 fetch --quiet origin main \
      || { echo "comment-lint-replay: cannot reach origin" >&2; exit 1; }
    total=0 advisory=0 gated=0 flagged=0
    for sha in $(git log --first-parent --format=%H -n {{ n }} origin/main); do
      git -C "$tmp/wt" checkout -q --force --detach "$sha"
      # Only the RULES are injected — YAML, outside the scanned pathspec.
      # Injecting the scripts counts their own comments as the commit's.
      rm -rf "$tmp/wt/.ast-grep"
      cp -R "$root/.ast-grep" "$root/sgconfig.yml" "$tmp/wt/"
      rc=0
      out="$(cd "$tmp/wt" && python3 "$root/scripts/comment-lint.py" "$sha^" --gate 2>&1)" || rc=$?
      # Every path through the script prints a `comment-lint:` line (pinned by its
      # selftest); a crash prints none, and would count as zero findings. Matched
      # off a HERESTRING: `grep -q` quits early, and through a pipe that SIGPIPEs
      # the writer, which `pipefail` then reads as no-match on a long verdict.
      if grep -qE '^comment-lint: [0-9]+ new comment-slop' <<<"$out"; then
        n_hit="$(printf '%s\n' "$out" | sed -n 's/^comment-lint: \([0-9]*\) new comment-slop.*/\1/p')"
      elif grep -q '^comment-lint: ' <<<"$out"; then
        n_hit=0
      else
        echo "comment-lint-replay: no verdict from ${sha:0:8} — the replay is broken, not the commit" >&2
        printf '%s\n' "$out" >&2
        exit 1
      fi
      total=$((total + 1)) flagged=$((flagged + n_hit))
      if [ "$rc" -ne 0 ]; then
        gated=$((gated + 1))
        echo "GATE REDS ${sha:0:8}  $(git log -1 --format=%s "$sha")"
      fi
      if [ "$n_hit" -gt 0 ]; then
        advisory=$((advisory + 1))
        printf '  %s  %-4s %s\n' "${sha:0:8}" "$n_hit" "$(git log -1 --format=%s "$sha" | cut -c1-52)"
      fi
    done
    # A bad N leaves the `for` word list empty without tripping `set -e`, and the
    # summary then reports its most reassuring shape.
    [ "$total" -gt 0 ] || { echo "comment-lint-replay: no commits walked — check N" >&2; exit 1; }
    echo "fn-body arm (advisory): $advisory/$total merged commits, $flagged lines"
    echo "blocking arms (--gate): $gated/$total merged commits — an arm reds the"
    echo "  commits that predate its own calibration, so read this against \`git log -S\`"

# The seam's WHY lives in scripts/gitenv.py's docstring. This pins the scrub AND
# sweeps scripts/ for anything spawning git outside `gitenv.git()` — the recurrence
# gate, because one of these leaks ate a developer's index before anyone noticed.
# Runs in `lint`; CI's hygiene job enumerates it separately.
[group('meta')]
[doc("Self-test the scripts' git-env scrub + sweep for bypasses")]
gitenv-selftest:
    python3 scripts/gitenv.py --selftest

# The pty driver's pure halves — the ANSI stripper, the composer comparison, the
# gate/menu wording. Each of those was a lost BILLED turn before it was code, and
# each fails silently: a broken stripper just stops matching, and the capture
# comes back empty blaming the CLI. Runs in `lint`; CI's hygiene job enumerates
# it separately.
[group('meta')]
[doc("Self-test the TUI capture driver's pure logic")]
tuidrive-selftest:
    python3 scripts/lib/tuidrive.py --selftest


# The half of the fixture-age report that runs ANYWHERE: the fields it reads
# (`cli`/`version`/`captured` on every recorded scenario) must be present and
# parseable, so the advisory below cannot rot into a report about nothing.
# Runs in `lint`; CI's hygiene job enumerates it separately.
[group('meta')]
[doc("Assert every recorded fixture declares the metadata the age report reads")]
fixture-metadata:
    python3 scripts/fixture-age.py --check-metadata

# Which recorded fixtures have drifted from the CLI that produced them — version
# first (the sharp signal), age second. LOCAL and advisory: CI has none of these
# CLIs to compare against, and a stale fixture is a re-capture candidate, not a
# defect. Exit 3 = candidates found (the `corpus-all` convention).
[group('rust')]
[doc('Report recorded fixtures whose CLI has moved on (advisory, exit 3 = stale)')]
fixture-age *args:
    python3 scripts/fixture-age.py {{ args }}

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
