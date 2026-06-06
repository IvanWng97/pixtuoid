use std::path::{Path, PathBuf};

use anyhow::Result;

/// One `[[pets]]` stanza. `kind` is an OPTIONAL raw `String` (NOT a required
/// field, NOT a serde-derived `PetKind`) on purpose: an unknown value (`kind =
/// "hamster"`) OR a missing/typo'd key (`knid = "cat"` → `kind` defaults to
/// `None`) is validated + warn-skipped in [`resolve_pets`], rather than failing
/// the whole `toml::from_str` and tripping `load`'s all-or-nothing malformed arm
/// — which would silently revert EVERY user setting (theme, etc.) to defaults.
/// (A wrong-TYPE value like `kind = 5` still fails the parse; not worth a custom
/// deserializer.) `name` is optional; omit it for the pet's default name.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PetEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct AppConfig {
    pub theme: Option<String>,
    /// Optional per-floor desk cap. When set, each floor holds at most
    /// this many desks — excess agents overflow to additional floors.
    /// When absent, capacity is fully auto-computed from terminal size.
    #[serde(rename = "max-desks")]
    pub max_desks: Option<usize>,
    /// Custom sprite pack directory. Supports ~ expansion.
    #[serde(rename = "pack-dir")]
    pub pack_dir: Option<String>,
    #[serde(
        rename = "last-seen-version",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_seen_version: Option<String>,
    /// The office's pets — one `[[pets]]` stanza each (`kind` + optional
    /// `name`). Absent = all kinds with default names; `pets = []` = no pets;
    /// an unknown `kind` is warn-skipped (non-fatal). Resolved into the runtime
    /// `Vec<Pet>` by [`resolve_pets`].
    ///
    /// Keep `pets` LAST in the struct by convention: an array-of-tables
    /// serializes cleanest after all scalar keys (matching where `pet_names`
    /// used to sit). `toml` does not *require* it — it tolerates a scalar after
    /// an AoT — but don't rely on its key/table interleaving; just keep it last.
    #[serde(rename = "pets", default, skip_serializing_if = "Option::is_none")]
    pub pets: Option<Vec<PetEntry>>,
}

pub fn resolve_pack_dir(config: &AppConfig, cli_pack_dir: Option<PathBuf>) -> Option<PathBuf> {
    cli_pack_dir.or_else(|| {
        config
            .pack_dir
            .as_ref()
            .map(|p| PathBuf::from(expand_tilde(p, std::env::var("HOME").ok().as_deref())))
    })
}

/// Expand a leading `~` (current user's home) in a path string. Only `~` alone
/// and a `~/`-prefixed path are expanded — `~user/...` is left untouched (we
/// don't resolve other users' homes) and a non-leading `~` is never replaced.
/// With no `home`, the input is returned unchanged.
fn expand_tilde(p: &str, home: Option<&str>) -> String {
    match home {
        Some(h) if p == "~" => h.to_string(),
        Some(h) if p.starts_with("~/") => format!("{h}{}", &p[1..]),
        _ => p.to_string(),
    }
}

pub fn config_path() -> PathBuf {
    if let Ok(base) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(base).join("pixtuoid").join("config.toml");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".config")
            .join("pixtuoid")
            .join("config.toml");
    }
    PathBuf::from(".config/pixtuoid/config.toml")
}

pub fn load(path: &Path) -> AppConfig {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return AppConfig::default(),
        Err(e) => {
            tracing::warn!(path = %path.display(), %e, "cannot read config — using defaults");
            return AppConfig::default();
        }
    };
    match toml::from_str(&contents) {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::warn!(path = %path.display(), %e, "malformed config — using defaults");
            AppConfig::default()
        }
    }
}

/// Load-modify-write the config atomically. `mutate` is called on the
/// loaded (or default) config to apply changes; the resulting struct is
/// serialized and atomically renamed into place.
///
/// Resolves symlinks so the atomic rename targets the real file, not the
/// symlink itself (critical for stow-managed configs).
fn update_config<F>(path: &Path, mutate: F) -> Result<()>
where
    F: FnOnce(&mut AppConfig),
{
    let real_path = crate::install::io::resolve_symlink(path);
    if let Some(parent) = real_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock_path = real_path.with_extension("toml.lock");
    let lock_file = std::fs::File::create(&lock_path)?;
    fs2::FileExt::try_lock_exclusive(&lock_file)
        .map_err(|e| anyhow::anyhow!("config lock held by another process: {e}"))?;

    let mut cfg = if real_path.exists() {
        load(&real_path)
    } else {
        AppConfig::default()
    };
    mutate(&mut cfg);

    let contents = toml::to_string_pretty(&cfg)?;
    let tmp = real_path.with_extension("toml.tmp");
    std::fs::write(&tmp, &contents)?;
    std::fs::rename(&tmp, &real_path)?;
    fs2::FileExt::unlock(&lock_file).ok();
    let _ = std::fs::remove_file(&lock_path);
    Ok(())
}

pub fn save(path: &Path, theme_name: &str) -> Result<()> {
    update_config(path, |cfg| cfg.theme = Some(theme_name.to_string()))
}

pub fn save_version(path: &Path, version: &str) -> Result<()> {
    update_config(path, |cfg| {
        cfg.last_seen_version = Some(version.to_string())
    })
}

/// Resolve CLI + config into the one `&'static Theme` the runtime uses
/// (CLI > config > `NORMAL`). The asymmetry is deliberate: a `--theme` typo is
/// explicit user intent and hard-errors (listing valid names), while a config
/// typo soft-warns and falls back so a stale config file never bricks startup.
pub fn resolve_theme(
    config: &AppConfig,
    cli_theme: Option<&str>,
) -> Result<&'static crate::tui::theme::Theme> {
    use crate::tui::theme::{theme_by_name, ALL_THEMES, NORMAL};

    // Validate the config theme even when the CLI overrides it — the warn is
    // the only signal that a persisted theme in config.toml has gone stale.
    let config_theme = config.theme.as_deref().and_then(|t| {
        let theme = theme_by_name(t);
        if theme.is_none() {
            tracing::warn!(theme = %t, "unknown theme in config — ignoring");
        }
        theme
    });
    if let Some(name) = cli_theme {
        return theme_by_name(name).ok_or_else(|| {
            let valid: Vec<&str> = ALL_THEMES.iter().map(|t| t.name).collect();
            anyhow::anyhow!("unknown theme: {name}. Valid: {}", valid.join(", "))
        });
    }
    Ok(config_theme.unwrap_or(&NORMAL))
}

/// Resolve config into the office's [`Pet`]s. `[[pets]]` absent → all kinds
/// with default names. `pets = []` → no pets. An unknown `kind` is warn-skipped
/// (non-fatal; the rest of the config and the remaining stanzas survive). A
/// `name` is trimmed; empty/absent → [`PetKind::default_name`]. Resolving HERE
/// (once, at startup) means the render path reads `pet.name` directly — no
/// per-frame lookup, no parallel kind→name map to keep in sync.
pub fn resolve_pets(config: &AppConfig) -> Vec<crate::tui::pet::Pet> {
    use crate::tui::pet::{Pet, PetKind};

    match &config.pets {
        None => PetKind::ALL.iter().map(|&k| Pet::defaulted(k)).collect(),
        Some(entries) => {
            let mut out = Vec::with_capacity(entries.len());
            for entry in entries {
                let Some(kind) = entry.kind.as_deref().and_then(PetKind::from_config_name) else {
                    tracing::warn!(
                        pet = ?entry.kind,
                        "missing or unknown pet `kind` in [[pets]] config — skipping"
                    );
                    continue;
                };
                let name = entry
                    .name
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| kind.default_name().to_string());
                out.push(Pet { kind, name });
            }
            if out.is_empty() && !entries.is_empty() {
                tracing::warn!("all [[pets]] entries had unknown kinds — no pets will appear");
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_missing_returns_defaults() {
        let cfg = load(Path::new("/nonexistent/path/config.toml"));
        assert!(cfg.theme.is_none());
    }

    // config_path reads process-global env, so save+restore both vars and drive
    // the three branches in one test. The TEST_ENV_LOCK serializes against the
    // other env-mutating test in this binary (embedded_pack's XDG test) so they
    // can't race under plain `cargo test`.
    #[test]
    fn config_path_xdg_home_and_relative_branches() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let saved_home = std::env::var_os("HOME");

        // XDG_CONFIG_HOME wins when set.
        std::env::set_var("XDG_CONFIG_HOME", "/xdg/base");
        std::env::set_var("HOME", "/home/u");
        assert_eq!(
            config_path(),
            PathBuf::from("/xdg/base/pixtuoid/config.toml")
        );

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
    }

    // load()'s non-NotFound read-error arm: pointing at a DIRECTORY makes
    // read_to_string error (IsADirectory) → warn + return defaults (never crash).
    #[test]
    fn load_unreadable_path_returns_defaults() {
        let dir = tempfile::tempdir().unwrap();
        // The directory itself is an existing, non-NotFound, unreadable "file".
        let cfg = load(dir.path());
        assert!(cfg.theme.is_none());
    }

    #[test]
    fn expand_tilde_only_expands_leading_current_user_home() {
        let home = Some("/Users/x");
        // ~ alone and ~/ prefix expand.
        assert_eq!(expand_tilde("~", home), "/Users/x");
        assert_eq!(expand_tilde("~/packs/robot", home), "/Users/x/packs/robot");
        // ~user/ is another user's home — leave it alone (don't produce /Users/xuser/).
        assert_eq!(expand_tilde("~user/p", home), "~user/p");
        // A non-leading ~ must never be replaced.
        assert_eq!(expand_tilde("rel/~/x", home), "rel/~/x");
        // Absolute / relative paths pass through untouched.
        assert_eq!(expand_tilde("/abs/p", home), "/abs/p");
        // No HOME → input returned unchanged.
        assert_eq!(expand_tilde("~/p", None), "~/p");
    }

    #[test]
    fn load_malformed_returns_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "not valid { toml }}}").unwrap();
        let cfg = load(&path);
        assert!(cfg.theme.is_none());
    }

    #[test]
    fn load_partial_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"cyberpunk\"\n").unwrap();
        let cfg = load(&path);
        assert_eq!(cfg.theme.as_deref(), Some("cyberpunk"));
    }

    #[test]
    fn load_ignores_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"normal\"\nfuture-key = 42\n").unwrap();
        let cfg = load(&path);
        assert_eq!(cfg.theme.as_deref(), Some("normal"));
    }

    #[test]
    fn save_then_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        save(&path, "dracula").unwrap();
        let cfg = load(&path);
        assert_eq!(cfg.theme.as_deref(), Some("dracula"));
    }

    #[test]
    fn resolve_cli_wins_over_config() {
        let cfg = AppConfig {
            theme: Some("normal".into()),
            ..AppConfig::default()
        };
        let theme = resolve_theme(&cfg, Some("dracula")).unwrap();
        assert_eq!(theme.name, "dracula");
    }

    #[test]
    fn resolve_config_wins_over_default() {
        let cfg = AppConfig {
            theme: Some("gruvbox".into()),
            ..AppConfig::default()
        };
        let theme = resolve_theme(&cfg, None).unwrap();
        assert_eq!(theme.name, "gruvbox");
    }

    #[test]
    fn resolve_all_none_uses_default() {
        let cfg = AppConfig::default();
        let theme = resolve_theme(&cfg, None).unwrap();
        assert_eq!(theme.name, "normal");
    }

    #[test]
    fn resolve_invalid_config_theme_falls_back_to_default() {
        let cfg = AppConfig {
            theme: Some("does-not-exist".into()),
            ..AppConfig::default()
        };
        let theme = resolve_theme(&cfg, None).unwrap();
        assert_eq!(theme.name, "normal");
    }

    #[test]
    fn resolve_invalid_cli_theme_hard_errors() {
        let cfg = AppConfig::default();
        let err = resolve_theme(&cfg, Some("definitely-not-a-theme")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown theme"), "got: {msg}");
        for t in crate::tui::theme::ALL_THEMES {
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
        let theme = resolve_theme(&cfg, Some("dracula")).unwrap();
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
        assert!(resolve_theme(&cfg, Some("definitely-not-a-theme")).is_err());
    }

    #[test]
    fn full_config_flow_file_drives_theme() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"cyberpunk\"\n").unwrap();
        let cfg = load(&path);
        let theme = resolve_theme(&cfg, None).unwrap();
        assert_eq!(theme.name, "cyberpunk");
    }

    #[test]
    fn full_config_flow_cli_overrides_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"cyberpunk\"\n").unwrap();
        let cfg = load(&path);
        let theme = resolve_theme(&cfg, Some("dracula")).unwrap();
        assert_eq!(theme.name, "dracula");
    }

    // --- max-desks cap flow -----------------------------------------------

    #[test]
    fn max_desks_config_set_no_cli() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "max-desks = 8\n").unwrap();
        let cfg = load(&path);
        let cli_max_desks: Option<usize> = None;
        let desk_cap = cli_max_desks.or(cfg.max_desks);
        assert_eq!(desk_cap, Some(8));
    }

    #[test]
    fn max_desks_cli_overrides_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "max-desks = 8\n").unwrap();
        let cfg = load(&path);
        let cli_max_desks: Option<usize> = Some(4);
        let desk_cap = cli_max_desks.or(cfg.max_desks);
        assert_eq!(desk_cap, Some(4));
    }

    #[test]
    fn max_desks_neither_set() {
        let cfg = AppConfig::default();
        let cli_max_desks: Option<usize> = None;
        let desk_cap = cli_max_desks.or(cfg.max_desks);
        assert_eq!(desk_cap, None);
    }

    #[test]
    fn max_desks_no_config_file() {
        let cfg = load(Path::new("/nonexistent/path/config.toml"));
        let cli_max_desks: Option<usize> = None;
        let desk_cap = cli_max_desks.or(cfg.max_desks);
        assert_eq!(desk_cap, None);
    }

    #[test]
    fn save_preserves_max_desks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"normal\"\nmax-desks = 8\n").unwrap();
        save(&path, "cyberpunk").unwrap();
        let cfg = load(&path);
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
        if let Ok(home) = std::env::var("HOME") {
            assert_eq!(result, Some(PathBuf::from(format!("{home}/my-pack"))));
        }
    }

    #[test]
    fn pack_dir_loaded_from_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "pack-dir = \"/custom/sprites\"\n").unwrap();
        let cfg = load(&path);
        assert_eq!(cfg.pack_dir.as_deref(), Some("/custom/sprites"));
    }

    // --- [[pets]] config ----------------------------------------------------

    #[test]
    fn pets_absent_returns_all_with_default_names() {
        let cfg = AppConfig::default();
        let pets = resolve_pets(&cfg);
        assert_eq!(pets.len(), crate::tui::pet::PetKind::ALL.len());
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
        assert!(resolve_pets(&cfg).is_empty());
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
        let pets = resolve_pets(&cfg);
        assert_eq!(pets.len(), 1);
        assert_eq!(pets[0].kind, crate::tui::pet::PetKind::Cat);
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
        assert!(resolve_pets(&cfg).is_empty());
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
        let pets = resolve_pets(&cfg);
        let name = |k| pets.iter().find(|p| p.kind == k).map(|p| p.name.as_str());
        assert_eq!(name(crate::tui::pet::PetKind::Cat), Some("Whiskers"));
        assert_eq!(name(crate::tui::pet::PetKind::Dog), Some("Rex"));
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
        assert_eq!(resolve_pets(&cfg)[0].name, "Office Dog");
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
        let pets = resolve_pets(&cfg);
        let name = |k| pets.iter().find(|p| p.kind == k).map(|p| p.name.as_str());
        assert_eq!(name(crate::tui::pet::PetKind::Cat), Some("Mittens"));
        assert_eq!(name(crate::tui::pet::PetKind::Dog), Some("Office Dog"));
    }

    #[test]
    fn pets_loaded_from_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[[pets]]\nkind = \"dog\"\n").unwrap();
        let cfg = load(&path);
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
        let cfg = load(&path);
        let pets = resolve_pets(&cfg);
        assert_eq!(pets.len(), 2);
        let name = |k| pets.iter().find(|p| p.kind == k).map(|p| p.name.as_str());
        assert_eq!(name(crate::tui::pet::PetKind::Cat), Some("Luna"));
        assert_eq!(name(crate::tui::pet::PetKind::Dog), Some("Office Dog"));
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
        let cfg = load(&path);
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
        let cfg = load(&path);
        assert_eq!(
            cfg.theme.as_deref(),
            Some("cyberpunk"),
            "theme must survive a kindless [[pets]] stanza (config not reset)"
        );
        let pets = resolve_pets(&cfg);
        assert_eq!(
            pets.len(),
            1,
            "the kindless stanza is skipped, the cat kept"
        );
        assert_eq!(pets[0].kind, crate::tui::pet::PetKind::Cat);
    }

    // --- save_version ---------------------------------------------------------

    #[test]
    fn save_version_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        save_version(&path, "0.4.0").unwrap();
        let cfg = load(&path);
        assert_eq!(cfg.last_seen_version.as_deref(), Some("0.4.0"));
    }

    #[test]
    fn save_version_preserves_theme() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"cyberpunk\"\n").unwrap();
        save_version(&path, "0.4.0").unwrap();
        let cfg = load(&path);
        assert_eq!(cfg.theme.as_deref(), Some("cyberpunk"));
        assert_eq!(cfg.last_seen_version.as_deref(), Some("0.4.0"));
    }
}
