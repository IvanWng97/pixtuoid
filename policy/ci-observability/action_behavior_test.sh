#!/usr/bin/env bash
set -euo pipefail

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

fake_bin="$test_dir/bin"
mkdir -p "$fake_bin"

# The queue's release-PR exclusion copies release-plz's branch prefix.
release_prefix="$(yq -p toml -oy -e -r '.workspace.pr_branch_prefix' release-plz.toml)" ||
    fail "release-plz.toml has no pr_branch_prefix"
yq -o=json '.' .mergify.yml | jq -e --arg c "-head ~= ^$release_prefix" \
    '(.queue_rules | length > 0) and all(.queue_rules[]; any(.queue_conditions[]; . == $c))' >/dev/null ||
    fail ".mergify.yml has a queue that admits release-plz's ${release_prefix}* PRs"

# ── release-plz.yml: the publish waits for its own commit's ci-gate ──
release_workflow=.github/workflows/release-plz.yml
wait_step="Wait for ci-gate on this commit"
wait_script="$(workflow_step_script "$release_workflow" "$wait_step")"
wait_check="$(STEP_NAME="$wait_step" yq -e -r '.jobs[].steps[] | select(.name == strenv(STEP_NAME)) | .env.CHECK_NAME' "$release_workflow")" ||
    fail "\"$wait_step\" has no CHECK_NAME"
[[ "$wait_check" == "$(yq -e -r '.jobs.gate.name' .github/workflows/ci.yml)" ]] ||
    fail "\"$wait_step\" waits for $wait_check, not ci.yml's gate"

# Each call answers the next of FAKE_CHECK_PAGES, the last one repeating, and
# logs its endpoint.
cat >"$fake_bin/gh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == api ]]
echo "$2" >>"$CHECK_CALLS.endpoints"
jq_expr="."
while (($#)); do
    [[ "$1" == --jq ]] && jq_expr="$2"
    shift
done
calls=$(($(cat "$CHECK_CALLS" 2>/dev/null || echo 0) + 1))
echo "$calls" >"$CHECK_CALLS"
page="$(sed -n "${calls}p" <<<"$FAKE_CHECK_PAGES")"
[[ -n "$page" ]] || page="$(tail -n 1 <<<"$FAKE_CHECK_PAGES")"
[[ "$page" != error ]] || exit 1
jq -r "$jq_expr" <<<"$page"
STUB
# A wait that never ends is cut off by its job timeout; here, by the fifth sleep.
cat >"$fake_bin/sleep" <<'STUB'
#!/usr/bin/env bash
n=$(($(cat "$CHECK_CALLS.sleeps" 2>/dev/null || echo 0) + 1))
echo "$n" >"$CHECK_CALLS.sleeps"
((n < 5))
STUB
chmod +x "$fake_bin/gh" "$fake_bin/sleep"

check_page() {
    jq -cn --arg app "$1" --arg status "$2" --arg conclusion "$3" \
        '{check_runs: [{app: {slug: $app}, status: $status, conclusion: (if $conclusion == "" then null else $conclusion end), started_at: "2026-10-01T00:00:00Z"}]}'
}
no_runs='{"check_runs":[]}'
assert_wait() {
    local expect="$1" label="$2" pages="$3"
    rm -f "$test_dir/check-calls" "$test_dir/check-calls.sleeps"
    if PATH="$fake_bin:$PATH" CHECK_CALLS="$test_dir/check-calls" CHECK_NAME="$wait_check" \
        FAKE_CHECK_PAGES="$pages" GH_TOKEN=test-token POLL_SECONDS=0 REPOSITORY=owner/repo SHA=abc1234 \
        bash -c "$wait_script" >/dev/null 2>&1; then
        [[ "$expect" == pass ]] || fail "the release wait passed $label"
    else
        [[ "$expect" == fail ]] || fail "the release wait failed $label"
    fi
}
assert_wait pass "a ci-gate that passes after running" \
    "$no_runs"$'\n'"$(check_page github-actions in_progress "")"$'\n'error$'\n'"$(check_page github-actions completed success)"
assert_wait fail "a failed ci-gate" "$(check_page github-actions completed failure)"
assert_wait fail "a cancelled ci-gate" "$(check_page github-actions completed cancelled)"
assert_wait fail "a skipped ci-gate" "$(check_page github-actions completed skipped)"
assert_wait fail "another app's passing check of that name" "$(check_page impostor completed success)"

# ── release-plz.yml: only a merged release PR waits for CI and publishes ──
detect_step="Detect a merged release PR; refuse a stale one"
detect_script="$(workflow_step_script "$release_workflow" "$detect_step")"
detect_prefix="$(STEP_NAME="$detect_step" yq -e -r '.jobs[].steps[] | select(.name == strenv(STEP_NAME)) | .env.PR_BRANCH_PREFIX' "$release_workflow")" ||
    fail "\"$detect_step\" has no PR_BRANCH_PREFIX"
[[ "$detect_prefix" == "$release_prefix" ]] ||
    fail "\"$detect_step\" looks for $detect_prefix*, not release-plz.toml's $release_prefix*"
assert_detect() {
    local expect="$1" label="$2" pages="$3" output_file="$test_dir/detect-output"
    : >"$output_file"
    rm -f "$test_dir/check-calls" "$test_dir/check-calls.endpoints"
    if ! PATH="$fake_bin:$PATH" CHECK_CALLS="$test_dir/check-calls" FAKE_CHECK_PAGES="$pages" GH_TOKEN=test-token \
        GITHUB_OUTPUT="$output_file" PR_BRANCH_PREFIX="$detect_prefix" REPOSITORY=owner/repo SHA=abc1234 \
        bash -c "$detect_script" >/dev/null 2>&1; then
        [[ "$expect" == error ]] || fail "the release detection exited non-zero on $label"
        grep -q '^release=true$' "$output_file" && fail "the release detection failed open on $label"
        return 0
    fi
    [[ "$expect" != error ]] || fail "the release detection exited zero on $label"
    grep -qx "release=$expect" "$output_file" ||
        fail "the release detection did not answer release=$expect for $label: $(<"$output_file")"
}
pr_heads() { jq -cn '[$ARGS.positional[] | {head: {ref: ., sha: "\(.)-head"}}]' --args "$@"; }
# After a release PR is found: the merge's parent, then the release commit as
# its comparison with the PR's head lists it first.
release_pages() {
    printf '%s\n' '{"parents":[{"sha":"main123"},{"sha":"side123"}]}' \
        "$(jq -cn --arg parent "$1" --arg committed "$2" '{commits: [
            {parents: [{sha: $parent}], commit: {author: {date: "2026-10-01T00:00:00Z"}, committer: {date: $committed}}},
            {parents: [{sha: "x"}, {sha: "y"}], commit: {author: {date: "z"}, committer: {date: "z"}}}]}')"
}
release_head="${detect_prefix}v1.2.3"
assert_endpoints() {
    local want
    want="$(printf 'repos/owner/repo/%s\n' commits/abc1234/pulls commits/abc1234 "compare/main123...$release_head-head")"
    [[ "$(<"$test_dir/check-calls.endpoints")" == "$want" ]] ||
        fail "the release detection on $1 asked $(<"$test_dir/check-calls.endpoints"), not $want"
}
written="2026-10-01T00:00:00Z"
assert_detect true "a merged release PR" "$(pr_heads "$release_head")"$'\n'"$(release_pages main123 "$written")"
assert_endpoints "a merged release PR"
assert_detect true "a release PR among others" "$(pr_heads feat/x "$release_head")"$'\n'"$(release_pages main123 "$written")"
assert_endpoints "a release PR among others"
assert_detect error "a release PR cut from an older main" "$(pr_heads "$release_head")"$'\n'"$(release_pages old123 "$written")"
assert_detect error "a release PR rebased onto main" \
    "$(pr_heads "$release_head")"$'\n'"$(release_pages main123 2026-10-01T00:05:00Z)"
assert_detect error "an API failure on the comparison" \
    "$(pr_heads "$release_head")"$'\n'"$(release_pages main123 "$written" | head -n 1)"$'\n'error
assert_detect false "an ordinary PR" "$(pr_heads feat/x)"
assert_detect false "a branch merely naming the prefix" "$(pr_heads "feat/${detect_prefix}x")"
assert_detect false "a direct push with no PR" "$(pr_heads)"
assert_detect error "an API failure" error

# ── release-plz.yml: a homebrew-core test miss warns, once per miss, and never
# holds the release PR ──
brew_step="homebrew-core's test calls only what this tree can run"
brew_script="$(workflow_step_script "$release_workflow" "$brew_step")"
brew_dir="$test_dir/brew"
brew_bin="$brew_dir/bin"
mkdir -p "$brew_bin" "$brew_dir/target/debug"
cat >"$brew_bin/curl" <<'STUB'
#!/usr/bin/env bash
[[ -n "${FAKE_FORMULA:-}" ]] || exit 22
cat "$FAKE_FORMULA"
STUB
printf '#!/usr/bin/env bash\n' >"$brew_bin/cargo"
# This tree dropped `gone-cmd`, and `connect needs-cli` fails over two lines.
cat >"$brew_dir/target/debug/pixtuoid" <<'STUB'
#!/usr/bin/env bash
case "$1 ${2:-}" in
"gone-cmd --help") exit 2 ;;
"connect needs-cli") printf 'line one\nline two\n' && exit 1 ;;
esac
STUB
chmod +x "$brew_bin/curl" "$brew_bin/cargo" "$brew_dir/target/debug/pixtuoid"
assert_brew() {
    local label="$1" formula="$2" want="$3" out lines=0 warnings=0 summary="$brew_dir/summary"
    : >"$summary"
    printf '%s\n' "$formula" >"$brew_dir/formula.rb"
    out="$(cd "$brew_dir" && PATH="$brew_bin:$PATH" GITHUB_STEP_SUMMARY="$summary" \
        FAKE_FORMULA="${formula:+$brew_dir/formula.rb}" bash -c "$brew_script" 2>&1)" ||
        fail "the homebrew step held the release on $label: $out"
    if [[ -n "$out" ]]; then
        lines="$(wc -l <<<"$out" | tr -d ' ')"
        warnings="$(grep -c '^::warning::' <<<"$out" || true)"
    fi
    [[ "$lines" == "$want" && "$warnings" == "$want" ]] ||
        fail "the homebrew step on $label wanted $want one-line warnings, printed: $out"
    if ((want == 0)); then
        [[ ! -s "$summary" ]] || fail "the homebrew step on $label wrote a summary: $(<"$summary")"
    else
        [[ "$(grep -c '^### ' "$summary")" == 1 && "$(grep -c '^- ' "$summary")" == "$want" ]] ||
            fail "the homebrew step on $label wanted one heading and $want items: $(<"$summary")"
    fi
}
assert_brew "a failed fetch" "" 1
assert_brew "a test block with no subcommand" $'  test do\n    assert_match version.to_s, shell_output("#{bin}/pixtuoid --version")\n  end' 1
grep -q 'shape changed' "$brew_dir/summary" || fail "the homebrew step didn't name an unrecognized test block"
assert_brew "a dropped subcommand and a failing connect" \
    $'  test do\n    system bin/"pixtuoid", "gone-cmd", testpath/"x"\n    shell_output("#{bin}/pixtuoid connect needs-cli --json")\n    shell_output("#{bin}/pixtuoid connect works --json")\n  end' 2
{ grep -q 'gone-cmd' "$brew_dir/summary" && grep -q 'needs-cli.*line one line two' "$brew_dir/summary"; } ||
    fail "the homebrew step didn't name each miss with its cause: $(<"$brew_dir/summary")"
assert_brew "a test block this tree runs" $'  test do\n    shell_output("#{bin}/pixtuoid connect works --json")\n  end' 0

# ── apt-install: a failed or stalled update or download retries under the
# action's own bound, and the install never runs under it or after a failed
# download.
apt_action=.github/actions/apt-install/action.yml
apt_script="$(yq -e -r '.runs.steps[0].run' "$apt_action")" || fail "apt-install has no run step"
apt_attempts="$(yq -e -r '.runs.steps[0].env.ATTEMPTS' "$apt_action")" || fail "apt-install sets no ATTEMPTS"
apt_bound="$(yq -e -r '.runs.steps[0].env.ATTEMPT_TIMEOUT' "$apt_action")" || fail "apt-install sets no ATTEMPT_TIMEOUT"
((apt_attempts > 1)) || fail "apt-install allows $apt_attempts attempt(s), so nothing retries"
apt_dir="$test_dir/apt"
mkdir -p "$apt_dir/bin"
# shellcheck disable=SC2016 # The stubs read their fixtures when they run.
printf '%s\n' '#!/usr/bin/env bash' 'printf "timeout %s\n" "$*" >>"$APT_LOG"; while [[ "$1" == -* ]]; do shift 2; done; shift; "$@"' >"$apt_dir/bin/timeout"
# shellcheck disable=SC2016
printf '%s\n' '#!/usr/bin/env bash' '"$@"' >"$apt_dir/bin/sudo"
# Fails the first APT_FAILS calls of the kind APT_FAIL_ON names.
# shellcheck disable=SC2016
printf '%s\n' '#!/usr/bin/env bash' 'printf "apt-get %s\n" "$*" >>"$APT_LOG"' \
    'case " $* " in *" update "*) kind=update ;; *" --download-only "*) kind=download ;; *) exit 0 ;; esac' \
    '[[ "$kind" == "$APT_FAIL_ON" ]] || exit 0' \
    'n=$(($(cat "$APT_LOG.n" 2>/dev/null || echo 0) + 1)); echo $n >"$APT_LOG.n"; ((n > APT_FAILS))' >"$apt_dir/bin/apt-get"
chmod +x "$apt_dir/bin/"*
run_apt() {
    rm -f "$apt_dir/log" "$apt_dir/log.n"
    PATH="$apt_dir/bin:$PATH" APT_LOG="$apt_dir/log" APT_FAIL_ON="$1" APT_FAILS="$2" PACKAGES="pkg-a pkg-b" \
        RECOMMENDS="${3:-true}" ATTEMPTS="$apt_attempts" ATTEMPT_TIMEOUT="$apt_bound" bash -c "$apt_script" >/dev/null 2>&1
}
apt_count() { grep -c -e "$1" "$apt_dir/log" || true; }
run_apt none 0 || fail "apt-install failed with a healthy mirror"
[[ "$(apt_count "^timeout -k 10 $apt_bound sudo apt-get .*install -y --download-only pkg-a pkg-b$")" == 1 ]] ||
    fail "apt-install did not bound the package download by its ATTEMPT_TIMEOUT: $(<"$apt_dir/log")"
[[ "$(apt_count '^apt-get .*install -y --no-download pkg-a pkg-b$')" == 1 ]] ||
    fail "apt-install did not install the downloaded packages: $(<"$apt_dir/log")"
if grep -q '^timeout .*--no-download' "$apt_dir/log"; then fail "apt-install bounded the install itself"; fi
for kind in update download; do
    run_apt "$kind" $((apt_attempts - 1)) || fail "apt-install did not recover from a failed $kind on its last attempt"
    pattern=" update\$"
    [[ "$kind" == download ]] && pattern=" --download-only pkg-a pkg-b\$"
    [[ "$(apt_count "^timeout -k 10 $apt_bound sudo apt-get .*$pattern")" == "$apt_attempts" ]] ||
        fail "apt-install did not retry each failed $kind: $(<"$apt_dir/log")"
    run_apt "$kind" "$apt_attempts" && fail "apt-install passed after every $kind failed"
    [[ "$(apt_count '--no-download')" == 0 ]] || fail "apt-install installed after every $kind failed"
done
run_apt none 0 false || fail "apt-install failed without recommends"
[[ "$(apt_count '^apt-get .*install -y --no-install-recommends')" == 2 ]] ||
    fail "apt-install did not drop recommends from both the download and the install: $(<"$apt_dir/log")"

# ── path-changed: a PR's merge commit against its base parent; anything it
# cannot diff counts as changed.
changed_script="$(yq -e -r '.runs.steps[0].run' .github/actions/path-changed/action.yml)" ||
    fail "path-changed has no run step"
repo="$test_dir/changed-repo"
git init -q "$repo" && git -C "$repo" config user.email t@t && git -C "$repo" config user.name t
echo a >"$repo/lock" && echo a >"$repo/other" && echo a >"$repo/gone" && git -C "$repo" add . && git -C "$repo" commit -qm base
git -C "$repo" checkout -qb pr && echo b >"$repo/other" && git -C "$repo" rm -q gone && git -C "$repo" commit -qam pr
git -C "$repo" checkout -q - && git -C "$repo" merge -q --no-ff --no-edit pr
changed() {
    : >"$test_dir/changed-output"
    (cd "$1" && GITHUB_EVENT_NAME="$2" GITHUB_OUTPUT="$test_dir/changed-output" CHANGED_PATHS="$3" bash -c "$changed_script") ||
        fail "path-changed exited non-zero"
    sed -n 's/^changed=//p' "$test_dir/changed-output"
}
[[ "$(changed "$repo" pull_request other)" == true ]] || fail "path-changed missed a path the PR changes"
[[ "$(changed "$repo" pull_request lock)" == false ]] || fail "path-changed reported a path the PR leaves alone"
[[ "$(changed "$repo" pull_request "lock other")" == true ]] || fail "path-changed missed one changed path among several"
[[ "$(changed "$repo" pull_request gone)" == true ]] || fail "path-changed missed a path the PR deletes"
if (cd "$repo" && GITHUB_EVENT_NAME=pull_request GITHUB_OUTPUT=/dev/null CHANGED_PATHS="lock no-such-file" bash -c "$changed_script") >/dev/null 2>&1; then
    fail "path-changed judged a path that exists on neither side"
fi
[[ "$(changed "$repo" push other)" == false ]] || fail "path-changed judged a push as a PR"
git init -q "$test_dir/no-parent" && echo a >"$test_dir/no-parent/lock" && git -C "$test_dir/no-parent" add lock &&
    git -C "$test_dir/no-parent" -c user.email=t@t -c user.name=t commit -q -m only
[[ "$(changed "$test_dir/no-parent" pull_request lock 2>"$test_dir/no-parent.err")" == true ]] ||
    fail "path-changed passed a diff it could not run: $(<"$test_dir/no-parent.err")"
grep -q 'bad revision' "$test_dir/no-parent.err" ||
    fail "the parentless repo's diff did not fail on its missing parent: $(<"$test_dir/no-parent.err")"

# ── hermetic-nightly.yml: a bump is proposed once, taken only from its own
# passed leg, and published only as its source's checked fixtures and pin ──
nightly=.github/workflows/hermetic-nightly.yml
semver="$(yq -e -r '.env.SEMVER' "$nightly")" || fail "$nightly has no SEMVER"
bot="$(yq -e -r '.env.BOT' "$nightly")" || fail "$nightly has no BOT"
bump_bin="$test_dir/bump-bin"
mkdir -p "$bump_bin"
cat >"$bump_bin/gh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
case "$1 ${3:-}" in
api*/jobs) echo "$FAKE_JOBS" ;;
api*/artifacts) echo "$FAKE_ARTIFACTS" ;;
pr*) echo "$FAKE_PRS" ;;
*) exit 1 ;;
esac
STUB
chmod +x "$bump_bin/gh"

plan_script="$(workflow_step_script "$nightly" "Keep each moved source that no bump PR already proposes")"
plan() {
    : >"$test_dir/plan-output"
    PATH="$bump_bin:$PATH" GH_TOKEN=t GH_REPO=o/r MOVED="$1" FAKE_PRS="${2:-[]}" SEMVER="$semver" BOT="$bot" \
        GITHUB_OUTPUT="$test_dir/plan-output" bash -c "$plan_script" >/dev/null 2>&1 || fail "plan exited non-zero on $1"
    sed -n 's/^bumps=//p' "$test_dir/plan-output"
}
dsh_bump='[{"source":"dsh","version":"0.2.0"}]'
dsh_pr() { jq -cn --arg s "$1" --arg m "$2" --arg l "$3" \
    '[{title: "test(dsh): re-record at 0.2.0 and pin it", state: $s,
       mergedAt: (if $m == "" then null else $m end), commits: [{authors: [{login: $l}]}]}]'; }
[[ "$(plan "$dsh_bump")" == "$dsh_bump" ]] || fail "plan dropped a moved source"
[[ "$(plan '[{"source":"Dsh","version":"0.2.0"},{"source":"dsh","version":"0.2"},{"source":"a b","version":"1.0.0"}]')" == '[]' ]] ||
    fail "plan kept a malformed row"
[[ "$(plan '{"source":"dsh"}')" == '[]' && "$(plan 'not json')" == '[]' ]] || fail "plan read something that is not a list"
[[ "$(plan "$dsh_bump" "$(dsh_pr OPEN "" "$bot")")" == '[]' ]] || fail "plan re-proposed an open bump"
[[ "$(plan "$dsh_bump" "$(dsh_pr CLOSED "" "$bot")")" == '[]' ]] || fail "plan re-proposed a declined bump"
[[ "$(plan "$dsh_bump" "$(dsh_pr OPEN "" someone | jq -c '.[0].title = "older"')")" == '[]' ]] ||
    fail "plan would reset a branch a session committed to"

leg_script="$(workflow_step_script "$nightly" "Take the patch only from its own passed leg")"
leg() {
    local expect="$1" label="$2" conclusion="$3" made="$4" prs="${5:-[]}"
    local jobs artifacts
    jobs=$(jq -cn --arg c "$conclusion" '{jobs: [{name: "bump (dsh)", conclusion: $c, completed_at: "2026-10-13T10:00:00Z"}]}')
    artifacts=$(jq -cn --arg m "$made" '{artifacts: [$m | select(. != "") | {name: "patch-dsh", created_at: .}]}')
    if PATH="$bump_bin:$PATH" GH_TOKEN=t GH_REPO=o/r RUN=1 SOURCE=dsh BOT="$bot" FAKE_JOBS="$jobs" \
        FAKE_ARTIFACTS="$artifacts" FAKE_PRS="$prs" GITHUB_OUTPUT=/dev/null bash -c "$leg_script" >/dev/null 2>&1; then
        [[ "$expect" == take ]] || fail "publish took a patch from $label"
    else
        [[ "$expect" == refuse ]] || fail "publish refused a patch from $label"
    fi
}
leg take "its passed leg" success 2026-10-13T09:59:00Z
leg take "its leg, with its own open PR" success 2026-10-13T09:59:00Z "$(dsh_pr OPEN "" "$bot")"
leg refuse "a failed leg" failure 2026-10-13T09:59:00Z
leg refuse "a patch written after its leg ended" success 2026-10-13T10:01:00Z
leg refuse "a leg that wrote none" success ""
leg refuse "a PR a session committed to" success 2026-10-13T09:59:00Z "$(dsh_pr OPEN "" someone)"

check_script="$(workflow_step_script "$nightly" "Check the patch is this source's bump and nothing else")"
bump_repo="$test_dir/bump-repo"
registry=crates/pixtuoid-core/src/source/registry.rs
fx=crates/pixtuoid-core/tests/sources
git init -q "$bump_repo"
mkdir -p "$bump_repo/${registry%/*}" "$bump_repo/$fx/fixtures/dsh/run" "$bump_repo/$fx/fixtures/codex/run" \
    "$bump_repo/$fx/delegation/fixtures/dsh" "$bump_repo/$fx/snapshots"
printf '%s\n' 'const CODEX: SourceDescriptor = SourceDescriptor {' '    verified_version: "0.155.1",' '};' \
    'const DSH: SourceDescriptor = SourceDescriptor {' '    verified_version: "0.2.0-rc.2",' '};' >"$bump_repo/$registry"
for f in fixtures/dsh/run/old-session.jsonl fixtures/dsh/run/provenance.json fixtures/codex/run/hook-payloads.jsonl \
    delegation/fixtures/dsh/hook-payloads.jsonl snapshots/sources__conformance__dsh__run.snap; do
    echo old >"$bump_repo/$fx/$f"
done
echo old >"$bump_repo/${registry%/*}/lib.rs"
git -C "$bump_repo" add -A && git -C "$bump_repo" -c user.email=t@t -c user.name=t commit -qm base

pin() { perl -pi -e "s/\"$1\"/\"$2\"/" "$registry"; }
rerecord() {
    pin 0.2.0-rc.2 0.2.0
    rm "$fx/fixtures/dsh/run/old-session.jsonl"
    echo new >"$fx/fixtures/dsh/run/new-session.jsonl"
    echo new >"$fx/fixtures/dsh/run/provenance.json"
    echo new >"$fx/delegation/fixtures/dsh/hook-payloads.jsonl"
    echo new >"$fx/snapshots/sources__conformance__dsh__run.snap"
}
# setup-npm's node_modules holds `fixtures/` of its own, which the patch leaves out.
beside_npm() { rerecord && mkdir -p "$fx/hermetic/aimock/node_modules/p/fixtures" &&
    echo x >"$fx/hermetic/aimock/node_modules/p/fixtures/a.json"; }
rerecorded=$(printf '%s\n' "$registry" "$fx/delegation/fixtures/dsh/hook-payloads.jsonl" \
    "$fx/fixtures/dsh/run/new-session.jsonl" "$fx/fixtures/dsh/run/old-session.jsonl" \
    "$fx/fixtures/dsh/run/provenance.json" "$fx/snapshots/sources__conformance__dsh__run.snap")
patch_script="$(workflow_step_script "$nightly" "Write the bump as a patch")"
# How the patch is made from what `edit` leaves: `honest` runs the bump job's
# own step; `forged` takes everything, as a CLI owning the runner could upload;
# `renamed` takes what `edit` staged, with rename detection on.
check() {
    local expect="$1" label="$2" edit="$3" made="${4:-forged}" version="${VERSION_OVERRIDE:-0.2.0}"
    (
        cd "$bump_repo"
        "$edit"
        case "$made" in
        honest) SOURCE=dsh RUNNER_TEMP="$test_dir" bash -c "$patch_script" ;;
        renamed) git diff --binary -M --cached HEAD >"$test_dir/bump.patch" ;;
        *) git add -N -- . && git diff --binary --no-renames HEAD -- . >"$test_dir/bump.patch" ;;
        esac
        git reset -q --hard && git clean -qfd
    )
    : >"$test_dir/check-output"
    if (cd "$bump_repo" && SOURCE=dsh VERSION="$version" PATCH="$test_dir/bump.patch" SEMVER="$semver" \
        GITHUB_OUTPUT="$test_dir/check-output" bash -c "$check_script") >/dev/null 2>&1; then
        [[ "$expect" != refuse ]] || fail "publish took $label"
        if [[ "$expect" == take ]]; then
            grep -qx "version=$version" "$test_dir/check-output" || fail "publish took $label without naming its version"
            [[ "$(git -C "$bump_repo" diff --cached --name-only | sort)" == "$rerecorded" ]] ||
                fail "publish took $label as other than the re-record: $(git -C "$bump_repo" diff --cached --name-only)"
            [[ "$(awk '/^PATHS_END$/ { p = 0 } p; /^paths<<PATHS_END$/ { p = 1 }' "$test_dir/check-output" | sort)" == "$rerecorded" ]] ||
                fail "publish would commit $label as other than the re-record: $(<"$test_dir/check-output")"
        fi
    else
        [[ "$expect" == refuse ]] || fail "publish refused $label"
    fi
    (cd "$bump_repo" && git reset -q --hard && git clean -qfd)
}
nothing() { :; }
outside() { rerecord && echo x >>"${registry%/*}/lib.rs"; }
other_source() { rerecord && echo x >>"$fx/fixtures/codex/run/hook-payloads.jsonl"; }
wrong_row() { pin 0.155.1 0.2.0; }
both_rows() { rerecord && pin 0.155.1 0.2.0; }
not_data() { rerecord && echo x >"$fx/fixtures/dsh/run/x.sh"; }
symlink() { rerecord && ln -s ../../../../src/source/lib.rs "$fx/fixtures/dsh/run/link.jsonl"; }
mode() { rerecord && chmod +x "$fx/fixtures/dsh/run/provenance.json"; }
binary() { rerecord && printf '\0\1\2' >"$fx/fixtures/dsh/run/b.jsonl"; }
renamed() { pin 0.2.0-rc.2 0.2.0 && git add -A && git mv "${registry%/*}/lib.rs" "$fx/fixtures/dsh/run/lib.jsonl"; }
planted() { rerecord && mkdir -p "$fx/hermetic/aimock/node_modules/p/fixtures/dsh" && echo x >"$fx/hermetic/aimock/node_modules/p/fixtures/dsh/x.jsonl"; }
check take "the bump job's own patch of a re-record" beside_npm honest
check skip "an empty patch" nothing honest
check refuse "a forged change outside the source's fixtures" outside
check refuse "setup-npm's node_modules" beside_npm
check refuse "a jsonl planted under node_modules' own fixtures" planted
check refuse "another source's fixture" other_source
check refuse "another row's pin" wrong_row
check refuse "a second pin" both_rows
check refuse "a file no decoder reads" not_data
check refuse "a symlink" symlink
check refuse "a mode change" mode
check refuse "a binary" binary
check refuse "a rename" renamed renamed
# A source whose fixtures live only under fixtures/<source> has no module
# root for its pathspec to match.
(
    cd "$bump_repo"
    pin 0.155.1 0.162.1
    echo new >"$fx/fixtures/codex/run/new-session.jsonl"
    SOURCE=codex RUNNER_TEMP="$test_dir" bash -c "$patch_script" >/dev/null 2>&1 ||
        fail "the bump's patch step failed for a source with no module fixtures"
    grep -qx "+++ b/$fx/fixtures/codex/run/new-session.jsonl" "$test_dir/bump.patch" ||
        fail "the bump's patch for codex lost its new transcript"
    git reset -q --hard && git clean -qfd
)
VERSION_OVERRIDE=0.2.1 check refuse "a pin other than the version plan named" rerecord honest
