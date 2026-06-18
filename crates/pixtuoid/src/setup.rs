//! First-run detection for the cinematic onboarding (PR2).
//!
//! `pixtuoid run` (the TUI) plays a one-time "move-in" overlay on its first ever
//! launch; the headless `pixtuoid setup [--yes]` presenter (in `main.rs`) is the
//! scriptable twin (Raycast / CI / scripting). The signal lives here as ONE pure
//! predicate so both surfaces — and their tests — agree on what "first run" means.
//!
//! `pub` (not `pub(crate)`) because the binary's `main.rs` is a separate crate
//! from this lib and computes it in `build_run_config` (the same reason
//! `install::has_hooks` is `pub`).

use std::path::Path;

use crate::config::AppConfig;

/// First run = no config file yet, OR a config that exists but has never written
/// a `[sources]` flag.
///
/// The second arm mirrors `config::resolve_connected`'s MIGRATE condition (an
/// absent `[sources]` table falls back to install-state defaults): a user who
/// has never bound a source through the panel/CLI is, by the same token, a user
/// who has never been onboarded. Once any connect/disconnect persists a flag the
/// table is non-empty and onboarding never re-triggers — even a *disconnected*
/// flag (`false`) counts as "seen", since the table is non-empty.
pub fn is_first_run(cfg: &AppConfig, path: &Path) -> bool {
    !path.exists() || cfg.sources.is_empty()
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
        assert!(is_first_run(&AppConfig::default(), &missing));
    }

    #[test]
    fn existing_config_with_no_sources_table_is_first_run() {
        // A config exists (a theme, say) but no `[sources]` flag was ever
        // written — the migrate condition, so still a first run.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"dracula\"\n").unwrap();
        assert!(is_first_run(&cfg_with(&[]), &path));
    }

    #[test]
    fn existing_config_with_a_connected_flag_is_not_first_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[sources]\ncodex = true\n").unwrap();
        assert!(!is_first_run(&cfg_with(&[("codex", true)]), &path));
    }

    #[test]
    fn even_a_disconnected_flag_counts_as_onboarded() {
        // Connected-then-disconnected (flag = false) ⇒ the user HAS seen
        // onboarding; the non-empty table suppresses a re-run.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[sources]\ncodex = false\n").unwrap();
        assert!(!is_first_run(&cfg_with(&[("codex", false)]), &path));
    }
}
