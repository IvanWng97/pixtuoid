#!/usr/bin/env bash
set -euo pipefail

CLAUDE_REVIEW_WORKFLOW_FILE="${CLAUDE_REVIEW_WORKFLOW_FILE:-.github/workflows/claude-readonly-review.yml}"

fail() {
    echo "ci-observability behavior test: $*" >&2
    exit 1
}

workflow_step_script() {
    local yaml_file="$1"
    local step_name="$2"
    STEP_NAME="$step_name" yq -e -r '
        [.jobs[].steps[] | select(.name == strenv(STEP_NAME)) | .run]
        | select(length == 1)
        | .[0]
    ' "$yaml_file" || fail "$yaml_file has no single step named \"$step_name\""
}

test_dir="$(mktemp -d)"
trap 'rm -rf "$test_dir"' EXIT
export RUNNER_TEMP="$test_dir"

fake_bin="$test_dir/bin"
mkdir -p "$fake_bin"

# The resolver lists a commit's PRs on workflow_run, then reads one PR.
# shellcheck disable=SC2016 # The generated gh stub reads the fixture when it runs.
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    '[[ "$1" == api ]]' \
    'case "$2" in' \
    '*/commits/*/pulls) printf "%s\n" "$FAKE_PULLS_JSON" ;;' \
    '*/pulls/42) printf "%s\n" "$FAKE_PR_JSON" ;;' \
    '*) exit 1 ;;' \
    'esac' \
    >"$fake_bin/gh"
chmod +x "$fake_bin/gh"

# An empty head_sha is the `/claude-review` path, which names the PR; a set one
# is workflow_run's, whose payload list is never read. `false` is a refusal the
# PR is told about, `skip` a silent one.
assert_reviewability() {
    local script="$1"
    local fixture="$2"
    local expected="$3"
    local label="$4"
    local head_sha="${5:-}"
    local head_repo="${6:-}"
    local pulls="${7:-[]}"
    local output_file="$test_dir/pr-resolution-output"
    local pr_number=42
    if [[ -n "$head_sha" ]]; then
        pr_number=0
    fi
    : >"$output_file"

    PATH="$fake_bin:$PATH" \
        DEFAULT_BRANCH="main" \
        FAKE_PR_JSON="$fixture" \
        FAKE_PULLS_JSON="$pulls" \
        GH_TOKEN="test-token" \
        GITHUB_OUTPUT="$output_file" \
        HEAD_REPO="$head_repo" \
        HEAD_SHA="$head_sha" \
        PR_NUMBER="$pr_number" \
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
    elif [[ "$output" != *"reviewable=$expected" || "$output" == *"reviewable=true"* ]]; then
        fail "$label resolver did not refuse with reviewable=$expected: $output"
    fi
}

valid_pr='{"number":42,"head":{"repo":{"full_name":"owner/repo"},"sha":"abc123"},"base":{"ref":"main"},"state":"open"}'
fork_pr='{"number":42,"head":{"repo":{"full_name":"fork/repo"},"sha":"abc123"},"base":{"ref":"main"},"state":"open"}'
wrong_base_pr='{"number":42,"head":{"repo":{"full_name":"owner/repo"},"sha":"abc123"},"base":{"ref":"release"},"state":"open"}'
closed_pr='{"number":42,"head":{"repo":{"full_name":"owner/repo"},"sha":"abc123"},"base":{"ref":"main"},"state":"closed"}'
resolver_script="$(workflow_step_script "$CLAUDE_REVIEW_WORKFLOW_FILE" "Resolve pull request")"
label="$(basename "$CLAUDE_REVIEW_WORKFLOW_FILE")"
assert_reviewability "$resolver_script" "$valid_pr" true "$label"
assert_reviewability "$resolver_script" "$fork_pr" false "$label fork"
assert_reviewability "$resolver_script" "$wrong_base_pr" false "$label base"
assert_reviewability "$resolver_script" "$closed_pr" false "$label state"

moved_pr='{"number":42,"head":{"repo":{"full_name":"owner/repo"},"sha":"def456"},"base":{"ref":"main"},"state":"open"}'
draft_pr='{"number":42,"head":{"repo":{"full_name":"owner/repo"},"sha":"abc123"},"base":{"ref":"main"},"state":"open","draft":true}'
stacked_pr='{"number":43,"head":{"repo":{"full_name":"owner/repo"},"sha":"def456"},"base":{"ref":"main"},"state":"open"}'
sibling_pr='{"number":44,"head":{"repo":{"full_name":"owner/repo"},"sha":"abc123"},"base":{"ref":"release"},"state":"open"}'
assert_reviewability "$resolver_script" "$valid_pr" true "$label workflow_run, no PR number in the payload" \
    abc123 owner/repo "[$closed_pr,$stacked_pr,$valid_pr]"
assert_reviewability "$resolver_script" "$valid_pr" true "$label workflow_run, head also open against another base" \
    abc123 owner/repo "[$sibling_pr,$valid_pr]"
assert_reviewability "$resolver_script" "$draft_pr" skip "$label workflow_run draft" \
    abc123 owner/repo "[$draft_pr]"
assert_reviewability "$resolver_script" "$valid_pr" skip "$label workflow_run fork, same commit as a branch" \
    abc123 fork/repo "[$valid_pr]"
assert_reviewability "$resolver_script" "$wrong_base_pr" skip "$label workflow_run base" \
    abc123 owner/repo "[$wrong_base_pr]"
assert_reviewability "$resolver_script" "$moved_pr" skip "$label workflow_run head moved mid-resolve" \
    abc123 owner/repo "[$valid_pr]"
assert_reviewability "$resolver_script" "$valid_pr" skip "$label workflow_run no PR" \
    abc123 owner/repo "[]"

# The callers name `ci` by its `name:`, so a rename would stop every review
# without turning anything red.
ci_name="$(yq -r '.name' .github/workflows/ci.yml)"
for caller in .github/workflows/claude-review.yml .github/workflows/claude-security-review.yml; do
    yq -o=json '.on.workflow_run.workflows' "$caller" |
        jq -e --arg name "$ci_name" '. == [$name]' >/dev/null ||
        fail "$caller does not wait on the workflow named \"$ci_name\""
done

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

# The head pair is parameterized for the one stale-review case; everything
# else varies only the review body.
run_publisher() {
    local review_json="$1"
    local fake_head="${2:-abc123}"
    local expected_head="${3:-abc123}"
    PATH="$fake_bin:$PATH" \
        FAKE_PR_HEAD="$fake_head" \
        PUBLISHED_COMMENT="$published_comment" \
        EXPECTED_HEAD_SHA="$expected_head" \
        PR_NUMBER="42" \
        REPOSITORY="owner/repo" \
        REVIEW_JSON="$review_json" \
        REVIEW_MARKER="claude-auto-review" \
        REVIEW_TITLE="Claude Review" \
        bash -c "$publisher_script"
}

valid_review='{"summary":"No correctness findings.","findings":[]}'
run_publisher "$valid_review" ||
    fail "Claude publisher rejected a valid zero-finding review"
[[ -s "$published_comment" ]] ||
    fail "Claude publisher posted no review body"
published_content="$(<"$published_comment")"
[[ "$published_content" == *"<!-- claude-auto-review:abc123 -->"* ]] ||
    fail "Claude publisher omitted the exact-head marker"
[[ "$published_content" == *"**Findings: 0**"* ]] ||
    fail "Claude publisher omitted the zero-finding count"

# A finding against a density-variant sprite (`<base>@<N>x.sprite`) must publish.
# The path allowlist is the FIRST thing the publisher runs, and it exits without
# a `::error` annotation — so a rejected character reads in the checks table as
# "the bot never posted", not "the bot was blocked", and the merge gate silently
# becomes unsatisfiable for every finding on those files.
variant_review='{"summary":"One finding on a density variant.","findings":[{"severity":"MEDIUM","path":"crates/pixtuoid-scene/sprites/default/desk@8x.sprite","line":6,"body":"Header names a scheme that does not exist."}]}'
run_publisher "$variant_review" ||
    fail "Claude publisher rejected a finding on an '@' density-variant path"
published_content="$(<"$published_comment")"
[[ "$published_content" == *"\`crates/pixtuoid-scene/sprites/default/desk@8x.sprite:6\`"* ]] ||
    fail "Claude publisher omitted the density-variant finding location"

if run_publisher "$valid_review" new-head old-head >/dev/null 2>&1; then
    fail "Claude publisher accepted a stale review"
fi

if run_publisher '{"summary":' >/dev/null 2>&1; then
    fail "Claude publisher accepted malformed JSON"
fi

unsafe_path_review='{"summary":"finding","findings":[{"severity":"HIGH","path":"../outside","line":1,"body":"bad"}]}'
if run_publisher "$unsafe_path_review" >/dev/null 2>&1; then
    fail "Claude publisher accepted an unsafe finding path"
fi

# claude-refuses-forks-before-the-action pins only that the fork refusal exists
# and runs before the action; what it actually does is asserted here.
CLAUDE_TAG_WORKFLOW_FILE="${CLAUDE_TAG_WORKFLOW_FILE:-.github/workflows/claude.yml}"

# This step passes --jq, which the resolver stub above does not model.
cat >"$fake_bin/gh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == api ]]
jq_expr=""
while [[ $# -gt 0 ]]; do
    [[ "$1" == --jq ]] && jq_expr="$2"
    shift
done
printf '%s' "$FAKE_PR_JSON" | jq -r "$jq_expr"
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

# ── require-jobs: the verdict ci-gate and every group's `required` job reach ──
# Anything but success is red, and an empty needs map must not pass vacuously.
REQUIRE_JOBS_ACTION_FILE="${REQUIRE_JOBS_ACTION_FILE:-.github/actions/require-jobs/action.yml}"
require_script="$(yq -e -r '.runs.steps[0].run' "$REQUIRE_JOBS_ACTION_FILE")"

assert_required() {
    local results="$1"
    local expect="$2"
    local label="$3"
    if RESULTS="$results" LABEL=selftest bash -eo pipefail -c "$require_script" >/dev/null 2>&1; then
        [[ "$expect" == pass ]] || fail "require-jobs passed $label"
    else
        [[ "$expect" == fail ]] || fail "require-jobs failed $label"
    fi
}

assert_required '{"a":{"result":"success"},"b":{"result":"success"}}' pass "every needed job succeeding"
assert_required '{"a":{"result":"success"},"b":{"result":"failure"}}' fail "a failed job"
assert_required '{"a":{"result":"success"},"b":{"result":"skipped"}}' fail "a skipped job"
assert_required '{"a":{"result":"success"},"b":{"result":"cancelled"}}' fail "a cancelled job"
assert_required '{}' fail "an empty needs map"
assert_required '' fail "no results at all"

health_script="$(workflow_step_script .github/workflows/codeql.yml "Verify Rust extraction health")"

rust_metrics() {
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

assert_rust_health() {
    local expect="$1"
    local label="$2"
    local sarif_dir="$test_dir/sarif-$label"
    local output
    mkdir -p "$sarif_dir"
    cat >"$sarif_dir/rust.sarif"
    if output="$(GITHUB_STEP_SUMMARY="$test_dir/summary" SARIF_DIR="$sarif_dir" bash -c "$health_script" 2>&1)"; then
        [[ "$expect" == pass ]] || fail "Rust extraction-health gate passed $label"
    else
        [[ "$output" == *"$expect"* ]] || fail "Rust extraction-health gate failed $label: $output"
    fi
}

rust_metrics 16 314 | assert_rust_health pass "a mostly clean database"
rust_metrics 99 100 | assert_rust_health pass "one more clean file than diagnostic ones"
rust_metrics 100 100 | assert_rust_health "Unhealthy Rust CodeQL database" "as many diagnostic files as clean ones"
rust_metrics 247 63 | assert_rust_health "Unhealthy Rust CodeQL database" "mostly diagnostic files"
echo '{"runs":[]}' | assert_rust_health "expected exactly one CodeQL metric" "SARIF without the metrics"
