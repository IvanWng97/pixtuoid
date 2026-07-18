//! End-to-end golden for the `sources --json` CLI contract (the shape the Raycast
//! extension parses). Runs the REAL `pixtuoid` binary — exercising clap parse →
//! `sources::status` → the JSON presenter → stdout — which the in-process
//! `source_status_*` unit tests (struct shape + committed schema) never cover.
//!
//! Determinism: each source's `connected`/`cli_present` is a function of whether
//! it is target-bearing (probed absent in an empty HOME → disconnected) or
//! no-target (always present + migrate-default connected), NOT of what's installed
//! on the test machine — SO LONG AS the environment is fully isolated. We clear the
//! env and point HOME at an empty tempdir so every presence/hook probe sees nothing
//! (see the e2e-isolate-home lesson). Unix-only: the Windows home-var isolation
//! differs and can't be verified from here; the wire SHAPE is pinned cross-platform
//! by `source_status_json_shape_is_the_raycast_contract` + the schema golden.
#![cfg(unix)]

#[test]
fn sources_json_lists_every_source_in_an_isolated_home() {
    let home = tempfile::tempdir().expect("tempdir");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_pixtuoid"))
        .args(["sources", "--json"])
        // Full isolation: an empty env + empty HOME means every CLI's presence /
        // hook probe resolves absent, so the output depends only on the registry —
        // deterministic across machines. A minimal PATH is kept for the spawn.
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("run pixtuoid sources --json");

    assert!(
        output.status.success(),
        "sources --json exited non-zero: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    // `.json` golden → snapbox compares structurally (key-order-insensitive), so a
    // serde field reorder doesn't churn it; update with `SNAPSHOTS=overwrite`.
    snapbox::assert_data_eq!(stdout, snapbox::file!["snapshots/cli/sources.json"]);
}

/// The `--json` DELIVERY contract, not just its row shape: a FAILING
/// `connect`/`disconnect` still prints the `OutcomeRow` array to STDOUT and
/// exits NON-ZERO. `run_change` emits BEFORE it bails, so a `$?`-checking caller
/// (Raycast's `execFile` catch recovers the rows via `stdout.startsWith("[")`,
/// then reads `rows[0]`) gets BOTH the per-source detail and a real error signal.
/// The exit-code + stream + cardinality invariant is invisible to the row-shape
/// schema goldens — this is its only gate (design review finding #2).
#[test]
fn a_failing_connect_emits_the_outcome_rows_and_exits_nonzero() {
    let home = tempfile::tempdir().expect("tempdir");
    // Block claude-code's hook install deterministically: make `~/.claude` a
    // regular FILE, so writing `~/.claude/settings.json` errors. The pixtuoid
    // config under `~/.config` still writes fine, so connect reaches the install
    // step, fails it, rolls the flag back, and surfaces a `failed` row.
    std::fs::write(home.path().join(".claude"), b"not a directory").expect("seed .claude file");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_pixtuoid"))
        .args(["connect", "claude-code", "--json"])
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("run pixtuoid connect --json");

    assert!(
        !output.status.success(),
        "a failing connect must exit non-zero (the $?-checking caller's signal); stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    // Stream: the rows land on STDOUT even though the process exits non-zero, and
    // they PARSE as the OutcomeRow array — the exact value the Raycast consumer
    // recovers from a rejected execFile (`stdout.startsWith("[")` then `rows[0]`).
    let rows: Vec<serde_json::Value> = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("failing connect must still print the OutcomeRow array to stdout: {e}: {stdout:?}")
    });
    // Cardinality: exactly one row per requested id.
    assert_eq!(
        rows.len(),
        1,
        "exactly one OutcomeRow per requested id: {rows:?}"
    );
    assert_eq!(
        rows[0]["id"], "claude-code",
        "the row names the requested id"
    );
    // The blocked install is a `failed` outcome, not a silent success — the token
    // Raycast's `rows[0].outcome === "failed"` branch surfaces per-source.
    assert_eq!(
        rows[0]["outcome"], "failed",
        "a blocked install surfaces as `failed`, never a clean success: {rows:?}"
    );
}
