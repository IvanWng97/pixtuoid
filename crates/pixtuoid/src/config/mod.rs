use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// One `[[pets]]` stanza. `kind` is an OPTIONAL raw `String` (NOT a serde-derived
/// `PetKind`) on purpose: an unknown or typo'd value is warn-skipped in
/// [`resolve_pets`] rather than failing the whole `toml::from_str` and tripping
/// `load`'s all-or-nothing malformed arm, which would silently revert EVERY user
/// setting to defaults.
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
    /// Per-floor desk cap; excess agents overflow to additional floors. Absent ⇒
    /// capacity is auto-computed from terminal size.
    #[serde(rename = "max-desks")]
    pub max_desks: Option<usize>,
    /// Supports `~` expansion.
    #[serde(rename = "pack-dir")]
    pub pack_dir: Option<String>,
    #[serde(
        rename = "last-seen-version",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_seen_version: Option<String>,
    /// Per-source connection flags (registry source id → connected); only an
    /// explicit `true` connects. Keep BEFORE `pets` (see `pets`).
    #[serde(
        rename = "sources",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub sources: BTreeMap<String, bool>,
    /// Keep BEFORE `pets` (see `pets`).
    #[serde(rename = "floating", default, skip_serializing_if = "Option::is_none")]
    pub floating: Option<FloatingConfigRaw>,
    /// Ambient office sound. Absent ⇒ MUTED — the office starts silent and `m`
    /// is the whole opt-in. Keep BEFORE `pets` (see `pets`).
    #[serde(rename = "audio", default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioConfigRaw>,
    /// Keep `pets` LAST in the struct: it is an array-of-tables, and any `[table]`
    /// serialized after it would re-parent under it.
    #[serde(rename = "pets", default, skip_serializing_if = "Option::is_none")]
    pub pets: Option<Vec<PetEntry>>,
}

/// Default `pixtuoid floating` window size (logical px) + the minimum below which
/// the half-block office art is unreadable.
pub const FLOATING_DEFAULT_W: u32 = 360;
pub const FLOATING_DEFAULT_H: u32 = 240;
pub const FLOATING_MIN_W: u32 = 240;
pub const FLOATING_MIN_H: u32 = 160;
/// Below this the window is too transparent to read.
pub const FLOATING_MIN_OPACITY: f32 = 0.2;

#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FloatingConfigRaw {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f32>,
}

/// Position stays `Option` — `None` lets the OS place the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FloatingConfig {
    pub width: u32,
    pub height: u32,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub opacity: f32,
}

pub fn resolve_floating(config: &AppConfig) -> FloatingConfig {
    let raw = config.floating.clone().unwrap_or_default();
    FloatingConfig {
        width: raw.width.unwrap_or(FLOATING_DEFAULT_W).max(FLOATING_MIN_W),
        height: raw.height.unwrap_or(FLOATING_DEFAULT_H).max(FLOATING_MIN_H),
        x: raw.x,
        y: raw.y,
        opacity: raw.opacity.unwrap_or(1.0).clamp(FLOATING_MIN_OPACITY, 1.0),
    }
}

#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AudioConfigRaw {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<f32>,
}

/// `muted` is the ONE sound switch — there is deliberately no second `enabled`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioConfig {
    pub muted: bool,
    pub volume: f32,
}

pub fn resolve_audio(config: &AppConfig) -> AudioConfig {
    let raw = config.audio.clone().unwrap_or_default();
    AudioConfig {
        muted: raw.muted.unwrap_or(true),
        volume: raw.volume.unwrap_or(1.0).clamp(0.0, 1.0),
    }
}

pub(crate) fn save_audio_muted(path: &Path, muted: bool) -> Result<()> {
    update_config(path, |doc| {
        doc["audio"]["muted"] = toml_edit::value(muted);
    })
}

pub(crate) fn save_audio_volume(path: &Path, volume: f32) -> Result<()> {
    // Quantize to the footer's percent vocabulary before widening — a raw
    // f32→f64 writes float noise (0.949999988079071) into a hand-edited file.
    let percent = (volume * 100.0).round() / 100.0;
    update_config(path, |doc| {
        doc["audio"]["volume"] = toml_edit::value(percent as f64);
    })
}

pub fn resolve_pack_dir(config: &AppConfig, cli_pack_dir: Option<PathBuf>) -> Option<PathBuf> {
    cli_pack_dir.or_else(|| {
        config.pack_dir.as_ref().map(|p| {
            // The ONE tilde-expander: it handles `~\` as well as `~/` and stays in
            // PathBuf-land, so `pack-dir` expands the same way on Windows.
            let home = pixtuoid_core::platform::user_home_opt();
            crate::install::io::expand_tilde(p, home.as_deref().map(Path::new))
        })
    })
}

pub fn config_path() -> PathBuf {
    // Empty/relative XDG_CONFIG_HOME is invalid (XDG spec), so `nonempty_abs_env`
    // falls to $HOME/.config rather than a CWD-relative `pixtuoid/config.toml`.
    let xdg = crate::install::io::nonempty_abs_env("XDG_CONFIG_HOME");
    if let Some(base) = xdg {
        return PathBuf::from(base).join("pixtuoid").join("config.toml");
    }
    if let Some(home) = pixtuoid_core::platform::user_home_opt() {
        return PathBuf::from(home)
            .join(".config")
            .join("pixtuoid")
            .join("config.toml");
    }
    PathBuf::from(".config/pixtuoid/config.toml")
}

/// Report ONE user-facing config warning to BOTH of its sinks from a single
/// control-char-stripped string. Both sinks are real terminals and these lines
/// interpolate config CONTENT — a `toml::de::Error` Display embeds the raw
/// offending source line — so an ANSI/OSC escape or a Trojan-Source bidi override
/// would render live. Keep it ONE emission point so neither sink drifts back to raw.
fn warn_user(warnings: &mut Vec<String>, line: String) {
    let line = crate::strip_control_chars(&line);
    tracing::warn!("{line}");
    warnings.push(line);
}

/// Load the config, never crashing: unreadable/malformed files fall back to
/// defaults. Fallbacks go onto `warnings` (as well as the log) so `main` can
/// print them to stderr BEFORE the alternate screen swallows them; callers with
/// no user to warn pass a throwaway Vec.
pub fn load(path: &Path, warnings: &mut Vec<String>) -> AppConfig {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return AppConfig::default(),
        Err(e) => {
            warn_user(
                warnings,
                format!(
                    "cannot read config {} ({e}) — using defaults",
                    path.display()
                ),
            );
            return AppConfig::default();
        }
    };
    match toml::from_str(&contents) {
        Ok(cfg) => cfg,
        Err(e) => {
            warn_user(
                warnings,
                format!(
                    "malformed config {} — ALL settings reset to defaults ({e})",
                    path.display()
                ),
            );
            AppConfig::default()
        }
    }
}

/// Load-modify-write under ONE advisory lock held across the whole
/// read→mutate→write round. The mutation edits the RAW TOML document, not a typed
/// `AppConfig` round-trip, so unknown keys (a newer pixtuoid's settings) and the
/// user's comments survive a save.
///
/// Data-safety contract: a config that EXISTS but does not parse is NEVER
/// rewritten — the save fails with the parse error, leaving the user's typo
/// fixable.
fn update_config<F>(path: &Path, mutate: F) -> Result<()>
where
    F: FnOnce(&mut toml_edit::DocumentMut),
{
    let lock = crate::install::io::lock_config(path)?;
    let real_path = lock.target();
    // Read through the guard's pinned resolution, NOT a raw read of a re-derived
    // path: every leg of the locked round must address the ONE file the flock
    // protects.
    let contents = lock.read().with_context(|| {
        format!(
            "refusing to rewrite {}: cannot read the existing config",
            real_path.display()
        )
    })?;
    let mut doc = if contents.is_empty() {
        toml_edit::DocumentMut::new()
    } else {
        let doc = contents.parse::<toml_edit::DocumentMut>().map_err(|e| {
            anyhow::anyhow!(
                "refusing to rewrite {}: it exists but is not valid TOML ({e}); fix or delete it",
                real_path.display()
            )
        })?;
        // Syntax alone isn't enough: a type-invalid value (`max-desks = "oops"`)
        // parses as a document but fails the typed `load`, so persisting over it
        // would make this save "succeed" while never taking effect. Unknown keys
        // still pass (forward-compat).
        toml::from_str::<AppConfig>(&contents).map_err(|e| {
            anyhow::anyhow!(
                "refusing to rewrite {}: it exists but has invalid values ({e}); fix or delete it",
                real_path.display()
            )
        })?;
        doc
    };
    mutate(&mut doc);
    lock.backup_once(crate::install::target::BACKUP_SUFFIX)?;
    lock.write_atomic(&doc.to_string())
}

pub(crate) fn save(path: &Path, theme_name: &str) -> Result<()> {
    update_config(path, |doc| {
        doc["theme"] = toml_edit::value(theme_name);
    })
}

pub(crate) fn save_version(path: &Path, version: &str) -> Result<()> {
    update_config(path, |doc| {
        doc["last-seen-version"] = toml_edit::value(version);
    })
}

pub(crate) fn save_source_connected(
    path: &Path,
    source_id: &'static str,
    connected: bool,
) -> Result<()> {
    update_config(path, |doc| {
        doc["sources"][source_id] = toml_edit::value(connected);
    })
}

/// Remove a source's connection flag — the connect-rollback restore. An absent
/// flag and an explicit `false` both read as disconnected, but only absence keeps
/// the `setup::is_first_run` empty-table signal intact, so an emptied `[sources]`
/// table is dropped too.
pub(crate) fn remove_source_connected(path: &Path, source_id: &str) -> Result<()> {
    update_config(path, |doc| {
        let emptied = match doc.get_mut("sources").and_then(|s| s.as_table_like_mut()) {
            Some(t) => {
                t.remove(source_id);
                t.is_empty()
            }
            None => false,
        };
        if emptied {
            doc.as_table_mut().remove("sources");
        }
    })
}

pub(crate) fn save_floating(
    path: &Path,
    width: u32,
    height: u32,
    x: Option<i32>,
    y: Option<i32>,
) -> Result<()> {
    update_config(path, |doc| {
        doc["floating"]["width"] = toml_edit::value(width as i64);
        doc["floating"]["height"] = toml_edit::value(height as i64);
        // Set-or-CLEAR x/y: a `None` means the OS couldn't report the position
        // (ALWAYS on Wayland, or a transient at close). Keeping the OLD coords
        // would restore a stale/offscreen spot next launch, so drop the keys and
        // let the OS place the window.
        for (key, val) in [("x", x), ("y", y)] {
            match val {
                Some(v) => doc["floating"][key] = toml_edit::value(v as i64),
                // `as_table_like_mut`, not `as_table_mut`: `floating` serializes as
                // an INLINE table, for which the standard-table accessor returns
                // None and the key would never drop.
                None => {
                    if let Some(t) = doc["floating"].as_table_like_mut() {
                        t.remove(key);
                    }
                }
            }
        }
    })
}

/// Resolve the runtime connected-set the office gates its sprites on: a
/// registered source is connected iff its `[sources]` flag is an explicit `true`.
/// An absent flag is plainly DISCONNECTED, so a config predating `[sources]`
/// reads as a first run and replays the onboarding wizard.
pub fn resolve_connected(config: &AppConfig) -> std::collections::HashSet<String> {
    pixtuoid_core::source::registry::registered_source_names()
        .filter(|src| config.sources.get(*src).copied().unwrap_or(false))
        .map(String::from)
        .collect()
}

/// Resolve the config `max-desks` into the runtime desk cap. `0` is treated as
/// unset with a warning: the cap clamps every floor via `min`, so an accepted 0
/// would permanently zero every floor and silently drop every SessionStart. The
/// `--max-desks` CLI flag rejects 0 at the clap seam; this is its config twin.
pub fn resolve_max_desks(config: &AppConfig, warnings: &mut Vec<String>) -> Option<usize> {
    match config.max_desks {
        Some(0) => {
            warn_user(
                warnings,
                "max-desks = 0 in config would hide every agent — ignoring it \
                 (the --max-desks flag or auto-computed capacity applies)"
                    .into(),
            );
            None
        }
        other => other,
    }
}

/// Resolve CLI + config into the one `&'static Theme` the runtime uses
/// (CLI > config > `NORMAL`). The asymmetry is deliberate: a `--theme` typo is
/// explicit user intent and hard-errors (listing valid names), while a config
/// typo soft-warns and falls back so a stale config file never bricks startup.
pub fn resolve_theme(
    config: &AppConfig,
    cli_theme: Option<&str>,
    warnings: &mut Vec<String>,
) -> Result<&'static pixtuoid_scene::theme::Theme> {
    use pixtuoid_scene::theme::{theme_by_name, ALL_THEMES, NORMAL};

    // Validate the config theme even when the CLI overrides it — the warn is the
    // only signal that a persisted theme has gone stale.
    let config_theme = config.theme.as_deref().and_then(|t| {
        let theme = theme_by_name(t);
        if theme.is_none() {
            warn_user(
                warnings,
                format!("unknown theme {t:?} in config — ignoring (falling back to the default)"),
            );
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

/// Resolve config into the office's `Pet`s. An unknown `kind` is warn-skipped —
/// the remaining stanzas survive. Resolving HERE (once, at startup) means the
/// render path reads `pet.name` directly, with no per-frame lookup.
pub fn resolve_pets(
    config: &AppConfig,
    warnings: &mut Vec<String>,
) -> Vec<pixtuoid_scene::pet::Pet> {
    use pixtuoid_scene::pet::{Pet, PetKind};

    match &config.pets {
        None => PetKind::ALL.iter().map(|&k| Pet::defaulted(k)).collect(),
        Some(entries) => {
            let mut out = Vec::with_capacity(entries.len());
            for entry in entries {
                let Some(kind) = entry.kind.as_deref().and_then(PetKind::from_config_name) else {
                    warn_user(
                        warnings,
                        format!(
                            "missing or unknown pet `kind` {:?} in [[pets]] config — skipping that pet",
                            entry.kind.as_deref().unwrap_or("<missing>")
                        ),
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
                warn_user(
                    warnings,
                    "all [[pets]] entries had unknown kinds — no pets will appear".into(),
                );
            }
            out
        }
    }
}

#[cfg(test)]
mod tests;
