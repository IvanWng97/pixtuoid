use std::path::{Path, PathBuf};

use anyhow::Result;

/// Builds a target's extra wholly-owned artifacts (absolute path + content) from
/// the resolved shim path — see `Target::extra_artifacts`.
pub(crate) type ExtraArtifactsFn = fn(hook_path: &Path) -> Result<Vec<(PathBuf, String)>>;

/// How a target resolves the hook binary into its config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryStrategy {
    /// Write the BARE name + rely on PATH (Claude). An unresolvable binary is
    /// non-fatal (falls back to the bare name); warn if not on PATH.
    BareNameOnPath,
    /// EMBED the resolved absolute path (Codex/Reasonix/CodeWhale/opencode/
    /// Cursor/OpenClaw). An unresolvable binary is FATAL.
    EmbedAbsolute,
}

/// Result of a merge: the reserialized config plus whether anything *semantically*
/// changed. `changed` is computed by comparing the PARSED document before and after
/// the merge — NOT by byte-comparing serialized output, which always differs from a
/// hand-formatted file (key reorder, indentation, stripped comments). A byte
/// comparison would make a semantic no-op look like a change, triggering a
/// destructive rewrite + backup deletion on `uninstall` (violating the load-bearing
/// "backup is the user's only recovery path" invariant).
#[derive(Debug)]
pub struct MergeOutcome {
    pub content: String,
    pub changed: bool,
}

/// A single install destination (one CLI's config file). Fixed set, resolved
/// at compile time as `const` data — no dyn dispatch (install runs once,
/// synchronously). `&CONST` in `const TARGETS` is legal via rvalue static
/// promotion (Rust 1.21+, MSRV 1.89), so `const` is correct here.
#[derive(Debug)]
pub struct Target {
    /// Stable lowercase id: "claude" | "codex" | "reasonix". This is the
    /// `--target` / CLI-facing name and does NOT always equal the core source
    /// id (Claude's target is "claude" but its source is "claude-code") — join
    /// against the source registry via `core_source`, never `name`.
    pub name: &'static str,
    /// The core `SourceDescriptor.name` this target installs hooks FOR. Usually
    /// equals `name`, but Claude's target is "claude" while its source is
    /// "claude-code". Pins the install↔source bridge: a target naming no
    /// registered source, or a hook-only source with no target (= its hooks
    /// never install, so its sprite never appears), is caught by the tests below.
    /// The Sources panel joins its rows to targets on this via `by_source`.
    pub core_source: &'static str,
    /// Human-readable name for CLI output.
    pub display_name: &'static str,
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
    /// Verify the installed config is structurally SOUND (#309) — PURE over the
    /// config content: the `_pixtuoid` sentinel present, every registered event
    /// still has a managed entry, target-specific extras (CodeWhale `enabled`),
    /// and the shim path extracted for `install::verify_target` to stat. Per-
    /// source format knowledge stays here (invariant #3), like the merge fns.
    pub verify_schema: fn(content: &str) -> crate::install::verify::SchemaParse,
    /// How this target resolves the hook binary into its config — encodes both
    /// whether the bare name relies on PATH (warn if absent) and whether an
    /// unresolvable binary is fatal. Claude = `BareNameOnPath` (soft, falls back
    /// to the bare name so a fresh-machine install still succeeds + the PATH
    /// warning covers the not-yet-on-PATH case); the embedders = `EmbedAbsolute`
    /// (an unresolvable binary aborts the install).
    pub binary_strategy: BinaryStrategy,
    /// Optional presence probe overriding the default config-file-exists check.
    /// Needed when the file we WRITE is not a file the CLI CREATES: Reasonix
    /// never writes `~/.reasonix/settings.json` itself (it is purely
    /// user-authored), so checking it would mean auto-detection can never fire
    /// for the one target it was added for — probe install markers instead.
    pub presence_probe: Option<fn() -> bool>,
    /// Extra wholly-owned artifacts written verbatim on install (absolute path +
    /// content) BEYOND the merged `config_path` — for OpenClaw, the plugin dir
    /// (manifest + package.json + entry module) that the openclaw.json
    /// `plugins.load.paths` entry points at. Takes the resolved shim path to bake
    /// into the entry module. `None` for single-file targets. Left in place on
    /// uninstall (the config un-merge disables loading) — an accepted residual
    /// like opencode's stub. **Rewritten verbatim on every (re)install**, even
    /// when the config merge is a semantic no-op — the deliberate refresh that
    /// repairs a tampered/stale plugin file (`install_target` does an
    /// unconditional write, NOT a content-diff).
    pub extra_artifacts: Option<ExtraArtifactsFn>,
}

/// Backup suffix — the same constant for every target (not a per-target field).
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
    // Unix: bare "pixtuoid-hook" relies on PATH — soft resolution (warn only).
    // Windows: exec form embeds the absolute path, so an unresolvable binary is
    // fatal (same as Codex) — the hook spawned without a shell can't PATH-search.
    binary_strategy: if cfg!(windows) {
        BinaryStrategy::EmbedAbsolute
    } else {
        BinaryStrategy::BareNameOnPath
    },
    presence_probe: None,
    extra_artifacts: None,
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
    // The plugin embeds the absolute shim path (opencode runs it under Bun, no
    // PATH reliance), so an unresolvable binary is fatal.
    binary_strategy: BinaryStrategy::EmbedAbsolute,
    // The plugin file we WRITE is also a file opencode could otherwise lack on a
    // fresh install, and a post-uninstall stub still exists — so detect on the
    // `@pixtuoid-opencode-plugin` sentinel, not mere file existence.
    presence_probe: Some(crate::install::opencode::detect_installed),
    extra_artifacts: None,
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
    // Cursor never creates ~/.cursor/hooks.json itself — probe ~/.cursor instead.
    presence_probe: Some(crate::install::cursor::detect_installed),
    extra_artifacts: None,
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
    // Hermes ARGV-execs the command with the absolute path embedded (no shell PATH
    // reliance), so an unresolvable binary is fatal.
    binary_strategy: BinaryStrategy::EmbedAbsolute,
    // Hermes creates ~/.hermes itself and OWNS config.yaml (model/provider); probe the
    // home dir rather than the shared config file.
    presence_probe: Some(crate::install::hermes::detect_installed),
    extra_artifacts: None,
};

pub(crate) const OPENCLAW: Target = Target {
    name: "openclaw",
    core_source: pixtuoid_core::source::openclaw::SOURCE_NAME,
    display_name: "OpenClaw",
    // We MERGE openclaw.json (plugins.load.paths + plugins.entries.<id>.enabled +
    // the hooks.allowConversationAccess grant) AND write the plugin dir as an
    // extra artifact — the two-ownership split (confirmed by capture: `plugins
    // install --link` writes exactly these config keys, no separate registry).
    default_config_path: crate::install::openclaw::default_config_path,
    hook_command: crate::install::openclaw::hook_command,
    merge_install: crate::install::openclaw::merge_install,
    merge_uninstall: crate::install::openclaw::merge_uninstall,
    // The plugin bakes the absolute shim path (run under the gateway's Node, no
    // PATH reliance), so an unresolvable binary is fatal.
    binary_strategy: BinaryStrategy::EmbedAbsolute,
    // openclaw.json is created by OpenClaw itself; probe OpenClaw's own dir so
    // auto-detect fires whether or not we've installed (the opencode rationale).
    presence_probe: Some(crate::install::openclaw::detect_installed),
    extra_artifacts: Some(crate::install::openclaw::plugin_artifacts),
    verify_schema: crate::install::openclaw::verify_schema,
};

pub const TARGETS: &[&Target] = &[
    &CLAUDE, &CODEX, &REASONIX, &CODEWHALE, &OPENCODE, &CURSOR, &HERMES, &OPENCLAW,
];

// Test-gated: lookup-by-CLI-name has no prod caller (prod joins on the source id
// via `by_source`); kept for the target-registry tests. Un-gate if prod needs it.
#[cfg(test)]
pub(crate) fn by_name(name: &str) -> Option<&'static Target> {
    TARGETS.iter().copied().find(|t| t.name == name)
}

/// Resolve the install target for a core source id (`SourceDescriptor.name`,
/// e.g. "claude-code"). This is the correct join for anything keyed on the
/// source registry (the Sources panel) — `by_name` keys on the CLI-facing
/// `--target` name, which differs from the source id for Claude.
pub(crate) fn by_source(source_id: &str) -> Option<&'static Target> {
    TARGETS.iter().copied().find(|t| t.core_source == source_id)
}

/// Detection = the config FILE exists (not merely its parent dir): an empty
/// ~/.codex must NOT count as present. Exception: a target whose written file
/// the CLI never creates itself (Reasonix) supplies a `presence_probe` over
/// real install markers instead.
pub(crate) fn config_present(path: &Path) -> bool {
    crate::install::io::resolve_symlink(path).exists()
}

pub(crate) fn is_present(t: &Target) -> bool {
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
        assert_eq!(by_name("cursor").unwrap().name, "cursor");
        assert_eq!(by_name("hermes").unwrap().name, "hermes");
        assert_eq!(by_name("openclaw").unwrap().name, "openclaw");
        assert!(by_name("nope").is_none());
        assert!(by_name("all").is_none()); // "all" is a meta-value, not a Target
    }

    #[test]
    fn by_source_resolves_claude_via_core_source_not_name() {
        // The flagship divergence: the Claude install target is named "claude"
        // but its core source id is "claude-code". The Sources panel joins on
        // core_source — `by_name` on the source id must NOT match.
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
        // No home dir → default_config_path errs → the target is simply not
        // detected (never a panic, never a CWD-relative probe).
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
        };
        assert!(!is_present(&NO_HOME));
    }

    // Bridge: the install TARGETS registry and core's source registry must not
    // silently diverge. The site manifest is already bridge-tested against
    // REGISTERED_SOURCES (`supported_sources_manifest`); the install targets were
    // NOT — the one dual-source-of-truth this codebase otherwise rigorously kills.
    #[test]
    fn every_target_names_a_registered_source() {
        use pixtuoid_core::source::REGISTERED_SOURCES;
        for t in TARGETS {
            assert!(
                REGISTERED_SOURCES.contains(&t.core_source),
                "install target {:?} names core_source {:?}, which is not a REGISTERED_SOURCE \
                 (typo, or a renamed source) — fix the target or register the source",
                t.name,
                t.core_source
            );
        }
    }

    // A source with no JSONL watcher (`line_decoder()` is `None` — a hook-only
    // agent OR a daemon) reaches pixtuoid ONLY through its installed hooks/plugin
    // — so it MUST have an install target, or it is invisible at runtime (hooks
    // never installed → no sprite/mascot ever appears), shipped green.
    // Transcript-bearing sources may legitimately have no target (Antigravity
    // reads its transcript, installs no hooks). Derived from
    // `line_decoder().is_none()`, so there is no hand-maintained exemption list to
    // drift.
    #[test]
    fn every_hook_only_source_has_an_install_target() {
        use pixtuoid_core::source::{registry::descriptor_for, REGISTERED_SOURCES};
        for &src in REGISTERED_SOURCES {
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

    // Two targets claiming one core source would double-install the same hooks.
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
