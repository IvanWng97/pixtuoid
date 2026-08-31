use std::path::{Path, PathBuf};

use anyhow::Result;

pub(crate) type ExtraArtifactsFn = fn(hook_path: &Path) -> Result<Vec<(PathBuf, String)>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryStrategy {
    /// Write the BARE name and rely on PATH. An unresolvable binary is non-fatal
    /// (falls back to the bare name); warn if not on PATH.
    BareNameOnPath,
    /// EMBED the resolved absolute path. An unresolvable binary is FATAL.
    EmbedAbsolute,
}

/// `changed` compares the PARSED document before and after the merge, never the
/// serialized bytes (which always differ from a hand-formatted file). A byte
/// comparison would make a semantic no-op look like a change, triggering a
/// destructive rewrite and backup deletion on `uninstall`.
#[derive(Debug)]
pub struct MergeOutcome {
    pub content: String,
    pub changed: bool,
}

/// A single install destination (one CLI's config file). `const` data, not
/// `static`: `&CONST` in `const TARGETS` is legal via rvalue static promotion.
#[derive(Debug)]
pub struct Target {
    /// The `--target` / CLI-facing id, which does NOT always equal the core
    /// source id — join against the source registry via `core_source`, never this.
    pub name: &'static str,
    /// The core `SourceDescriptor.name` this target installs hooks FOR — usually
    /// `name`, but Claude's target is "claude" while its source is "claude-code".
    /// The Sources panel joins its rows to targets on this via `by_source`.
    pub core_source: &'static str,
    pub display_name: &'static str,
    /// Default config path (reads $HOME, hence a fn not a const). Errs when no
    /// home dir resolves — a CWD fallback would "succeed" with a file the CLI
    /// never reads.
    pub default_config_path: fn() -> Result<PathBuf>,
    /// Build the command string written into config from the resolved binary.
    /// `explicit` (the user passed `--hook-path`) always wins over a bare
    /// PATH-resolved name. Usually this IS the verbatim command written for every
    /// event, but a target's `merge_install` MAY append a per-entry suffix —
    /// CodeWhale bakes ` --event <name>` onto each entry, so its `hook_command`
    /// returns a per-source BASE that `merge_install` extends.
    pub hook_command: fn(resolved: &Path, explicit: bool) -> Result<String>,
    /// Parse `content`, inject managed hook entries, reserialize. MUST treat
    /// empty/whitespace-only content as the empty document — never error on empty.
    pub merge_install: fn(content: &str, hook_cmd: &str) -> Result<MergeOutcome>,
    /// Remove only managed entries. Same empty rule.
    pub merge_uninstall: fn(content: &str) -> Result<MergeOutcome>,
    /// Verify the installed config is structurally sound — PURE over the config
    /// content, extracting the shim path for `install::verify_target` to stat.
    /// Per-source format knowledge stays here, like the merge fns.
    pub verify_schema: fn(content: &str) -> crate::install::verify::SchemaParse,
    pub binary_strategy: BinaryStrategy,
    /// Presence probe overriding the default config-file-exists check. Needed
    /// when the file we WRITE is not a file the CLI CREATES — checking it would
    /// mean auto-detection can never fire for such a target.
    pub presence_probe: Option<fn() -> bool>,
    /// Extra wholly-owned artifacts written verbatim on install (absolute path +
    /// content) BEYOND the merged `config_path`, taking the resolved shim path to
    /// bake in. Left in place on uninstall (the config un-merge disables loading).
    /// **Rewritten verbatim on every (re)install**, even when the config merge is
    /// a semantic no-op — the deliberate refresh that repairs a tampered or stale
    /// plugin file. The returned dir comes from the CLI's OWN state resolver, not
    /// the `--config` override a test isolates itself with, so a test must
    /// redirect that resolver too or it writes the developer's real plugin.
    pub extra_artifacts: Option<ExtraArtifactsFn>,

    /// A one-line note the presenters append after a SUCCESSFUL connect, when
    /// installing is not the last step the user must take. `None` for every target
    /// whose hooks take effect on the CLI's next run.
    pub post_install_hint: Option<&'static str>,
}

pub(crate) const BACKUP_SUFFIX: &str = "pixtuoid.bak";

pub(crate) const CLAUDE: Target = Target {
    name: "claude",
    core_source: pixtuoid_core::source::claude_code::SOURCE_NAME,
    display_name: "Claude Code",
    default_config_path: crate::install::claude::default_config_path,
    hook_command: crate::install::claude::hook_command,
    merge_install: crate::install::claude::merge_install,
    merge_uninstall: crate::install::claude::merge_uninstall,
    verify_schema: crate::install::claude::verify_schema,
    // Windows: the exec form embeds the absolute path, and a hook spawned without
    // a shell can't PATH-search, so an unresolvable binary is fatal there.
    binary_strategy: if cfg!(windows) {
        BinaryStrategy::EmbedAbsolute
    } else {
        BinaryStrategy::BareNameOnPath
    },
    presence_probe: None,
    extra_artifacts: None,
    post_install_hint: None,
};

pub(crate) const CODEX: Target = Target {
    name: "codex",
    core_source: pixtuoid_core::source::codex::SOURCE_NAME,
    display_name: "Codex",
    default_config_path: crate::install::codex::default_config_path,
    hook_command: crate::install::codex::hook_command,
    merge_install: crate::install::codex::merge_install,
    merge_uninstall: crate::install::codex::merge_uninstall,
    verify_schema: crate::install::codex::verify_schema,
    binary_strategy: BinaryStrategy::EmbedAbsolute,
    presence_probe: None,
    extra_artifacts: None,
    post_install_hint: None,
};

pub(crate) const REASONIX: Target = Target {
    name: "reasonix",
    core_source: pixtuoid_core::source::reasonix::SOURCE_NAME,
    display_name: "Reasonix",
    default_config_path: crate::install::reasonix::default_config_path,
    hook_command: crate::install::reasonix::hook_command,
    merge_install: crate::install::reasonix::merge_install,
    merge_uninstall: crate::install::reasonix::merge_uninstall,
    verify_schema: crate::install::reasonix::verify_schema,
    binary_strategy: BinaryStrategy::EmbedAbsolute,
    presence_probe: Some(crate::install::reasonix::detect_installed),
    extra_artifacts: None,
    post_install_hint: None,
};

pub(crate) const CODEWHALE: Target = Target {
    name: "codewhale",
    core_source: pixtuoid_core::source::codewhale::SOURCE_NAME,
    display_name: "CodeWhale",
    default_config_path: crate::install::codewhale::default_config_path,
    hook_command: crate::install::codewhale::hook_command,
    merge_install: crate::install::codewhale::merge_install,
    merge_uninstall: crate::install::codewhale::merge_uninstall,
    verify_schema: crate::install::codewhale::verify_schema,
    binary_strategy: BinaryStrategy::EmbedAbsolute,
    presence_probe: Some(crate::install::codewhale::detect_installed),
    extra_artifacts: None,
    post_install_hint: None,
};

pub(crate) const OPENCODE: Target = Target {
    name: "opencode",
    core_source: pixtuoid_core::source::opencode::SOURCE_NAME,
    display_name: "opencode",
    default_config_path: crate::install::opencode::default_config_path,
    hook_command: crate::install::opencode::hook_command,
    merge_install: crate::install::opencode::merge_install,
    merge_uninstall: crate::install::opencode::merge_uninstall,
    verify_schema: crate::install::opencode::verify_schema,
    binary_strategy: BinaryStrategy::EmbedAbsolute,
    // A post-uninstall stub still exists, so detect on opencode's own dirs
    // rather than our plugin file.
    presence_probe: Some(crate::install::opencode::detect_installed),
    extra_artifacts: None,
    post_install_hint: None,
};

pub(crate) const GROK: Target = Target {
    name: "grok",
    core_source: pixtuoid_core::source::grok::SOURCE_NAME,
    display_name: "Grok Build",
    default_config_path: crate::install::grok::default_config_path,
    hook_command: crate::install::grok::hook_command,
    merge_install: crate::install::grok::merge_install,
    merge_uninstall: crate::install::grok::merge_uninstall,
    verify_schema: crate::install::grok::verify_schema,
    binary_strategy: BinaryStrategy::EmbedAbsolute,
    presence_probe: Some(crate::install::grok::detect_installed),
    extra_artifacts: None,
    post_install_hint: None,
};

pub(crate) const CURSOR: Target = Target {
    name: "cursor",
    core_source: pixtuoid_core::source::cursor::SOURCE_NAME,
    display_name: "Cursor CLI",
    default_config_path: crate::install::cursor::default_config_path,
    hook_command: crate::install::cursor::hook_command,
    merge_install: crate::install::cursor::merge_install,
    merge_uninstall: crate::install::cursor::merge_uninstall,
    verify_schema: crate::install::cursor::verify_schema,
    binary_strategy: BinaryStrategy::EmbedAbsolute,
    presence_probe: Some(crate::install::cursor::detect_installed),
    extra_artifacts: None,
    post_install_hint: None,
};

pub(crate) const HERMES: Target = Target {
    name: "hermes",
    core_source: pixtuoid_core::source::hermes::SOURCE_NAME,
    display_name: "Hermes Agent",
    default_config_path: crate::install::hermes::default_config_path,
    hook_command: crate::install::hermes::hook_command,
    merge_install: crate::install::hermes::merge_install,
    merge_uninstall: crate::install::hermes::merge_uninstall,
    verify_schema: crate::install::hermes::verify_schema,
    binary_strategy: BinaryStrategy::EmbedAbsolute,
    // Hermes OWNS config.yaml (model/provider), so probe the home dir instead.
    presence_probe: Some(crate::install::hermes::detect_installed),
    extra_artifacts: None,
    post_install_hint: None,
};

pub(crate) const OPENCLAW: Target = Target {
    name: "openclaw",
    core_source: pixtuoid_core::source::openclaw::SOURCE_NAME,
    display_name: "OpenClaw",
    // A two-ownership split: we merge openclaw.json's plugin keys AND write the
    // plugin dir as an extra artifact.
    default_config_path: crate::install::openclaw::default_config_path,
    hook_command: crate::install::openclaw::hook_command,
    merge_install: crate::install::openclaw::merge_install,
    merge_uninstall: crate::install::openclaw::merge_uninstall,
    binary_strategy: BinaryStrategy::EmbedAbsolute,
    // openclaw.json is created by OpenClaw itself; probe its own dir so
    // auto-detect fires whether or not we've installed.
    presence_probe: Some(crate::install::openclaw::detect_installed),
    extra_artifacts: Some(crate::install::openclaw::plugin_artifacts),
    // `plugins.load` is `kind: "restart"` upstream, so a RUNNING gateway does not
    // hot-load the plugin — the lobster appears on its next restart.
    post_install_hint: Some("restart the gateway to load it (openclaw gateway restart)"),
    verify_schema: crate::install::openclaw::verify_schema,
};

pub(crate) const KIMI: Target = Target {
    name: "kimi",
    core_source: pixtuoid_core::source::kimi::SOURCE_NAME,
    display_name: "Kimi Code CLI",
    default_config_path: crate::install::kimi::default_config_path,
    hook_command: crate::install::kimi::hook_command,
    merge_install: crate::install::kimi::merge_install,
    merge_uninstall: crate::install::kimi::merge_uninstall,
    verify_schema: crate::install::kimi::verify_schema,
    binary_strategy: BinaryStrategy::EmbedAbsolute,
    // Kimi may not create config.toml until a provider is configured, so probe the
    // data root rather than the file we write.
    presence_probe: Some(crate::install::kimi::detect_installed),
    extra_artifacts: None,
    post_install_hint: None,
};

pub(crate) const OMP: Target = Target {
    name: "omp",
    core_source: pixtuoid_core::source::omp::SOURCE_NAME,
    display_name: "Oh My Pi",
    default_config_path: crate::install::omp::default_config_path,
    hook_command: crate::install::omp::hook_command,
    merge_install: crate::install::omp::merge_install,
    merge_uninstall: crate::install::omp::merge_uninstall,
    verify_schema: crate::install::omp::verify_schema,
    binary_strategy: BinaryStrategy::EmbedAbsolute,
    // A post-uninstall stub still exists, so detect on omp's own agent dir
    // rather than our file.
    presence_probe: Some(crate::install::omp::detect_installed),
    extra_artifacts: None,
    // Extensions load once at startup (`discoverAndLoadExtensions`).
    post_install_hint: Some("already-running omp sessions must restart to load the bridge"),
};

pub const TARGETS: &[&Target] = &[
    &CLAUDE, &CODEX, &REASONIX, &CODEWHALE, &OPENCODE, &CURSOR, &HERMES, &OPENCLAW, &GROK, &KIMI,
    &OMP,
];

// No prod caller: prod joins on the source id via `by_source`. Kept for the
// target-registry tests; un-gate if prod ever needs it.
#[cfg(test)]
pub(crate) fn by_name(name: &str) -> Option<&'static Target> {
    TARGETS.iter().copied().find(|t| t.name == name)
}

/// The correct join for anything keyed on the source registry — `by_name` keys
/// on the CLI-facing `--target` name, which differs from the source id.
pub(crate) fn by_source(source_id: &str) -> Option<&'static Target> {
    TARGETS.iter().copied().find(|t| t.core_source == source_id)
}

/// Detection = the config FILE exists, not merely its parent dir: an empty
/// ~/.codex must NOT count as present.
pub(crate) fn config_present(path: &Path) -> bool {
    crate::install::io::resolve_symlink(path).exists()
}

pub(crate) fn is_present(t: &Target) -> bool {
    match t.presence_probe {
        Some(probe) => probe(),
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
        assert_eq!(by_name("cursor").unwrap().name, "cursor");
        assert_eq!(by_name("hermes").unwrap().name, "hermes");
        assert_eq!(by_name("openclaw").unwrap().name, "openclaw");
        assert_eq!(by_name("kimi").unwrap().name, "kimi");
        assert!(by_name("nope").is_none());
        assert!(by_name("all").is_none()); // "all" is a meta-value, not a Target
    }

    #[test]
    fn by_source_resolves_claude_via_core_source_not_name() {
        assert_eq!(by_source("claude-code").unwrap().name, "claude");
        assert!(by_name("claude-code").is_none());
        assert!(by_source("nope").is_none());
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
        static NO_HOME: Target = Target {
            name: "nohome",
            core_source: "nohome",
            display_name: "NoHome",
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
            verify_schema: |_| crate::install::verify::SchemaParse::broken("test fake"),
            binary_strategy: BinaryStrategy::EmbedAbsolute,
            presence_probe: None,
            extra_artifacts: None,
            post_install_hint: None,
        };
        assert!(!is_present(&NO_HOME));
    }

    #[test]
    fn every_target_names_a_registered_source() {
        use pixtuoid_core::source::registry;
        for t in TARGETS {
            assert!(
                registry::registered_source_names().any(|s| s == t.core_source),
                "install target {:?} names core_source {:?}, which is not a registered source \
                 (typo, or a renamed source) — fix the target or register the source",
                t.name,
                t.core_source
            );
        }
    }

    // Transcript-bearing sources may legitimately have no target, so the rule is
    // derived from `line_decoder().is_none()` — no exemption list to drift.
    #[test]
    fn every_hook_only_source_has_an_install_target() {
        use pixtuoid_core::source::registry::{self, descriptor_for};
        for src in registry::registered_source_names() {
            let d = descriptor_for(src).expect("registered source must have a descriptor row");
            if d.line_decoder().is_none() {
                assert!(
                    TARGETS.iter().any(|t| t.core_source == src),
                    "hook-only source {src:?} has no install target — its hooks would never \
                     install, so its sprite never appears. Add a Target in install/target.rs."
                );
            }
        }
    }

    #[test]
    fn target_core_sources_are_unique() {
        use std::collections::HashSet;
        let set: HashSet<&str> = TARGETS.iter().map(|t| t.core_source).collect();
        assert_eq!(
            set.len(),
            TARGETS.len(),
            "two install targets claim the same core_source — one source, double hooks"
        );
    }
}
