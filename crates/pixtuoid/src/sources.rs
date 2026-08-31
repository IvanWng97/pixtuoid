//! The source-control CORE: detect / connect / disconnect / reconcile, TUI-free.
//!
//! The mutating ops here are the PERSISTED half — they write the `[sources]`
//! flag + install/uninstall hooks, but DON'T touch a running instance's live
//! `ConnectedSources`, which reflects the change on its next launch.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use pixtuoid_core::source::registry;

use crate::config;
use crate::install::{
    self,
    target::{by_source, is_present, Target},
    InstallReport, UninstallReport,
};

/// The wire-facing outcome token — a CLOSED set, published in the JSON schema
/// as an `enum` so the generated Raycast type is a string-literal UNION.
/// Widening it is a wire change under the `OutcomeRow` handshake rule below,
/// not a free extension: an installed store copy won't match a new token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireOutcome {
    Connected,
    Disconnected,
    NoOp,
    Failed,
}

impl WireOutcome {
    pub fn token(self) -> &'static str {
        match self {
            WireOutcome::Connected => "connected",
            WireOutcome::Disconnected => "disconnected",
            WireOutcome::NoOp => "no_op",
            WireOutcome::Failed => "failed",
        }
    }
}

impl std::fmt::Display for WireOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.token())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeOutcome {
    Connected,
    Disconnected,
    NoOp,
    Failed(String),
}

impl ChangeOutcome {
    /// Kept separate from the enum's `Debug` so the JSON contract can't drift
    /// if a variant is renamed.
    pub fn wire_outcome(&self) -> WireOutcome {
        match self {
            ChangeOutcome::Connected => WireOutcome::Connected,
            ChangeOutcome::Disconnected => WireOutcome::Disconnected,
            ChangeOutcome::NoOp => WireOutcome::NoOp,
            ChangeOutcome::Failed(_) => WireOutcome::Failed,
        }
    }

    pub fn wire_token(&self) -> &'static str {
        self.wire_outcome().token()
    }

    pub fn message(&self) -> Option<&str> {
        match self {
            ChangeOutcome::Failed(msg) => Some(msg),
            _ => None,
        }
    }
}

/// One `{id, outcome, message?}` row of the `--json` batch envelope
/// `connect`/`disconnect`/`sources set` print.
///
/// Treat this wire as PUBLISHED: installed Raycast store copies parse it
/// independently of the binary's version, so a further shape change needs a
/// version handshake, never another flag-day edit. The token spelling is pinned
/// by `change_outcome_wire_tokens_are_stable`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
// `deny_unknown_fields` ⇒ `additionalProperties: false` (rationale on `SourceStatus`).
#[cfg_attr(test, derive(schemars::JsonSchema), schemars(deny_unknown_fields))]
pub struct OutcomeRow {
    /// The registry source id the outcome applies to (e.g. `codex`).
    pub id: String,
    /// The bare machine token; human text rides in `message`.
    pub outcome: WireOutcome,
    /// Human-readable detail, present exactly when the outcome carries any
    /// (`failed`) and OMITTED rather than `null` otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl OutcomeRow {
    /// The message is control-char-stripped HERE, where the untrusted value
    /// enters the row: it folds another CLI's config content verbatim (a failed
    /// `connect codex` embeds the RAW offending source line) and
    /// `sources_cli::text_line` prints it to a real terminal (R0615-06).
    pub fn new(id: String, outcome: &ChangeOutcome) -> Self {
        OutcomeRow {
            id,
            outcome: outcome.wire_outcome(),
            message: outcome.message().map(crate::strip_control_chars),
        }
    }
}

/// The STABLE `pixtuoid sources --json` wire contract the Raycast extension
/// parses. Deliberately a flat DTO, NOT the internal `ConnectionRow` (whose
/// shape is a UI concern free to change).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
// `deny_unknown_fields` ⇒ `additionalProperties: false`, so the generated TS type
// has no index signature and a consumer typo is a `tsc` error.
#[cfg_attr(test, derive(schemars::JsonSchema), schemars(deny_unknown_fields))]
pub struct SourceStatus {
    pub id: String,
    pub display_name: String,
    pub connected: bool,
    pub cli_present: bool,
    /// A health/issue summary (install-broken / decode-drift), or `null` when n/a.
    // Generates `health?: string | null`. Do NOT add `schemars(required)` to force
    // it required: that STRIPS the `| null` → the WRONG `health: string`, and the
    // wire CAN be null. Optional is a harmless superset; nullable is load-bearing.
    pub health: Option<String>,
}

/// Resolve a user-supplied id to the `'static` registry id, or a clear error —
/// the CLI surface takes arbitrary input and `config::save_source_connected`
/// needs `&'static str`.
pub fn registered_id(id: &str) -> Result<&'static str> {
    registry::registered_source_names()
        .find(|s| *s == id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "unknown source '{id}' (known: {})",
                registry::registered_source_names()
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

/// `FlagOnly` for a no-target (JSONL-only) source.
#[derive(Debug)]
pub enum ConnectOutcome {
    FlagOnly,
    Installed(InstallReport),
}

/// Result of a disconnect whose FLAG was persisted false. `Err` from
/// `disconnect` is reserved for the persist-failure abort; a failed hook removal
/// folds in here so the gate still closes (connect rolls back, disconnect does not).
/// Pinned by `map_disconnect_outcome_surfaces_a_folded_hook_removal_failure`.
#[derive(Debug)]
pub enum DisconnectOutcome {
    FlagOnly,
    Uninstalled(UninstallReport),
    HookRemovalFailed(String),
}

/// The step (if any) a user must still take after a successful `connect` —
/// `None` for a target whose hooks take effect on the CLI's next run.
pub fn post_install_hint(id: &str) -> Option<&'static str> {
    crate::install::target::by_source(id).and_then(|t| t.post_install_hint)
}

/// Connect a source: PERSIST the `[sources]` flag FIRST, then — only for a
/// target-bearing source — install its hooks, rolling the flag back if the
/// install fails.
///
/// **Honors the explicit id — it does NOT gate on CLI presence.** Unlike the
/// in-TUI panel (which renders an absent CLI as `NoCli` and refuses the toggle),
/// this installs for any registered id even if that CLI isn't installed yet —
/// pre-provisioning for automation/onboarding where the caller stated intent.
pub fn connect(cfg: &Path, id: &str) -> Result<ConnectOutcome> {
    let sid = registered_id(id)?;
    connect_target(cfg, sid, by_source(sid))
}

/// The persist + install + rollback core, with `target` passed EXPLICITLY so
/// tests can inject a deterministic-fail fake.
fn connect_target(
    cfg: &Path,
    sid: &'static str,
    target: Option<&Target>,
) -> Result<ConnectOutcome> {
    // Capture the PRIOR flag before the optimistic save: a failed re-connect of
    // an ALREADY-connected source must not force `false` — its old working hooks
    // are still on disk, so that would silently disconnect it on the next launch.
    let prior = config::load(cfg, &mut Vec::new()).sources.get(sid).copied();
    config::save_source_connected(cfg, sid, true)?;
    match target {
        Some(t) => match install::install_target(t, None, None) {
            Ok(r) => Ok(ConnectOutcome::Installed(r)),
            Err(e) => {
                // An absent flag rolls back to ABSENT, not an explicit `false` —
                // preserving the `is_first_run` empty-table signal.
                let restore = match prior {
                    Some(v) => config::save_source_connected(cfg, sid, v),
                    None => config::remove_source_connected(cfg, sid),
                };
                if let Err(re) = restore {
                    // The error chain can carry raw config content (a `toml_edit`
                    // parse failure) and `connect` writes tracing to RAW stderr.
                    tracing::warn!(
                        source = sid,
                        error = %crate::strip_control_chars(&format!("{re:#}")),
                        "connect rollback failed to restore the prior [sources] flag"
                    );
                }
                Err(e)
            }
        },
        None => Ok(ConnectOutcome::FlagOnly),
    }
}

/// No rollback — a failed uninstall still leaves the user disconnected (the
/// safer direction).
pub fn disconnect(cfg: &Path, id: &str) -> Result<DisconnectOutcome> {
    let sid = registered_id(id)?;
    disconnect_target(cfg, sid, by_source(sid))
}

fn disconnect_target(
    cfg: &Path,
    sid: &'static str,
    target: Option<&Target>,
) -> Result<DisconnectOutcome> {
    // `?` here = the persist-failure abort (flip nothing). Past it, the flag is
    // false, so a hook-removal error folds into the outcome rather than erroring.
    config::save_source_connected(cfg, sid, false)?;
    Ok(match target {
        Some(t) => match install::uninstall_target(t, None) {
            Ok(r) => DisconnectOutcome::Uninstalled(r),
            Err(e) => DisconnectOutcome::HookRemovalFailed(format!("{e:#}")),
        },
        None => DisconnectOutcome::FlagOnly,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Connect,
    Disconnect,
    NoOp,
}

/// PURE diff of the CURRENT connected-set against the DESIRED one — the
/// declarative "connected set = exactly these" semantics `sources set` needs.
/// Ids outside the source registry are ignored here; the I/O wrapper validates
/// them up front so an unknown id is a loud error, not a silent drop.
pub(crate) fn plan_reconcile(
    current: &HashSet<String>,
    desired: &HashSet<String>,
) -> Vec<(&'static str, Action)> {
    registry::registered_source_names()
        .map(|sid| {
            let want = desired.contains(sid);
            let have = current.contains(sid);
            let action = match (want, have) {
                (true, false) => Action::Connect,
                (false, true) => Action::Disconnect,
                _ => Action::NoOp,
            };
            (sid, action)
        })
        .collect()
}

/// Declarative apply: make the connected set EXACTLY `desired`, reporting each
/// source (a failed item doesn't abort the batch). CURRENT is resolved the same
/// way the boot seed is — explicit `true` flags only.
pub fn reconcile_to(cfg: &Path, desired: &HashSet<String>) -> Vec<(String, ChangeOutcome)> {
    let app = config::load(cfg, &mut Vec::new());
    let current = config::resolve_connected(&app);
    plan_reconcile(&current, desired)
        .into_iter()
        .map(|(sid, action)| (sid.to_string(), apply_one(cfg, sid, action)))
        .collect()
}

fn apply_one(cfg: &Path, sid: &'static str, action: Action) -> ChangeOutcome {
    match action {
        Action::Connect => match connect(cfg, sid) {
            Ok(_) => ChangeOutcome::Connected,
            Err(e) => ChangeOutcome::Failed(format!("{e:#}")),
        },
        Action::Disconnect => match disconnect(cfg, sid) {
            Ok(o) => map_disconnect_outcome(o),
            Err(e) => ChangeOutcome::Failed(format!("{e:#}")),
        },
        Action::NoOp => ChangeOutcome::NoOp,
    }
}

/// The marker a folded hook-removal failure carries into [`ChangeOutcome::Failed`]
/// — a presenter reads it back to tell the fold (the disconnect SUCCEEDED; only
/// the hook removal didn't) apart from a real failure.
pub(crate) const HOOK_REMOVAL_FAILED_PREFIX: &str = "hooks not removed: ";

/// How BOTH presenters word that same fold for a human. `pub` (not `pub(crate)`)
/// because `disconnect`'s CLI arm lives in `main.rs`, a separate crate the lib's
/// `pub(crate)` can't reach. It is the PHRASE only — each surface adds its own
/// framing, so neither can reword the fold alone.
pub const HOOK_REMOVAL_FAILED_PHRASE: &str = "disconnected, but hook removal failed";

/// A folded hook-removal failure MUST surface as `Failed` (with the reason),
/// NEVER a clean `Disconnected` — else a caller hides stale hooks behind it.
fn map_disconnect_outcome(o: DisconnectOutcome) -> ChangeOutcome {
    match o {
        DisconnectOutcome::HookRemovalFailed(e) => {
            ChangeOutcome::Failed(format!("{HOOK_REMOVAL_FAILED_PREFIX}{e}"))
        }
        DisconnectOutcome::FlagOnly | DisconnectOutcome::Uninstalled(_) => {
            ChangeOutcome::Disconnected
        }
    }
}

/// Apply an EXPLICIT per-source decision list (the first-run onboarding apply).
/// Unlike the declarative `reconcile_to`, this touches ONLY the ids passed — a
/// source absent from the list keeps its existing flag, never a surprise write.
pub fn apply_choices(cfg: &Path, choices: &[(&'static str, bool)]) -> Vec<(String, ChangeOutcome)> {
    choices
        .iter()
        .map(|&(sid, want)| {
            let action = if want {
                Action::Connect
            } else {
                Action::Disconnect
            };
            (sid.to_string(), apply_one(cfg, sid, action))
        })
        .collect()
}

/// The onboarding SKIP freeze (pure core): a detected source freezes `true` if
/// it is in the live gate OR already carries installed hooks. The live gate
/// alone is EMPTY on a first run, so a hooked-but-unflagged source would freeze
/// `false` and the skip would UNINSTALL its working hooks.
pub(crate) fn freeze_for_skip(
    detected: impl IntoIterator<Item = &'static str>,
    connected: &HashSet<String>,
    is_hooked: impl Fn(&'static str) -> bool,
) -> Vec<(&'static str, bool)> {
    detected
        .into_iter()
        .map(|id| (id, connected.contains(id) || is_hooked(id)))
        .collect()
}

/// The production onboarding-SKIP freeze. Does blocking per-target config reads
/// (`has_hooks`) inline on the caller's thread — a brief one-shot stall.
pub(crate) fn skip_freeze(
    detected: impl IntoIterator<Item = &'static str>,
    connected: &HashSet<String>,
) -> Vec<(&'static str, bool)> {
    freeze_for_skip(detected, connected, |id| {
        by_source(id).is_some_and(|t| install::has_hooks(t, None))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Connected,
    Disconnected,
    /// A target-bearing CLI that isn't installed on this machine. Carries the
    /// persisted `[sources]` intent because a connected-but-absent source is
    /// still disconnectable — its hooks live in the config, not the missing
    /// binary — so the toggle needs the bit the `NoCli` display hides.
    NoCli {
        connected: bool,
    },
}

impl ConnState {
    pub fn connected(self) -> bool {
        match self {
            ConnState::Connected => true,
            ConnState::Disconnected => false,
            ConnState::NoCli { connected } => connected,
        }
    }
}

/// One row = one agent CLI (the union of registry sources + install targets).
#[derive(Debug, Clone)]
pub struct ConnectionRow {
    /// The core source id — joined to an install target via `Target.core_source`.
    pub source_id: &'static str,
    /// 2-char badge id (`cc`/`cx`/…), from the source descriptor.
    pub label_prefix: &'static str,
    pub display_name: &'static str,
    pub state: ConnState,
    /// The config the hooks live in; `None` for no-target (JSONL-only) rows.
    pub config_path: Option<PathBuf>,
    /// `None` ⇒ connect/disconnect is a flag-only flip (no hooks to write).
    pub target: Option<&'static Target>,
    /// Cached health summary, computed for CONNECTED rows only.
    pub health: Option<String>,
}

/// Per-target filesystem facts, injected so `build_rows_from` is pure. `Some`
/// exactly when the row has an install target.
#[derive(Debug, Clone)]
pub struct RowFacts {
    pub present: bool,
    pub config_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct RowInput {
    pub source_id: &'static str,
    pub label_prefix: &'static str,
    pub target: Option<&'static Target>,
    pub facts: Option<RowFacts>,
    pub connected: bool,
    pub health: Option<String>,
}

/// Title-case the no-target sources — the registry omits their display names.
fn display_name_for(source_id: &'static str) -> &'static str {
    match source_id {
        "antigravity" => "Antigravity",
        "copilot" => "Copilot CLI",
        other => other,
    }
}

pub fn build_rows_from(inputs: Vec<RowInput>) -> Vec<ConnectionRow> {
    inputs
        .into_iter()
        .map(|input| {
            let absent_cli = matches!(
                (&input.target, &input.facts),
                (Some(_), Some(f)) if !f.present
            );
            let state = if absent_cli {
                ConnState::NoCli {
                    connected: input.connected,
                }
            } else if input.connected {
                ConnState::Connected
            } else {
                ConnState::Disconnected
            };
            ConnectionRow {
                source_id: input.source_id,
                label_prefix: input.label_prefix,
                display_name: input
                    .target
                    .map_or_else(|| display_name_for(input.source_id), |t| t.display_name),
                state,
                config_path: input.facts.and_then(|f| f.config_path),
                target: input.target,
                health: input.health,
            }
        })
        .collect()
}

/// Performs FS reads AND, for connected rows, the health rollup
/// (`doctor::diagnose`). `log` is the warn-floor log text.
pub fn build_rows(connected: &HashSet<String>, log: &str) -> Vec<ConnectionRow> {
    let inputs = pixtuoid_core::source::registry::REGISTRY
        .iter()
        .map(|d| {
            // Join on the SOURCE id via `core_source`, NOT `by_name`: Claude's
            // target is "claude" but its source is "claude-code".
            let target = by_source(d.name);
            let facts = target.map(|t| RowFacts {
                present: is_present(t),
                config_path: (t.default_config_path)().ok(),
            });
            let connected = connected.contains(d.name);
            RowInput {
                source_id: d.name,
                label_prefix: d.label_prefix,
                target,
                facts,
                connected,
                health: connected
                    .then(|| crate::doctor::diagnose(d.name, log, None).summary())
                    .flatten(),
            }
        })
        .collect();
    build_rows_from(inputs)
}

/// The wire `connected` is deliberately PRESENT-AND-BOUND (`state == Connected`),
/// NOT the persisted `[sources]` intent bit (`ConnState::connected`, which stays
/// `true` for a connected-but-absent `NoCli` source). Changing it is a `--json`
/// contract change needing `gen-contract`.
fn status_from_row(r: &ConnectionRow) -> SourceStatus {
    SourceStatus {
        id: r.source_id.to_string(),
        display_name: r.display_name.to_string(),
        connected: matches!(r.state, ConnState::Connected),
        cli_present: !matches!(r.state, ConnState::NoCli { .. }),
        health: r.health.clone(),
    }
}

pub fn status(cfg: &Path, log: &str) -> Vec<SourceStatus> {
    let app = config::load(cfg, &mut Vec::new());
    let connected = config::resolve_connected(&app);
    build_rows(&connected, log)
        .iter()
        .map(status_from_row)
        .collect()
}

/// Which agent CLIs are installed on this machine (target-bearing + probed
/// present) — the "offer to connect these" set for first-run onboarding.
pub fn detect() -> Vec<&'static str> {
    registry::registered_source_names()
        .filter(|sid| by_source(sid).is_some_and(is_present))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn post_install_hint_names_a_real_step_only_for_targets_that_need_one() {
        let hint = post_install_hint("openclaw").expect("openclaw needs a restart step");
        assert!(
            hint.contains("restart") && hint.contains("gateway"),
            "the step must actually say to restart the gateway — got {hint:?}"
        );
        assert!(
            hint.contains("openclaw gateway restart"),
            "and name the runnable command, so the user need not guess — got {hint:?}"
        );

        let hint = post_install_hint("omp").expect("omp needs a restart step");
        assert!(
            hint.contains("restart"),
            "extensions load once at startup, so the step is a session restart — got {hint:?}"
        );

        for id in pixtuoid_core::source::registry::registered_source_names() {
            if id == "openclaw" || id == "omp" {
                continue;
            }
            assert!(
                post_install_hint(id).is_none(),
                "{id} declares a post-install step — if that is intended, assert it here"
            );
        }
        assert!(post_install_hint("not-a-source").is_none());
    }

    #[test]
    fn status_from_row_connected_is_present_and_bound_not_persisted_intent() {
        let row = |state| ConnectionRow {
            source_id: "claude-code",
            label_prefix: "cc",
            display_name: "Claude Code",
            state,
            config_path: None,
            target: None,
            health: None,
        };
        let connected = status_from_row(&row(ConnState::Connected));
        assert!(connected.connected, "Connected → wire connected:true");
        assert!(connected.cli_present, "Connected → present");

        let nocli_intent_on = status_from_row(&row(ConnState::NoCli { connected: true }));
        assert!(
            !nocli_intent_on.connected,
            "NoCli persisted-intent true must NOT leak as wire connected (present-and-bound is false)"
        );
        assert!(!nocli_intent_on.cli_present, "an absent CLI is not present");
    }

    #[test]
    fn registered_id_accepts_known_rejects_unknown() {
        assert_eq!(registered_id("antigravity").unwrap(), "antigravity");
        let err = registered_id("not-a-source").unwrap_err().to_string();
        assert!(err.contains("unknown source 'not-a-source'"), "{err}");
        assert!(err.contains("antigravity"), "lists known sources: {err}");
    }

    #[test]
    fn freeze_for_skip_keeps_a_hooked_but_unflagged_source_connected() {
        let connected = HashSet::new();
        let freeze = freeze_for_skip(
            ["claude-code", "codex"],
            &connected,
            |id| id == "claude-code", // only claude-code has installed hooks
        );
        assert_eq!(freeze, vec![("claude-code", true), ("codex", false)]);
    }

    #[test]
    fn freeze_for_skip_honors_the_live_connected_gate() {
        let connected = set(&["antigravity"]);
        let freeze = freeze_for_skip(["antigravity", "codex"], &connected, |_| false);
        assert_eq!(freeze, vec![("antigravity", true), ("codex", false)]);
    }

    #[test]
    fn map_disconnect_outcome_surfaces_a_folded_hook_removal_failure() {
        match map_disconnect_outcome(DisconnectOutcome::HookRemovalFailed("boom".into())) {
            ChangeOutcome::Failed(m) => assert_eq!(m, "hooks not removed: boom"),
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(matches!(
            map_disconnect_outcome(DisconnectOutcome::FlagOnly),
            ChangeOutcome::Disconnected
        ));
    }

    #[test]
    fn connect_then_disconnect_a_no_target_source_persists_the_flag() {
        // Antigravity has no install target → a pure flag flip, so this touches no
        // real agent config and mutates no env (no TEST_ENV_LOCK needed).
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");

        assert!(matches!(
            connect(&cfg, "antigravity").unwrap(),
            ConnectOutcome::FlagOnly
        ));
        let app = config::load(&cfg, &mut Vec::new());
        assert_eq!(
            app.sources.get("antigravity"),
            Some(&true),
            "flag persisted true"
        );

        assert!(matches!(
            disconnect(&cfg, "antigravity").unwrap(),
            DisconnectOutcome::FlagOnly
        ));
        let app = config::load(&cfg, &mut Vec::new());
        assert_eq!(
            app.sources.get("antigravity"),
            Some(&false),
            "flag persisted false"
        );
    }

    // Its `default_config_path` errs, so `install_target` bails before any FS —
    // a deterministic, cross-platform install failure.
    static FAIL_TARGET: Target = Target {
        name: "rollbacktest",
        core_source: "rollbacktest",
        display_name: "RollbackTest",
        default_config_path: || Err(anyhow::anyhow!("forced install failure")),
        hook_command: |_, _| Ok(String::new()),
        merge_install: |c, _| {
            Ok(crate::install::target::MergeOutcome {
                content: c.to_string(),
                changed: false,
            })
        },
        merge_uninstall: |c| {
            Ok(crate::install::target::MergeOutcome {
                content: c.to_string(),
                changed: false,
            })
        },
        verify_schema: |_| crate::install::verify::SchemaParse::broken("test fake"),
        binary_strategy: crate::install::target::BinaryStrategy::EmbedAbsolute,
        presence_probe: None,
        extra_artifacts: None,
        post_install_hint: None,
    };

    #[test]
    fn connect_target_rolls_the_flag_back_when_install_fails() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        let err = connect_target(&cfg, "rollbacktest", Some(&FAIL_TARGET)).unwrap_err();
        assert!(err.to_string().contains("forced install failure"), "{err}");
        let app = config::load(&cfg, &mut Vec::new());
        assert_eq!(
            app.sources.get("rollbacktest"),
            None,
            "a previously-absent flag rolls back to ABSENT, not false"
        );
    }

    #[test]
    fn connect_target_rollback_restores_a_previously_connected_flag() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        config::save_source_connected(&cfg, "rollbacktest", true).unwrap();

        let err = connect_target(&cfg, "rollbacktest", Some(&FAIL_TARGET)).unwrap_err();
        assert!(err.to_string().contains("forced install failure"), "{err}");
        let app = config::load(&cfg, &mut Vec::new());
        assert_eq!(
            app.sources.get("rollbacktest"),
            Some(&true),
            "a previously-connected flag must survive a failed re-install"
        );
    }

    #[test]
    fn connect_target_rollback_restores_a_previously_disconnected_flag() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        config::save_source_connected(&cfg, "rollbacktest", false).unwrap();

        connect_target(&cfg, "rollbacktest", Some(&FAIL_TARGET)).unwrap_err();
        let app = config::load(&cfg, &mut Vec::new());
        assert_eq!(app.sources.get("rollbacktest"), Some(&false));
    }

    #[test]
    fn disconnect_target_folds_a_hook_removal_failure_into_the_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        let outcome = disconnect_target(&cfg, "rollbacktest", Some(&FAIL_TARGET)).unwrap();
        assert!(matches!(outcome, DisconnectOutcome::HookRemovalFailed(_)));
        let app = config::load(&cfg, &mut Vec::new());
        assert_eq!(
            app.sources.get("rollbacktest"),
            Some(&false),
            "the flag is persisted false even though hook removal failed"
        );
    }

    #[test]
    fn connect_rejects_an_unknown_source_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        assert!(connect(&cfg, "bogus").is_err());
        assert!(
            !cfg.exists(),
            "a rejected id must not create/write the config"
        );
    }

    #[test]
    fn plan_reconcile_is_declarative_and_idempotent() {
        let current = set(&["claude-code", "codex"]);
        let desired = set(&["claude-code", "cursor"]);
        let plan: std::collections::HashMap<_, _> =
            plan_reconcile(&current, &desired).into_iter().collect();
        assert_eq!(plan["codex"], Action::Disconnect, "in current, not desired");
        assert_eq!(plan["cursor"], Action::Connect, "in desired, not current");
        assert_eq!(plan["claude-code"], Action::NoOp, "in both");
        assert_eq!(plan["antigravity"], Action::NoOp);

        let steady = plan_reconcile(&desired, &desired);
        assert!(
            steady.iter().all(|(_, a)| *a == Action::NoOp),
            "matching state ⇒ no changes"
        );
    }

    #[test]
    fn wire_outcome_serializes_as_its_token() {
        for w in [
            WireOutcome::Connected,
            WireOutcome::Disconnected,
            WireOutcome::NoOp,
            WireOutcome::Failed,
        ] {
            assert_eq!(
                serde_json::to_value(w).unwrap(),
                serde_json::Value::String(w.token().to_string())
            );
        }
    }

    #[test]
    fn change_outcome_wire_tokens_are_stable() {
        assert_eq!(ChangeOutcome::Connected.wire_token(), "connected");
        assert_eq!(ChangeOutcome::Disconnected.wire_token(), "disconnected");
        assert_eq!(ChangeOutcome::NoOp.wire_token(), "no_op");
        assert_eq!(ChangeOutcome::Failed("boom".into()).wire_token(), "failed");
        assert_eq!(ChangeOutcome::Failed("boom".into()).message(), Some("boom"));
        assert_eq!(ChangeOutcome::Connected.message(), None);
    }

    #[test]
    fn source_status_json_shape_is_the_raycast_contract() {
        let s = SourceStatus {
            id: "codex".into(),
            display_name: "Codex".into(),
            connected: true,
            cli_present: true,
            health: None,
        };
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            r#"{"id":"codex","display_name":"Codex","connected":true,"cli_present":true,"health":null}"#
        );
    }

    #[test]
    fn outcome_row_json_shape_is_the_raycast_contract() {
        let ok = OutcomeRow::new("codex".into(), &ChangeOutcome::Connected);
        let failed = OutcomeRow::new("cursor".into(), &ChangeOutcome::Failed("boom".into()));
        assert_eq!(
            serde_json::to_string(&ok).unwrap(),
            r#"{"id":"codex","outcome":"connected"}"#
        );
        assert_eq!(
            serde_json::to_string(&failed).unwrap(),
            r#"{"id":"cursor","outcome":"failed","message":"boom"}"#
        );
    }

    #[test]
    fn outcome_row_message_is_control_char_stripped_at_the_authority() {
        let row = OutcomeRow::new(
            "codex".into(),
            &ChangeOutcome::Failed("bad\u{1b}]0;PWNED\u{7}key\u{202e}txet".into()),
        );
        assert_eq!(row.message.as_deref(), Some("bad]0;PWNEDkeytxet"));

        let clean = "processing /home/u/.codex/config.toml: not valid TOML";
        assert_eq!(
            OutcomeRow::new("codex".into(), &ChangeOutcome::Failed(clean.into()))
                .message
                .as_deref(),
            Some(clean)
        );
    }

    #[test]
    fn outcome_row_schema_matches_the_committed_contract() {
        let schema = schemars::schema_for!(OutcomeRow);
        let generated = serde_json::to_string_pretty(&schema).unwrap() + "\n";
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../integrations/raycast/contract/outcome-row.schema.json"
        );
        if std::env::var_os("UPDATE_CONTRACT_SCHEMA").is_some() {
            let p = std::path::Path::new(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, &generated).unwrap();
        }
        let committed = std::fs::read_to_string(path).unwrap_or_default();
        assert_eq!(
            generated, committed,
            "OutcomeRow schema drifted from the committed contract \
             (integrations/raycast/contract/outcome-row.schema.json). \
             Run `just gen-contract`, then regen + commit the raycast .d.ts."
        );
    }

    #[test]
    fn source_status_schema_matches_the_committed_contract() {
        let schema = schemars::schema_for!(SourceStatus);
        let generated = serde_json::to_string_pretty(&schema).unwrap() + "\n";
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../integrations/raycast/contract/source-status.schema.json"
        );
        if std::env::var_os("UPDATE_CONTRACT_SCHEMA").is_some() {
            let p = std::path::Path::new(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, &generated).unwrap();
        }
        let committed = std::fs::read_to_string(path).unwrap_or_default();
        assert_eq!(
            generated, committed,
            "SourceStatus schema drifted from the committed contract \
             (integrations/raycast/contract/source-status.schema.json). \
             Run `just gen-contract`, then regen + commit the raycast .d.ts."
        );
    }

    #[test]
    fn reconcile_to_disconnects_the_complement_and_noops_the_rest() {
        // Drive only the no-target source (antigravity) to avoid agent-config I/O;
        // every other source has no flag ⇒ resolves "not connected", so no
        // install-state injection is needed.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        connect(&cfg, "antigravity").unwrap();

        let outcomes: std::collections::HashMap<_, _> =
            reconcile_to(&cfg, &HashSet::new()).into_iter().collect();

        assert_eq!(outcomes["antigravity"], ChangeOutcome::Disconnected);
        assert_eq!(
            outcomes["codex"],
            ChangeOutcome::NoOp,
            "not connected → no change"
        );
        let app = config::load(&cfg, &mut Vec::new());
        assert_eq!(app.sources.get("antigravity"), Some(&false));
    }

    #[test]
    fn apply_choices_writes_only_the_listed_sources() {
        // Drive only the no-target source so there's no agent-config I/O.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");

        let outcomes: std::collections::HashMap<_, _> =
            apply_choices(&cfg, &[("antigravity", true)])
                .into_iter()
                .collect();
        assert_eq!(outcomes["antigravity"], ChangeOutcome::Connected);

        let app = config::load(&cfg, &mut Vec::new());
        assert_eq!(
            app.sources.get("antigravity"),
            Some(&true),
            "listed → written"
        );
        assert_eq!(app.sources.get("codex"), None, "unlisted → untouched");

        apply_choices(&cfg, &[("antigravity", false)]);
        let app = config::load(&cfg, &mut Vec::new());
        assert_eq!(app.sources.get("antigravity"), Some(&false));
    }
}
