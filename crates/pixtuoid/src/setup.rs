//! First-run detection for the cinematic onboarding (PR2).
//!
//! `pixtuoid run` (the TUI) plays a one-time "move-in" overlay on its first ever
//! launch; the headless `pixtuoid setup [--yes]` presenter (in the binary's
//! `sources_cli.rs`) is the scriptable twin (Raycast / CI / scripting). The signal lives here as ONE pure
//! predicate so both surfaces — and their tests — agree on what "first run" means.
//!
//! `pub` (not `pub(crate)`) because the binary's `main.rs` is a separate crate
//! from this lib and computes it in `build_run_config`.

use std::path::Path;

use crate::config::AppConfig;

/// First run = no config file yet, OR a config that exists but has never written
/// a `[sources]` flag — UNLESS the load itself degraded (`load_degraded`: the
/// file exists but is malformed/unreadable, so `config::load` fell back to
/// defaults and `cfg.sources` is empty regardless of what's really in the file).
/// An existing-but-broken config means "previously configured", NOT "first run":
/// replaying onboarding over it would (a) mislabel a long-time user as new and
/// (b) funnel them into an apply whose every write is refused by the
/// malformed-config-never-wiped rule (`update_config`) — a flow that cannot
/// succeed. The caller passes `!load_warnings.is_empty()` right after `load`
/// (a missing file returns defaults WITHOUT a warning, so it stays a first run).
///
/// The second arm matches `config::resolve_connected`'s plain default (since
/// 0.12.0 an absent/empty `[sources]` table means NOTHING connected — the old
/// v0.4–0.7 install-state migrate inference is gone): a user who has never
/// bound a source through the panel/CLI is, by the same token, a user who has
/// never been onboarded — including a v0.4–0.7 upgrader whose config predates
/// the flags; onboarding replays for them and IS their re-connect flow. Once
/// any connect/disconnect persists a flag the table is non-empty and
/// onboarding never re-triggers — even a *disconnected* flag (`false`) counts
/// as "seen", since the table is non-empty.
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
        // Missing file ⇒ first run regardless of the (default-empty) cfg.
        // (load() returns defaults on NotFound with NO warning ⇒ not degraded.)
        assert!(is_first_run(&AppConfig::default(), &missing, false));
    }

    #[test]
    fn existing_config_with_no_sources_table_is_first_run() {
        // A config exists (a theme, say) but no `[sources]` flag was ever
        // written — never onboarded, so still a first run.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"dracula\"\n").unwrap();
        assert!(is_first_run(&cfg_with(&[]), &path, false));
    }

    // The 0.12.0 consequence of dropping resolve_connected's migrate
    // inference: a v0.4–0.7 upgrader's config (exists, parses fine, no
    // [sources] table) resolves to NOTHING connected AND reads as a first
    // run — the onboarding wizard replays and they re-connect there
    // (acceptable: onboarding IS the connect flow).
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
        // Connected-then-disconnected (flag = false) ⇒ the user HAS seen
        // onboarding; the non-empty table suppresses a re-run.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[sources]\ncodex = false\n").unwrap();
        assert!(!is_first_run(&cfg_with(&[("codex", false)]), &path, false));
    }

    #[test]
    fn a_malformed_existing_config_is_not_a_first_run() {
        // The user fat-fingers a TOML edit: load() falls back to defaults (so
        // cfg.sources is empty) and pushes a warning ⇒ degraded. Replaying the
        // onboarding cinematic over a real-but-broken config would funnel an
        // existing user into an apply that update_config REFUSES on every row
        // (the malformed-config-never-wiped rule) — so it is NOT a first run.
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
