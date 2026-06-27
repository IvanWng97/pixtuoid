//! Integration coverage for the `completions` / `man` packaging dispatch in
//! `main.rs`. The clap_complete / clap_mangen GENERATION is unit-tested in
//! `cli.rs`, but the DISPATCH — that the SHELL arg actually reaches clap_complete
//! and that stdout stays the clean artifact channel (tracing → stderr) — had no
//! test tooth (the codecov-excluded `main.rs` arms, #398).

fn run(args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_pixtuoid"))
        .args(args)
        .output()
        .expect("run pixtuoid")
}

#[test]
fn completions_emit_a_shell_script_naming_the_binary() {
    let out = run(&["completions", "bash"]);
    assert!(
        out.status.success(),
        "completions bash exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("pixtuoid"),
        "completion script omits the binary name"
    );
}

#[test]
fn completions_shell_arg_actually_reaches_clap_complete() {
    // Proves the dispatch passes the SHELL through (not a hardcoded one): the bash
    // and zsh completion scripts are structurally different.
    let bash = run(&["completions", "bash"]).stdout;
    let zsh = run(&["completions", "zsh"]).stdout;
    assert_ne!(
        bash, zsh,
        "bash and zsh completions are identical — the shell arg is ignored"
    );
}

#[test]
fn man_emits_roff_to_stdout() {
    let out = run(&["man"]);
    assert!(out.status.success(), "man exited non-zero");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(".TH"),
        "man output is not roff (.TH header missing)"
    );
}
