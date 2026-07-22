//! Process-level contracts of the REAL `pixtuoid` binary — anything that only
//! holds once clap, config resolution, and the runtime are wired together, which
//! no in-process test can reach.
//!
//! SCOPE NOTE: this file began as the `sources --json` golden alone and was
//! widened deliberately. The second contract here is the CONNECTION GATE, which
//! is process-level by nature: it is the composition of `resolve_connected`
//! (config) with `reducer_task`'s per-event drop (runtime). Each half is
//! individually correct and the composition still silently ate every Codex
//! event, which no in-process test could observe. Keep new tests to that bar —
//! a contract that genuinely needs the real process — rather than letting this
//! become a general binary-test dumping ground.
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

/// The gate's own announcement, from `runtime/driver.rs`'s reducer_task. Paired
/// by this literal because an integration test cannot see a `pub(crate)` const
/// and the message is not worth widening the API for; the negative arm below is
/// what fails if the two drift apart.
const GATE_DROP_MSG: &str = "dropping events: source not connected";

/// Everything a caller needs to tell "the gate kept the scene empty" apart from
/// "the replay never happened" — an assertion of ABSENCE is only evidence when
/// the thing that should have produced presence demonstrably reached the code.
struct Replay {
    /// The child's stdout: the `agents=[…]` summary lines.
    out: String,
    /// The child's stderr, captured to its OWN file. Headless routes tracing to
    /// stderr, so this carries the gate's announcement; and discarding it would
    /// make a binary that died at startup indistinguishable from one that ran
    /// and rendered nothing. It must not share `out`'s file: two handles on one
    /// `NamedTempFile` keep independent offsets and overwrite each other.
    err: String,
    /// Whether the fixture is readable under the WATCHED sessions root.
    fixture_landed: bool,
}

/// `Child::drop` neither kills nor waits, so any panic between spawn and the
/// explicit kill would orphan a `run --headless` loop — which exits only on
/// Ctrl-C — reparented to init and writing to an already-deleted temp file.
struct Reaped(std::process::Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Run a headless pixtuoid against an isolated everything and drip a real Codex
/// rollout into its sessions root.
///
/// `sources_toml` is the `[sources]` body and the ONLY difference between the
/// two arms below, so any behavioural difference is attributable to it alone —
/// which is why the log level is fixed here rather than varied per arm.
fn headless_replay(sources_toml: &str, budget: std::time::Duration) -> Replay {
    use std::io::Write;

    // Read the fixture BEFORE spawning: it has no dependency on the child, and
    // read-then-spawn keeps the panic off the far side of the kill.
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../pixtuoid-core/tests/sources/fixtures/codex/permission-flow")
        .join("rollout-2026-01-01T00-00-00-01000000-0000-7000-8000-000000000001.jsonl");
    let body = std::fs::read_to_string(&fixture).expect("committed permission-flow fixture");

    let home = tempfile::tempdir().expect("home");
    let cfg = tempfile::tempdir().expect("config");
    let sessions = tempfile::tempdir().expect("sessions");
    let projects = tempfile::tempdir().expect("projects");
    let out = tempfile::NamedTempFile::new().expect("stdout file");
    let err = tempfile::NamedTempFile::new().expect("stderr file");

    std::fs::create_dir_all(cfg.path().join("pixtuoid")).unwrap();
    std::fs::write(
        cfg.path().join("pixtuoid/config.toml"),
        format!("[sources]\n{sources_toml}"),
    )
    .unwrap();

    // Isolate the SOCKET too, and not merely for hygiene: on the default socket
    // a live CC session's hook traffic on the developer's machine lands in this
    // run's scene, and the negative arm would see a sprite unrelated to Codex.
    let sock = home.path().join("hook.sock");

    // `debug` is required, not incidental: the gate's announcement is the only
    // observable proof of WHY a scene stayed empty, and headless floors nothing
    // (unlike TUI mode, which caps at warn).
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_pixtuoid"))
        .args(["run", "--headless"])
        .arg("--codex-sessions-root")
        .arg(sessions.path())
        .arg("--projects-root")
        .arg(projects.path())
        .args(["--log-level", "debug"])
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .env("XDG_CONFIG_HOME", cfg.path())
        .env("PIXTUOID_SOCKET", &sock)
        .stdout(out.reopen().expect("stdout handle"))
        .stderr(err.reopen().expect("stderr handle"))
        .spawn()
        .expect("spawn pixtuoid run --headless");
    let mut child = Reaped(child);

    // Give the watcher a moment to bind. Deliberately short: the rollout is
    // picked up whether it arrives as an append or is already present at first
    // sight, so this is startup slack, not a path selector.
    std::thread::sleep(std::time::Duration::from_millis(200));

    let landed = sessions
        .path()
        .join("rollout-2026-01-01T00-00-00-0a0a0a0a-0b0b-0c0c-0d0d-0e0e0e0e0e0e.jsonl");
    let mut f = std::fs::File::create(&landed).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    f.sync_all().unwrap();
    drop(f);

    // Poll until the run has DECIDED — a sprite rendered, or the gate announced
    // the drop. Both arms therefore exit on a POSITIVE signal and share one
    // budget; neither waits out a fixed timeout to conclude an absence.
    // `if let Ok` rather than `unwrap_or_default`: a torn read mid-write must
    // not blank an already-good buffer ("·" is two bytes).
    let deadline = std::time::Instant::now() + budget;
    let (mut seen_out, mut seen_err) = (String::new(), String::new());
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if let Ok(s) = std::fs::read_to_string(out.path()) {
            seen_out = s;
        }
        if let Ok(s) = std::fs::read_to_string(err.path()) {
            seen_err = s;
        }
        // Each arm waits for ALL the evidence it will assert, not just its
        // first signal: the gate announces the drop before the summary loop has
        // printed its first line, so breaking on the announcement alone returns
        // an empty stdout and reds the very assertion it was meant to satisfy.
        let rendered = seen_out.contains("cx·");
        let gated = seen_err.contains(GATE_DROP_MSG) && seen_out.contains("agents=[");
        if rendered || gated {
            break;
        }
    }

    let _ = child.0.kill();
    let _ = child.0.wait();
    Replay {
        out: seen_out,
        err: seen_err,
        fixture_landed: landed.metadata().is_ok_and(|m| m.len() > 0),
    }
}

/// A connected source's rollout becomes a sprite — the assertion whose only
/// carrier used to be a manual script that could not fail.
#[test]
fn connected_codex_rollout_becomes_a_sprite() {
    let r = headless_replay("codex = true\n", std::time::Duration::from_secs(20));
    assert!(
        r.fixture_landed,
        "the fixture must reach the watched sessions root\nstderr:\n{}",
        r.err
    );
    assert!(
        r.out.contains("cx·"),
        "a connected Codex rollout must render a cx· sprite\nstdout:\n{}\nstderr:\n{}",
        r.out,
        r.err
    );
    assert!(
        !r.err.contains(GATE_DROP_MSG),
        "a CONNECTED source must not be announced as gated\nstderr:\n{}",
        r.err
    );
}

/// The same rollout with the flag absent renders nothing — and, crucially, for
/// the RIGHT reason. `resolve_connected` treats a missing key as disconnected
/// (0.12.0 dropped the install-state inference) and reducer_task drops the
/// events ahead of the reducer.
///
/// Asserting only the absence would be satisfied by any breakage that stopped
/// the rollout reaching the watcher at all, so the gate's own announcement is
/// what this pins; the fixture-landed check closes the remaining gap.
#[test]
fn disconnected_codex_rollout_is_dropped_by_the_gate() {
    let r = headless_replay("claude-code = true\n", std::time::Duration::from_secs(20));
    assert!(
        r.fixture_landed,
        "the fixture must reach the watched sessions root\nstderr:\n{}",
        r.err
    );
    assert!(
        r.err.contains(GATE_DROP_MSG) && r.err.contains("codex"),
        "the gate must announce dropping codex's events\nstderr:\n{}",
        r.err
    );
    assert!(
        !r.out.contains("cx·"),
        "a DISCONNECTED Codex rollout must render no sprite\nstdout:\n{}",
        r.out
    );
    assert!(
        r.out.contains("agents=[]"),
        "the run must still be alive and reporting an empty scene\nstdout:\n{}",
        r.out
    );
}
