#!/usr/bin/env bash
# codeql.yml's Rust extraction-health gate is the only red signal for a
# degraded Rust database: codeql-action fails only when no file extracts at
# all, and stayed green through both recorded degradations (#783, #1017).
set -euo pipefail

fail() {
    echo "codeql health test: $*" >&2
    exit 1
}

gate="$(yq -e -r '
    [.jobs[].steps[] | select(.name == "Verify Rust extraction health") | .run]
    | select(length == 1)
    | .[0]
' .github/workflows/codeql.yml)"

test_dir="$(mktemp -d)"
trap 'rm -rf "$test_dir"' EXIT

# CodeQL's SARIF names a metric only through indexes into its rule table.
metrics_sarif() {
    jq -n --argjson with_errors "$1" --argjson clean "$2" '{runs: [{
        tool: {extensions: [{rules: [
            {id: "rust/summary/number-of-files-extracted-with-errors"},
            {id: "rust/summary/number-of-successfully-extracted-files"}
        ]}]},
        properties: {metricResults: [
            {rule: {index: 0, toolComponent: {index: 0}}, value: $with_errors},
            {rule: {index: 1, toolComponent: {index: 0}}, value: $clean}
        ]}
    }]}'
}

run_gate() {
    local sarif_dir="$test_dir/$1"
    mkdir -p "$sarif_dir"
    cat >"$sarif_dir/rust.sarif"
    GITHUB_STEP_SUMMARY="$test_dir/summary" SARIF_DIR="$sarif_dir" bash -c "$gate" 2>&1
}

metrics_sarif 16 314 | run_gate healthy >/dev/null ||
    fail "rejected a mostly clean database"

if output="$(metrics_sarif 247 63 | run_gate degraded)"; then
    fail "accepted a database with diagnostics in most files"
fi
[[ "$output" == *"::error title=Unhealthy Rust CodeQL database::"* ]] ||
    fail "failed before its threshold on a degraded database: $output"

if printf '{"runs":[]}\n' | run_gate no-metrics >/dev/null; then
    fail "accepted SARIF without the extraction metrics"
fi
