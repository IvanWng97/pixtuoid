//! The hermetic tier's per-CLI facts: how each real CLI is installed, isolated,
//! pointed at a scripted model (aimock) and driven, and which hook events each
//! scenario must fire. The registry already owns the binary (`version_probe`),
//! the home variable (`home_env`) and the pinned version (`verified_version`);
//! a profile holds only what it doesn't.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::harness::CaptureRecord;
use crate::source::registry;

pub mod verify;

/// The key every profile's CLI is handed for its scripted model: aimock
/// accepts any, and a real provider must refuse it.
pub const DUMMY_KEY: &str = "hermetic-dummy-key";

/// What a `system_config` path spells the operator's user name as.
pub const USER_PLACEHOLDER: &str = "{user}";

/// How users install the CLI, so the tier runs the artifact they run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Package {
    /// An npm package; the tier installs `<name>@<version>` into its own prefix.
    Npm(&'static str),
}

/// What a scenario's script drives a CLI through. Shared by every CLI, so one CLI
/// cannot be held to a weaker bar than another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// A turn that reads a file: the session registers in the workspace, a tool
    /// starts with a detail naming it, and it ends.
    ToolRun,
}

impl Shape {
    /// The scenario's name: its script's file stem and its run directory.
    pub const fn name(self) -> &'static str {
        match self {
            Self::ToolRun => "tool-run",
        }
    }
}

/// One scripted turn of a CLI.
#[derive(Debug)]
pub struct Scenario {
    /// What the run must look like.
    pub shape: Shape,
    /// The hook events this run must fire, as `Profile::event_name` reads them.
    pub expects: &'static [&'static str],
}

/// A config file the tier writes into a sandbox home BEFORE `pixtuoid connect`,
/// which then merges into the same file.
#[derive(Debug)]
pub struct ConfigFile {
    /// The environment variable naming the home the file lives under.
    pub home: &'static str,
    /// The file's path under that home.
    pub path: &'static str,
    /// Its contents; `{port}` and `{key}` expand.
    pub template: &'static str,
}

/// One CLI's hermetic facts.
#[derive(Debug)]
pub struct Profile {
    /// The registered source id.
    pub source: &'static str,
    /// How users install it.
    pub package: Package,
    /// The headless invocation; `{prompt}` expands (the recorder's own rule).
    pub invocation: &'static [&'static str],
    /// Environment for the CLI: endpoint, dummy key and opt-outs; `{port}` and
    /// `{key}` expand.
    pub env: &'static [(&'static str, &'static str)],
    /// Homes the registry row does not name, each pointed into the sandbox.
    pub homes: &'static [&'static str],
    /// Fixed system paths the CLI reads policy or config from, which no home
    /// redirects: a run on a machine that has one is not hermetic.
    /// [`USER_PLACEHOLDER`] is the operator's user name. A macOS managed-preferences domain whose
    /// plist path upstream does not fix is not checked.
    pub system_config: &'static [&'static str],
    /// Names the CLI reads from the workspace's ancestor directories, which
    /// sit outside the sandbox: a run under one that has any is not hermetic.
    pub ancestor_config: &'static [&'static str],
    /// Config written before `connect`.
    pub config: &'static [ConfigFile],
    /// The scripted turns.
    pub scenarios: &'static [Scenario],
    /// Registered hooks no scenario expects yet. Shrink-only: a run that fires
    /// one is Stale (`verify::verify_source`).
    pub not_yet: &'static [&'static str],
    /// The hook event a captured payload is, in `registers()` terms.
    pub(crate) event_name: fn(&Value) -> Option<&str>,
    /// Fields a decoder reads in a committed capture that no scenario carries
    /// yet, as `(kind, path)` the field census names them. Shrink-only: a run
    /// that carries one is Stale.
    pub(crate) not_yet_fields: &'static [(&'static str, &'static str)],
}

impl Profile {
    /// Its registry row.
    ///
    /// # Panics
    ///
    /// If the profile names an unregistered source, which the roster test refuses.
    pub fn descriptor(&self) -> &'static registry::SourceDescriptor {
        registry::descriptor_for(self.source).expect("a profile names a registered source")
    }
}

/// A run directory's manifest, beside the raw capture.
pub const MANIFEST: &str = "run.json";
/// The headless pixtuoid's output (scene lines and its log) for a run.
pub const SCENE_LOG: &str = "scene.log";

/// What one run was, written by the orchestrator beside the run's raw capture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// The registered source id.
    pub source: String,
    /// The scenario's shape name.
    pub scenario: String,
    /// The recorder's record of the CLI's run.
    pub capture: CaptureRecord,
    /// The aimock version that played the model.
    pub aimock: String,
    /// Requests aimock answered.
    pub requests: usize,
    /// Requests no script fixture matched.
    pub unmatched: usize,
}

/// Why a registered source has no profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Absence {
    /// Its model endpoint cannot be redirected, so no scripted turn can run.
    NoModelEndpoint,
    /// A profile is owed. Shrink-only.
    AwaitingProfile,
}

fn hook_event_name(v: &Value) -> Option<&str> {
    v.get("hook_event_name")?.as_str()
}

fn type_field(v: &Value) -> Option<&str> {
    v.get("type")?.as_str()
}

/// The profiled CLIs.
pub const PROFILES: &[Profile] = &[
    Profile {
        source: "claude-code",
        package: Package::Npm("@anthropic-ai/claude-code"),
        invocation: &["claude", "-p", "{prompt}"],
        env: &[
            ("ANTHROPIC_BASE_URL", "http://127.0.0.1:{port}"),
            ("ANTHROPIC_API_KEY", "{key}"),
            // Auto-updates, telemetry, error reporting and the fast-mode probe
            // (code.claude.com/docs/en/env-vars); the plugin marketplace's
            // auto-install has its own switch.
            ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
            ("CLAUDE_CODE_DISABLE_OFFICIAL_MARKETPLACE_AUTOINSTALL", "1"),
        ],
        homes: &[],
        // Managed settings, managed MCP servers and a managed CLAUDE.md, on
        // macOS and Linux (code.claude.com/docs/en/managed-settings).
        system_config: &[
            "/Library/Application Support/ClaudeCode",
            "/etc/claude-code",
        ],
        // Memory files up to `/`, and AGENTS.md where none is found
        // (code.claude.com/docs/en/memory).
        ancestor_config: &["CLAUDE.md", "CLAUDE.local.md", ".claude", "AGENTS.md"],
        config: &[],
        scenarios: &[Scenario {
            shape: Shape::ToolRun,
            expects: &["SessionStart", "PreToolUse", "PostToolUse", "SessionEnd"],
        }],
        not_yet: &[
            "Notification",
            "PermissionRequest",
            "SubagentStart",
            "SubagentStop",
        ],
        event_name: hook_event_name,
        not_yet_fields: &[
            // A Bash call's; the scripted turn reads a file.
            ("hook:PreToolUse", "/tool_input/command"),
            ("tx:assistant", "/message/content/*/input/command"),
            // `claude -p` sends no model; the interactive TUI does
            // (fixtures/claude-code/permission-recorded).
            ("hook:SessionStart", "/model"),
            // A subagent transcript's (fixtures/claude-code/subagent-recorded).
            ("tx:assistant", "/attributionAgent"),
            // aimock scripts only input and output tokens for Anthropic.
            ("tx:assistant", "/message/usage/cache_creation_input_tokens"),
        ],
    },
    Profile {
        source: "codex",
        package: Package::Npm("@openai/codex"),
        // A hook runs only once trusted, and trust is a hash of codex's own
        // normalized hook identity (`codex-rs/hooks/src/engine/discovery.rs:775`)
        // that seeding would have to re-implement; this flag is upstream's path
        // for "automation that already vets hook sources"
        // (`codex-rs/utils/cli/src/shared_options.rs:62-64`).
        invocation: &[
            "codex",
            "exec",
            "--dangerously-bypass-hook-trust",
            "{prompt}",
        ],
        env: &[("HERMETIC_API_KEY", "{key}")],
        homes: &[],
        // System config, requirements and managed config; `CODEX_HOME` never
        // reaches them (`codex-rs/config/src/loader/mod.rs:69`, `:102-107`,
        // `layer_io.rs:222-227`).
        system_config: &["/etc/codex"],
        // Its project config and AGENTS.md stop at the project root
        // (`codex-rs/config/src/loader/mod.rs:1662`, `core/src/agents_md.rs:8-12`),
        // which the recorder's `git init` makes the workspace.
        ancestor_config: &[],
        config: &[ConfigFile {
            home: "CODEX_HOME",
            path: "config.toml",
            template: r#"model = "gpt-5.5"
model_provider = "aimock"
model_reasoning_effort = "high"

[model_providers.aimock]
name = "aimock"
base_url = "http://127.0.0.1:{port}/v1"
env_key = "HERMETIC_API_KEY"
wire_api = "responses"
"#,
        }],
        scenarios: &[Scenario {
            shape: Shape::ToolRun,
            expects: &[
                "SessionStart",
                "UserPromptSubmit",
                "PreToolUse",
                "PostToolUse",
                "Stop",
                "SessionEnd",
            ],
        }],
        not_yet: &["SubagentStart", "SubagentStop", "PermissionRequest"],
        event_name: hook_event_name,
        // aimock scripts only input and output tokens for the Responses API.
        not_yet_fields: &[
            (
                "tx:event_msg",
                "/payload/info/last_token_usage/cached_input_tokens",
            ),
            (
                "tx:event_msg",
                "/payload/info/last_token_usage/reasoning_output_tokens",
            ),
        ],
    },
    // opencode's cites are at v2.0.28, npm's `latest`, the release that added
    // the system config below.
    Profile {
        source: "opencode",
        package: Package::Npm("@opencode/cli"),
        // A plain `run` attaches to the background service, whose env never
        // carries the run's PIXTUOID_SOCKET; a private server inherits it
        // (`packages/cli/src/services/standalone.ts:24-25`). Only an explicit
        // `--model` reaches the session's events (`packages/cli/src/run/run.ts:91`).
        invocation: &[
            "opencode",
            "run",
            "--standalone",
            "--model",
            "openai/gpt-5.2",
            "{prompt}",
        ],
        // The models.dev fetch and the update check
        // (`packages/cli/src/server-process.ts:113`, `services/updater.ts:534`).
        env: &[
            ("OPENCODE_DISABLE_MODELS_FETCH", "1"),
            ("OPENCODE_DISABLE_AUTOUPDATE", "1"),
        ],
        homes: &["OPENCODE_CONFIG_DIR"],
        // System config and managed preferences
        // (`packages/core/src/config/discovery.ts:27-38`).
        system_config: &[
            "/Library/Application Support/opencode",
            "/etc/opencode",
            "/Library/Managed Preferences/ai.opencode.managed.plist",
            "/Library/Managed Preferences/{user}/ai.opencode.managed.plist",
        ],
        // Project config up to `/` (`packages/core/src/config/discovery.ts:12`,
        // `:80`); its AGENTS.md walk stops at the git worktree
        // (`config/plugin/instruction.ts:37`), the workspace the recorder inits.
        ancestor_config: &[
            ".claude",
            ".agents",
            ".opencode",
            "opencode.jsonc",
            "opencode.json",
        ],
        config: &[ConfigFile {
            home: "OPENCODE_CONFIG_DIR",
            path: "opencode.jsonc",
            template: r#"{
  "model": "openai/gpt-5.2",
  "providers": {
    "openai": {
      "settings": { "baseURL": "http://127.0.0.1:{port}/v1", "apiKey": "{key}" },
      "models": { "gpt-5.2": {} }
    }
  }
}
"#,
        }],
        scenarios: &[Scenario {
            shape: Shape::ToolRun,
            expects: &[
                "session.created",
                "session.execution.started",
                "session.tool.input.started",
                "session.tool.success",
                "session.execution.succeeded",
            ],
        }],
        not_yet: &[
            "session.deleted",
            "session.execution.failed",
            "session.execution.interrupted",
            "session.tool.failed",
            "permission.asked",
        ],
        event_name: type_field,
        // A child session's (tests/sources/opencode/fixtures).
        not_yet_fields: &[
            ("hook:session.created", "/data/parentID"),
            ("hook:session.execution.started", "/data/parentID"),
        ],
    },
];

/// Every registered source without a profile, and why.
pub const NOT_HERMETIC: &[(&str, Absence)] = &[
    // Owner, 2026-10-13: no custom model endpoint, so no upstream detection.
    ("cursor", Absence::NoModelEndpoint),
    ("antigravity", Absence::AwaitingProfile),
    ("reasonix", Absence::AwaitingProfile),
    ("dsh", Absence::AwaitingProfile),
    ("codewhale", Absence::AwaitingProfile),
    ("openclaw", Absence::AwaitingProfile),
    ("copilot", Absence::AwaitingProfile),
    ("hermes", Absence::AwaitingProfile),
    ("grok", Absence::AwaitingProfile),
    ("omp", Absence::AwaitingProfile),
    ("kimi", Absence::AwaitingProfile),
];

/// The profile for `source`, if it has one.
pub fn profile(source: &str) -> Option<&'static Profile> {
    PROFILES.iter().find(|p| p.source == source)
}

/// Where each profile's scenario scripts live, as `<source>/<shape>.json`, and
/// the scripted model's npm lockfile, under `aimock/`.
pub fn scripts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/sources/hermetic")
}

impl Scenario {
    /// `source`'s script for this scenario.
    pub fn script(&self, source: &str) -> PathBuf {
        scripts_dir()
            .join(source)
            .join(format!("{}.json", self.shape.name()))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn every_registered_source_is_profiled_or_says_why_not_and_never_both() {
        let registered: BTreeSet<&str> = registry::registered_source_names().collect();
        let profiled: BTreeSet<&str> = PROFILES.iter().map(|p| p.source).collect();
        let absent: BTreeSet<&str> = NOT_HERMETIC.iter().map(|(s, _)| *s).collect();
        assert_eq!(profiled.len(), PROFILES.len(), "a source profiled twice");
        assert_eq!(absent.len(), NOT_HERMETIC.len(), "a source listed twice");
        assert!(
            profiled.is_disjoint(&absent),
            "profiled AND listed absent: {:?}",
            profiled.intersection(&absent).collect::<Vec<_>>()
        );
        let covered: BTreeSet<&str> = profiled.union(&absent).copied().collect();
        assert_eq!(
            covered, registered,
            "the tier's population is the roster: profile each source or say why not"
        );
    }

    #[test]
    fn every_scenario_has_its_script_and_one_scenario_per_shape() {
        for p in PROFILES {
            let mut shapes = BTreeSet::new();
            for s in p.scenarios {
                assert!(
                    shapes.insert(s.shape.name()),
                    "{}: {} twice",
                    p.source,
                    s.shape.name()
                );
                let script = s.script(p.source);
                assert!(
                    script.is_file(),
                    "{}: no script at {}",
                    p.source,
                    script.display()
                );
                assert!(
                    !s.expects.is_empty(),
                    "{}: a scenario expecting nothing",
                    p.source
                );
            }
            assert!(
                !p.scenarios.is_empty(),
                "{}: a profile with no scenario",
                p.source
            );
        }
    }

    #[test]
    fn every_profile_runs_its_registered_binary() {
        for p in PROFILES {
            let probe = p
                .descriptor()
                .version_probe
                .expect("a profiled CLI has a version probe");
            assert_eq!(
                p.invocation.first(),
                probe.first(),
                "{}: the invocation runs a binary the registry does not name",
                p.source
            );
        }
    }

    #[test]
    fn a_config_file_lives_under_a_home_the_sandbox_owns() {
        for p in PROFILES {
            for c in p.config {
                let owned = p.homes.contains(&c.home) || p.descriptor().home_env == Some(c.home);
                assert!(owned, "{}: {} is not a sandboxed home", p.source, c.home);
            }
        }
    }

    #[test]
    fn the_drift_watchs_npm_release_installs_the_profiles_package() {
        use crate::source::upstream::Release;
        let Release::Npm { package, .. } = crate::source::opencode::REPO.release else {
            panic!("opencode's watch reads its npm release");
        };
        let p = profile("opencode").expect("profiled");
        assert_eq!(p.package, Package::Npm(package));
    }

    #[test]
    fn every_not_yet_field_is_one_a_decoder_reads() {
        for p in PROFILES {
            let reads = verify::committed_reads(p).expect("the committed corpus reads");
            for (kind, path) in p.not_yet_fields {
                assert!(
                    reads.has(kind, path),
                    "{}: no committed capture shows a decoder reading {kind} {path}",
                    p.source
                );
            }
        }
    }

    #[test]
    fn a_system_config_path_is_absolute_and_an_ancestor_config_name_is_one_component() {
        for p in PROFILES {
            // Unix paths, as the tier runs only on Unix: `Path::is_absolute`
            // would want a drive on Windows.
            for path in p.system_config {
                assert!(path.starts_with('/'), "{}: {path}", p.source);
                // A misspelled placeholder would be checked as a literal path
                // that never exists.
                assert!(
                    !path.replace(USER_PLACEHOLDER, "").contains(['{', '}']),
                    "{}: {path}",
                    p.source
                );
            }
            for name in p.ancestor_config {
                assert_eq!(
                    Path::new(name).components().count(),
                    1,
                    "{}: {name}",
                    p.source
                );
            }
        }
    }

    #[test]
    fn a_hook_both_expected_and_not_yet_is_a_stale_entry() {
        for p in PROFILES {
            let expected: BTreeSet<&str> = p
                .scenarios
                .iter()
                .flat_map(|s| s.expects.iter().copied())
                .collect();
            for n in p.not_yet {
                assert!(
                    !expected.contains(n),
                    "{}: {n} is fired by a scenario, so it leaves not_yet",
                    p.source
                );
            }
        }
    }
}
