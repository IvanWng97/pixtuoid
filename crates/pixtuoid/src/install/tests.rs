use super::*;
use crate::install::target::{MergeOutcome, Target, CLAUDE, CODEX, OPENCLAW};

/// Callers must hold `TEST_ENV_LOCK` first, declared BEFORE this guard: locals
/// drop in reverse order, so the env restore happens while the lock is held.
struct EnvVarOverride {
    key: &'static str,
    prior: Option<std::ffi::OsString>,
}

impl EnvVarOverride {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let prior = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, prior }
    }
}

impl Drop for EnvVarOverride {
    fn drop(&mut self) {
        match self.prior.take() {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

static FAKE: Target = Target {
    name: "fake",
    core_source: "fake",
    display_name: "Fake",
    default_config_path: || Ok(std::path::PathBuf::from("/nonexistent/fake")),
    hook_command: |_, _| Ok("x".into()),
    merge_install: |c, _| {
        Ok(MergeOutcome {
            content: c.to_string(),
            changed: false,
        })
    },
    merge_uninstall: |c| {
        Ok(MergeOutcome {
            content: c.to_string(),
            changed: false,
        })
    },
    verify_schema: |_| crate::install::verify::SchemaParse::broken("test fake"),
    binary_strategy: BinaryStrategy::EmbedAbsolute,
    presence_probe: None,
    extra_artifacts: None,
    post_install_hint: None,
};

// A fn-pointer `default_config_path` can't capture a TempDir, so FAKE2/FAKE_DIR
// use a fixed temp path; the PID suffix keeps concurrent runs from racing on it.
fn fake2_config_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("pixtuoid-test-fake2-{}.toml", std::process::id()))
}

fn fake_dir_config_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("pixtuoid-test-fake-dir-{}", std::process::id()))
}

// FAKE2's merge_uninstall reports `changed` iff the content is non-empty, so
// has_hooks can be driven through both arms by controlling the on-disk content.
static FAKE2: Target = Target {
    name: "fake2",
    core_source: "fake2",
    display_name: "Fake2",
    default_config_path: || Ok(fake2_config_path()),
    hook_command: |_, _| Ok("x".into()),
    merge_install: |c, _| {
        Ok(MergeOutcome {
            content: c.to_string(),
            changed: false,
        })
    },
    merge_uninstall: |c| {
        Ok(MergeOutcome {
            content: c.to_string(),
            changed: !c.trim().is_empty(),
        })
    },
    verify_schema: |_| crate::install::verify::SchemaParse::broken("test fake"),
    binary_strategy: BinaryStrategy::EmbedAbsolute,
    presence_probe: None,
    extra_artifacts: None,
    post_install_hint: None,
};

// FAKE_DIR's config path is created as a DIRECTORY, so read_config errors.
static FAKE_DIR: Target = Target {
    name: "fakedir",
    core_source: "fakedir",
    display_name: "FakeDir",
    default_config_path: || Ok(fake_dir_config_path()),
    hook_command: |_, _| Ok("x".into()),
    merge_install: |c, _| {
        Ok(MergeOutcome {
            content: c.to_string(),
            changed: false,
        })
    },
    merge_uninstall: |c| {
        Ok(MergeOutcome {
            content: c.to_string(),
            changed: false,
        })
    },
    verify_schema: |_| crate::install::verify::SchemaParse::broken("test fake"),
    binary_strategy: BinaryStrategy::EmbedAbsolute,
    presence_probe: None,
    extra_artifacts: None,
    post_install_hint: None,
};

// FAKE_NO_HOME's default_config_path Errs — the no-home-dir arm.
static FAKE_NO_HOME: Target = Target {
    name: "fakenohome",
    core_source: "fakenohome",
    display_name: "FakeNoHome",
    default_config_path: || Err(anyhow::anyhow!("no home dir")),
    hook_command: |_, _| Ok("x".into()),
    merge_install: |c, _| {
        Ok(MergeOutcome {
            content: c.to_string(),
            changed: false,
        })
    },
    merge_uninstall: |c| {
        Ok(MergeOutcome {
            content: c.to_string(),
            changed: false,
        })
    },
    verify_schema: |_| crate::install::verify::SchemaParse::broken("test fake"),
    binary_strategy: BinaryStrategy::EmbedAbsolute,
    presence_probe: None,
    extra_artifacts: None,
    post_install_hint: None,
};

/// `/x/hook` is DRIVE-RELATIVE on Windows, where the absolutization rewrites it.
fn abs_fixture(unix: &str, windows: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(windows)
    } else {
        PathBuf::from(unix)
    }
}

#[test]
fn resolve_hook_binary_explicit_path_wins() {
    let p = abs_fixture("/x/hook", r"C:\x\hook");
    let got = resolve_hook_binary_from(&CLAUDE, Some(p.clone()), None, || {
        panic!("locate must not be called when --hook-path is given")
    });
    assert_eq!(got.unwrap(), (p, true));
}

#[test]
fn resolve_hook_binary_absolutizes_a_relative_explicit_path() {
    // An embedded relative path would resolve against the CLI's cwd at hook time
    // and silently never fire from other dirs.
    let (got, explicit) = resolve_hook_binary_from(
        &CLAUDE,
        Some(PathBuf::from("target/debug/pixtuoid-hook")),
        None,
        || unreachable!("explicit path must win"),
    )
    .unwrap();
    assert!(explicit);
    assert!(got.is_absolute(), "expected absolutized path, got {got:?}");
    assert!(got.ends_with("target/debug/pixtuoid-hook"));
}

#[cfg(unix)]
#[test]
fn resolve_hook_binary_claude_falls_back_to_bare_name_when_unresolvable() {
    // Regression: a fresh-machine connect hard-failed when pixtuoid-hook wasn't
    // yet on PATH. Routed through the injected seam (env_hook: None) so an ambient
    // PIXTUOID_HOOK on the dev machine can't short-circuit the staged failure.
    let got = resolve_hook_binary_from(&CLAUDE, None, None, || {
        Err(anyhow::anyhow!("could not locate"))
    });
    assert_eq!(got.unwrap(), (PathBuf::from("pixtuoid-hook"), false));
}

// The bare-name fallback is a unix-only contract: the Windows exec form embeds
// the absolute path, so an unresolvable binary is fatal there.
#[cfg(windows)]
#[test]
fn resolve_hook_binary_claude_errors_when_unresolvable_on_windows() {
    let got = resolve_hook_binary_from(&CLAUDE, None, None, || {
        Err(anyhow::anyhow!("could not locate"))
    });
    assert!(got.is_err(), "exec form requires a real resolved .exe");
}

#[test]
fn resolve_hook_binary_codex_errors_when_unresolvable() {
    let got = resolve_hook_binary_from(&CODEX, None, None, || {
        Err(anyhow::anyhow!("could not locate"))
    });
    assert!(got.is_err());
}

#[test]
fn resolve_hook_binary_env_override_routes_through_the_explicit_arm() {
    let (got, explicit) = resolve_hook_binary_from(
        &CODEX,
        None,
        Some(PathBuf::from("target/debug/pixtuoid-hook")),
        || unreachable!("the env override must win over locate"),
    )
    .unwrap();
    assert!(
        got.is_absolute(),
        "expected absolutized env path, got {got:?}"
    );
    assert!(got.ends_with("target/debug/pixtuoid-hook"));
    assert!(explicit);
}

#[test]
fn resolve_hook_binary_cli_flag_outranks_env_override() {
    let cli = abs_fixture("/cli/hook", r"C:\cli\hook");
    let env = abs_fixture("/env/hook", r"C:\env\hook");
    let got = resolve_hook_binary_from(&CLAUDE, Some(cli.clone()), Some(env), || {
        unreachable!("an explicit path must win over locate")
    });
    assert_eq!(got.unwrap(), (cli, true));
}

#[test]
fn resolve_hook_binary_no_overrides_uses_locate() {
    let located = abs_fixture("/located/hook", r"C:\located\hook");
    let expect = located.clone();
    let got = resolve_hook_binary_from(&CLAUDE, None, None, || Ok(located));
    assert_eq!(got.unwrap(), (expect, false));
}

#[test]
fn empty_env_override_counts_as_unset_at_the_live_read() {
    // io::nonempty_env is the live seam install_target reads PIXTUOID_HOOK
    // through: empty/whitespace must read as unset, or "" becomes the command.
    let _env = crate::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let saved = std::env::var_os("PIXTUOID_HOOK");
    std::env::set_var("PIXTUOID_HOOK", "");
    let empty = io::nonempty_env("PIXTUOID_HOOK");
    std::env::set_var("PIXTUOID_HOOK", "   ");
    let blank = io::nonempty_env("PIXTUOID_HOOK");
    std::env::set_var("PIXTUOID_HOOK", "/real/hook");
    let real = io::nonempty_env("PIXTUOID_HOOK");
    match saved {
        Some(v) => std::env::set_var("PIXTUOID_HOOK", v),
        None => std::env::remove_var("PIXTUOID_HOOK"),
    }
    assert_eq!(empty, None);
    assert_eq!(blank, None);
    assert_eq!(real, Some("/real/hook".into()));
}

#[test]
fn is_drive_relative_only_matches_prefix_without_root() {
    use std::path::Path;
    #[cfg(windows)]
    {
        assert!(is_drive_relative(Path::new(r"C:rel\hook.exe")));
        assert!(!is_drive_relative(Path::new(r"C:\abs\hook.exe")));
        assert!(!is_drive_relative(Path::new(r"rel\hook.exe")));
        // Rooted-no-prefix (`\x\hook`) IS handled by join (it keeps the cwd's
        // drive), so it must not trip the hard error.
        assert!(!is_drive_relative(Path::new(r"\rooted\hook.exe")));
    }
    // Unix has no path prefixes — `C:foo` is an ordinary relative path there.
    #[cfg(unix)]
    assert!(!is_drive_relative(Path::new("C:foo.exe")));
}

// For drive-relative `C:foo.exe` (prefix, no root) is_relative() is true but
// `cwd.join` no-ops, so the "absolutized" embed would still resolve against a
// per-drive cwd at hook time — hence the hard error.
#[cfg(windows)]
#[test]
fn resolve_hook_binary_rejects_a_drive_relative_explicit_path() {
    let err = resolve_hook_binary_from(
        &CLAUDE,
        Some(PathBuf::from(r"C:rel\hook.exe")),
        None,
        || unreachable!("the explicit path must win"),
    )
    .unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("drive-relative") && msg.contains("absolute path"),
        "got: {msg}"
    );
}

#[cfg(windows)]
#[test]
fn resolve_hook_binary_rejects_a_drive_relative_env_override() {
    let err =
        resolve_hook_binary_from(&CODEX, None, Some(PathBuf::from(r"C:rel\hook.exe")), || {
            unreachable!("the env override must win")
        })
        .unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("PIXTUOID_HOOK") && msg.contains("drive-relative"),
        "the error must name the seam that supplied the bad path: {msg}"
    );
}

#[test]
fn has_hooks_empty_config_is_false() {
    assert!(!has_hooks(&FAKE, None));
}

#[test]
fn has_hooks_unreadable_config_is_true() {
    let dir = fake_dir_config_path();
    let _ = std::fs::remove_file(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    assert!(has_hooks(&FAKE_DIR, None));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn has_hooks_changed_vs_unchanged_arms() {
    let path = fake2_config_path();
    std::fs::write(&path, "model = \"x\"\n").unwrap();
    assert!(has_hooks(&FAKE2, None));
    // Whitespace-only content trims to empty, so the empty arm answers and the
    // changed arm is never reached.
    std::fs::write(&path, "   \n").unwrap();
    assert!(!has_hooks(&FAKE2, None));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn verify_target_and_has_hooks_handle_unresolvable_config_path() {
    assert!(
        !has_hooks(&FAKE_NO_HOME, None),
        "no resolvable config path → no hooks"
    );
    let v = verify_target(&FAKE_NO_HOME, None);
    assert!(!v.is_sound());
    assert_eq!(
        v.issues,
        vec!["no config path resolves (no home dir)".to_string()]
    );
    assert!(v.notes.is_empty(), "the early return emits no notes");
}

#[test]
fn install_target_claude_writes_sentinel_and_backs_up() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("settings.json");
    std::fs::write(&cfg, "{}\n").unwrap();

    install_target(
        &CLAUDE,
        Some(cfg.clone()),
        Some(PathBuf::from("/fake/pixtuoid-hook")),
    )
    .unwrap();

    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert!(v["hooks"]["PreToolUse"][0]["_pixtuoid"].as_bool().unwrap());
    assert!(
        tmp.path().join("settings.json.pixtuoid.bak").exists(),
        "a backup of the prior content was written"
    );

    install_target(
        &CLAUDE,
        Some(cfg.clone()),
        Some(PathBuf::from("/fake/pixtuoid-hook")),
    )
    .unwrap();
}

#[test]
fn install_target_fails_fast_while_the_config_lock_is_held() {
    // Lock BEFORE read: even the up-to-date no-op path, which never reaches the
    // write, can't safely read/decide mid-flight of another writer.
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("settings.json");
    install_target(
        &CLAUDE,
        Some(cfg.clone()),
        Some(PathBuf::from("/fake/pixtuoid-hook")),
    )
    .unwrap();

    let _guard = io::lock_config(&cfg).unwrap();
    let err = install_target(
        &CLAUDE,
        Some(cfg.clone()),
        Some(PathBuf::from("/fake/pixtuoid-hook")),
    )
    .unwrap_err();
    assert!(err.to_string().contains("could not lock"), "got: {err:#}");
}

#[test]
fn uninstall_target_fails_fast_while_the_config_lock_is_held() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("settings.json");
    install_target(
        &CLAUDE,
        Some(cfg.clone()),
        Some(PathBuf::from("/fake/pixtuoid-hook")),
    )
    .unwrap();

    let _guard = io::lock_config(&cfg).unwrap();
    let err = uninstall_target(&CLAUDE, Some(cfg.clone())).unwrap_err();
    assert!(err.to_string().contains("could not lock"), "got: {err:#}");
}

#[test]
fn uninstall_target_unchanged_preserves_backup() {
    // FAKE.merge_uninstall reports changed=false → the semantic no-op branch.
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("config.toml");
    std::fs::write(&cfg, "anything\n").unwrap();
    let bak = tmp.path().join("config.toml.pixtuoid.bak");
    std::fs::write(&bak, "backup").unwrap();

    uninstall_target(&FAKE, Some(cfg.clone())).unwrap();

    assert!(bak.exists(), "a no-op uninstall must NOT delete the backup");
}

#[test]
fn install_target_reports_installed_then_up_to_date() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("settings.json");
    std::fs::write(&cfg, "{}\n").unwrap();

    let r = install_target(
        &CLAUDE,
        Some(cfg.clone()),
        Some(PathBuf::from("/fake/pixtuoid-hook")),
    )
    .unwrap();
    assert!(matches!(r.outcome, InstallOutcome::Installed));
    assert!(
        r.backup.is_some(),
        "first install of an existing file takes a backup"
    );
    assert_eq!(r.config_path, cfg);

    let r2 = install_target(
        &CLAUDE,
        Some(cfg.clone()),
        Some(PathBuf::from("/fake/pixtuoid-hook")),
    )
    .unwrap();
    assert!(matches!(r2.outcome, InstallOutcome::AlreadyUpToDate));
    assert!(r2.backup.is_none(), "a no-op install reports no backup");
}

#[test]
fn install_target_explicit_hook_suppresses_path_warning() {
    // An explicit --hook-path embeds the absolute path, so PATH resolution never
    // happens and the expectation is deterministic — unlike the no-hook case.
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("settings.json");
    let r = install_target(
        &CLAUDE,
        Some(cfg),
        Some(PathBuf::from("/fake/pixtuoid-hook")),
    )
    .unwrap();
    assert!(!r.path_warning);
}

#[test]
fn uninstall_target_reports_removed_then_nothing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("config.toml");
    std::fs::write(&cfg, "model = \"x\"\n").unwrap();
    let bak = tmp.path().join("config.toml.pixtuoid.bak");
    std::fs::write(&bak, "backup").unwrap();

    let r = uninstall_target(&FAKE2, Some(cfg.clone())).unwrap();
    assert!(matches!(r.outcome, UninstallOutcome::Removed));
    assert_eq!(r.removed_backup.as_deref(), Some(bak.as_path()));
    assert!(!bak.exists());

    // An absent config is decided BEFORE locking, so there are no side effects.
    let missing = tmp.path().join("missing").join("settings.json");
    let r2 = uninstall_target(&CLAUDE, Some(missing.clone())).unwrap();
    assert!(matches!(r2.outcome, UninstallOutcome::NothingToRemove));
    assert!(r2.removed_backup.is_none());
    assert!(
        !missing.parent().unwrap().exists(),
        "a no-op uninstall leaves no dirs"
    );
}

#[test]
fn install_target_round_trips_every_registered_target() {
    // OpenClaw's plugin dir resolves from openclaw_state_dir(), NOT the config
    // override, so a temp home keeps this off the real ~/.openclaw; TEST_ENV_LOCK
    // serializes that process-global set against sibling env-mutating tests.
    let _env = crate::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let oc_home = tempfile::TempDir::new().unwrap();
    let _state = EnvVarOverride::set("OPENCLAW_STATE_DIR", oc_home.path());
    for t in target::TARGETS {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("cfg");
        let hook = || Some(PathBuf::from("/fake/pixtuoid-hook"));

        let r = install_target(t, Some(cfg.clone()), hook()).unwrap();
        assert!(
            matches!(r.outcome, InstallOutcome::Installed),
            "{}: first install must write hooks",
            t.name
        );
        assert!(cfg.exists(), "{}: install wrote a config", t.name);

        let r2 = install_target(t, Some(cfg.clone()), hook()).unwrap();
        assert!(
            matches!(r2.outcome, InstallOutcome::AlreadyUpToDate),
            "{}: re-install must be a no-op (sentinel idempotency)",
            t.name
        );

        let u = uninstall_target(t, Some(cfg.clone())).unwrap();
        assert!(
            matches!(u.outcome, UninstallOutcome::Removed),
            "{}: uninstall must remove the managed entries",
            t.name
        );
        let u2 = uninstall_target(t, Some(cfg.clone())).unwrap();
        assert!(
            matches!(u2.outcome, UninstallOutcome::NothingToRemove),
            "{}: re-uninstall must find nothing to remove",
            t.name
        );
    }
}

// The detect⇄install symmetry, per detection mechanism. A literal
// false→true→false does NOT hold: uninstall PRESERVES the user's file, so
// detection stays TRUE afterwards.
#[test]
fn config_present_target_file_is_absent_before_then_present_after_install() {
    use crate::install::target::config_present;
    // CLAUDE + CODEX are the only `presence_probe: None` (config_present) targets.
    for t in [&CLAUDE, &CODEX] {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("cfg");
        assert!(
            !config_present(&cfg),
            "{}: config_present must be FALSE before any write",
            t.name
        );
        install_target(
            t,
            Some(cfg.clone()),
            Some(PathBuf::from("/fake/pixtuoid-hook")),
        )
        .unwrap();
        assert!(
            config_present(&cfg),
            "{}: config_present must be TRUE after install writes the config",
            t.name
        );
        uninstall_target(t, Some(cfg.clone())).unwrap();
        assert!(
            config_present(&cfg),
            "{}: uninstall preserves the user's config file → still present",
            t.name
        );
    }
}

#[test]
fn openclaw_is_present_is_false_before_then_true_after_install() {
    use crate::install::target::is_present;
    // OPENCLAW_STATE_DIR points at a NON-EXISTENT dir so the probe starts FALSE.
    let _env = crate::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let oc_home = tempfile::TempDir::new().unwrap();
    let state = oc_home.path().join("ocstate"); // not yet created
    let _state = EnvVarOverride::set("OPENCLAW_STATE_DIR", &state);

    assert!(
        !is_present(&OPENCLAW),
        "OpenClaw must be undetected before install (empty isolated state dir)"
    );

    let exe = std::env::current_exe().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("openclaw.json");
    install_target(&OPENCLAW, Some(cfg), Some(exe)).unwrap();

    assert!(
        is_present(&OPENCLAW),
        "install must create the state dir the presence probe detects \
         (detect⇄install symmetry — else installed-but-invisible)"
    );
}

#[test]
fn uninstall_preserves_the_config_file_even_when_it_merges_to_empty() {
    // Codex installed ALONE: the config holds only our managed entry, so uninstall
    // un-merges to an effectively EMPTY TOML doc — the exact case where a naive
    // "delete if empty" would lose the user's file.
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("config.toml");

    let r = install_target(
        &CODEX,
        Some(cfg.clone()),
        Some(PathBuf::from("/fake/pixtuoid-hook")),
    )
    .unwrap();
    assert!(matches!(r.outcome, InstallOutcome::Installed));
    assert!(cfg.exists());

    let u = uninstall_target(&CODEX, Some(cfg.clone())).unwrap();
    assert!(
        matches!(u.outcome, UninstallOutcome::Removed),
        "uninstall must have removed the managed entry (the merge produced a change)"
    );
    assert!(
        cfg.exists(),
        "uninstall must PRESERVE the config file (un-merge, never delete)"
    );
    let content = io::read_config(&cfg).unwrap();
    assert!(
        !content.contains(SENTINEL_KEY),
        "the managed hook entry must be un-merged out: {content:?}"
    );
}

#[test]
fn install_on_a_malformed_config_errors_without_rewriting_or_backing_up() {
    for (t, malformed) in [
        (&CODEX, "this is = = not valid toml [[["),
        (&CLAUDE, "{ not valid json,,, "),
    ] {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("cfg");
        std::fs::write(&cfg, malformed).unwrap();
        let before = std::fs::read_to_string(&cfg).unwrap();

        let err = install_target(
            t,
            Some(cfg.clone()),
            Some(PathBuf::from("/fake/pixtuoid-hook")),
        )
        .unwrap_err();
        // "refusing to overwrite" is what proves the error came from the parse
        // step and not a downstream write failure.
        let msg = format!("{err:#}");
        assert!(
            msg.contains("refusing to overwrite"),
            "{}: the error must come from the parse guard, got: {msg}",
            t.name
        );

        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            before,
            "{}: a malformed config must NOT be rewritten/truncated",
            t.name
        );
        // The .lock sidecar may exist (the lock is taken before the read by
        // design); the BACKUP must not.
        let bak = tmp.path().join(format!("cfg.{BACKUP_SUFFIX}"));
        assert!(
            !bak.exists(),
            "{}: a failed install must NOT mint a {BACKUP_SUFFIX} backup",
            t.name
        );
    }
}

#[test]
fn install_on_a_malformed_config_leaves_no_orphan_extra_artifacts() {
    // A present-but-malformed config must bail BEFORE the extra artifacts are
    // written, else a partial install strands orphan plugin files.
    let _env = crate::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let oc_home = tempfile::TempDir::new().unwrap();
    let _state = EnvVarOverride::set("OPENCLAW_STATE_DIR", oc_home.path());

    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("openclaw.json");
    std::fs::write(&cfg, "{ not valid json,,, ").unwrap();
    let before = std::fs::read_to_string(&cfg).unwrap();

    let err = install_target(
        &OPENCLAW,
        Some(cfg.clone()),
        Some(PathBuf::from("/fake/pixtuoid-hook")),
    )
    .unwrap_err();
    // OpenClaw's parse guard words itself differently (its config is JSON5, so a
    // document our strict parser rejects may be perfectly valid).
    assert!(
        format!("{err:#}").contains("will not rewrite the file"),
        "the bail must come from the parse guard, got: {err:#}"
    );
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), before);
    assert!(
        !oc_home.path().join("plugins").exists(),
        "a malformed-config bail must not leave orphan plugin artifacts on disk"
    );
}

#[test]
fn verify_target_is_sound_after_a_real_install_for_every_target() {
    let _env = crate::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let oc_home = tempfile::TempDir::new().unwrap();
    let _state = EnvVarOverride::set("OPENCLAW_STATE_DIR", oc_home.path());
    let exe = std::env::current_exe().unwrap(); // a real, executable file
    for &t in target::TARGETS {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("cfg");
        install_target(t, Some(cfg.clone()), Some(exe.clone())).unwrap();
        let v = verify_target(t, Some(cfg));
        assert!(
            v.is_sound(),
            "{}: a fresh install must verify sound, got issues {:?}",
            t.name,
            v.issues
        );
    }
}

#[test]
fn verify_target_flags_a_missing_shim_binary() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("settings.json");
    let ghost = tmp.path().join("ghost-pixtuoid-hook");
    install_target(&CLAUDE, Some(cfg.clone()), Some(ghost)).unwrap();
    let v = verify_target(&CLAUDE, Some(cfg));
    assert!(!v.is_sound());
    assert!(
        v.issues.iter().any(|i| i.contains("shim binary missing")),
        "{:?}",
        v.issues
    );
}

#[test]
fn verify_target_flags_an_empty_config_as_not_installed() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("settings.json");
    std::fs::write(&cfg, "   \n").unwrap();
    let v = verify_target(&CLAUDE, Some(cfg));
    assert!(!v.is_sound());
    assert!(
        v.issues.iter().any(|i| i.contains("config is empty")),
        "{:?}",
        v.issues
    );
}

// A DIRECTORY exists, so read_config's missing-file early-Ok doesn't apply.
#[test]
fn verify_target_flags_an_unreadable_config() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("cfgdir");
    std::fs::create_dir_all(&dir).unwrap();
    let v = verify_target(&CLAUDE, Some(dir));
    assert!(!v.is_sound());
    assert!(
        v.issues.iter().any(|i| i.contains("config unreadable")),
        "{:?}",
        v.issues
    );
}

// CODEX embeds an ABSOLUTE shim path, so a shim file present with no exec bits
// reaches the not-executable arm rather than the missing-shim one.
#[cfg(unix)]
#[test]
fn verify_target_flags_a_non_executable_shim() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("config.toml");
    let shim = tmp.path().join("hook");
    std::fs::write(&shim, b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o644)).unwrap();

    install_target(&CODEX, Some(cfg.clone()), Some(shim)).unwrap();
    let v = verify_target(&CODEX, Some(cfg));
    assert!(!v.is_sound());
    assert!(
        v.issues
            .iter()
            .any(|i| i.contains("shim binary not executable")),
        "{:?}",
        v.issues
    );
}

// INVARIANT (#387): a config can verify clean while the plugin FILES the runtime
// actually loads are missing or clobbered — the silent-dead class doctor exists
// to catch. Loops EVERY `extra_artifacts` target, and deletes the artifacts each
// target itself declares, so a new code-shipping path in `install_target` with
// no matching check in `verify_target` fails here.
#[test]
fn verify_target_hard_flags_a_missing_code_artifact_for_every_extra_artifacts_target() {
    let _env = crate::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let oc_home = tempfile::TempDir::new().unwrap();
    let _state = EnvVarOverride::set("OPENCLAW_STATE_DIR", oc_home.path());
    let exe = std::env::current_exe().unwrap();
    let mut covered = 0;
    for &t in target::TARGETS {
        let Some(make) = t.extra_artifacts else {
            continue;
        };
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("config");
        install_target(t, Some(cfg.clone()), Some(exe.clone())).unwrap();
        assert!(
            verify_target(t, Some(cfg.clone())).is_sound(),
            "{}: a fresh install must verify sound",
            t.name
        );
        for (p, _) in make(&exe).unwrap() {
            let _ = std::fs::remove_file(&p).or_else(|_| std::fs::remove_dir_all(&p));
        }
        let v = verify_target(t, Some(cfg));
        // Form-agnostic on purpose: the INVARIANT is a hard issue naming the
        // artifacts, not a fixed sentence.
        assert!(
            !v.is_sound()
                && v.issues
                    .iter()
                    .any(|i| i.contains("artifact") && i.contains("missing")),
            "{}: a missing code artifact must be a HARD verify issue (the silent-dead \
             invariant) — got {:?}",
            t.name,
            v.issues
        );
        covered += 1;
    }
    assert!(
        covered >= 1,
        "expected at least one extra_artifacts target (OpenClaw) — did the registry change?"
    );
}

/// The collapse itself, which the sweep above deliberately cannot see: it accepts
/// BOTH forms, so nothing there pins the shortening.
#[test]
fn same_dir_artifact_misses_collapse_to_one_short_line_scattered_ones_do_not() {
    let dir = std::path::Path::new("/o/plugins/pixtuoid");

    assert!(missing_artifact_issue(&[]).is_empty());

    let one = missing_artifact_issue(&[dir.join("index.js")]);
    assert_eq!(one.len(), 1);
    assert!(
        one[0].starts_with("plugin artifact missing:") && one[0].contains("index.js"),
        "a lone miss names its own path — got {one:?}"
    );

    let same: Vec<PathBuf> = ["index.js", "package.json", "pixtuoid-hook"]
        .iter()
        .map(|n| dir.join(n))
        .collect();
    let collapsed = missing_artifact_issue(&same);
    assert_eq!(collapsed.len(), 1, "same-dir misses are ONE fact");
    let line = &collapsed[0];
    assert!(
        line.starts_with("3 plugin artifacts missing from ")
            && line.contains("/o/plugins/pixtuoid"),
        "the collapsed line counts them and names the dir ONCE — got {line}"
    );
    for n in ["index.js", "package.json", "pixtuoid-hook"] {
        assert!(line.contains(n), "{n} must still be named — got {line}");
    }
    let dir_str = dir.to_string_lossy().into_owned();
    assert_eq!(
        line.matches(&dir_str).count(),
        1,
        "the shared dir appears exactly once — got {line}"
    );
    let per_path: Vec<String> = same
        .iter()
        .map(|p| format!("plugin artifact missing: {}", p.display()))
        .collect();
    assert_eq!(
        per_path.iter().filter(|i| i.contains(&dir_str)).count(),
        same.len(),
        "the form being replaced repeats it per path (that is the cost)"
    );
    assert!(
        line.chars().count() < per_path.iter().map(|i| i.chars().count()).sum::<usize>(),
        "and the one line is shorter than the {} it replaces — got {} chars",
        same.len(),
        line.chars().count()
    );

    let scattered = vec![dir.join("index.js"), PathBuf::from("/elsewhere/hook")];
    let scattered_issues = missing_artifact_issue(&scattered);
    assert_eq!(
        scattered_issues.len(),
        2,
        "scattered misses stay one line each"
    );
    assert!(
        scattered_issues
            .iter()
            .all(|i| i.starts_with("plugin artifact missing:")),
        "got {scattered_issues:?}"
    );
}

/// WHY `has_hooks`' empty-config guard is only a fast path: every target's
/// uninstall merge already reports an empty document as unchanged. A future target
/// that claimed `changed` for an empty config would read as INSTALLED without the
/// guard, making it load-bearing — and reds here instead.
#[test]
fn no_targets_uninstall_merge_claims_a_change_on_an_empty_config() {
    for t in crate::install::target::TARGETS {
        let changed = (t.merge_uninstall)("").map(|o| o.changed);
        assert!(
            matches!(changed, Ok(false)),
            "{}: an empty config bears no hooks to remove — got {changed:?}",
            t.name
        );
    }
}

// The silent-dead class the EXISTENCE stat above is blind to: every artifact is
// present and the plugin loads, but the shim path BAKED INTO the entry module
// points at a binary that moved. The plugin swallows spawn errors by design, so
// the mascot never appears while doctor reports the source healthy.
#[test]
fn verify_target_hard_flags_a_moved_baked_shim_for_every_extra_artifacts_target() {
    let _env = crate::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let oc_home = tempfile::TempDir::new().unwrap();
    let _state = EnvVarOverride::set("OPENCLAW_STATE_DIR", oc_home.path());
    let mut covered = 0;
    for &t in target::TARGETS {
        if t.extra_artifacts.is_none() {
            continue;
        }
        let shim_dir = tempfile::TempDir::new().unwrap();
        let shim = shim_dir.path().join("pixtuoid-hook");
        std::fs::copy(std::env::current_exe().unwrap(), &shim).unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("config");
        install_target(t, Some(cfg.clone()), Some(shim.clone())).unwrap();
        assert!(
            verify_target(t, Some(cfg.clone())).is_sound(),
            "{}: a fresh install with a real shim must verify sound",
            t.name
        );
        std::fs::remove_file(&shim).unwrap();
        let v = verify_target(t, Some(cfg));
        assert!(
            !v.is_sound() && v.issues.iter().any(|i| i.contains("shim binary missing")),
            "{}: a moved baked shim must be a HARD verify issue — got {:?} / notes {:?}",
            t.name,
            v.issues,
            v.notes
        );
        covered += 1;
    }
    assert!(
        covered >= 1,
        "expected at least one extra_artifacts target (OpenClaw) — did the registry change?"
    );
}

#[test]
fn reinstall_heals_a_deleted_extra_artifact_even_on_a_config_no_op() {
    let _env = crate::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let oc_home = tempfile::TempDir::new().unwrap();
    let _state = EnvVarOverride::set("OPENCLAW_STATE_DIR", oc_home.path());
    let exe = std::env::current_exe().unwrap();
    let mut covered = 0;
    for &t in target::TARGETS {
        let Some(make) = t.extra_artifacts else {
            continue;
        };
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("config");
        install_target(t, Some(cfg.clone()), Some(exe.clone())).unwrap();

        let (victim, want) = make(&exe).unwrap().into_iter().next().unwrap();
        std::fs::remove_file(&victim).unwrap();
        assert!(
            !victim.exists(),
            "{}: precondition — artifact deleted",
            t.name
        );

        let r = install_target(t, Some(cfg), Some(exe.clone())).unwrap();
        assert!(
            matches!(r.outcome, InstallOutcome::AlreadyUpToDate),
            "{}: config already current — the heal must fire despite the no-op",
            t.name
        );
        // The artifact write runs BEFORE the `!changed` early-return; moving it
        // after would leave the deleted file gone.
        assert!(
            victim.exists(),
            "{}: a no-op re-install must re-create the deleted plugin file",
            t.name
        );
        assert_eq!(
            std::fs::read_to_string(&victim).unwrap(),
            want,
            "{}: the healed artifact must carry the correct baked content",
            t.name
        );
        covered += 1;
    }
    assert!(
        covered >= 1,
        "expected at least one extra_artifacts target (OpenClaw) — did the registry change?"
    );
}

#[test]
fn verify_target_flags_a_missing_event() {
    let exe = std::env::current_exe().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("settings.json");
    install_target(&CLAUDE, Some(cfg.clone()), Some(exe)).unwrap();
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    v["hooks"].as_object_mut().unwrap().remove("SessionEnd");
    std::fs::write(&cfg, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    let res = verify_target(&CLAUDE, Some(cfg));
    assert!(!res.is_sound());
    assert!(
        res.issues
            .iter()
            .any(|i| i.contains("missing hook entries") && i.contains("SessionEnd")),
        "{:?}",
        res.issues
    );
}

// After a DISCONNECT the doctor/health logic must NOT spuriously flag "broken".
// The protection is the `has_hooks` gate every caller applies.
#[test]
fn a_disconnected_source_is_gated_out_of_the_broken_check() {
    let exe = std::env::current_exe().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("settings.json");
    install_target(&CLAUDE, Some(cfg.clone()), Some(exe)).unwrap();
    uninstall_target(&CLAUDE, Some(cfg.clone())).unwrap();
    let content = io::read_config(&cfg).unwrap();
    assert!(
        !(CLAUDE.merge_uninstall)(&content).unwrap().changed,
        "uninstalled config must report no managed hooks (the has_hooks gate)"
    );
    assert!(
        !verify_target(&CLAUDE, Some(cfg)).is_sound(),
        "ungated verify of an uninstalled config is broken — the gate is what protects it"
    );
}

#[test]
fn verify_target_flags_codewhale_disabled() {
    // CodeWhale gates ALL hooks on [hooks].enabled, so false-with-entries is a
    // silent-dead the sentinel/event-set checks would miss.
    let exe = std::env::current_exe().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("config.toml");
    install_target(&target::CODEWHALE, Some(cfg.clone()), Some(exe)).unwrap();
    let content = std::fs::read_to_string(&cfg)
        .unwrap()
        .replace("enabled = true", "enabled = false");
    std::fs::write(&cfg, content).unwrap();
    let v = verify_target(&target::CODEWHALE, Some(cfg));
    assert!(!v.is_sound());
    assert!(
        v.issues.iter().any(|i| i.contains("enabled = false")),
        "{:?}",
        v.issues
    );
}

#[test]
fn json_value_equality_ignores_key_order_under_preserve_order() {
    // `preserve_order` swaps serde_json's `Map` to IndexMap so a merge re-emits the
    // user's key order. `Value: PartialEq` must therefore stay order-INDEPENDENT:
    // every target's `changed` flag is a semantic diff, so order-SENSITIVE equality
    // would report `changed` for a mere re-order — rewriting the file, taking a
    // backup, and flipping `has_hooks` on every single connect.
    let a: serde_json::Value = serde_json::from_str(r#"{"b":1,"a":{"y":2,"x":3}}"#).unwrap();
    let b: serde_json::Value = serde_json::from_str(r#"{"a":{"x":3,"y":2},"b":1}"#).unwrap();
    assert_eq!(a, b, "object equality must not depend on key order");
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        r#"{"b":1,"a":{"y":2,"x":3}}"#,
        "the user's key order must survive the round-trip"
    );
    assert_ne!(
        serde_json::json!([1, 2]),
        serde_json::json!([2, 1]),
        "array order is data and must still compare unequal"
    );
}

#[test]
fn a_config_we_cannot_parse_but_never_wrote_is_not_reported_as_installed() {
    // The JSON5 trap: OpenClaw's config is legal JSON5, so a user's comment makes
    // our strict merge Err — and a conservative `unwrap_or(true)` then claims hooks
    // are INSTALLED for someone who never connected, whom doctor then tells to
    // reconnect (an unsatisfiable remedy the merge refuses by design). The honest
    // fallback asks whether the document mentions us at all.
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("openclaw.json");

    // Valid JSON5, strict-JSON-rejected, and NOT ours.
    std::fs::write(
        &cfg,
        "{\n  // my gateway notes\n  \"gateway\": { \"port\": 18789 },\n}\n",
    )
    .unwrap();
    assert!(
        !has_hooks(&target::OPENCLAW, Some(cfg.clone())),
        "a config we cannot parse and never wrote must not count as installed"
    );

    std::fs::write(
        &cfg,
        "{\n  // mine\n  \"plugins\": { \"entries\": { \"pixtuoid\": { \"enabled\": true } } },\n}\n",
    )
    .unwrap();
    assert!(
        has_hooks(&target::OPENCLAW, Some(cfg.clone())),
        "an unparseable config that names us still bears hooks"
    );

    let dir_cfg = tmp.path().join("as-a-dir.json");
    std::fs::create_dir(&dir_cfg).unwrap();
    assert!(
        has_hooks(&target::OPENCLAW, Some(dir_cfg)),
        "an unreadable config keeps the conservative default"
    );
}

#[test]
fn every_target_that_writes_a_config_names_us_in_it() {
    // The invariant `has_hooks`'s unparseable-config fallback rests on: a config we
    // wrote mentions us, so a substring probe answers "is this ours?" when the parse
    // fails. The fixture shim must therefore be named `pixtuoid-hook`, as in prod.
    let tmpdir = tempfile::TempDir::new().unwrap();
    let hook = tmpdir.path().join("pixtuoid-hook");
    std::fs::write(&hook, b"#!/bin/sh\n").unwrap();
    for t in crate::install::TARGETS {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join(format!("{}-cfg", t.name));
        install_target(t, Some(cfg.clone()), Some(hook.clone()))
            .unwrap_or_else(|e| panic!("{}: install failed: {e:#}", t.name));
        let content = std::fs::read_to_string(&cfg)
            .unwrap_or_else(|e| panic!("{}: config unreadable: {e}", t.name));
        assert!(
            super::config_mentions_us(&content),
            "{}: a config we wrote must satisfy the PRODUCTION fallback predicate",
            t.name
        );
        assert!(
            has_hooks(t, Some(cfg)),
            "{}: has_hooks must see the install it just wrote",
            t.name
        );
    }
}

/// The fallback's ONE marker-carrier that is not the shim path: kimi ships no
/// `_pixtuoid` sentinel, so where the embedded path happens not to name us its only
/// trace is the UPPERCASE `PIXTUOID_SOURCE=kimi` — which is why `config_mentions_us`
/// folds case. Unix-only by construction: Windows uses the bare exec form, so there
/// a marker-less path leaves no trace and the probe answers "not ours" (safe — it
/// under-reports on a CORRUPT config rather than inventing "install broken").
#[cfg(unix)]
#[test]
fn kimis_uppercase_env_marker_alone_satisfies_the_fallback_probe() {
    let tmp = tempfile::TempDir::new().unwrap();
    // A shim path that deliberately does NOT contain our name, so the env prefix is
    // the only carrier left; without the case fold this reds.
    let hook = tmp.path().join("HOOK-shim");
    std::fs::write(&hook, b"#!/bin/sh\n").unwrap();
    let cfg = tmp.path().join("kimi-cfg");
    install_target(&target::KIMI, Some(cfg.clone()), Some(hook)).expect("kimi install");
    let content = std::fs::read_to_string(&cfg).expect("config readable");
    assert!(
        content.contains("PIXTUOID_SOURCE=kimi"),
        "kimi's only marker here is the env prefix: {content}"
    );
    assert!(
        !content.contains(PLUGIN_MENTION),
        "fixture must not leak the lowercase form, or it cannot pin the fold"
    );
    assert!(
        super::config_mentions_us(&content),
        "the fallback must recognise the UPPERCASE marker"
    );
}
