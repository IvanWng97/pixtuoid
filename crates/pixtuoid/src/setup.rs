//! First-run detection for the cinematic onboarding: ONE pure predicate, so the
//! TUI's one-time "move-in" overlay and the headless `pixtuoid setup [--yes]`
//! presenter agree on what "first run" means.
//!
//! `pub` (not `pub(crate)`) because the binary's `main.rs` is a separate crate
//! from this lib and computes it in `build_run_config`.

use std::path::Path;

use crate::config::AppConfig;

/// First run = no config file yet, OR a config that exists but has never written
/// a `[sources]` flag — UNLESS the load itself degraded (`load_degraded`: the file
/// exists but is malformed/unreadable, so `cfg.sources` is empty regardless of
/// what's really in it). An existing-but-broken config means "previously
/// configured", NOT "first run": replaying onboarding over it would funnel a
/// long-time user into an apply whose every write `update_config` refuses. The
/// caller passes `!load_warnings.is_empty()` right after `load` — a missing file
/// returns defaults WITHOUT a warning, so it stays a first run.
///
/// The second arm matches `config::resolve_connected`'s plain default: an
/// absent/empty `[sources]` table means NOTHING connected, so a user who has never
/// bound a source is a user who has never been onboarded. Once any
/// connect/disconnect persists a flag — even a *disconnected* `false` — the table
/// is non-empty and onboarding never re-triggers.
pub fn is_first_run(cfg: &AppConfig, path: &Path, load_degraded: bool) -> bool {
    !load_degraded && (!path.exists() || cfg.sources.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn cfg_with(sources: &[(&str, bool)]) -> AppConfig {
        AppConfig {
            sources: sources
                .iter()
                .map(|(k, v)| (k.to_string(), *v))
                .collect::<BTreeMap<_, _>>(),
            ..Default::default()
        }
    }

    #[test]
    fn absent_config_is_first_run() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.toml");
        assert!(is_first_run(&AppConfig::default(), &missing, false));
    }

    #[test]
    fn existing_config_with_no_sources_table_is_first_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"dracula\"\n").unwrap();
        assert!(is_first_run(&cfg_with(&[]), &path, false));
    }

    #[test]
    fn v04_v07_upgrader_config_without_sources_replays_onboarding() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"cyberpunk\"\nmax-desks = 8\n").unwrap();
        let mut warnings = Vec::new();
        let cfg = crate::config::load(&path, &mut warnings);
        assert!(warnings.is_empty(), "a healthy old config is not degraded");
        assert!(
            crate::config::resolve_connected(&cfg).is_empty(),
            "no [sources] ⇒ nothing connected (plain default, no install-state inference)"
        );
        assert!(
            is_first_run(&cfg, &path, !warnings.is_empty()),
            "…and the empty table replays onboarding, the re-connect path"
        );
    }

    #[test]
    fn existing_config_with_a_connected_flag_is_not_first_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[sources]\ncodex = true\n").unwrap();
        assert!(!is_first_run(&cfg_with(&[("codex", true)]), &path, false));
    }

    #[test]
    fn even_a_disconnected_flag_counts_as_onboarded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[sources]\ncodex = false\n").unwrap();
        assert!(!is_first_run(&cfg_with(&[("codex", false)]), &path, false));
    }

    #[test]
    fn a_malformed_existing_config_is_not_a_first_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = [unclosed\n[sources]\ncodex = true\n").unwrap();
        let mut warnings = Vec::new();
        let cfg = crate::config::load(&path, &mut warnings);
        assert!(cfg.sources.is_empty(), "load degraded to defaults");
        assert!(!warnings.is_empty(), "…with a warning");
        assert!(
            !is_first_run(&cfg, &path, !warnings.is_empty()),
            "a malformed config means previously configured, not first run"
        );
    }
}
