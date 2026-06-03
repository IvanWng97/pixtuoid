# Project task runner — single source of truth for local + CI checks.
# `just` lists recipes; `just preflight` is the full pre-push gate.
# .github/workflows/ci.yml and the .githooks/ hooks call these recipes, so
# there is exactly ONE place that defines what each check actually runs.

features := "pixtuoid-core/test-renderer"

# List available recipes.
default:
    @just --list

# Format check only — fast, gates pre-commit.
fmt-check:
    cargo fmt --all --check

# Apply formatting in place.
fmt:
    cargo fmt --all

# Clippy across the workspace, warnings denied.
clippy:
    cargo clippy --workspace --all-targets --features {{ features }} -- -D warnings

# Unused-dependency check.
machete:
    cargo machete

# License + advisory audit.
deny:
    cargo deny check

# Fast, independent lint checks in parallel (fmt + machete + deny).
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
hack:
    cargo hack --feature-powerset --no-dev-deps check --workspace

# SemVer-check the published library against its crates.io baseline. CI-only in
# practice: needs network to fetch the baseline crate. Scoped to pixtuoid-core
# (the headless lib others depend on); the binary crates' libs aren't public API.
semver:
    cargo semver-checks --package pixtuoid-core

# Install the dev tools every check relies on (idempotent). Prefers
# cargo-binstall (prebuilt) and falls back to cargo install (compiles).
setup-tools:
    #!/usr/bin/env bash
    set -euo pipefail
    tools=(cargo-nextest cargo-machete cargo-deny cargo-hack cargo-semver-checks)
    if command -v cargo-binstall &>/dev/null; then
        cargo binstall -y "${tools[@]}"
    else
        echo "cargo-binstall not found — compiling from source (slow)." >&2
        echo "brew install cargo-binstall (or cargo install cargo-binstall) to grab prebuilt binaries instead." >&2
        cargo install "${tools[@]}"
    fi

# Full pre-push gate: the checks worth running locally before a push.
# (semver, coverage, and smoke are CI-only — network baseline / heavy builds.)
preflight: lint clippy hack test

# Regenerate every docs/images screenshot + demo.gif from a release build.
# Single source of truth for the office images — the render params, crop
# quadrants, and the themes-composite diagonal angle all live in the script, so
# the screenshots never "drift". Run after any change to the office's look.
# Requires the .venv (Pillow): see README "Visual verification".
demo:
    .venv/bin/python3 scripts/gen-docs-images.py
