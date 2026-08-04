//! Integration coverage for the `completions` / `man` packaging dispatch in
//! `main.rs`: the generation itself is unit-tested in `cli.rs`, so these spawn the
//! REAL binary and assert the dispatch — that the SHELL arg reaches clap_complete
//! and that stdout stays the clean artifact channel homebrew-core captures.

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
