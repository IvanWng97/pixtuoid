use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

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
    /// Per-source connection flags (registry source id → connected). An absent
    /// id is simply DISCONNECTED ([`resolve_connected`] — only an explicit
    /// `true` connects; the old v0.4–0.7 "absent ⇒ connected iff hooks
    /// installed" migrate inference was dropped in 0.12.0). The `s` Sources
    /// panel writes a flag on toggle. A `[sources]` table; empty ⇒ omitted on
    /// save. Keep BEFORE `pets` — pets must stay last (its array-of-tables
    /// serializes cleanest after all tables, and a `[sources]` table written
    /// after `[[pets]]` would re-parent).
    #[serde(
        rename = "sources",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub sources: BTreeMap<String, bool>,
    /// `pixtuoid floating` desktop-window geometry — a single `[floating]` table
    /// (size/position/opacity). Absent ⇒ defaults from [`resolve_floating`]. Keep
    /// BEFORE `pets`: it's a `[table]`, and the `[[pets]]` array-of-tables must
    /// stay last (a table written after an AoT would re-parent under it).
    #[serde(rename = "floating", default, skip_serializing_if = "Option::is_none")]
    pub floating: Option<FloatingConfigRaw>,
    /// Ambient office sound — a single `[audio]` table (#633). Absent ⇒
    /// MUTED (the office starts silent; `m` is the whole opt-in and persists
    /// here). Resolved by [`resolve_audio`]. Keep BEFORE `pets` (the
    /// table-before-array-of-tables rule above).
    #[serde(rename = "audio", default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioConfigRaw>,
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

/// Default `pixtuoid floating` window size (logical px) + the minimum below which the
/// half-block office art is unreadable — `resolve_floating` clamps up to it.
pub const FLOATING_DEFAULT_W: u32 = 360;
pub const FLOATING_DEFAULT_H: u32 = 240;
pub const FLOATING_MIN_W: u32 = 240;
pub const FLOATING_MIN_H: u32 = 160;
/// Floor the parsed floating-window opacity is clamped up to — below this the
/// window is too transparent to read.
pub const FLOATING_MIN_OPACITY: f32 = 0.2;

/// Raw `[floating]` table as parsed — every field optional so a partial table (or an
/// absent one) is valid; [`resolve_floating`] fills defaults + clamps.
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

/// Resolved floating-window geometry: defaults applied, size clamped up to the legible
/// minimum, opacity clamped to `[0.2, 1.0]` (fully transparent / over-opaque are both
/// useless). Position stays `Option` — `None` lets the OS place the window.
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

/// Raw `[audio]` table as parsed — both fields optional; [`resolve_audio`]
/// applies the defaults + the volume clamp.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AudioConfigRaw {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<f32>,
}

/// Resolved ambient-audio settings: the ONE sound switch is `muted`,
/// default TRUE (#633 — the office starts silent; `m` unmutes and
/// persists). The old dual `enabled` knob was owner-cut as redundant —
/// muted-by-default + lazy spawn gives the same strict opt-in with one
/// keypress instead of a config edit. (`enabled` never shipped in a
/// release; a leftover key is silently ignored like any unknown key.)
/// Volume clamped to `[0.0, 1.0]`.
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

/// Persist the `[audio] muted` flag (the `m` toggle writes through here —
/// the theme-save precedent: ConfigLock round via `update_config`, comments
/// and unknown keys survive, a malformed existing config is never
/// rewritten).
pub(crate) fn save_audio_muted(path: &Path, muted: bool) -> Result<()> {
    update_config(path, |doc| {
        doc["audio"]["muted"] = toml_edit::value(muted);
    })
}

/// Persist the `[audio] volume` level (the +/- nudge keys write through
/// here, same ConfigLock round).
pub(crate) fn save_audio_volume(path: &Path, volume: f32) -> Result<()> {
    // quantize to the footer's percent vocabulary before widening — a raw
    // f32→f64 writes float noise (0.949999988079071) into a file the repo
    // deliberately keeps human-edited
    let percent = (volume * 100.0).round() / 100.0;
    update_config(path, |doc| {
        doc["audio"]["volume"] = toml_edit::value(percent as f64);
    })
}

pub fn resolve_pack_dir(config: &AppConfig, cli_pack_dir: Option<PathBuf>) -> Option<PathBuf> {
    cli_pack_dir.or_else(|| {
        config.pack_dir.as_ref().map(|p| {
            // Delegate to the ONE tilde-expander (`install::io::expand_tilde`): it
            // handles `~/` AND `~\` (Windows), trims, and stays in PathBuf-land, so
            // a `pack-dir` override expands the SAME way a config-location override
            // does. The previous local copy only knew `~/` and round-tripped through
            // `String` → `--pack-dir ~\packs` silently never expanded on Windows.
            let home = pixtuoid_core::platform::user_home_opt();
            crate::install::io::expand_tilde(p, home.as_deref().map(Path::new))
        })
    })
}

pub fn config_path() -> PathBuf {
    // Empty/relative XDG_CONFIG_HOME is invalid (XDG spec) → `nonempty_abs_env`
    // falls to $HOME/.config, never a CWD-relative `pixtuoid/config.toml`.
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

/// Load the config, never crashing: unreadable/malformed files fall back to
/// defaults. Each fallback is reported twice on purpose (#87): a
/// `tracing::warn!` for the log file, and a line pushed onto `warnings` so
/// `main` can print it to stderr BEFORE the alternate screen swallows it —
/// the resolvers stay layer-clean (no printing here; the caller picks the
/// sink). Callers that have no user to warn (the save path's internal
/// reload, the in-TUI version re-load) pass a throwaway Vec.
pub fn load(path: &Path, warnings: &mut Vec<String>) -> AppConfig {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return AppConfig::default(),
        Err(e) => {
            tracing::warn!(path = %path.display(), %e, "cannot read config — using defaults");
            warnings.push(format!(
                "cannot read config {} ({e}) — using defaults",
                path.display()
            ));
            return AppConfig::default();
        }
    };
    match toml::from_str(&contents) {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::warn!(path = %path.display(), %e, "malformed config — using defaults");
            warnings.push(format!(
                "malformed config {} — ALL settings reset to defaults ({e})",
                path.display()
            ));
            AppConfig::default()
        }
    }
}

/// Load-modify-write the config atomically through the install/io.rs write
/// authority: ONE advisory lock held across the whole read→mutate→write
/// round ([`crate::install::io::lock_config`] — symlink-resolved target,
/// fsync + atomic rename + Windows retry, lock file left in place).
///
/// The mutation edits the RAW TOML document (`toml_edit`), not a typed
/// `AppConfig` round-trip — unknown keys (a newer pixtuoid's settings) and
/// the user's comments/formatting survive a theme/version save, matching the
/// deliberately-tolerant read side (`load_ignores_unknown_keys`).
///
/// Data-safety contract: a config that EXISTS but does not parse is NEVER
/// rewritten — the save fails with the parse error (both callers
/// warn-and-continue), leaving the user's typo fixable. The first overwrite
/// of an existing file takes a one-time sibling backup
/// (`config.toml.pixtuoid.bak`, `io::backup_once` semantics).
fn update_config<F>(path: &Path, mutate: F) -> Result<()>
where
    F: FnOnce(&mut toml_edit::DocumentMut),
{
    let lock = crate::install::io::lock_config(path)?;
    let real_path = lock.target();
    // Read through the guard's pinned resolution (ConfigLock::read — "" for a
    // missing/empty file), NOT a raw read of a re-derived path: every leg of
    // the locked round must address the ONE file the flock protects.
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
        // Syntax alone isn't enough: a type-invalid value (`max-desks =
        // "oops"`) parses as a document but fails the typed `load`, which
        // resets everything to defaults in memory each boot — persisting
        // over it would make this save "succeed" while never taking
        // effect. Unknown keys still pass (forward-compat, pinned by
        // `load_ignores_unknown_keys`).
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

/// Persist a single source's connection flag, auto-vivifying the `[sources]`
/// table, through the comment/unknown-key-preserving `update_config` path. The
/// `s` Sources panel calls this on every connect/disconnect toggle.
pub(crate) fn save_source_connected(
    path: &Path,
    source_id: &'static str,
    connected: bool,
) -> Result<()> {
    update_config(path, |doc| {
        doc["sources"][source_id] = toml_edit::value(connected);
    })
}

/// Remove a single source's connection flag — the connect-rollback restore for a
/// flag that was ABSENT before the attempt (rollback must restore absence, the
/// pre-attempt state; an absent flag and an explicit `false` both read as
/// disconnected in [`resolve_connected`], but only absence keeps the
/// `setup::is_first_run` empty-table signal intact). Drops an emptied
/// `[sources]` table so a rolled-back first connect leaves the config exactly
/// as it was.
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

/// Persist the `pixtuoid floating` window geometry into the `[floating]` table (size always;
/// position when the OS reported it). Same `toml_edit` ConfigLock round as
/// `save_source_connected`, so the user's other settings + hand-formatting survive.
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
        // Set-or-CLEAR x/y: a `None` means the OS couldn't report the window position
        // (`outer_position()` returned `Err` — ALWAYS on Wayland, or a transient at close).
        // Persisting the OLD coords would (1) leave width/height/x/y internally inconsistent
        // (new size, stale position) and (2) restore a stale/offscreen spot next launch — so
        // drop the keys instead and let the OS place the window.
        for (key, val) in [("x", x), ("y", y)] {
            match val {
                Some(v) => doc["floating"][key] = toml_edit::value(v as i64),
                // `as_table_like_mut` (not `as_table_mut`): save_floating serializes
                // `floating` as an INLINE table (`floating = { … }`), so the standard-table
                // accessor returns None and the key would never drop.
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
/// registered source is connected iff its `[sources]` flag is an explicit
/// `true`. An absent flag (or an absent/empty `[sources]` table) is plainly
/// DISCONNECTED — the v0.4–0.7 "absent ⇒ connected iff hooks installed"
/// migrate inference was dropped in 0.12.0 (too old to keep supporting).
/// CONSEQUENCE: a v0.4–0.7 upgrader's config (exists, no `[sources]`, not
/// degraded) now reads as a first run (`setup::is_first_run`), so the
/// onboarding wizard replays and they re-connect there — acceptable, that IS
/// the connect flow.
pub fn resolve_connected(config: &AppConfig) -> std::collections::HashSet<String> {
    pixtuoid_core::source::registry::registered_source_names()
        .filter(|src| config.sources.get(*src).copied().unwrap_or(false))
        .map(String::from)
        .collect()
}

/// Resolve the config `max-desks` into the runtime desk cap. `0` is treated
/// as unset with a collected warning (#87 channel): the cap clamps every
/// floor via `min`, and the per-frame capacity re-seed only grows atomics
/// when `capacity > 0` — so an accepted 0 would permanently zero every floor
/// and silently drop every SessionStart (a permanently empty office with no
/// in-TUI signal). The hidden `--max-desks` CLI flag rejects 0 at the clap
/// seam (`range(1..)`); this is the config file's twin of that guard.
pub fn resolve_max_desks(config: &AppConfig, warnings: &mut Vec<String>) -> Option<usize> {
    match config.max_desks {
        Some(0) => {
            tracing::warn!("max-desks = 0 in config would hide every agent — ignoring");
            warnings.push(
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

    // Validate the config theme even when the CLI overrides it — the warn is
    // the only signal that a persisted theme in config.toml has gone stale.
    let config_theme = config.theme.as_deref().and_then(|t| {
        let theme = theme_by_name(t);
        if theme.is_none() {
            tracing::warn!(theme = %t, "unknown theme in config — ignoring");
            warnings.push(format!(
                "unknown theme {t:?} in config — ignoring (falling back to the default)"
            ));
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
                    tracing::warn!(
                        pet = ?entry.kind,
                        "missing or unknown pet `kind` in [[pets]] config — skipping"
                    );
                    warnings.push(format!(
                        "missing or unknown pet `kind` {:?} in [[pets]] config — skipping that pet",
                        entry.kind.as_deref().unwrap_or("<missing>")
                    ));
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
                warnings
                    .push("all [[pets]] entries had unknown kinds — no pets will appear".into());
            }
            out
        }
    }
}

#[cfg(test)]
mod tests;
