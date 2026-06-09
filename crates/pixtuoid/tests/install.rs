use tempfile::TempDir;

#[test]
fn install_then_uninstall_round_trip() {
    let dir = TempDir::new().unwrap();
    let settings = dir.path().join("settings.json");

    let bin = env!("CARGO_BIN_EXE_pixtuoid");
    let status = std::process::Command::new(bin)
        .args([
            "install-hooks",
            "--settings",
            settings.to_str().unwrap(),
            "--hook-path",
            "/fake/path",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let contents = std::fs::read_to_string(&settings).unwrap();
    let v: serde_json::Value = serde_json::from_str(&contents).unwrap();
    assert!(v["hooks"]["PreToolUse"][0]["_pixtuoid"].as_bool().unwrap());

    let status = std::process::Command::new(bin)
        .args(["uninstall-hooks", "--settings", settings.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());

    let contents = std::fs::read_to_string(&settings).unwrap();
    let v: serde_json::Value = serde_json::from_str(&contents).unwrap();
    assert!(v.get("hooks").is_none(), "got {v}");
}

#[test]
fn install_with_config_and_target_flags() {
    let dir = TempDir::new().unwrap();
    let settings = dir.path().join("settings.json");
    let bin = env!("CARGO_BIN_EXE_pixtuoid");
    let status = std::process::Command::new(bin)
        .args([
            "install-hooks",
            "--target",
            "claude",
            "--config",
            settings.to_str().unwrap(),
            "--hook-path",
            "/fake/path",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert!(v["hooks"]["PreToolUse"][0]["_pixtuoid"].as_bool().unwrap());
}

// The Unix install flow (env-prefix command form). The Windows twin below
// exercises the same merge/backup/uninstall round trip with the bare exec form.
#[cfg(unix)]
#[test]
fn install_codex_writes_toml_with_sentinel_and_backup() {
    let dir = TempDir::new().unwrap();
    let cfg = dir.path().join("config.toml");
    std::fs::write(&cfg, "model = \"o1\"\n").unwrap(); // pre-existing user content
    let bin = env!("CARGO_BIN_EXE_pixtuoid");
    let status = std::process::Command::new(bin)
        .args([
            "install-hooks",
            "--target",
            "codex",
            "--config",
            cfg.to_str().unwrap(),
            "--hook-path",
            "/fake/pixtuoid-hook",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let v: toml::Value = toml::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(v["model"].as_str().unwrap(), "o1", "user content preserved");
    assert!(v["hooks"]["PreToolUse"][0]["hooks"][0]["_pixtuoid"]
        .as_bool()
        .unwrap());
    assert!(v.get("features").is_none(), "no [features] hooks = true");
    // backup created with the correct multi-dot name
    assert!(dir.path().join("config.toml.pixtuoid.bak").exists());

    // uninstall restores + removes backup
    let status = std::process::Command::new(bin)
        .args([
            "uninstall-hooks",
            "--target",
            "codex",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let v: toml::Value = toml::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert!(v.get("hooks").is_none());
    assert_eq!(v["model"].as_str().unwrap(), "o1");
    assert!(!dir.path().join("config.toml.pixtuoid.bak").exists());
}

// The Windows twin: `install-hooks --target codex` now SUCCEEDS on Windows
// (the bail was lifted) and writes the BARE exec form `<path> --source codex`
// into config.toml — NOT a quoted path (codex's cmd.exe /C + Command::arg
// escaping mangles a quoted path; see install::codex::hook_command).
#[cfg(windows)]
#[test]
fn install_codex_writes_bare_exec_form_on_windows() {
    let dir = TempDir::new().unwrap();
    let cfg = dir.path().join("config.toml");
    std::fs::write(&cfg, "model = \"o1\"\n").unwrap(); // pre-existing user content
    let bin = env!("CARGO_BIN_EXE_pixtuoid");
    let status = std::process::Command::new(bin)
        .args([
            "install-hooks",
            "--target",
            "codex",
            "--config",
            cfg.to_str().unwrap(),
            "--hook-path",
            r"C:\fake\pixtuoid-hook.exe",
        ])
        .status()
        .unwrap();
    assert!(status.success(), "codex install must succeed on Windows");

    let v: toml::Value = toml::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(v["model"].as_str().unwrap(), "o1", "user content preserved");
    let handler = &v["hooks"]["PreToolUse"][0]["hooks"][0];
    assert!(handler["_pixtuoid"].as_bool().unwrap());
    let cmd = handler["command"].as_str().unwrap();
    assert!(
        cmd.ends_with("--source codex"),
        "command must carry the --source flag: {cmd:?}"
    );
    assert!(
        !cmd.contains('"'),
        "command must be the BARE form (no quotes — they break cmd.exe /C): {cmd:?}"
    );
    assert!(
        cmd.contains("pixtuoid-hook"),
        "command runs the shim: {cmd:?}"
    );
    assert!(dir.path().join("config.toml.pixtuoid.bak").exists());

    // uninstall restores + removes backup (keyed on the _pixtuoid sentinel)
    let status = std::process::Command::new(bin)
        .args([
            "uninstall-hooks",
            "--target",
            "codex",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let v: toml::Value = toml::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert!(v.get("hooks").is_none());
    assert_eq!(v["model"].as_str().unwrap(), "o1");
    assert!(!dir.path().join("config.toml.pixtuoid.bak").exists());
}

// Regression guard for the byte-vs-semantic no-op bug: uninstall on a config
// that has user content but NO pixtuoid hooks must NOT rewrite the file and must
// NOT delete the user's backup (the only recovery path).
#[test]
fn uninstall_noop_preserves_file_and_backup() {
    let dir = TempDir::new().unwrap();
    let cfg = dir.path().join("config.toml");
    // Hand-formatted user config with a comment + a non-pixtuoid hook, no managed entries.
    let original = "# my codex config\nmodel = \"o1\"\n\n[[hooks.PreToolUse]]\nmatcher = \"*\"\n\n[[hooks.PreToolUse.hooks]]\ntype = \"command\"\ncommand = \"/usr/bin/mytool\"\n";
    std::fs::write(&cfg, original).unwrap();
    // A backup the user must never lose on a no-op uninstall.
    let bak = dir.path().join("config.toml.pixtuoid.bak");
    std::fs::write(&bak, "sentinel-backup").unwrap();

    let bin = env!("CARGO_BIN_EXE_pixtuoid");
    let status = std::process::Command::new(bin)
        .args([
            "uninstall-hooks",
            "--target",
            "codex",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());

    assert_eq!(
        std::fs::read_to_string(&cfg).unwrap(),
        original,
        "no-op uninstall must not rewrite/reformat the file"
    );
    assert!(bak.exists(), "no-op uninstall must NOT delete the backup");
    assert_eq!(std::fs::read_to_string(&bak).unwrap(), "sentinel-backup");
}

// Regression guard: uninstall on a missing Claude settings.json must not CREATE
// the file (the old code's early-return-on-missing behavior, preserved via the
// semantic no-op).
#[test]
fn uninstall_missing_file_creates_nothing() {
    let dir = TempDir::new().unwrap();
    let settings = dir.path().join("settings.json");
    let bin = env!("CARGO_BIN_EXE_pixtuoid");
    let status = std::process::Command::new(bin)
        .args([
            "uninstall-hooks",
            "--target",
            "claude",
            "--config",
            settings.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(
        !settings.exists(),
        "uninstall must not create a missing config"
    );
}

#[test]
fn install_unknown_target_errors() {
    let bin = env!("CARGO_BIN_EXE_pixtuoid");
    let status = std::process::Command::new(bin)
        .args(["install-hooks", "--target", "bogus"])
        .status()
        .unwrap();
    // clap rejects an invalid ValueEnum value → non-zero exit.
    assert!(!status.success());
}

#[test]
fn install_reasonix_writes_flat_json_with_sentinel_and_backup() {
    let dir = TempDir::new().unwrap();
    let settings = dir.path().join("settings.json");
    // Pre-existing user hook must survive both install and uninstall.
    std::fs::write(
        &settings,
        r#"{ "hooks": { "PreToolUse": [ { "match": "bash", "command": "my-guard.sh" } ] } }"#,
    )
    .unwrap();
    let bin = env!("CARGO_BIN_EXE_pixtuoid");
    let status = std::process::Command::new(bin)
        .args([
            "install-hooks",
            "--target",
            "reasonix",
            "--config",
            settings.to_str().unwrap(),
            "--hook-path",
            "/fake/pixtuoid-hook",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(pre.len(), 2, "user entry + managed entry");
    assert_eq!(pre[0]["command"], "my-guard.sh", "user entry preserved");
    // FLAT Reasonix entry: command + source stamp directly on the entry. The
    // command form is OS-specific (Reasonix shells via sh -c on Unix, cmd.exe /c
    // on Windows); the rest of this test is OS-independent and runs on both.
    assert!(pre[1]["_pixtuoid"].as_bool().unwrap());
    #[cfg(unix)]
    assert_eq!(
        pre[1]["command"].as_str().unwrap(),
        "PIXTUOID_SOURCE=reasonix '/fake/pixtuoid-hook'"
    );
    #[cfg(windows)]
    assert_eq!(
        pre[1]["command"].as_str().unwrap(),
        "/fake/pixtuoid-hook --source reasonix"
    );
    assert!(pre[1].get("hooks").is_none(), "no CC-style nested group");
    assert!(
        pre[1].get("match").is_none(),
        "must not write a match key (omitted = every tool)"
    );
    assert!(dir.path().join("settings.json.pixtuoid.bak").exists());

    // uninstall strips only the managed entry + removes the backup
    let status = std::process::Command::new(bin)
        .args([
            "uninstall-hooks",
            "--target",
            "reasonix",
            "--config",
            settings.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(pre.len(), 1);
    assert_eq!(pre[0]["command"], "my-guard.sh");
    assert!(!dir.path().join("settings.json.pixtuoid.bak").exists());
}
