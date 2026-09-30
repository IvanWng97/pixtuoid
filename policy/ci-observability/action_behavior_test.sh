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

# ── dispositions: a disposition line must be terminal ──────────────────────────
# The stub applies the caller's own --jq to each fixture, so the trust filter is
# under test too.
DISPOSITIONS_WORKFLOW_FILE="${DISPOSITIONS_WORKFLOW_FILE:-.github/workflows/dispositions.yml}"
DISPOSITIONS_RERUN_WORKFLOW_FILE="${DISPOSITIONS_RERUN_WORKFLOW_FILE:-.github/workflows/dispositions-rerun.yml}"
dispo_bin="$test_dir/dispo-bin"
mkdir -p "$dispo_bin"
# shellcheck disable=SC2016 # The generated gh stub expands its own variables.
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'if [[ "$1" != api ]]; then' \
    '    printf "%s\n" "$*" >>"$DISPO_GH_LOG"' \
    '    case "$1 $2" in' \
    '    "pr view") jq -r "${@: -1}" <<<"$FAKE_PR_VIEW" ;;' \
    '    "run list") jq -r "${@: -1}" <<<"$FAKE_RUNS" ;;' \
    '    esac' \
    '    exit 0' \
    'fi' \
    'shift' \
    'path="" filter="."' \
    'while (($#)); do' \
    '    case "$1" in' \
    '    --jq) filter=$2; shift 2 ;;' \
    '    -*) shift ;;' \
    '    *) path=$1; shift ;;' \
    '    esac' \
    'done' \
    '[[ "$path" != "${FAKE_FAIL_PATH:-}" ]] || { echo "gh: Server Error (HTTP 502)" >&2; exit 1; }' \
    'case "$path" in' \
    '"repos/$GH_REPO/pulls/$PR") json=$FAKE_PR_BODY ;;' \
    '"repos/$GH_REPO/issues/$PR/comments") json=$FAKE_COMMENTS ;;' \
    '"repos/$GH_REPO/pulls/$PR/reviews") json=$FAKE_REVIEWS ;;' \
    '"repos/$GH_REPO/pulls/$PR/comments") json=${FAKE_THREADS:-[]} ;;' \
    '"repos/$GH_REPO/pulls/"*)' \
    '    json=$(jq -ce --arg n "${path##*/}" '"'"'.[$n] // empty'"'"' <<<"$FAKE_PRS") ||' \
    '        { echo "gh: Not Found (HTTP 404)" >&2; exit 1; } ;;' \
    '*) echo "gh stub: unexpected path $path" >&2; exit 2 ;;' \
    'esac' \
    'jq -r "$filter" <<<"$json"' \
    >"$dispo_bin/gh"
chmod +x "$dispo_bin/gh"

dispositions_script="$(workflow_step_script "$DISPOSITIONS_WORKFLOW_FILE" "Check disposition lines")"
known_prs='{"10":{"state":"open","merged_at":null},"11":{"state":"closed","merged_at":"2026-09-29T00:00:00Z"},"12":{"state":"closed","merged_at":null}}'

assert_dispositions() {
    local body="$1" comments="$2" reviews="$3" expect="$4" label="$5" names="${6:-}" fail_path="${7:-}"
    local output rc=0
    output="$(
        PATH="$dispo_bin:$PATH" \
            GH_REPO="owner/repo" \
            PR="7" \
            FAKE_PR_BODY="$(jq -cn --arg b "$body" '{body: $b}')" \
            FAKE_COMMENTS="$comments" \
            FAKE_REVIEWS="$reviews" \
            FAKE_PRS="$known_prs" \
            FAKE_FAIL_PATH="$fail_path" \
            FAKE_THREADS="${FAKE_THREADS:-[]}" \
            bash -c "$dispositions_script" 2>&1
    )" || rc=$?
    if [[ "$expect" == pass ]]; then
        [[ "$rc" == 0 ]] || fail "dispositions rejected $label: $output"
    else
        [[ "$rc" != 0 ]] || fail "dispositions accepted $label"
        [[ "$output" == *"$names"* ]] || fail "dispositions failed $label without naming $names: $output"
    fi
}

# $1 body, $2 author type (User|Bot), $3 author_association.
comment() { jq -cn --arg b "$1" --arg t "$2" --arg a "$3" '[{body: $b, user: {type: $t}, author_association: $a}]'; }
terminal_body=$'## Dispositions\n- FIXED: the clamp\n- FOLLOW-UP → #10: the stale doc\n- **RE-SCOPED** -> #11: the split-off half\n1. REFUTED: the premise (test `pins_it`)'

assert_dispositions "$terminal_body" '[]' '[]' pass "every state terminal (open + merged #N)"
assert_dispositions "No review yet." '[]' '[]' pass "a body with no dispositions"
assert_dispositions "This replaces \`SURFACED\` with **FOLLOW-UP → #N**; a line naming no FOLLOW-UP number fails." \
    '[]' '[]' pass "the vocabulary in prose"
assert_dispositions "ok" "$(comment $'```\n- SURFACED\n- FOLLOW-UP: x\n```' User OWNER)" '[]' pass \
    "the old vocabulary inside a fence"
assert_dispositions "ok" "$(comment "- SURFACED: x" Bot NONE)" '[]' pass "a bot's comment"
assert_dispositions "ok" "$(comment "- SURFACED: x" User NONE)" '[]' pass "an outsider's comment"
assert_dispositions "ok" "$(comment "- FOLLOW-UP: the stale doc" User OWNER)" '[]' fail \
    "a FOLLOW-UP naming no #N" "names no → #N"
assert_dispositions "- FOLLOW-UP: see #10" '[]' '[]' fail "a #N without the arrow" "names no → #N"
assert_dispositions "- RE-SCOPED: split later" '[]' '[]' fail "a RE-SCOPED naming no #N" "names no → #N"
assert_dispositions "ok" '[]' "$(comment "- FOLLOW-UP → #99: gone" User MEMBER)" fail \
    "a dangling #N in a review body" "#99"
assert_dispositions "- FOLLOW-UP → #12: closed" '[]' '[]' fail "a #N that closed unmerged" "#12"
assert_dispositions "- FOLLOW-UP → #7: me" '[]' '[]' fail "the PR citing itself" "#7 is this PR"
assert_dispositions "ok" "$(comment "**SURFACED** — the owner decides" User COLLABORATOR)" '[]' fail \
    "a bare SURFACED" "SURFACED is not a terminal disposition"
assert_dispositions $'body\n```\nunclosed' "$(comment "- SURFACED: x" User OWNER)" '[]' fail \
    "a later comment after a body's unclosed fence" "SURFACED is not a terminal disposition"
assert_dispositions "ok" "$(comment "- SURFACED: x" User OWNER)" '[]' fail \
    "a comments fetch that failed" "502" "repos/owner/repo/issues/7/comments"
FAKE_THREADS="$(comment "- FOLLOW-UP: in a thread" User OWNER)" \
    assert_dispositions "ok" '[]' '[]' fail "an inline review-thread reply" "names no → #N"

rerun_script="$(workflow_step_script "$DISPOSITIONS_RERUN_WORKFLOW_FILE" "Re-run the dispositions check")"
assert_rerun() {
    local pr_view="$1" runs="$2" expect="$3" label="$4"
    local log="$test_dir/dispo-gh.log"
    : >"$log"
    PATH="$dispo_bin:$PATH" \
        GH_REPO="owner/repo" \
        PR="7" \
        DISPO_GH_LOG="$log" \
        FAKE_PR_VIEW="$pr_view" \
        FAKE_RUNS="$runs" \
        bash -c "$rerun_script" >/dev/null 2>&1 ||
        fail "dispositions re-run exited non-zero for $label"
    if [[ -n "$expect" ]]; then
        grep -qx "run rerun $expect" "$log" || fail "dispositions re-run did not re-run $expect for $label: $(<"$log")"
    elif grep -q "run rerun" "$log"; then
        fail "dispositions re-run re-ran something for $label: $(<"$log")"
    fi
}
assert_rerun '{"state":"OPEN","headRefOid":"abc123"}' '[{"databaseId":555}]' 555 "an open PR with a run"
assert_rerun '{"state":"CLOSED","headRefOid":"abc123"}' '[{"databaseId":555}]' "" "a closed PR"
assert_rerun '{"state":"OPEN","headRefOid":"abc123"}' '[]' "" "a head with no run yet"

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
