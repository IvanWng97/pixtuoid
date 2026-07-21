//! Process-level contracts of the REAL `pixtuoid` binary — anything that only
//! holds once clap, config resolution, and the runtime are wired together, which
//! no in-process test can reach.
//!
//! SCOPE NOTE: this file began as the `sources --json` golden alone and was
//! widened deliberately. The second contract here is the CONNECTION GATE, which
//! is process-level by nature: it is the composition of `resolve_connected`
//! (config) with `reducer_task`'s per-event drop (runtime), and each half is
//! individually correct while the composition silently ate every Codex event for
//! five weeks. Keep new tests here to that bar — a contract that needs the real
//! process — rather than letting this become a general binary-test dumping
//! ground.
//!
//! 1. `sources --json` — the shape the Raycast extension parses. Exercises clap
//!    parse → `sources::status` → the JSON presenter → stdout, which the
//!    in-process `source_status_*` unit tests (struct shape + committed schema)
//!    never cover.
//! 2. The connection gate end-to-end — a real rollout dripped into a real
//!    `run --headless` appears as a sprite iff its source is connected.
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

// ── the connection gate, end to end ─────────────────────────────────────────

/// Run a headless pixtuoid against an isolated everything, drip a real Codex
/// rollout into its sessions root, and return whatever it printed.
///
/// `sources_toml` is the `[sources]` body — the ONLY difference between the two
/// arms below, so any behaviour difference is attributable to it alone.
fn headless_replay(sources_toml: &str, settle: std::time::Duration) -> String {
    use std::io::Write;

    let home = tempfile::tempdir().expect("home");
    let cfg = tempfile::tempdir().expect("config");
    let sessions = tempfile::tempdir().expect("sessions");
    let projects = tempfile::tempdir().expect("projects");
    let out = tempfile::NamedTempFile::new().expect("stdout file");

    std::fs::create_dir_all(cfg.path().join("pixtuoid")).unwrap();
    std::fs::write(
        cfg.path().join("pixtuoid/config.toml"),
        format!("[sources]\n{sources_toml}"),
    )
    .unwrap();

    // Isolate the SOCKET too, and not merely for hygiene: on the default socket
    // a real CC session's hook traffic on the developer's machine lands in this
    // run's scene, and the negative arm would see a sprite unrelated to Codex.
    let sock = home.path().join("hook.sock");

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_pixtuoid"))
        .args(["run", "--headless"])
        .arg("--codex-sessions-root")
        .arg(sessions.path())
        .arg("--projects-root")
        .arg(projects.path())
        .args(["--log-level", "error"])
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .env("XDG_CONFIG_HOME", cfg.path())
        .env("PIXTUOID_SOCKET", &sock)
        .stdout(out.reopen().expect("stdout handle"))
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn pixtuoid run --headless");

    // Let the watcher bind before the first write, so the rollout arrives as an
    // append rather than being present at first sight — the path the replay
    // harness exercises, and the one that broke.
    std::thread::sleep(std::time::Duration::from_millis(800));

    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../pixtuoid-core/tests/sources/fixtures/codex/permission-flow")
        .join("rollout-2026-01-01T00-00-00-01000000-0000-7000-8000-000000000001.jsonl");
    let body = std::fs::read_to_string(&fixture).expect("committed permission-flow fixture");
    let mut f = std::fs::File::create(
        sessions
            .path()
            .join("rollout-2026-01-01T00-00-00-0a0a0a0a-0b0b-0c0c-0d0d-0e0e0e0e0e0e.jsonl"),
    )
    .unwrap();
    f.write_all(body.as_bytes()).unwrap();
    f.sync_all().unwrap();
    drop(f);

    // Poll rather than sleeping the whole budget: the positive arm settles well
    // under a second, so only the negative arm pays the full wait.
    let deadline = std::time::Instant::now() + settle;
    let mut seen = String::new();
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
        seen = std::fs::read_to_string(out.path()).unwrap_or_default();
        if seen.contains("cx·") {
            break;
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    seen
}

/// A connected source's rollout becomes a sprite. This is the assertion the
/// manual `scripts/replay-fixture.sh` was the only carrier for — and it exited 0
/// unconditionally (`if ! grep -q ...; then echo; fi` takes the `if` from the
/// `echo`), so it reported success for five weeks while producing no agent.
#[test]
fn connected_codex_rollout_becomes_a_sprite() {
    let out = headless_replay("codex = true\n", std::time::Duration::from_secs(20));
    assert!(
        out.contains("cx·"),
        "a connected Codex rollout must render a cx· sprite; headless printed:\n{out}"
    );
}

/// The same rollout with the flag absent renders NOTHING — `resolve_connected`
/// treats a missing key as disconnected (0.12.0 dropped the install-state
/// inference) and `reducer_task` drops the events before the reducer.
///
/// This half is why the replay harness broke: it read the developer's real
/// config, where `codex` is simply not in `[sources]`, so it silently exercised
/// THIS arm while asserting the one above.
#[test]
fn disconnected_codex_rollout_renders_nothing() {
    // No early-exit path here, so this arm pays the full budget — kept small
    // deliberately; the positive arm is what proves the wait is long enough for
    // a sprite to appear at all.
    let out = headless_replay("claude-code = true\n", std::time::Duration::from_secs(6));
    assert!(
        !out.contains("cx·"),
        "a DISCONNECTED Codex rollout must render no sprite; headless printed:\n{out}"
    );
    assert!(
        out.contains("agents=[]"),
        "the headless summary should still be running and reporting an empty scene:\n{out}"
    );
}
