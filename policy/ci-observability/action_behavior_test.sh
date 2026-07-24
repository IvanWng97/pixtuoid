#!/usr/bin/env bash
set -euo pipefail

CODECOV_ACTION_FILE="${CODECOV_ACTION_FILE:-.github/actions/upload-codecov/action.yml}"
CODEQL_WORKFLOW_FILE="${CODEQL_WORKFLOW_FILE:-.github/workflows/codeql.yml}"

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
# shellcheck disable=SC2016 # The generated stub expands these variables when it runs.
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'printf "%s\n" "$*" >> "$RUSTUP_LOG"' \
    'if [[ "$*" == "run stable rustc --print sysroot" ]]; then printf "%s\n" "$FAKE_SYSROOT"; fi' \
    'exit 0' \
    >"$fake_bin/rustup"
chmod +x "$fake_bin/rustup"

rustup_log="$test_dir/rustup.log"
github_env="$test_dir/github_env"
PATH="$fake_bin:$PATH" \
    RUSTUP_LOG="$rustup_log" \
    FAKE_SYSROOT="$fake_sysroot" \
    GITHUB_ENV="$github_env" \
    bash -c "$semantic_script" ||
    fail "Rust semantic-input setup failed with a complete standard-library source"

rustup_calls="$(<"$rustup_log")"
[[ "$rustup_calls" == *"component add rust-src rust-analyzer --toolchain stable"* ]] ||
    fail "Rust semantic-input setup did not install rust-src and rust-analyzer"
github_env_content="$(<"$github_env")"
[[ "$github_env_content" == "CODEQL_EXTRACTOR_RUST_OPTION_CARGO_ALL_TARGETS=true" ]] ||
    fail "Rust semantic-input setup did not enable all Cargo targets"

if PATH="$fake_bin:$PATH" \
    RUSTUP_LOG="$rustup_log" \
    FAKE_SYSROOT="$test_dir/missing-sysroot" \
    GITHUB_ENV="$github_env" \
    bash -c "$semantic_script" >/dev/null 2>&1; then
    fail "Rust semantic-input setup accepted a missing standard-library source"
fi

chmod -x "$fake_sysroot/libexec/rust-analyzer-proc-macro-srv"
if PATH="$fake_bin:$PATH" \
    RUSTUP_LOG="$rustup_log" \
    FAKE_SYSROOT="$fake_sysroot" \
    GITHUB_ENV="$github_env" \
    bash -c "$semantic_script" >/dev/null 2>&1; then
    fail "Rust semantic-input setup accepted a missing proc-macro server"
fi
