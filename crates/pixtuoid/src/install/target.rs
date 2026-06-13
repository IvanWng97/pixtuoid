use std::path::{Path, PathBuf};

use anyhow::Result;

/// Result of a merge: the reserialized config plus whether anything *semantically*
/// changed. `changed` is computed by comparing the PARSED document before and after
/// the merge — NOT by byte-comparing serialized output, which always differs from a
/// hand-formatted file (key reorder, indentation, stripped comments). A byte
/// comparison would make a semantic no-op look like a change, triggering a
/// destructive rewrite + backup deletion on `uninstall` (violating the load-bearing
/// "backup is the user's only recovery path" invariant).
pub struct MergeOutcome {
    pub content: String,
    pub changed: bool,
}

/// A single install destination (one CLI's config file). Fixed set, resolved
/// at compile time as `const` data — no dyn dispatch (install runs once,
/// synchronously). `&CONST` in `const TARGETS` is legal via rvalue static
/// promotion (Rust 1.21+, MSRV 1.89), so `const` is correct here.
pub struct Target {
    /// Stable lowercase id: "claude" | "codex" | "reasonix".
    pub name: &'static str,
    /// Human-readable name for CLI output.
    pub display_name: &'static str,
    /// Restart noun for the "→ start a new <noun> session" hint.
    pub restart_noun: &'static str,
    /// Default config path (reads $HOME, hence a fn not a const). Errs when
    /// the path is home-anchored and no home dir resolves — writing into a
    /// CWD fallback would "succeed" with a file the CLI never reads.
    pub default_config_path: fn() -> Result<PathBuf>,
    /// Build the command string written into config from the resolved binary.
    /// Claude returns bare "pixtuoid-hook" on Unix UNLESS `explicit` (the user
    /// passed `--hook-path`, which always wins — then the absolute path is
    /// embedded); Codex/Reasonix always embed the full path (Err on
    /// non-UTF-8). Takes the resolved binary so each target decides how to use it.
    /// Usually this IS the verbatim command written for every event, but a
    /// target's `merge_install` MAY append a per-entry suffix — CodeWhale bakes
    /// ` --event <name>` onto each entry (it sets no event env var), so its
    /// `hook_command` returns a per-source BASE that `merge_install` extends.
    pub hook_command: fn(resolved: &Path, explicit: bool) -> Result<String>,
    /// Parse `content`, inject managed hook entries, reserialize. MUST treat
    /// empty/whitespace-only content as the empty document — never error on empty.
    /// `changed` reflects a SEMANTIC (parsed) diff, not a byte diff.
    pub merge_install: fn(content: &str, hook_cmd: &str) -> Result<MergeOutcome>,
    /// Parse `content`, remove only managed entries, reserialize. Same empty rule.
    pub merge_uninstall: fn(content: &str) -> Result<MergeOutcome>,
    /// True if the bare hook name must resolve on PATH (Claude writes the bare name).
    pub needs_path_warning: bool,
    /// True if `hook_command` EMBEDS the resolved binary path (Codex), so an
    /// unresolvable binary is fatal. False for targets that write the bare name
    /// and rely on PATH (Claude) — those fall back to the bare name rather than
    /// aborting, so a fresh-machine install still succeeds (the PATH warning
    /// covers the not-yet-on-PATH case).
    pub needs_resolved_binary: bool,
    /// Optional courtesy note printed after a successful install — e.g. Codex's
    /// `config.toml` loses comments/ordering on the `toml::Value` round-trip.
    /// Format-agnostic: the orchestrator just prints it, no per-target name-matching.
    pub post_install_note: Option<&'static str>,
    /// Optional presence probe overriding the default config-file-exists check.
    /// Needed when the file we WRITE is not a file the CLI CREATES: Reasonix
    /// never writes `~/.reasonix/settings.json` itself (it is purely
    /// user-authored), so checking it would mean auto-detection can never fire
    /// for the one target it was added for — probe install markers instead.
    pub presence_probe: Option<fn() -> bool>,
}

/// Backup suffix — the same constant for every target (not a per-target field).
pub const BACKUP_SUFFIX: &str = "pixtuoid.bak";

pub const CLAUDE: Target = Target {
    name: "claude",
    display_name: "Claude Code",
    restart_noun: "Claude Code",
    default_config_path: crate::install::claude::default_config_path,
    hook_command: crate::install::claude::hook_command,
    merge_install: crate::install::claude::merge_install,
    merge_uninstall: crate::install::claude::merge_uninstall,
    // Unix: bare "pixtuoid-hook" relies on PATH — soft resolution (warn only).
    // Windows: exec form embeds the absolute path, so an unresolvable binary is
    // fatal (same as Codex) — the hook spawned without a shell can't PATH-search.
    needs_path_warning: !cfg!(windows),
    needs_resolved_binary: cfg!(windows),
    post_install_note: None,
    presence_probe: None,
};

pub const CODEX: Target = Target {
    name: "codex",
    display_name: "Codex",
    restart_noun: "Codex",
    default_config_path: crate::install::codex::default_config_path,
    hook_command: crate::install::codex::hook_command,
    merge_install: crate::install::codex::merge_install,
    merge_uninstall: crate::install::codex::merge_uninstall,
    needs_path_warning: false,
    needs_resolved_binary: true,
    post_install_note: Some(
        "note: comments and formatting in config.toml are not preserved (restore from the backup if needed).",
    ),
    presence_probe: None,
};

pub const REASONIX: Target = Target {
    name: "reasonix",
    display_name: "Reasonix",
    restart_noun: "Reasonix",
    default_config_path: crate::install::reasonix::default_config_path,
    hook_command: crate::install::reasonix::hook_command,
    merge_install: crate::install::reasonix::merge_install,
    merge_uninstall: crate::install::reasonix::merge_uninstall,
    needs_path_warning: false,
    needs_resolved_binary: true,
    post_install_note: None,
    presence_probe: Some(crate::install::reasonix::detect_installed),
};

pub const CODEWHALE: Target = Target {
    name: "codewhale",
    display_name: "CodeWhale",
    restart_noun: "CodeWhale",
    default_config_path: crate::install::codewhale::default_config_path,
    hook_command: crate::install::codewhale::hook_command,
    merge_install: crate::install::codewhale::merge_install,
    merge_uninstall: crate::install::codewhale::merge_uninstall,
    needs_path_warning: false,
    needs_resolved_binary: true,
    post_install_note: Some(
        "note: comments and formatting in config.toml are not preserved (restore from the backup if needed).",
    ),
    presence_probe: Some(crate::install::codewhale::detect_installed),
};

pub const OPENCODE: Target = Target {
    name: "opencode",
    display_name: "opencode",
    restart_noun: "opencode",
    default_config_path: crate::install::opencode::default_config_path,
    hook_command: crate::install::opencode::hook_command,
    merge_install: crate::install::opencode::merge_install,
    merge_uninstall: crate::install::opencode::merge_uninstall,
    needs_path_warning: false,
    // The plugin embeds the absolute shim path (opencode runs it under Bun, no
    // PATH reliance), so an unresolvable binary is fatal.
    needs_resolved_binary: true,
    // The managed file is a CODE artifact wholly owned by pixtuoid, not a shared
    // config — uninstall replaces it with a no-op stub (it can't be deleted via
    // the write-only orchestrator); the sentinel-based probe still reports it
    // correctly as removed. Noted so the residual isn't a surprise.
    post_install_note: Some(
        "note: uninstall replaces the plugin with a no-op stub at <config>/plugins/pixtuoid.ts rather than deleting it.",
    ),
    // The plugin file we WRITE is also a file opencode could otherwise lack on a
    // fresh install, and a post-uninstall stub still exists — so detect on the
    // `@pixtuoid-opencode-plugin` sentinel, not mere file existence.
    presence_probe: Some(crate::install::opencode::detect_installed),
};

pub const TARGETS: &[&Target] = &[&CLAUDE, &CODEX, &REASONIX, &CODEWHALE, &OPENCODE];

pub fn by_name(name: &str) -> Option<&'static Target> {
    TARGETS.iter().copied().find(|t| t.name == name)
}

/// Detection = the config FILE exists (not merely its parent dir): an empty
/// ~/.codex must NOT count as present. Exception: a target whose written file
/// the CLI never creates itself (Reasonix) supplies a `presence_probe` over
/// real install markers instead.
pub fn config_present(path: &Path) -> bool {
    crate::install::io::resolve_symlink(path).exists()
}

pub fn is_present(t: &Target) -> bool {
    match t.presence_probe {
        Some(probe) => probe(),
        // An unresolvable default path (no home dir) means there is no config
        // anywhere we could detect — not present.
        None => (t.default_config_path)()
            .map(|p| config_present(&p))
            .unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_name_resolves_claude_and_rejects_unknown() {
        assert_eq!(by_name("claude").unwrap().name, "claude");
        assert_eq!(by_name("codex").unwrap().name, "codex");
        assert_eq!(by_name("reasonix").unwrap().name, "reasonix");
        assert_eq!(by_name("codewhale").unwrap().name, "codewhale");
        assert_eq!(by_name("opencode").unwrap().name, "opencode");
        assert!(by_name("nope").is_none());
        assert!(by_name("all").is_none()); // "all" is a meta-value, not a Target
    }

    #[test]
    fn config_present_checks_file_existence() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("x.json");
        assert!(!config_present(&p));
        std::fs::write(&p, "{}").unwrap();
        assert!(config_present(&p));
    }

    #[test]
    fn is_present_false_when_default_path_unresolvable() {
        // No home dir → default_config_path errs → the target is simply not
        // detected (never a panic, never a CWD-relative probe).
        static NO_HOME: Target = Target {
            name: "nohome",
            display_name: "NoHome",
            restart_noun: "NoHome",
            default_config_path: || Err(anyhow::anyhow!("cannot resolve the home directory")),
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
            needs_path_warning: false,
            needs_resolved_binary: false,
            post_install_note: None,
            presence_probe: None,
        };
        assert!(!is_present(&NO_HOME));
    }
}
