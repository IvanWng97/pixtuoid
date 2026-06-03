use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

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
        rename = "enabled-pets",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub enabled_pets: Option<Vec<String>>,
    #[serde(
        rename = "last-seen-version",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_seen_version: Option<String>,
    /// Optional per-kind custom display names for pets, e.g.
    /// `[pet-names]\ncat = "Whiskers"\ndog = "Rex"`. Keys are pet config
    /// names (`cat`/`dog`); an unset kind falls back to its default name.
    /// MUST stay LAST in the struct: a `HashMap` serializes as a TOML
    /// `[pet-names]` table, which `toml::to_string_pretty` requires after all
    /// scalar keys (declaration order) — a scalar field below it would emit
    /// invalid TOML.
    #[serde(rename = "pet-names", default, skip_serializing_if = "Option::is_none")]
    pub pet_names: Option<HashMap<String, String>>,
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

pub fn resolve_theme(config: &AppConfig, cli_theme: Option<String>) -> String {
    let config_theme = config.theme.as_deref().and_then(|t| {
        if crate::tui::theme::theme_by_name(t).is_some() {
            Some(t.to_string())
        } else {
            tracing::warn!(theme = %t, "unknown theme in config — ignoring");
            None
        }
    });
    cli_theme
        .or(config_theme)
        .unwrap_or_else(|| "normal".to_string())
}

pub fn resolve_pets(config: &AppConfig) -> Vec<crate::tui::pet::PetKind> {
    match &config.enabled_pets {
        None => crate::tui::pet::PetKind::ALL.to_vec(),
        Some(names) => {
            let pets: Vec<_> = names
                .iter()
                .filter_map(|n| {
                    let kind = crate::tui::pet::PetKind::from_config_name(n);
                    if kind.is_none() {
                        tracing::warn!(pet = %n, "unknown pet in config — skipping");
                    }
                    kind
                })
                .collect();
            if pets.is_empty() && !names.is_empty() {
                tracing::warn!("all enabled-pets names were unknown — no pets will appear");
            }
            pets
        }
    }
}

/// Resolve the `[pet-names]` table to a `PetKind`-keyed map of custom display
/// names. Unknown keys are warned + skipped; values are trimmed and an
/// empty/whitespace-only value is dropped (the kind then falls back to its
/// default name). Absent table → empty map (all kinds use defaults). Mirrors
/// [`resolve_pets`].
pub fn resolve_pet_names(config: &AppConfig) -> HashMap<crate::tui::pet::PetKind, String> {
    let mut out = HashMap::new();
    let Some(names) = &config.pet_names else {
        return out;
    };
    for (key, value) in names {
        let Some(kind) = crate::tui::pet::PetKind::from_config_name(key) else {
            tracing::warn!(pet = %key, "unknown pet in [pet-names] — skipping");
            continue;
        };
        let trimmed = value.trim();
        if trimmed.is_empty() {
            tracing::warn!(pet = %key, "empty pet name in [pet-names] — using default");
            continue;
        }
        out.insert(kind, trimmed.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_missing_returns_defaults() {
        let cfg = load(Path::new("/nonexistent/path/config.toml"));
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
        let theme = resolve_theme(&cfg, Some("dracula".into()));
        assert_eq!(theme, "dracula");
    }

    #[test]
    fn resolve_config_wins_over_default() {
        let cfg = AppConfig {
            theme: Some("gruvbox".into()),
            ..AppConfig::default()
        };
        let theme = resolve_theme(&cfg, None);
        assert_eq!(theme, "gruvbox");
    }

    #[test]
    fn resolve_all_none_uses_default() {
        let cfg = AppConfig::default();
        let theme = resolve_theme(&cfg, None);
        assert_eq!(theme, "normal");
    }

    #[test]
    fn resolve_invalid_config_theme_falls_back_to_default() {
        let cfg = AppConfig {
            theme: Some("does-not-exist".into()),
            ..AppConfig::default()
        };
        let theme = resolve_theme(&cfg, None);
        assert_eq!(theme, "normal");
    }

    #[test]
    fn full_config_flow_file_drives_theme() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"cyberpunk\"\n").unwrap();
        let cfg = load(&path);
        let theme = resolve_theme(&cfg, None);
        assert_eq!(theme, "cyberpunk");
    }

    #[test]
    fn full_config_flow_cli_overrides_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"cyberpunk\"\n").unwrap();
        let cfg = load(&path);
        let theme = resolve_theme(&cfg, Some("dracula".into()));
        assert_eq!(theme, "dracula");
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

    // --- enabled-pets -------------------------------------------------------

    #[test]
    fn enabled_pets_none_returns_all() {
        let cfg = AppConfig::default();
        let pets = resolve_pets(&cfg);
        assert_eq!(pets.len(), crate::tui::pet::PetKind::ALL.len());
    }

    #[test]
    fn enabled_pets_empty_returns_none() {
        let cfg = AppConfig {
            enabled_pets: Some(vec![]),
            ..AppConfig::default()
        };
        let pets = resolve_pets(&cfg);
        assert!(pets.is_empty());
    }

    #[test]
    fn enabled_pets_filters_unknown() {
        let cfg = AppConfig {
            enabled_pets: Some(vec!["cat".into(), "hamster".into()]),
            ..AppConfig::default()
        };
        let pets = resolve_pets(&cfg);
        assert_eq!(pets, vec![crate::tui::pet::PetKind::Cat]);
    }

    #[test]
    fn enabled_pets_loaded_from_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "enabled-pets = [\"dog\"]\n").unwrap();
        let cfg = load(&path);
        assert_eq!(cfg.enabled_pets, Some(vec!["dog".to_string()]));
    }

    #[test]
    fn save_preserves_enabled_pets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "theme = \"normal\"\nenabled-pets = [\"cat\", \"dog\"]\n",
        )
        .unwrap();
        save(&path, "cyberpunk").unwrap();
        let cfg = load(&path);
        assert_eq!(cfg.theme.as_deref(), Some("cyberpunk"));
        assert_eq!(
            cfg.enabled_pets,
            Some(vec!["cat".to_string(), "dog".to_string()])
        );
    }

    #[test]
    fn enabled_pets_all_unknown_returns_empty() {
        let cfg = AppConfig {
            enabled_pets: Some(vec!["hamster".into(), "parrot".into()]),
            ..AppConfig::default()
        };
        let pets = resolve_pets(&cfg);
        assert!(pets.is_empty());
    }

    // --- pet-names ----------------------------------------------------------

    #[test]
    fn resolve_pet_names_none_returns_empty() {
        let cfg = AppConfig::default();
        assert!(resolve_pet_names(&cfg).is_empty());
    }

    #[test]
    fn resolve_pet_names_cat_and_dog_set() {
        let mut m = HashMap::new();
        m.insert("cat".to_string(), "Whiskers".to_string());
        m.insert("dog".to_string(), "Rex".to_string());
        let cfg = AppConfig {
            pet_names: Some(m),
            ..AppConfig::default()
        };
        let names = resolve_pet_names(&cfg);
        assert_eq!(
            names
                .get(&crate::tui::pet::PetKind::Cat)
                .map(String::as_str),
            Some("Whiskers")
        );
        assert_eq!(
            names
                .get(&crate::tui::pet::PetKind::Dog)
                .map(String::as_str),
            Some("Rex")
        );
    }

    #[test]
    fn resolve_pet_names_trims_whitespace() {
        let mut m = HashMap::new();
        m.insert("cat".to_string(), "  Mittens  ".to_string());
        let cfg = AppConfig {
            pet_names: Some(m),
            ..AppConfig::default()
        };
        let names = resolve_pet_names(&cfg);
        assert_eq!(
            names
                .get(&crate::tui::pet::PetKind::Cat)
                .map(String::as_str),
            Some("Mittens")
        );
    }

    #[test]
    fn resolve_pet_names_skips_empty_value() {
        let mut m = HashMap::new();
        m.insert("cat".to_string(), "   ".to_string());
        m.insert("dog".to_string(), "Rex".to_string());
        let cfg = AppConfig {
            pet_names: Some(m),
            ..AppConfig::default()
        };
        let names = resolve_pet_names(&cfg);
        assert!(!names.contains_key(&crate::tui::pet::PetKind::Cat));
        assert_eq!(
            names
                .get(&crate::tui::pet::PetKind::Dog)
                .map(String::as_str),
            Some("Rex")
        );
    }

    #[test]
    fn resolve_pet_names_skips_unknown_key() {
        let mut m = HashMap::new();
        m.insert("hamster".to_string(), "Nibbles".to_string());
        m.insert("cat".to_string(), "Luna".to_string());
        let cfg = AppConfig {
            pet_names: Some(m),
            ..AppConfig::default()
        };
        let names = resolve_pet_names(&cfg);
        assert_eq!(names.len(), 1);
        assert_eq!(
            names
                .get(&crate::tui::pet::PetKind::Cat)
                .map(String::as_str),
            Some("Luna")
        );
    }

    #[test]
    fn pet_names_loaded_from_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[pet-names]\ncat = \"Luna\"\n").unwrap();
        let cfg = load(&path);
        assert_eq!(
            cfg.pet_names
                .as_ref()
                .and_then(|m| m.get("cat"))
                .map(String::as_str),
            Some("Luna")
        );
    }

    #[test]
    fn resolve_pet_names_from_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[pet-names]\ncat = \"Luna\"\ndog = \"Rex\"\n").unwrap();
        let cfg = load(&path);
        let names = resolve_pet_names(&cfg);
        assert_eq!(
            names
                .get(&crate::tui::pet::PetKind::Cat)
                .map(String::as_str),
            Some("Luna")
        );
        assert_eq!(
            names
                .get(&crate::tui::pet::PetKind::Dog)
                .map(String::as_str),
            Some("Rex")
        );
    }

    #[test]
    fn save_preserves_pet_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"normal\"\n[pet-names]\ncat = \"Luna\"\n").unwrap();
        save(&path, "cyberpunk").unwrap();
        let cfg = load(&path);
        assert_eq!(cfg.theme.as_deref(), Some("cyberpunk"));
        assert_eq!(
            cfg.pet_names
                .as_ref()
                .and_then(|m| m.get("cat"))
                .map(String::as_str),
            Some("Luna")
        );
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
