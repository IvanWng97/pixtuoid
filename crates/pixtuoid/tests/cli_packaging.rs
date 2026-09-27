//! Integration coverage for what only the REAL binary shows of `main.rs`: the
//! `completions` / `man` packaging dispatch (the generation itself is unit-tested
//! in `cli.rs`) — that the SHELL arg reaches clap_complete and that stdout stays
//! the clean artifact channel homebrew-core captures — and the fatal-error exit.

use clap::ValueEnum;

/// Spawn the built binary with a HERMETIC env: the binary HONORS a non-empty
/// `$RUST_LOG`, so a test asserting a clean channel must clear it rather than
/// assume an inherited dev/CI verbosity is unset.
fn run(args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_pixtuoid"))
        .args(args)
        .env_remove("RUST_LOG")
        .env_remove("PIXTUOID_LOG")
        .output()
        .expect("run pixtuoid")
}

/// Iterating `Shell::value_variants()` rather than a hardcoded subset means a
/// shell clap_complete adds later is covered automatically.
#[test]
fn completions_emit_a_clean_script_for_every_supported_shell() {
    let shells = clap_complete::Shell::value_variants();
    // Negative control: with 0 or 1 shell the per-shell loop AND the pairwise
    // distinctness tooth below would pass vacuously.
    assert!(
        shells.len() >= 2,
        "clap_complete exposes < 2 shells ({}) — the per-shell + distinctness coverage below is vacuous",
        shells.len()
    );

    let mut scripts = std::collections::HashSet::new();
    for shell in shells {
        let name = shell
            .to_possible_value()
            .expect("every Shell variant has a value name")
            .get_name()
            .to_owned();
        let out = run(&["completions", &name]);
        assert!(
            out.status.success(),
            "completions {name} exited non-zero: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            out.stderr.is_empty(),
            "completions {name} wrote to stderr — the artifact channel must stay clean: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("pixtuoid"),
            "completions {name} script omits the binary name"
        );
        assert!(
            scripts.insert(out.stdout),
            "completions {name} produced a script identical to another shell's — the shell arg is ignored"
        );
    }
}

/// The homebrew `Utils.safe_popen_read` capture greps `^.TH`.
#[test]
fn man_emits_clean_roff_to_stdout() {
    let out = run(&["man"]);
    assert!(
        out.status.success(),
        "man exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stderr.is_empty(),
        "man wrote to stderr — the artifact channel must stay clean: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains(".TH"),
        "man output is not roff (.TH header missing)"
    );
}

/// A pack whose one frame file name carries an ESC and a bidi override, so the
/// load error `validate-pack` exits with quotes pack text.
fn pack_with_hostile_frame_name() -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(
        tmp.path().join("pack.toml"),
        "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\"A\"=\"#010203\"\n\
         [animations.seated]\nframes=[\"x\\u001B[31m\\u202E.sprite\"]\nframe_ms=100\n",
    )
    .expect("write pack.toml");
    tmp
}

#[test]
fn a_fatal_error_reaches_stderr_stripped_and_exits_1() {
    let pack = pack_with_hostile_frame_name();
    let out = run(&[
        "validate-pack",
        pack.path().to_str().expect("utf-8 tempdir"),
    ]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("Error: "), "{stderr:?}");
    assert!(
        !stderr.contains('\u{1b}') && !stderr.contains('\u{202e}'),
        "{stderr:?}"
    );
}

#[test]
fn a_fatal_error_into_a_broken_pipe_still_exits_1() {
    // A pipe whose reader is gone, not a closed fd: std swallows EBADF on
    // stderr, so only a write that fails reaches the panic this guards.
    let (reader, writer) = std::io::pipe().expect("pipe");
    drop(reader);
    // A regression crashes, and the crash hook must not log into the real
    // state dir.
    let state = tempfile::TempDir::new().expect("tempdir");
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_pixtuoid"))
        .args(["validate-pack", "/nonexistent-pixtuoid-pack"])
        .env_remove("RUST_LOG")
        .env_remove("PIXTUOID_LOG")
        .env("XDG_STATE_HOME", state.path())
        .stderr(writer)
        .status()
        .expect("run pixtuoid");
    assert_eq!(status.code(), Some(1), "{status:?}");
}

/// `pixtuoid … | head` closes the pipe early; the reader got what it wanted,
/// so that is a clean exit, not a crash or a failure.
#[test]
fn every_printing_command_exits_0_when_its_reader_leaves() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let pack = home.path().join("pack");
    for args in [
        &["man"][..],
        &["completions", "bash"],
        &["sources", "--json"],
        &["sources"],
        &["setup"],
        &["doctor"],
        &["init-pack", pack.to_str().expect("utf-8 tempdir")],
        &["validate-pack", pack.to_str().expect("utf-8 tempdir")],
    ] {
        let (reader, writer) = std::io::pipe().expect("pipe");
        drop(reader);
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_pixtuoid"))
            .args(args)
            .env_remove("RUST_LOG")
            .env_remove("PIXTUOID_LOG")
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path().join("config"))
            .env("XDG_STATE_HOME", home.path().join("state"))
            .stdout(writer)
            .output()
            .expect("run pixtuoid");
        assert!(
            out.status.success(),
            "{args:?}: {:?}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
