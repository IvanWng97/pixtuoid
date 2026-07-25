#!/usr/bin/env bash
set -euo pipefail

CODECOV_ACTION_FILE="${CODECOV_ACTION_FILE:-.github/actions/upload-codecov/action.yml}"
CODEQL_WORKFLOW_FILE="${CODEQL_WORKFLOW_FILE:-.github/workflows/codeql.yml}"
GEMINI_WORKFLOW_FILE="${GEMINI_WORKFLOW_FILE:-.github/workflows/gemini-review.yml}"
CLAUDE_REVIEW_WORKFLOW_FILE="${CLAUDE_REVIEW_WORKFLOW_FILE:-.github/workflows/claude-readonly-review.yml}"

fail() {
    echo "ci-observability behavior test: $*" >&2
    exit 1
}

step_script() {
    local yaml_file="$1"
    local step_name="$2"
    STEP_NAME="$step_name" yq -e -r '
        [.runs.steps[] | select(.name == strenv(STEP_NAME)) | .run]
        | select(length == 1)
        | .[0]
    ' "$yaml_file"
}

workflow_step_script() {
    local yaml_file="$1"
    local step_name="$2"
    STEP_NAME="$step_name" yq -e -r '
        [.jobs[].steps[] | select(.name == strenv(STEP_NAME)) | .run]
        | select(length == 1)
        | .[0]
    ' "$yaml_file"
}

test_dir="$(mktemp -d)"
trap 'rm -rf "$test_dir"' EXIT

presence_script="$(step_script "$CODECOV_ACTION_FILE" "Require a generated report")"
missing_report="$test_dir/missing"
if REPORT_FILE="$missing_report" bash -c "$presence_script" >/dev/null 2>&1; then
    fail "missing report passed its hard gate"
fi

empty_report="$test_dir/empty"
: >"$empty_report"
if REPORT_FILE="$empty_report" bash -c "$presence_script" >/dev/null 2>&1; then
    fail "empty report passed its hard gate"
fi

generated_report="$test_dir/generated"
printf 'report\n' >"$generated_report"
REPORT_FILE="$generated_report" bash -c "$presence_script" >/dev/null ||
    fail "generated non-empty report failed its hard gate"

warning_script="$(step_script "$CODECOV_ACTION_FILE" "Surface advisory upload failure")"
summary_file="$test_dir/summary"
warning_output="$(
    REPORT_FILE="lcov.info" \
        REPORT_FLAG="unit" \
        REPORT_TYPE="coverage" \
        GITHUB_STEP_SUMMARY="$summary_file" \
        bash -c "$warning_script"
)" || fail "advisory failure handler exited non-zero"

[[ "$warning_output" == *"::warning title=Codecov upload failed::"* ]] ||
    fail "advisory failure emitted no workflow warning"
[[ -s "$summary_file" ]] || fail "advisory failure wrote no job summary"
summary_content="$(<"$summary_file")"
[[ "$summary_content" == *"### Codecov upload warning"* ]] ||
    fail "job summary omitted its warning heading"
[[ "$summary_content" == *"Codecov coverage upload failed for lcov.info (flag: unit)"* ]] ||
    fail "job summary omitted the report identity"

semantic_script="$(workflow_step_script "$CODEQL_WORKFLOW_FILE" "Prepare Rust semantic analysis")"
fake_bin="$test_dir/bin"
fake_sysroot="$test_dir/sysroot"
mkdir -p "$fake_bin" "$fake_sysroot/lib/rustlib/src/rust/library/std/src" "$fake_sysroot/libexec"
printf 'pub mod std;\n' >"$fake_sysroot/lib/rustlib/src/rust/library/std/src/lib.rs"
printf '#!/usr/bin/env bash\nexit 0\n' >"$fake_sysroot/libexec/rust-analyzer-proc-macro-srv"
chmod +x "$fake_sysroot/libexec/rust-analyzer-proc-macro-srv"
metadata_file="$test_dir/metadata.json"
printf '%s\n' \
    '{"packages":[{"name":"pixtuoid","rust_version":"1.89"},{"name":"pixtuoid-core","rust_version":"1.89"}]}' \
    >"$metadata_file"
# shellcheck disable=SC2016 # The generated stub expands these variables when it runs.
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'printf "%s\n" "$*" >> "$RUSTUP_LOG"' \
    'if [[ "$*" == "run 1.89 rustc --print sysroot" ]]; then printf "%s\n" "$FAKE_SYSROOT"; fi' \
    'exit 0' \
    >"$fake_bin/rustup"
chmod +x "$fake_bin/rustup"
# shellcheck disable=SC2016 # The generated stub reads the fixture path when it runs.
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    '[[ "$*" == "metadata --no-deps --format-version 1" ]]' \
    'command cat "$CARGO_METADATA_FILE"' \
    >"$fake_bin/cargo"
chmod +x "$fake_bin/cargo"

rustup_log="$test_dir/rustup.log"
github_env="$test_dir/github_env"
PATH="$fake_bin:$PATH" \
    RUSTUP_LOG="$rustup_log" \
    FAKE_SYSROOT="$fake_sysroot" \
    CARGO_METADATA_FILE="$metadata_file" \
    GITHUB_ENV="$github_env" \
    bash -c "$semantic_script" ||
    fail "Rust semantic-input setup failed with a complete standard-library source"

rustup_calls="$(<"$rustup_log")"
[[ "$rustup_calls" == *"toolchain install 1.89 --profile minimal --component rust-src,rust-analyzer"* ]] ||
    fail "Rust semantic-input setup did not install rust-src and rust-analyzer for the declared MSRV"
github_env_content="$(<"$github_env")"
expected_github_env="$(
    printf '%s\n' \
        "CODEQL_EXTRACTOR_RUST_OPTION_SYSROOT=$fake_sysroot" \
        "CODEQL_EXTRACTOR_RUST_OPTION_SYSROOT_SRC=$fake_sysroot/lib/rustlib/src/rust/library" \
        "CODEQL_EXTRACTOR_RUST_OPTION_PROC_MACRO_SERVER=$fake_sysroot/libexec/rust-analyzer-proc-macro-srv" \
        "CODEQL_EXTRACTOR_RUST_OPTION_CARGO_ALL_TARGETS=true"
)"
[[ "$github_env_content" == "$expected_github_env" ]] ||
    fail "Rust semantic-input setup did not pass its verified sysroot, source, proc-macro server, and Cargo targets to CodeQL"

if PATH="$fake_bin:$PATH" \
    RUSTUP_LOG="$rustup_log" \
    FAKE_SYSROOT="$test_dir/missing-sysroot" \
    CARGO_METADATA_FILE="$metadata_file" \
    GITHUB_ENV="$github_env" \
    bash -c "$semantic_script" >/dev/null 2>&1; then
    fail "Rust semantic-input setup accepted a missing standard-library source"
fi

chmod -x "$fake_sysroot/libexec/rust-analyzer-proc-macro-srv"
if PATH="$fake_bin:$PATH" \
    RUSTUP_LOG="$rustup_log" \
    FAKE_SYSROOT="$fake_sysroot" \
    CARGO_METADATA_FILE="$metadata_file" \
    GITHUB_ENV="$github_env" \
    bash -c "$semantic_script" >/dev/null 2>&1; then
    fail "Rust semantic-input setup accepted a missing proc-macro server"
fi

mixed_metadata_file="$test_dir/mixed-metadata.json"
printf '%s\n' \
    '{"packages":[{"name":"pixtuoid","rust_version":"1.89"},{"name":"pixtuoid-core","rust_version":"1.90"}]}' \
    >"$mixed_metadata_file"
if PATH="$fake_bin:$PATH" \
    RUSTUP_LOG="$rustup_log" \
    FAKE_SYSROOT="$fake_sysroot" \
    CARGO_METADATA_FILE="$mixed_metadata_file" \
    GITHUB_ENV="$github_env" \
    bash -c "$semantic_script" >/dev/null 2>&1; then
    fail "Rust semantic-input setup accepted inconsistent workspace MSRVs"
fi

health_script="$(workflow_step_script "$CODEQL_WORKFLOW_FILE" "Verify Rust extraction health")"
healthy_sarif_dir="$test_dir/healthy-sarif"
unhealthy_sarif_dir="$test_dir/unhealthy-sarif"
missing_metric_sarif_dir="$test_dir/missing-metric-sarif"
mkdir -p "$healthy_sarif_dir" "$unhealthy_sarif_dir" "$missing_metric_sarif_dir"

write_sarif_metrics() {
    local output_file="$1"
    local with_diagnostics="$2"
    local without_diagnostics="$3"
    jq -n \
        --argjson with_diagnostics "$with_diagnostics" \
        --argjson without_diagnostics "$without_diagnostics" \
        '{
            runs: [{
                tool: {
                    extensions: [{
                        rules: [
                            {id: "rust/summary/number-of-files-extracted-with-errors"},
                            {id: "rust/summary/number-of-successfully-extracted-files"}
                        ]
                    }]
                },
                properties: {
                    metricResults: [
                        {
                            rule: {index: 0, toolComponent: {index: 0}},
                            value: $with_diagnostics
                        },
                        {
                            rule: {index: 1, toolComponent: {index: 0}},
                            value: $without_diagnostics
                        }
                    ]
                }
            }]
        }' >"$output_file"
}

write_sarif_metrics "$healthy_sarif_dir/rust.sarif" 3 97
healthy_summary="$test_dir/healthy-summary"
GITHUB_STEP_SUMMARY="$healthy_summary" \
    SARIF_DIR="$healthy_sarif_dir" \
    bash -c "$health_script" >/dev/null ||
    fail "Rust extraction-health gate rejected a predominantly clean database"
[[ -s "$healthy_summary" ]] ||
    fail "Rust extraction-health gate wrote no job summary"
healthy_summary_content="$(<"$healthy_summary")"
[[ "$healthy_summary_content" == *"Rust CodeQL extraction health"* ]] ||
    fail "Rust extraction-health summary omitted its heading"
[[ "$healthy_summary_content" == *"| Clean files | 97 |"* ]] ||
    fail "Rust extraction-health summary omitted the clean-file count"
[[ "$healthy_summary_content" == *"| Files with diagnostics | 3 |"* ]] ||
    fail "Rust extraction-health summary omitted the diagnostics count"

write_sarif_metrics "$unhealthy_sarif_dir/rust.sarif" 223 57
if SARIF_DIR="$unhealthy_sarif_dir" bash -c "$health_script" >/dev/null 2>&1; then
    fail "Rust extraction-health gate accepted a database with diagnostics in most files"
fi

printf '{"runs":[]}\n' >"$missing_metric_sarif_dir/rust.sarif"
if SARIF_DIR="$missing_metric_sarif_dir" bash -c "$health_script" >/dev/null 2>&1; then
    fail "Rust extraction-health gate accepted missing CodeQL metrics"
fi

gemini_review_script="$(workflow_step_script "$GEMINI_WORKFLOW_FILE" "Require a non-empty Gemini review")"
if REVIEW="" bash -c "$gemini_review_script" >/dev/null 2>&1; then
    fail "Gemini review gate accepted an empty review"
fi

if REVIEW=$' \t\n' bash -c "$gemini_review_script" >/dev/null 2>&1; then
    fail "Gemini review gate accepted a whitespace-only review"
fi

REVIEW="Findings: 0" bash -c "$gemini_review_script" >/dev/null ||
    fail "Gemini review gate rejected a non-empty review"

publisher_script="$(workflow_step_script "$CLAUDE_REVIEW_WORKFLOW_FILE" "Publish validated Claude review")"
published_comment="$test_dir/published-comment"
export RUNNER_TEMP="$test_dir"
# shellcheck disable=SC2016 # The generated gh stub expands these variables when it runs.
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'case "$1" in' \
    'api)' \
    '    printf "%s\n" "$FAKE_PR_HEAD"' \
    '    ;;' \
    'pr)' \
    '    [[ "$2" == "comment" ]]' \
    '    while (($#)); do' \
    '        if [[ "$1" == "--body-file" ]]; then' \
    '            command cp "$2" "$PUBLISHED_COMMENT"' \
    '            exit 0' \
    '        fi' \
    '        shift' \
    '    done' \
    '    exit 1' \
    '    ;;' \
    '*) exit 1 ;;' \
    'esac' \
    >"$fake_bin/gh"
chmod +x "$fake_bin/gh"

valid_review='{"summary":"No correctness findings.","findings":[]}'
PATH="$fake_bin:$PATH" \
    FAKE_PR_HEAD="abc123" \
    PUBLISHED_COMMENT="$published_comment" \
    EXPECTED_HEAD_SHA="abc123" \
    PR_NUMBER="42" \
    REPOSITORY="owner/repo" \
    REVIEW_JSON="$valid_review" \
    REVIEW_MARKER="claude-auto-review" \
    REVIEW_TITLE="Claude Review" \
    bash -c "$publisher_script" ||
    fail "Claude publisher rejected a valid zero-finding review"
[[ -s "$published_comment" ]] ||
    fail "Claude publisher posted no review body"
published_content="$(<"$published_comment")"
[[ "$published_content" == *"<!-- claude-auto-review:abc123 -->"* ]] ||
    fail "Claude publisher omitted the exact-head marker"
[[ "$published_content" == *"**Findings: 0**"* ]] ||
    fail "Claude publisher omitted the zero-finding count"

multi_review='{"summary":"Two verified findings.","findings":[{"severity":"HIGH","path":"src/lib.rs","line":7,"body":"Primary invariant violation."},{"severity":"MEDIUM","path":"tests/review.rs","line":11,"body":"Missing refusal-path coverage."}]}'
PATH="$fake_bin:$PATH" \
    FAKE_PR_HEAD="abc123" \
    PUBLISHED_COMMENT="$published_comment" \
    EXPECTED_HEAD_SHA="abc123" \
    PR_NUMBER="42" \
    REPOSITORY="owner/repo" \
    REVIEW_JSON="$multi_review" \
    REVIEW_MARKER="claude-auto-review" \
    REVIEW_TITLE="Claude Review" \
    bash -c "$publisher_script" ||
    fail "Claude publisher rejected a valid multi-finding review"
published_content="$(<"$published_comment")"
[[ "$published_content" == *"**Findings: 2** (1 high, 1 medium)"* ]] ||
    fail "Claude publisher miscounted finding severities"
expected_location="\`src/lib.rs:7\`"
[[ "$published_content" == *"$expected_location"* ]] ||
    fail "Claude publisher omitted a finding location"

if PATH="$fake_bin:$PATH" \
    FAKE_PR_HEAD="new-head" \
    PUBLISHED_COMMENT="$published_comment" \
    EXPECTED_HEAD_SHA="old-head" \
    PR_NUMBER="42" \
    REPOSITORY="owner/repo" \
    REVIEW_JSON="$valid_review" \
    REVIEW_MARKER="claude-auto-review" \
    REVIEW_TITLE="Claude Review" \
    bash -c "$publisher_script" >/dev/null 2>&1; then
    fail "Claude publisher accepted a stale review"
fi

oversized_findings="$(
    jq -n '{
        summary: "too many",
        findings: [
            range(0; 6) | {
                severity: "MEDIUM",
                path: "src/lib.rs",
                line: 1,
                body: "finding"
            }
        ]
    }'
)"
if PATH="$fake_bin:$PATH" \
    FAKE_PR_HEAD="abc123" \
    PUBLISHED_COMMENT="$published_comment" \
    EXPECTED_HEAD_SHA="abc123" \
    PR_NUMBER="42" \
    REPOSITORY="owner/repo" \
    REVIEW_JSON="$oversized_findings" \
    REVIEW_MARKER="claude-auto-review" \
    REVIEW_TITLE="Claude Review" \
    bash -c "$publisher_script" >/dev/null 2>&1; then
    fail "Claude publisher accepted more than five findings"
fi

unsafe_path_review='{"summary":"finding","findings":[{"severity":"HIGH","path":"../outside","line":1,"body":"bad"}]}'
if PATH="$fake_bin:$PATH" \
    FAKE_PR_HEAD="abc123" \
    PUBLISHED_COMMENT="$published_comment" \
    EXPECTED_HEAD_SHA="abc123" \
    PR_NUMBER="42" \
    REPOSITORY="owner/repo" \
    REVIEW_JSON="$unsafe_path_review" \
    REVIEW_MARKER="claude-auto-review" \
    REVIEW_TITLE="Claude Review" \
    bash -c "$publisher_script" >/dev/null 2>&1; then
    fail "Claude publisher accepted an unsafe finding path"
fi
