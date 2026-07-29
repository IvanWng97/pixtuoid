#!/usr/bin/env bash
set -euo pipefail

CODECOV_ACTION_FILE="${CODECOV_ACTION_FILE:-.github/actions/upload-codecov/action.yml}"
CODEQL_WORKFLOW_FILE="${CODEQL_WORKFLOW_FILE:-.github/workflows/codeql.yml}"
CLAUDE_REVIEW_WORKFLOW_FILE="${CLAUDE_REVIEW_WORKFLOW_FILE:-.github/workflows/claude-readonly-review.yml}"
CI_LINT_WORKFLOW_FILE="${CI_LINT_WORKFLOW_FILE:-.github/workflows/ci-lint.yml}"

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
export RUNNER_TEMP="$test_dir"

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

# shellcheck disable=SC2016 # The generated gh stub reads the fixture when it runs.
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    '[[ "$1" == api ]]' \
    'printf "%s\n" "$FAKE_PR_JSON"' \
    >"$fake_bin/gh"
chmod +x "$fake_bin/gh"

assert_reviewability() {
    local script="$1"
    local fixture="$2"
    local expected="$3"
    local label="$4"
    local output_file="$test_dir/pr-resolution-output"
    : >"$output_file"

    PATH="$fake_bin:$PATH" \
        DEFAULT_BRANCH="main" \
        FAKE_PR_JSON="$fixture" \
        GH_TOKEN="test-token" \
        GITHUB_OUTPUT="$output_file" \
        PR_NUMBER="42" \
        REPOSITORY="owner/repo" \
        bash -c "$script" ||
        fail "$label resolver exited non-zero"

    local output
    output="$(<"$output_file")"
    if [[ "$expected" == true ]]; then
        [[ "$output" == *"reviewable=true"* ]] ||
            fail "$label resolver rejected an open internal default-branch PR"
        [[ "$output" == *"number=42"* && "$output" == *"head_sha=abc123"* ]] ||
            fail "$label resolver omitted the immutable PR identity"
    elif [[ "$output" != "reviewable=false" ]]; then
        fail "$label resolver accepted a PR outside its trust boundary"
    fi
}

valid_pr='{"head":{"repo":{"full_name":"owner/repo"},"sha":"abc123"},"base":{"ref":"main"},"state":"open"}'
fork_pr='{"head":{"repo":{"full_name":"fork/repo"},"sha":"abc123"},"base":{"ref":"main"},"state":"open"}'
wrong_base_pr='{"head":{"repo":{"full_name":"owner/repo"},"sha":"abc123"},"base":{"ref":"release"},"state":"open"}'
closed_pr='{"head":{"repo":{"full_name":"owner/repo"},"sha":"abc123"},"base":{"ref":"main"},"state":"closed"}'
resolver_script="$(workflow_step_script "$CLAUDE_REVIEW_WORKFLOW_FILE" "Resolve pull request")"
label="$(basename "$CLAUDE_REVIEW_WORKFLOW_FILE")"
assert_reviewability "$resolver_script" "$valid_pr" true "$label"
assert_reviewability "$resolver_script" "$fork_pr" false "$label fork"
assert_reviewability "$resolver_script" "$wrong_base_pr" false "$label base"
assert_reviewability "$resolver_script" "$closed_pr" false "$label state"

lychee_fallback="$(
    yq -e -r '
        [.jobs.hygiene.steps[]
            | select(.uses == "taiki-e/install-action@v2")
            | select(.with.tool | contains("lychee@"))
            | .with.fallback]
        | select(length == 1)
        | .[0]
    ' "$CI_LINT_WORKFLOW_FILE"
)"
[[ "$lychee_fallback" == "cargo-binstall" ]] ||
    fail "pinned lychee install must allow the documented cargo-binstall fallback"

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
duplicate_metric_sarif_dir="$test_dir/duplicate-metric-sarif"
mkdir -p \
    "$healthy_sarif_dir" \
    "$unhealthy_sarif_dir" \
    "$missing_metric_sarif_dir" \
    "$duplicate_metric_sarif_dir"

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

assert_metric_cardinality_rejected() {
    local sarif_dir="$1"
    local label="$2"
    local summary_file="$test_dir/$label-summary"
    local output
    if output="$(
        GITHUB_STEP_SUMMARY="$summary_file" \
            SARIF_DIR="$sarif_dir" \
            bash -c "$health_script" 2>&1
    )"; then
        fail "Rust extraction-health gate accepted $label CodeQL metrics"
    fi
    [[ "$output" == *"expected exactly one CodeQL metric"* ]] ||
        fail "Rust extraction-health gate failed before rejecting $label CodeQL metrics"
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
unhealthy_summary="$test_dir/unhealthy-summary"
if unhealthy_output="$(
    GITHUB_STEP_SUMMARY="$unhealthy_summary" \
        SARIF_DIR="$unhealthy_sarif_dir" \
        bash -c "$health_script" 2>&1
)"; then
    fail "Rust extraction-health gate accepted a database with diagnostics in most files"
fi
[[ "$unhealthy_output" == *"::error title=Unhealthy Rust CodeQL database::"* ]] ||
    fail "Rust extraction-health gate failed before evaluating its unhealthy threshold"

printf '{"runs":[]}\n' >"$missing_metric_sarif_dir/rust.sarif"
assert_metric_cardinality_rejected "$missing_metric_sarif_dir" "missing"

base_sarif="$test_dir/base.sarif"
write_sarif_metrics "$base_sarif" 3 97
jq '
    .runs[0].properties.metricResults +=
        [.runs[0].properties.metricResults[0]]
' "$base_sarif" >"$duplicate_metric_sarif_dir/rust.sarif"
assert_metric_cardinality_rejected "$duplicate_metric_sarif_dir" "duplicate"

publisher_script="$(workflow_step_script "$CLAUDE_REVIEW_WORKFLOW_FILE" "Publish validated Claude review")"
published_comment="$test_dir/published-comment"
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

if PATH="$fake_bin:$PATH" \
    FAKE_PR_HEAD="abc123" \
    PUBLISHED_COMMENT="$published_comment" \
    EXPECTED_HEAD_SHA="abc123" \
    PR_NUMBER="42" \
    REPOSITORY="owner/repo" \
    REVIEW_JSON='{"summary":' \
    REVIEW_MARKER="claude-auto-review" \
    REVIEW_TITLE="Claude Review" \
    bash -c "$publisher_script" >/dev/null 2>&1; then
    fail "Claude publisher accepted malformed JSON"
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

oversized_summary="$(
    jq -n --arg summary "$(printf 's%.0s' {1..8001})" \
        '{summary: $summary, findings: []}'
)"
if PATH="$fake_bin:$PATH" \
    FAKE_PR_HEAD="abc123" \
    PUBLISHED_COMMENT="$published_comment" \
    EXPECTED_HEAD_SHA="abc123" \
    PR_NUMBER="42" \
    REPOSITORY="owner/repo" \
    REVIEW_JSON="$oversized_summary" \
    REVIEW_MARKER="claude-auto-review" \
    REVIEW_TITLE="Claude Review" \
    bash -c "$publisher_script" >/dev/null 2>&1; then
    fail "Claude publisher accepted an oversized summary"
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

# --- #799 fork refusal + #819 absence notice -------------------------------
# Both are shell that GUARDS something, and the rego rules pin only that they
# exist and where they sit. What they actually do is asserted here.
CLAUDE_TAG_WORKFLOW_FILE="${CLAUDE_TAG_WORKFLOW_FILE:-.github/workflows/claude.yml}"

# These steps pass --jq and hit two different endpoints, neither of which the
# resolver stub above models.
cat >"$fake_bin/gh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == api ]]
jq_expr="" url="" body="" method=GET
while [[ $# -gt 0 ]]; do
    case "$1" in
        --jq) jq_expr="$2"; shift ;;
        -X) method="$2"; shift ;;
        -f) body="${2#body=}"; shift ;;
        repos/*) url="$1" ;;
    esac
    shift
done
if [[ "$method" != GET ]]; then
    printf '%s' "$body" >"$ABSENCE_CAPTURE"
    printf '%s' "$url" >"$ABSENCE_CAPTURE.url"
    exit 0
fi
case "$url" in
    */comments)
        [[ -z "${FAKE_LIST_FAILS:-}" ]] || exit 1
        printf '%s' "${FAKE_COMMENTS:-[]}" | jq -r "$jq_expr"
        ;;
    *) printf '%s' "$FAKE_PR_JSON" | jq -r "$jq_expr" ;;
esac
STUB
chmod +x "$fake_bin/gh"

refusal_script="$(workflow_step_script "$CLAUDE_TAG_WORKFLOW_FILE" "Refuse fork pull requests")"

assert_refusal() {
    local fixture="$1"
    local expect_refused="$2"
    local label="$3"
    if PATH="$fake_bin:$PATH" \
        FAKE_PR_JSON="$fixture" \
        GH_TOKEN="test-token" \
        PR_NUMBER="42" \
        REPOSITORY="owner/repo" \
        bash -c "$refusal_script" >/dev/null 2>&1; then
        [[ "$expect_refused" == false ]] || fail "@claude fork guard admitted $label"
    else
        [[ "$expect_refused" == true ]] || fail "@claude fork guard rejected $label"
    fi
}

assert_refusal "$valid_pr" false "an internal pull request"
assert_refusal "$fork_pr" true "a fork pull request"
assert_refusal '{"head":{"repo":null},"base":{"ref":"main"},"state":"open"}' true "a deleted fork head"

absence_script="$(workflow_step_script "$CLAUDE_REVIEW_WORKFLOW_FILE" "Say the second lens did not run")"
absence_capture="$test_dir/absence-body"

run_absence() {
    : >"$absence_capture"
    : >"$absence_capture.url"
    PATH="$fake_bin:$PATH" \
        ABSENCE_CAPTURE="$absence_capture" \
        ANALYZE_RESULT="$1" \
        FAKE_COMMENTS="${2:-[]}" \
        FAKE_LIST_FAILS="${3:-}" \
        FAKE_PR_JSON="$valid_pr" \
        GH_TOKEN="test-token" \
        PR_NUMBER="42" \
        REPOSITORY="owner/repo" \
        REVIEW_MARKER="claude-auto-review" \
        REVIEW_TITLE="Claude Review" \
        RUN_URL="https://example.invalid/run" \
        bash -c "$absence_script" >/dev/null 2>&1 ||
        fail "absence notice exited non-zero for result=$1"
}

run_absence failure
absence_body="$(<"$absence_capture")"
[[ "$absence_body" == *"ABSENT, not clean"* ]] ||
    fail "absence notice did not say the review is absent rather than clean"
[[ "$absence_body" == *"<!-- absent-claude-auto-review:abc123 -->"* ]] ||
    fail "absence notice omitted its own marker"
# A prefix matcher on the published review's marker must not read this notice
# as a review.
[[ "$absence_body" != *"<!-- claude-auto-review"* ]] ||
    fail "absence marker collides with the published review marker"
[[ "$absence_body" == *"job summary names the cause"* ]] ||
    fail "absence notice did not route a failed analysis to its summary"

run_absence success
absence_body="$(<"$absence_capture")"
[[ "$absence_body" == *"out of scope for the reviewer"* ]] ||
    fail "a declined review was reported as a failure"

# A re-run of the same head must REPLACE its notice, not stack another.
run_absence failure '[{"id":7,"body":"<!-- absent-claude-auto-review:abc123 -->\nstale"}]'
[[ "$(<"$absence_capture.url")" == *"/issues/comments/7" ]] ||
    fail "an existing notice was duplicated instead of refreshed in place"

# `--jq` runs per page, so two matching notices yield two ids; splicing both
# into the PATCH url would wedge the job exactly when it should self-heal.
run_absence failure '[{"id":7,"body":"<!-- absent-claude-auto-review:abc123 -->\na"},{"id":8,"body":"<!-- absent-claude-auto-review:abc123 -->\nb"}]'
[[ "$(<"$absence_capture.url")" == *"/issues/comments/7" ]] ||
    fail "a second matching notice was spliced into the update target"

# A listing that ERRORS is not "no notice yet": writing here posts the duplicate
# the guard exists to prevent.
run_absence failure '[]' fail-the-listing
[[ ! -s "$absence_capture" ]] ||
    fail "a failed comment listing still wrote a notice"
