#[test]
fn audio_resolve_clamps_and_defaults() {
    // absent table → MUTED, full volume: the office starts silent and
    // `m` is the one opt-in (#633 — the enabled knob was owner-cut)
    let cfg = AppConfig::default();
    let a = resolve_audio(&cfg);
    assert!(a.muted, "audio starts muted (strictly opt-in via m)");
    assert_eq!(a.volume, 1.0);

    // explicit unmute + out-of-range volumes clamp BOTH sides
    let mut cfg = AppConfig {
        audio: Some(AudioConfigRaw {
            muted: Some(false),
            volume: Some(-0.5),
        }),
        ..Default::default()
    };
    let a = resolve_audio(&cfg);
    assert!(!a.muted);
    assert_eq!(a.volume, 0.0, "negative volume clamps up");
    cfg.audio = Some(AudioConfigRaw {
        muted: Some(false),
        volume: Some(1.5),
    });
    assert_eq!(resolve_audio(&cfg).volume, 1.0, "over-1 clamps down");
    // partial table: muted without volume keeps the default
    cfg.audio = Some(AudioConfigRaw {
        muted: Some(false),
        volume: None,
    });
    assert_eq!(resolve_audio(&cfg).volume, 1.0);
}

#[test]
fn audio_table_round_trips_through_toml() {
    let toml = "[audio]\nmuted = false\nvolume = 0.4\n";
    let cfg: AppConfig = toml::from_str(toml).expect("parses");
    let a = resolve_audio(&cfg);
    assert!(!a.muted);
    assert!((a.volume - 0.4).abs() < 1e-6);

    // the retired (never-released) `enabled` key is an unknown key:
    // ignored like any other, and it does NOT unmute
    let toml = "[audio]\nenabled = true\n";
    let cfg: AppConfig = toml::from_str(toml).expect("unknown keys tolerated");
    assert!(
        resolve_audio(&cfg).muted,
        "a leftover enabled=true stays muted"
    );
}

#[test]
fn save_audio_muted_persists_and_preserves_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "# my config\ntheme = \"normal\"\n[audio]\nvolume = 0.4\n",
    )
    .unwrap();
    save_audio_muted(&path, false).unwrap();
    let s = std::fs::read_to_string(&path).unwrap();
    assert!(s.contains("muted = false"));
    assert!(s.contains("# my config"), "comments survive");
    assert!(s.contains("volume = 0.4"), "sibling keys survive");
    let cfg: AppConfig = toml::from_str(&s).unwrap();
    assert!(!resolve_audio(&cfg).muted);
    // flip back
    save_audio_muted(&path, true).unwrap();
    let cfg: AppConfig = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(resolve_audio(&cfg).muted);
}

#[test]
fn save_audio_volume_persists_the_nudged_level() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[audio]\nmuted = false\n").unwrap();
    save_audio_volume(&path, 0.65).unwrap();
    let cfg: AppConfig = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let a = resolve_audio(&cfg);
    assert!((a.volume - 0.65).abs() < 1e-6);
    assert!(!a.muted, "the sibling muted key survives");
}

use super::*;

#[test]
fn load_missing_returns_defaults() {
    let cfg = load(Path::new("/nonexistent/path/config.toml"), &mut Vec::new());
    assert!(cfg.theme.is_none());
}

// Exercises update_config's write path (now an OpenOptions write + fsync
// before the atomic rename): content must round-trip and leave no tmp
// sidecar behind.
#[test]
fn save_then_load_roundtrips_and_leaves_no_tmp_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config.toml");
    save(&p, "cyberpunk").expect("save");
    let cfg = load(&p, &mut Vec::new());
    assert_eq!(cfg.theme.as_deref(), Some("cyberpunk"));
    assert!(
        !p.with_extension("toml.tmp").exists(),
        "the tmp sidecar must be consumed by the atomic rename"
    );
}

// --- collected warnings (#87): the resolvers stay layer-clean and the
// caller (main) picks the sink, so the COLLECTION is the contract. -----

#[test]
fn load_missing_collects_no_warning() {
    let mut w = Vec::new();
    load(Path::new("/nonexistent/path/config.toml"), &mut w);
    assert!(w.is_empty(), "a missing config is normal, not a warning");
}

#[test]
fn load_malformed_collects_reset_warning() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config.toml");
    std::fs::write(&p, "theme = [unclosed").unwrap();
    let mut w = Vec::new();
    load(&p, &mut w);
    assert_eq!(w.len(), 1);
    assert!(
        w[0].contains("malformed config") && w[0].contains("ALL settings reset"),
        "the all-settings-reset case is the highest-stakes warning: {w:?}"
    );
}

#[test]
fn resolve_theme_collects_unknown_config_theme_warning() {
    let cfg = AppConfig {
        theme: Some("not-a-theme".into()),
        ..AppConfig::default()
    };
    let mut w = Vec::new();
    let theme = resolve_theme(&cfg, None, &mut w).unwrap();
    assert_eq!(theme.name, "normal", "falls back");
    assert_eq!(w.len(), 1);
    assert!(w[0].contains("unknown theme \"not-a-theme\""), "got: {w:?}");
}

#[test]
fn resolve_pets_collects_unknown_kind_warnings() {
    let cfg = AppConfig {
        pets: Some(vec![
            PetEntry {
                kind: Some("hamster".into()),
                name: None,
            },
            PetEntry {
                kind: None,
                name: Some("Rex".into()),
            },
        ]),
        ..AppConfig::default()
    };
    let mut w = Vec::new();
    let pets = resolve_pets(&cfg, &mut w);
    assert!(pets.is_empty());
    assert_eq!(
        w.len(),
        3,
        "one per skipped stanza + the all-unknown summary: {w:?}"
    );
    assert!(w[0].contains("hamster"), "got: {w:?}");
    assert!(w[1].contains("<missing>"), "got: {w:?}");
    assert!(w[2].contains("no pets will appear"), "got: {w:?}");
}

// config_path reads process-global env, so save+restore both vars and drive
// the three branches in one test. The TEST_ENV_LOCK serializes against the
// binary's OTHER env-mutating tests (the install/* HOME/USERPROFILE tests) so
// they can't race under plain `cargo test`. (The embedded_pack XDG test that
// used to share this lock moved to the pixtuoid-scene crate, which has its own.)
#[test]
fn config_path_xdg_home_and_relative_branches() {
    let _env = crate::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let saved_xdg = std::env::var_os("XDG_CONFIG_HOME");
    let saved_home = std::env::var_os("HOME");
    let saved_userprofile = std::env::var_os("USERPROFILE");

    // Clear USERPROFILE for the whole test: on Windows it outranks HOME
    // in user_home(), so both the HOME arm and the relative-fallback arm
    // need it absent to assert their branches.
    std::env::remove_var("USERPROFILE");

    // XDG_CONFIG_HOME wins when set to an ABSOLUTE path — platform-specific,
    // since a leading-slash path is not absolute on Windows (no drive prefix).
    let abs_xdg = if cfg!(windows) {
        "C:/xdg/base"
    } else {
        "/xdg/base"
    };
    std::env::set_var("XDG_CONFIG_HOME", abs_xdg);
    std::env::set_var("HOME", "/home/u");
    assert_eq!(
        config_path(),
        PathBuf::from(abs_xdg).join("pixtuoid").join("config.toml")
    );

    // Empty/whitespace/relative XDG is invalid (XDG spec) → falls to
    // $HOME/.config, never the CWD-relative `pixtuoid/config.toml`.
    for invalid in ["", "   ", "rel/xdg"] {
        std::env::set_var("XDG_CONFIG_HOME", invalid);
        assert_eq!(
            config_path(),
            PathBuf::from("/home/u/.config/pixtuoid/config.toml"),
            "invalid XDG_CONFIG_HOME {invalid:?} must fall to $HOME/.config"
        );
    }

    // No XDG → fall back to $HOME/.config.
    std::env::remove_var("XDG_CONFIG_HOME");
    assert_eq!(
        config_path(),
        PathBuf::from("/home/u/.config/pixtuoid/config.toml")
    );

    // Neither → relative fallback.
    std::env::remove_var("HOME");
    assert_eq!(config_path(), PathBuf::from(".config/pixtuoid/config.toml"));

    // Restore.
    match saved_xdg {
        Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
        None => std::env::remove_var("XDG_CONFIG_HOME"),
    }
    match saved_home {
        Some(v) => std::env::set_var("HOME", v),
        None => std::env::remove_var("HOME"),
    }
    match saved_userprofile {
        Some(v) => std::env::set_var("USERPROFILE", v),
        None => std::env::remove_var("USERPROFILE"),
    }
}

// load()'s non-NotFound read-error arm: pointing at a DIRECTORY makes
// read_to_string error (IsADirectory) → warn + return defaults (never crash).
#[test]
fn load_unreadable_path_returns_defaults() {
    let dir = tempfile::tempdir().unwrap();
    // The directory itself is an existing, non-NotFound, unreadable "file".
    let cfg = load(dir.path(), &mut Vec::new());
    assert!(cfg.theme.is_none());
}

#[test]
fn load_malformed_returns_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "not valid { toml }}}").unwrap();
    let cfg = load(&path, &mut Vec::new());
    assert!(cfg.theme.is_none());
}

#[test]
fn load_partial_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "theme = \"cyberpunk\"\n").unwrap();
    let cfg = load(&path, &mut Vec::new());
    assert_eq!(cfg.theme.as_deref(), Some("cyberpunk"));
}

#[test]
fn load_ignores_unknown_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "theme = \"normal\"\nfuture-key = 42\n").unwrap();
    let cfg = load(&path, &mut Vec::new());
    assert_eq!(cfg.theme.as_deref(), Some("normal"));
}

#[test]
fn save_then_load_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    save(&path, "dracula").unwrap();
    let cfg = load(&path, &mut Vec::new());
    assert_eq!(cfg.theme.as_deref(), Some("dracula"));
}

#[test]
fn resolve_cli_wins_over_config() {
    let cfg = AppConfig {
        theme: Some("normal".into()),
        ..AppConfig::default()
    };
    let theme = resolve_theme(&cfg, Some("dracula"), &mut Vec::new()).unwrap();
    assert_eq!(theme.name, "dracula");
}

#[test]
fn resolve_config_wins_over_default() {
    let cfg = AppConfig {
        theme: Some("gruvbox".into()),
        ..AppConfig::default()
    };
    let theme = resolve_theme(&cfg, None, &mut Vec::new()).unwrap();
    assert_eq!(theme.name, "gruvbox");
}

#[test]
fn resolve_all_none_uses_default() {
    let cfg = AppConfig::default();
    let theme = resolve_theme(&cfg, None, &mut Vec::new()).unwrap();
    assert_eq!(theme.name, "normal");
}

#[test]
fn resolve_invalid_config_theme_falls_back_to_default() {
    let cfg = AppConfig {
        theme: Some("does-not-exist".into()),
        ..AppConfig::default()
    };
    let theme = resolve_theme(&cfg, None, &mut Vec::new()).unwrap();
    assert_eq!(theme.name, "normal");
}

#[test]
fn resolve_invalid_cli_theme_hard_errors() {
    let cfg = AppConfig::default();
    let err = resolve_theme(&cfg, Some("definitely-not-a-theme"), &mut Vec::new()).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("unknown theme"), "got: {msg}");
    for t in pixtuoid_scene::theme::ALL_THEMES {
        assert!(
            msg.contains(t.name),
            "should list every valid theme, missing {:?} in: {msg}",
            t.name
        );
    }
}

#[test]
fn resolve_valid_cli_wins_even_when_config_theme_invalid() {
    let cfg = AppConfig {
        theme: Some("does-not-exist".into()),
        ..AppConfig::default()
    };
    let theme = resolve_theme(&cfg, Some("dracula"), &mut Vec::new()).unwrap();
    assert_eq!(theme.name, "dracula");
}

#[test]
fn resolve_invalid_cli_theme_errors_even_with_valid_config() {
    // A CLI typo must NOT silently fall back to the config theme — explicit
    // user intent on the command line fails loudly.
    let cfg = AppConfig {
        theme: Some("gruvbox".into()),
        ..AppConfig::default()
    };
    assert!(resolve_theme(&cfg, Some("definitely-not-a-theme"), &mut Vec::new()).is_err());
}

#[test]
fn full_config_flow_file_drives_theme() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "theme = \"cyberpunk\"\n").unwrap();
    let cfg = load(&path, &mut Vec::new());
    let theme = resolve_theme(&cfg, None, &mut Vec::new()).unwrap();
    assert_eq!(theme.name, "cyberpunk");
}

#[test]
fn full_config_flow_cli_overrides_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "theme = \"cyberpunk\"\n").unwrap();
    let cfg = load(&path, &mut Vec::new());
    let theme = resolve_theme(&cfg, Some("dracula"), &mut Vec::new()).unwrap();
    assert_eq!(theme.name, "dracula");
}

// --- max-desks cap flow -----------------------------------------------

#[test]
fn max_desks_config_set_no_cli() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "max-desks = 8\n").unwrap();
    let cfg = load(&path, &mut Vec::new());
    let cli_max_desks: Option<usize> = None;
    let mut w = Vec::new();
    let desk_cap = cli_max_desks.or(resolve_max_desks(&cfg, &mut w));
    assert_eq!(desk_cap, Some(8));
    assert!(w.is_empty(), "a valid cap collects no warning: {w:?}");
}

#[test]
fn max_desks_cli_overrides_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "max-desks = 8\n").unwrap();
    let cfg = load(&path, &mut Vec::new());
    let cli_max_desks: Option<usize> = Some(4);
    let desk_cap = cli_max_desks.or(resolve_max_desks(&cfg, &mut Vec::new()));
    assert_eq!(desk_cap, Some(4));
}

#[test]
fn max_desks_neither_set() {
    let cfg = AppConfig::default();
    let cli_max_desks: Option<usize> = None;
    let desk_cap = cli_max_desks.or(resolve_max_desks(&cfg, &mut Vec::new()));
    assert_eq!(desk_cap, None);
}

#[test]
fn max_desks_no_config_file() {
    let cfg = load(Path::new("/nonexistent/path/config.toml"), &mut Vec::new());
    let cli_max_desks: Option<usize> = None;
    let desk_cap = cli_max_desks.or(resolve_max_desks(&cfg, &mut Vec::new()));
    assert_eq!(desk_cap, None);
}

#[test]
fn max_desks_zero_in_config_is_ignored_with_warning() {
    // 0 would permanently zero every floor (the per-frame re-seed guards
    // `capacity > 0`, so the boot atomics never grow) — every agent
    // silently dropped. The config seam must degrade to auto capacity
    // and say so on the #87 warning channel.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "max-desks = 0\n").unwrap();
    let cfg = load(&path, &mut Vec::new());
    assert_eq!(cfg.max_desks, Some(0), "the raw key still deserializes");
    let mut w = Vec::new();
    assert_eq!(resolve_max_desks(&cfg, &mut w), None, "0 resolves to unset");
    assert_eq!(w.len(), 1);
    assert!(
        w[0].contains("max-desks = 0"),
        "the warning names the bad key: {w:?}"
    );
}

#[test]
fn save_preserves_max_desks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "theme = \"normal\"\nmax-desks = 8\n").unwrap();
    save(&path, "cyberpunk").unwrap();
    let cfg = load(&path, &mut Vec::new());
    assert_eq!(cfg.theme.as_deref(), Some("cyberpunk"));
    assert_eq!(cfg.max_desks, Some(8));
}

// --- pack-dir resolution -----------------------------------------------

#[test]
fn pack_dir_cli_wins_over_config() {
    let cfg = AppConfig {
        pack_dir: Some("/config/pack".into()),
        ..AppConfig::default()
    };
    let result = resolve_pack_dir(&cfg, Some(PathBuf::from("/cli/pack")));
    assert_eq!(result, Some(PathBuf::from("/cli/pack")));
}

#[test]
fn pack_dir_config_used_when_no_cli() {
    let cfg = AppConfig {
        pack_dir: Some("/config/pack".into()),
        ..AppConfig::default()
    };
    let result = resolve_pack_dir(&cfg, None);
    assert_eq!(result, Some(PathBuf::from("/config/pack")));
}

#[test]
fn pack_dir_neither_returns_none() {
    let cfg = AppConfig::default();
    let result = resolve_pack_dir(&cfg, None);
    assert_eq!(result, None);
}

#[test]
fn pack_dir_config_expands_tilde() {
    let cfg = AppConfig {
        pack_dir: Some("~/my-pack".into()),
        ..AppConfig::default()
    };
    let result = resolve_pack_dir(&cfg, None);
    // Build the expectation with the SAME `.join()` the impl uses
    // (install::io::expand_tilde) so the comparison is STRUCTURAL — a
    // hardcoded `/` would drift from `\` under the Windows runner's Git Bash.
    match pixtuoid_core::platform::user_home_opt() {
        Some(home) => {
            assert_eq!(result, Some(PathBuf::from(home).join("my-pack")));
        }
        None => assert_eq!(result, Some(PathBuf::from("~/my-pack"))),
    }
}

#[test]
fn pack_dir_loaded_from_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "pack-dir = \"/custom/sprites\"\n").unwrap();
    let cfg = load(&path, &mut Vec::new());
    assert_eq!(cfg.pack_dir.as_deref(), Some("/custom/sprites"));
}

// --- [[pets]] config ----------------------------------------------------

#[test]
fn pets_absent_returns_all_with_default_names() {
    let cfg = AppConfig::default();
    let pets = resolve_pets(&cfg, &mut Vec::new());
    assert_eq!(pets.len(), pixtuoid_scene::pet::PetKind::ALL.len());
    for pet in &pets {
        assert_eq!(pet.name, pet.kind.default_name());
    }
}

#[test]
fn pets_empty_vec_returns_none() {
    let cfg = AppConfig {
        pets: Some(vec![]),
        ..AppConfig::default()
    };
    assert!(resolve_pets(&cfg, &mut Vec::new()).is_empty());
}

#[test]
fn pets_unknown_kind_warns_and_skips() {
    let cfg = AppConfig {
        pets: Some(vec![
            PetEntry {
                kind: Some("cat".into()),
                name: None,
            },
            PetEntry {
                kind: Some("hamster".into()),
                name: None,
            },
        ]),
        ..AppConfig::default()
    };
    let pets = resolve_pets(&cfg, &mut Vec::new());
    assert_eq!(pets.len(), 1);
    assert_eq!(pets[0].kind, pixtuoid_scene::pet::PetKind::Cat);
    assert_eq!(pets[0].name, "Office Cat");
}

#[test]
fn pets_all_unknown_returns_empty() {
    let cfg = AppConfig {
        pets: Some(vec![
            PetEntry {
                kind: Some("hamster".into()),
                name: None,
            },
            PetEntry {
                kind: Some("parrot".into()),
                name: None,
            },
        ]),
        ..AppConfig::default()
    };
    assert!(resolve_pets(&cfg, &mut Vec::new()).is_empty());
}

#[test]
fn pets_entry_custom_name_attached() {
    let cfg = AppConfig {
        pets: Some(vec![
            PetEntry {
                kind: Some("cat".into()),
                name: Some("Whiskers".into()),
            },
            PetEntry {
                kind: Some("dog".into()),
                name: Some("Rex".into()),
            },
        ]),
        ..AppConfig::default()
    };
    let pets = resolve_pets(&cfg, &mut Vec::new());
    let name = |k| pets.iter().find(|p| p.kind == k).map(|p| p.name.as_str());
    assert_eq!(name(pixtuoid_scene::pet::PetKind::Cat), Some("Whiskers"));
    assert_eq!(name(pixtuoid_scene::pet::PetKind::Dog), Some("Rex"));
}

#[test]
fn pets_entry_absent_name_falls_back_to_default() {
    let cfg = AppConfig {
        pets: Some(vec![PetEntry {
            kind: Some("dog".into()),
            name: None,
        }]),
        ..AppConfig::default()
    };
    assert_eq!(resolve_pets(&cfg, &mut Vec::new())[0].name, "Office Dog");
}

#[test]
fn pets_entry_name_trimmed_empty_falls_back() {
    let cfg = AppConfig {
        pets: Some(vec![
            PetEntry {
                kind: Some("cat".into()),
                name: Some("  Mittens  ".into()),
            },
            PetEntry {
                kind: Some("dog".into()),
                name: Some("   ".into()), // whitespace-only → default
            },
        ]),
        ..AppConfig::default()
    };
    let pets = resolve_pets(&cfg, &mut Vec::new());
    let name = |k| pets.iter().find(|p| p.kind == k).map(|p| p.name.as_str());
    assert_eq!(name(pixtuoid_scene::pet::PetKind::Cat), Some("Mittens"));
    assert_eq!(name(pixtuoid_scene::pet::PetKind::Dog), Some("Office Dog"));
}

#[test]
fn pets_loaded_from_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[[pets]]\nkind = \"dog\"\n").unwrap();
    let cfg = load(&path, &mut Vec::new());
    assert_eq!(
        cfg.pets,
        Some(vec![PetEntry {
            kind: Some("dog".into()),
            name: None
        }])
    );
}

#[test]
fn pets_full_toml_resolves_names() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[[pets]]\nkind = \"cat\"\nname = \"Luna\"\n\n[[pets]]\nkind = \"dog\"\n",
    )
    .unwrap();
    let cfg = load(&path, &mut Vec::new());
    let pets = resolve_pets(&cfg, &mut Vec::new());
    assert_eq!(pets.len(), 2);
    let name = |k| pets.iter().find(|p| p.kind == k).map(|p| p.name.as_str());
    assert_eq!(name(pixtuoid_scene::pet::PetKind::Cat), Some("Luna"));
    assert_eq!(name(pixtuoid_scene::pet::PetKind::Dog), Some("Office Dog"));
}

#[test]
fn save_preserves_pets() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "theme = \"normal\"\n[[pets]]\nkind = \"cat\"\nname = \"Luna\"\n",
    )
    .unwrap();
    save(&path, "cyberpunk").unwrap();
    let cfg = load(&path, &mut Vec::new());
    assert_eq!(cfg.theme.as_deref(), Some("cyberpunk"));
    assert_eq!(
        cfg.pets,
        Some(vec![PetEntry {
            kind: Some("cat".into()),
            name: Some("Luna".into())
        }])
    );
}

#[test]
fn pets_empty_vec_serializes_as_inline_empty_array() {
    let cfg = AppConfig {
        pets: Some(vec![]),
        ..AppConfig::default()
    };
    let s = toml::to_string_pretty(&cfg).unwrap();
    assert!(s.contains("pets = []"), "expected 'pets = []' in:\n{s}");
    let reloaded: AppConfig = toml::from_str(&s).unwrap();
    assert_eq!(reloaded.pets, Some(vec![]));
}

#[test]
fn pets_section_is_last_in_serialized_toml() {
    // The AoT must serialize after the scalar keys (the must-be-last
    // convention); a scalar after `[[pets]]` would be invalid TOML.
    let cfg = AppConfig {
        theme: Some("normal".into()),
        pets: Some(vec![PetEntry {
            kind: Some("cat".into()),
            name: None,
        }]),
        ..AppConfig::default()
    };
    let s = toml::to_string_pretty(&cfg).unwrap();
    let theme_pos = s.find("theme").expect("theme not in output");
    let pets_pos = s.find("[[pets]]").expect("[[pets]] not in output");
    assert!(theme_pos < pets_pos, "theme must precede [[pets]]:\n{s}");
}

#[test]
fn pets_missing_kind_is_non_fatal() {
    // A `[[pets]]` stanza with no `kind` (user typo) must NOT trip load()'s
    // all-or-nothing malformed arm — the rest of the config survives and the
    // bad stanza is warn-skipped. Regression for the `kind: String` footgun.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "theme = \"cyberpunk\"\n[[pets]]\nname = \"Ghost\"\n\n[[pets]]\nkind = \"cat\"\n",
    )
    .unwrap();
    let cfg = load(&path, &mut Vec::new());
    assert_eq!(
        cfg.theme.as_deref(),
        Some("cyberpunk"),
        "theme must survive a kindless [[pets]] stanza (config not reset)"
    );
    let pets = resolve_pets(&cfg, &mut Vec::new());
    assert_eq!(
        pets.len(),
        1,
        "the kindless stanza is skipped, the cat kept"
    );
    assert_eq!(pets[0].kind, pixtuoid_scene::pet::PetKind::Cat);
}

// --- data safety: malformed-config refusal + one-time backup (#3) ---------

#[test]
fn update_config_refuses_a_type_invalid_config() {
    // Valid TOML syntax but a type-invalid value: the typed `load` fails
    // (resetting to defaults in memory each boot), so persisting over it
    // would make this save "succeed" while never taking effect. Refuse
    // with the same fix-or-delete contract as the syntax-level gate.
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config.toml");
    let original = "theme = \"normal\"\nmax-desks = \"oops\"\n";
    std::fs::write(&p, original).unwrap();
    let err = save(&p, "cyberpunk").expect_err("a type-invalid config must not be persisted");
    assert!(
        format!("{err:#}").contains("invalid values"),
        "error must name the value failure: {err:#}"
    );
    assert_eq!(std::fs::read_to_string(&p).unwrap(), original);
}

#[test]
fn update_config_still_accepts_unknown_keys() {
    // Forward-compat must survive the typed gate: a key written by a
    // newer binary is unknown here but NOT type-invalid.
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config.toml");
    std::fs::write(&p, "future-key = 1\n").unwrap();
    save(&p, "cyberpunk").expect("unknown keys must not block saves");
    let after = std::fs::read_to_string(&p).unwrap();
    assert!(after.contains("future-key = 1"));
    assert!(after.contains("theme = \"cyberpunk\""));
}

#[test]
fn update_config_refuses_to_overwrite_a_malformed_config() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config.toml");
    let original = "theme = [unclosed";
    std::fs::write(&p, original).unwrap();
    let err = save(&p, "cyberpunk").expect_err("a malformed config must not be persisted over");
    let msg = format!("{err:#}");
    assert!(
        msg.contains(&p.display().to_string()) && msg.to_lowercase().contains("toml"),
        "error must name the file and the parse failure: {msg}"
    );
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        original,
        "the file content must be untouched — the user's typo is still fixable"
    );
}

#[test]
fn save_version_refuses_to_overwrite_a_malformed_config() {
    // The boot save_version path (tui/mod.rs) is the automatic trigger that
    // used to wipe a hand-written config on the first boot after a typo.
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config.toml");
    let original = "theme = \"cyberpunk\"\nmax-desks = oops\n";
    std::fs::write(&p, original).unwrap();
    assert!(save_version(&p, "9.9.9").is_err());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), original);
}

#[test]
fn save_backs_up_an_existing_config_once() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config.toml");
    let original = "theme = \"normal\"\nmax-desks = 8\n";
    std::fs::write(&p, original).unwrap();
    let bak = dir.path().join("config.toml.pixtuoid.bak");

    save(&p, "cyberpunk").unwrap();
    assert_eq!(
        std::fs::read_to_string(&bak).unwrap(),
        original,
        "first overwrite of an existing config takes a one-time backup"
    );

    save(&p, "dracula").unwrap();
    assert_eq!(
        std::fs::read_to_string(&bak).unwrap(),
        original,
        "the backup is once — later saves must not churn it"
    );
    assert_eq!(load(&p, &mut Vec::new()).theme.as_deref(), Some("dracula"));
}

#[test]
fn save_on_a_missing_config_creates_it_without_a_backup() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config.toml");
    save(&p, "cyberpunk").unwrap();
    assert!(p.exists());
    assert!(
        !dir.path().join("config.toml.pixtuoid.bak").exists(),
        "nothing existed to back up"
    );
}

// --- format preservation: unknown keys + comments survive a save (#15) ----

#[test]
fn save_preserves_comments_and_unknown_keys_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config.toml");
    let original = "# pixtuoid config — hand-tuned\ntheme = \"normal\"\nfuture-key = 1 # written by a newer pixtuoid\n\n[[pets]]\nkind = \"cat\" # the office cat\n";
    std::fs::write(&p, original).unwrap();

    save(&p, "cyberpunk").unwrap();

    let after = std::fs::read_to_string(&p).unwrap();
    assert_eq!(
        after,
        original.replace("theme = \"normal\"", "theme = \"cyberpunk\""),
        "everything but the mutated key must survive byte-for-byte"
    );
}

#[test]
fn save_version_inserts_new_key_before_pets_section() {
    // A NEW scalar key must land with the other scalars, never after the
    // [[pets]] array-of-tables (which would re-parent it into the pet).
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config.toml");
    std::fs::write(&p, "theme = \"normal\"\n\n[[pets]]\nkind = \"cat\"\n").unwrap();

    save_version(&p, "9.9.9").unwrap();

    let after = std::fs::read_to_string(&p).unwrap();
    let ver_pos = after.find("last-seen-version").expect("key written");
    let pets_pos = after.find("[[pets]]").expect("pets kept");
    assert!(ver_pos < pets_pos, "scalar must precede [[pets]]:\n{after}");
    let cfg = load(&p, &mut Vec::new());
    assert_eq!(cfg.last_seen_version.as_deref(), Some("9.9.9"));
    assert_eq!(
        cfg.pets,
        Some(vec![PetEntry {
            kind: Some("cat".into()),
            name: None
        }])
    );
}

// --- sources / connection flags -------------------------------------------

#[test]
fn sources_table_roundtrips_and_empty_is_omitted() {
    let cfg: AppConfig =
        toml::from_str("theme = \"normal\"\n[sources]\nclaude-code = false\ncodex = true\n")
            .unwrap();
    assert_eq!(cfg.sources.get("claude-code"), Some(&false));
    assert_eq!(cfg.sources.get("codex"), Some(&true));
    assert_eq!(cfg.sources.get("antigravity"), None);
    // An empty map is omitted on serialize (skip_serializing_if).
    let c = AppConfig {
        theme: Some("normal".into()),
        ..Default::default()
    };
    assert!(!toml::to_string(&c).unwrap().contains("[sources]"));
}

#[test]
fn floating_config_defaults_and_explicit_roundtrip() {
    // Absent [floating] → defaults, OS-placed (x/y None), opaque.
    let cfg: AppConfig = toml::from_str("theme = \"normal\"\n").unwrap();
    let f = resolve_floating(&cfg);
    assert_eq!(
        (f.width, f.height),
        (FLOATING_DEFAULT_W, FLOATING_DEFAULT_H)
    );
    assert_eq!((f.x, f.y), (None, None));
    assert!((f.opacity - 1.0).abs() < f32::EPSILON);
    // Explicit values parse through.
    let cfg: AppConfig =
        toml::from_str("[floating]\nwidth = 480\nheight = 300\nx = 10\ny = 20\nopacity = 0.8\n")
            .unwrap();
    let f = resolve_floating(&cfg);
    assert_eq!(
        (f.width, f.height, f.x, f.y),
        (480, 300, Some(10), Some(20))
    );
    assert!((f.opacity - 0.8).abs() < 1e-6);
    // An absent [floating] is omitted on serialize (skip_serializing_if + None).
    assert!(!toml::to_string(&AppConfig::default())
        .unwrap()
        .contains("[floating]"));
}

#[test]
fn save_floating_roundtrips_geometry_and_preserves_other_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "theme = \"normal\"\n").unwrap();
    save_floating(&path, 480, 320, Some(12), Some(34)).unwrap();
    let cfg = load(&path, &mut Vec::new());
    let f = resolve_floating(&cfg);
    assert_eq!(
        (f.width, f.height, f.x, f.y),
        (480, 320, Some(12), Some(34))
    );
    // toml_edit preserves the user's other settings (not an all-or-nothing rewrite).
    assert_eq!(cfg.theme.as_deref(), Some("normal"));
}

#[test]
fn save_floating_clears_stale_position_when_os_cannot_report_it() {
    // A `None` x/y (outer_position() Err — always on Wayland) must DROP the prior coords,
    // not leave them: a new size + stale position would restore an offscreen window.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "theme = \"normal\"\n").unwrap();
    save_floating(&path, 480, 320, Some(12), Some(34)).unwrap();
    // A later save where the OS can't report position: size updates, x/y are cleared.
    save_floating(&path, 500, 360, None, None).unwrap();
    let cfg = load(&path, &mut Vec::new());
    let f = resolve_floating(&cfg);
    assert_eq!((f.width, f.height), (500, 360));
    assert_eq!((f.x, f.y), (None, None), "stale position keys were dropped");
    // Unrelated settings still survive the rewrite.
    assert_eq!(cfg.theme.as_deref(), Some("normal"));
}

#[test]
fn floating_size_clamps_to_legible_min_and_opacity_is_bounded() {
    // Below-min size clamps UP so the office stays legible; over-opacity clamps to 1.0.
    let cfg: AppConfig =
        toml::from_str("[floating]\nwidth = 1\nheight = 1\nopacity = 9.0\n").unwrap();
    let f = resolve_floating(&cfg);
    assert_eq!((f.width, f.height), (FLOATING_MIN_W, FLOATING_MIN_H));
    assert!((f.opacity - 1.0).abs() < f32::EPSILON);
    // Opacity floors at 0.2 (a fully-transparent window is useless).
    let cfg: AppConfig = toml::from_str("[floating]\nopacity = 0.0\n").unwrap();
    assert!((resolve_floating(&cfg).opacity - 0.2).abs() < 1e-6);
}

#[test]
fn resolve_connected_only_explicit_true_connects() {
    let mut cfg = AppConfig::default();
    cfg.sources.insert("claude-code".into(), false);
    cfg.sources.insert("codex".into(), true);
    let set = resolve_connected(&cfg);
    assert!(!set.contains("claude-code"), "explicit false disconnects");
    assert!(set.contains("codex"), "explicit true connects");
    assert!(
        !set.contains("antigravity"),
        "an absent flag is plainly disconnected (no migrate inference)"
    );
}

// The 0.12.0 removal of the v0.4–0.7 migrate inference: an absent/empty
// [sources] means NOTHING connected — an upgrader's old config (exists,
// no [sources], not degraded) reads as a first run, so the onboarding
// wizard replays and they re-connect there (that IS the connect flow).
#[test]
fn resolve_connected_absent_sources_table_connects_nothing() {
    let cfg = AppConfig::default(); // no [sources]
    assert!(
        resolve_connected(&cfg).is_empty(),
        "absent [sources] is the plain default: nothing connected"
    );
}

// A newly-added source must self-gate on first boot: resolve_connected
// iterates the registry, so every REGISTERED_SOURCES entry is decided
// (here, all explicitly connected). A source added to the registry without
// a config flag can't silently fall through — and a flag for an
// UNREGISTERED id never leaks into the set.
#[test]
fn resolve_connected_covers_every_registered_source() {
    let mut cfg = AppConfig::default();
    for src in pixtuoid_core::source::REGISTERED_SOURCES {
        cfg.sources.insert((*src).into(), true);
    }
    cfg.sources.insert("not-a-registered-source".into(), true);
    let set = resolve_connected(&cfg);
    let expected: std::collections::HashSet<String> = pixtuoid_core::source::REGISTERED_SOURCES
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        set, expected,
        "resolve_connected must decide every registered source and only those"
    );
}

#[test]
fn save_source_connected_roundtrips_and_preserves_other_keys() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config.toml");
    std::fs::write(
        &p,
        "# hand-tuned\ntheme = \"normal\"\nfuture-key = 1\n\n[[pets]]\nkind = \"cat\"\n",
    )
    .unwrap();

    save_source_connected(&p, "claude-code", false).unwrap();
    let cfg = load(&p, &mut Vec::new());
    assert_eq!(cfg.sources.get("claude-code"), Some(&false));
    assert_eq!(cfg.theme.as_deref(), Some("normal"), "theme survives");
    assert_eq!(
        cfg.pets,
        Some(vec![PetEntry {
            kind: Some("cat".into()),
            name: None
        }]),
        "pets survive"
    );
    let after = std::fs::read_to_string(&p).unwrap();
    assert!(after.contains("# hand-tuned"), "comment survives");
    assert!(after.contains("future-key = 1"), "unknown key survives");

    // A second flip updates the same key in place.
    save_source_connected(&p, "claude-code", true).unwrap();
    assert_eq!(
        load(&p, &mut Vec::new()).sources.get("claude-code"),
        Some(&true)
    );
}

#[test]
fn remove_source_connected_drops_the_key_and_an_emptied_table() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config.toml");
    std::fs::write(&p, "theme = \"normal\"\n").unwrap();

    save_source_connected(&p, "claude-code", true).unwrap();
    save_source_connected(&p, "codex", true).unwrap();
    remove_source_connected(&p, "claude-code").unwrap();
    let cfg = load(&p, &mut Vec::new());
    assert_eq!(cfg.sources.get("claude-code"), None, "key removed");
    assert_eq!(cfg.sources.get("codex"), Some(&true), "sibling survives");
    assert_eq!(cfg.theme.as_deref(), Some("normal"), "other keys survive");

    // Removing the last key drops the now-empty [sources] table entirely,
    // so a rolled-back first connect leaves no `[sources]` residue (the
    // is_first_run signal reads table emptiness, but an empty table header
    // in the file is still pointless noise).
    remove_source_connected(&p, "codex").unwrap();
    let after = std::fs::read_to_string(&p).unwrap();
    assert!(
        !after.contains("[sources]"),
        "emptied table dropped: {after}"
    );
    // Removing an absent key / from an absent table is a quiet no-op.
    remove_source_connected(&p, "codex").unwrap();
}

// --- write seam parity with install/io.rs (#16) ----------------------------

#[test]
fn save_leaves_the_lock_file_in_place() {
    // Parity with io.rs::write_config_atomic, which deliberately never
    // unlinks its lock file (unlock-then-unlink lets two later writers both
    // "hold" the lock on different inodes).
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config.toml");
    save(&p, "cyberpunk").unwrap();
    assert!(
        dir.path().join("config.toml.lock").exists(),
        "the lock file must stay in place"
    );
}

// --- save_version ---------------------------------------------------------

#[test]
fn save_version_persists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    save_version(&path, "0.4.0").unwrap();
    let cfg = load(&path, &mut Vec::new());
    assert_eq!(cfg.last_seen_version.as_deref(), Some("0.4.0"));
}

#[test]
fn save_version_preserves_theme() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "theme = \"cyberpunk\"\n").unwrap();
    save_version(&path, "0.4.0").unwrap();
    let cfg = load(&path, &mut Vec::new());
    assert_eq!(cfg.theme.as_deref(), Some("cyberpunk"));
    assert_eq!(cfg.last_seen_version.as_deref(), Some("0.4.0"));
}
