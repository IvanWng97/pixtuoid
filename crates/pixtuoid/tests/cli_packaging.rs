//! Integration coverage for what only the REAL binary shows of `main.rs`: the
//! `completions` / `man` packaging dispatch (the generation itself is unit-tested
//! in `cli.rs`) — that the SHELL arg reaches clap_complete and that stdout stays
//! the clean artifact channel homebrew-core captures — the fatal-error exit, and
//! the clean exit when a printing command's reader leaves.

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

/// A pipe whose reader is gone, not a closed fd: std swallows EBADF on
/// stdout/stderr, so only a write that fails reaches the panic this guards.
fn reader_gone() -> std::io::PipeWriter {
    let (reader, writer) = std::io::pipe().expect("pipe");
    drop(reader);
    writer
}

/// `pixtuoid args` from an env cleared to `home` and a minimal PATH, so
/// nothing reads the developer's real config or CLI dirs.
#[cfg(unix)]
fn isolated(args: &[&str], home: &std::path::Path) -> std::process::Command {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_pixtuoid"));
    cmd.args(args)
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin");
    cmd
}

#[test]
fn a_fatal_error_into_a_broken_pipe_still_exits_1() {
    // A regression crashes, and the crash hook must not log into the real
    // state dir.
    let state = tempfile::TempDir::new().expect("tempdir");
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_pixtuoid"))
        .args(["validate-pack", "/nonexistent-pixtuoid-pack"])
        .env_remove("RUST_LOG")
        .env_remove("PIXTUOID_LOG")
        .env("XDG_STATE_HOME", state.path())
        .stderr(reader_gone())
        .status()
        .expect("run pixtuoid");
    assert_eq!(status.code(), Some(1), "{status:?}");
}

/// Every platform: these read no home, so they need no env isolation, and
/// Windows reports a gone reader through its own error code.
#[test]
fn artifact_commands_exit_0_when_their_reader_leaves() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let pack = tmp.path().join("pack");
    let pack = pack.to_str().expect("utf-8 tempdir");
    for args in [
        &["man"][..],
        &["completions", "bash"],
        &["init-pack", pack],
        &["validate-pack", pack],
    ] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_pixtuoid"))
            .args(args)
            .env_remove("RUST_LOG")
            .env_remove("PIXTUOID_LOG")
            .env("XDG_STATE_HOME", tmp.path())
            .stdout(reader_gone())
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

#[cfg(unix)]
#[test]
fn home_reading_commands_exit_0_when_their_reader_leaves() {
    let home = tempfile::TempDir::new().expect("tempdir");
    for args in [
        &["sources", "--json"][..],
        &["sources"],
        &["setup"],
        &["doctor"],
        &["connect", "claude-code", "--json"],
        &["disconnect", "claude-code"],
        &["sources", "set", "claude-code"],
        // After `connect`: its config is what `setup` detects, and only a
        // detected CLI reaches the apply loop's rows.
        &["setup", "--yes"],
    ] {
        let out = isolated(args, home.path())
            .stdout(reader_gone())
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

/// The tracing sink's own report of a failed write is an `eprintln!` onto the
/// stream that failed; `PIXTUOID_LOG` naming a directory makes `sources` log a
/// warning on stderr.
#[cfg(unix)]
#[test]
fn a_tracing_warning_into_a_gone_stderr_exits_0() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let gone = reader_gone();
    let status = isolated(&["sources"], home.path())
        .env("PIXTUOID_LOG", home.path())
        .stdout(gone.try_clone().expect("clone pipe"))
        .stderr(gone)
        .status()
        .expect("run pixtuoid");
    assert_eq!(status.code(), Some(0), "{status:?}");
}
