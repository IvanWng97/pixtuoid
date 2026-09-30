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
posted_threads="$test_dir/posted-threads"
cat >"$fake_bin/gh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
case "$1" in
api)
    shift
    path="" jq_expr="."
    while (($#)); do
        case "$1" in
        --jq) jq_expr="$2" && shift 2 ;;
        --method) shift 2 ;;
        -*) shift ;;
        *) path="$1" && shift ;;
        esac
    done
    case "$path" in
    repos/owner/repo/pulls/42)
        sha=$FAKE_PR_HEAD
        [[ -z "$FAKE_HEAD_AFTER_FILES" || ! -e "$POSTED_THREADS.files-read" ]] || sha=$FAKE_HEAD_AFTER_FILES
        jq -n -r --arg sha "$sha" "{head: {sha: \$sha}} | $jq_expr"
        ;;
    repos/owner/repo/pulls/42/files)
        touch "$POSTED_THREADS.files-read"
        jq -r "$jq_expr" <<<"$FAKE_PR_FILES"
        ;;
    repos/owner/repo/pulls/42/comments)
        posts=$(($(cat "$POSTED_THREADS.count" 2>/dev/null || echo 0) + 1))
        echo "$posts" >"$POSTED_THREADS.count"
        [[ "$posts" != "${FAKE_FAIL_POST:-}" ]] || exit 1
        jq -c . >>"$POSTED_THREADS"
        ;;
    *) exit 1 ;;
    esac
    ;;
pr)
    [[ "$2" == "comment" ]]
    while (($#)); do
        if [[ "$1" == "--body-file" ]]; then
            command cp "$2" "$PUBLISHED_COMMENT"
            exit 0
        fi
        shift
    done
    exit 1
    ;;
*) exit 1 ;;
esac
STUB
chmod +x "$fake_bin/gh"

pr_files='[
  {"filename": "gone.rs", "status": "removed", "patch": "@@ -1,2 +0,0 @@\n-a\n-b"},
  {"filename": "src/a.rs", "status": "modified", "patch": "@@ -1,2 +1,3 @@\n a\n+b\n c\n@@ -10,0 +20,2 @@\n+x\n+y"},
  {"filename": "src/b.rs", "status": "modified", "patch": "@@ -5 +5 @@\n-q\n+r"},
  {"filename": "img.png", "status": "added"}
]'

# The head pair is parameterized for the one stale-review case; everything
# else varies only the review body.
run_publisher() {
    local review_json="$1"
    local fake_head="${2:-abc123}"
    local expected_head="${3:-abc123}"
    rm -f "$posted_threads" "$posted_threads".{count,files-read} "$published_comment"
    PATH="$fake_bin:$PATH" \
        FAKE_PR_HEAD="$fake_head" \
        FAKE_PR_FILES="${PR_FILES:-$pr_files}" \
        FAKE_HEAD_AFTER_FILES="${FAKE_HEAD_AFTER_FILES:-}" \
        FAKE_FAIL_POST="${FAKE_FAIL_POST:-}" \
        POSTED_THREADS="$posted_threads" \
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
[[ ! -e "$posted_threads" ]] ||
    fail "Claude publisher posted a review thread for zero findings"

assert_threads() {
    jq -e -s "$1" "$posted_threads" >/dev/null ||
        fail "Claude publisher's review threads: $2: $(<"$posted_threads")"
}

in_diff_review='{"summary":"s","findings":[
  {"severity":"HIGH","path":"src/a.rs","line":2,"body":"added line"},
  {"severity":"MEDIUM","path":"src/a.rs","line":21,"body":"second hunk"},
  {"severity":"MEDIUM","path":"src/b.rs","line":5,"body":"count-less hunk header"},
  {"severity":"HIGH","path":"src/a.rs","line":10,"body":"between hunks"},
  {"severity":"MEDIUM","path":"img.png","line":1,"body":"no patch"}]}'
run_publisher "$in_diff_review" ||
    fail "Claude publisher rejected findings inside the diff"
assert_threads 'length == 5 and all(.[]; .commit_id == "abc123")' \
    "one thread per finding, at the reviewed head"
assert_threads '[.[:3][] | [.path, .line, .side]] == [["src/a.rs", 2, "RIGHT"], ["src/a.rs", 21, "RIGHT"], ["src/b.rs", 5, "RIGHT"]]' \
    "a finding on a diff line is an inline comment on that line"
assert_threads '[.[3:][] | [.path, .subject_type, has("line")]] == [["src/a.rs", "file", false], ["img.png", "file", false]]' \
    "a finding off the diff's lines is file-level on its own file"
assert_threads '[.[].body | split(" — ")[0][1:-1]] == ["src/a.rs:2", "src/a.rs:21", "src/b.rs:5", "src/a.rs:10", "img.png:1"]' \
    "every thread opens with the finding's own location"

FAKE_FAIL_POST=2 run_publisher "$in_diff_review" >/dev/null 2>&1 &&
    fail "Claude publisher exited zero with a thread not opened"
assert_threads 'length == 4 and ([.[].body] | any(contains("src/a.rs:21")) | not)' \
    "one failed thread stops none of the others"
[[ "$(<"$published_comment")" == *"not opened:** src/a.rs:21"* ]] ||
    fail "Claude publisher's summary does not name the thread it failed to open"

hostile_body="it's \"quoted\" \$(touch $test_dir/pwned) \`touch $test_dir/pwned\` \\n end"
hostile_review="$(jq -cn --arg b "$hostile_body" \
    '{summary: "s", findings: [{severity: "HIGH", path: "docs/other.md", line: 9, body: $b},
        {severity: "MEDIUM", path: "gone.rs", line: 1, body: "removed file"}]}')"
run_publisher "$hostile_review" ||
    fail "Claude publisher rejected a finding outside the diff"
[[ ! -e "$test_dir/pwned" ]] ||
    fail "Claude publisher executed finding text"
assert_threads 'length == 2 and all(.[]; .path == "src/a.rs" and .subject_type == "file")' \
    "a finding outside the diff or on a removed file anchors to the first surviving file"
assert_threads '.[1].body | split(" — ")[0][1:-1] == "gone.rs:1"' \
    "a finding on a removed file keeps its location"

PR_FILES='[{"filename": "gone.rs", "status": "removed", "patch": "@@ -1,2 +0,0 @@\n-a\n-b"}]' \
    run_publisher "$hostile_review" ||
    fail "Claude publisher rejected findings on a diff that only removes files"
assert_threads 'length == 2 and all(.[]; .path == "gone.rs" and .subject_type == "file")' \
    "a diff that only removes files still anchors every finding on a changed file"
jq -e -s --arg b "$hostile_body" '.[0].body | contains("docs/other.md:9") and contains($b)' \
    "$posted_threads" >/dev/null ||
    fail "Claude publisher did not carry hostile finding text literally: $(<"$posted_threads")"

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

if run_publisher "$in_diff_review" new-head old-head >/dev/null 2>&1; then
    fail "Claude publisher accepted a stale review"
fi
[[ ! -e "$published_comment" && ! -e "$posted_threads" ]] ||
    fail "Claude publisher posted a stale review"

FAKE_HEAD_AFTER_FILES=new-head run_publisher "$in_diff_review" >/dev/null 2>&1 &&
    fail "Claude publisher accepted a head that moved while it read the files"
[[ ! -e "$published_comment" && ! -e "$posted_threads" ]] ||
    fail "Claude publisher posted threads anchored on a moved head's files"

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
