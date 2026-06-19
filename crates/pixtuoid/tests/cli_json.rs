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
