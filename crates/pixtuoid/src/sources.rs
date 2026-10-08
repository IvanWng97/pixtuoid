//! The source-control CORE: detect / connect / disconnect / reconcile, TUI-free.
//!
//! A source is connected by ONE fact: a hook-bearing source's installed hooks,
//! a flag-only source's `[sources]` flag. The mutating ops here change that
//! fact but not a running instance's live `ConnectedSources`, which reflects
//! the change on its next launch.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use pixtuoid_core::source::registry;

use crate::config;
use crate::install::{
    self, InstallReport, UninstallReport,
    target::{Target, by_source, is_present},
};

/// The wire-facing outcome token — a CLOSED set, published in the JSON schema
/// as an `enum` so the generated Raycast type is a string-literal UNION.
/// Widening it is a wire change under the `OutcomeRow` handshake rule below,
/// not a free extension: an installed store copy won't match a new token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub(crate) enum WireOutcome {
    Connected,
    Disconnected,
    NoOp,
    Failed,
}

impl WireOutcome {
    pub(crate) fn token(self) -> &'static str {
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
pub(crate) enum ChangeOutcome {
    Connected,
    Disconnected,
    NoOp,
    Failed(String),
}

impl ChangeOutcome {
    /// Kept separate from the enum's `Debug` so the JSON contract can't drift
    /// if a variant is renamed.
    pub(crate) fn wire_outcome(&self) -> WireOutcome {
        match self {
            ChangeOutcome::Connected => WireOutcome::Connected,
            ChangeOutcome::Disconnected => WireOutcome::Disconnected,
            ChangeOutcome::NoOp => WireOutcome::NoOp,
            ChangeOutcome::Failed(_) => WireOutcome::Failed,
        }
    }

    pub(crate) fn message(&self) -> Option<&str> {
        match self {
            ChangeOutcome::Failed(msg) => Some(msg),
            _ => None,
        }
    }
}

/// An attempted connect or disconnect — [`ChangeOutcome`] without its `NoOp`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AppliedChange {
    Connected,
    Disconnected,
    Failed(String),
}

impl From<AppliedChange> for ChangeOutcome {
    fn from(c: AppliedChange) -> Self {
        match c {
            AppliedChange::Connected => ChangeOutcome::Connected,
            AppliedChange::Disconnected => ChangeOutcome::Disconnected,
            AppliedChange::Failed(e) => ChangeOutcome::Failed(e),
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
pub(crate) struct OutcomeRow {
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
    pub(crate) fn new(id: String, outcome: &ChangeOutcome) -> Self {
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
pub(crate) struct SourceStatus {
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

/// Resolve a user-supplied id to the registry id, or a clear error — the CLI
/// surface takes arbitrary input.
///
/// # Errors
///
/// If `id` is not a registered source name.
pub(crate) fn registered_id(id: &str) -> Result<&'static str> {
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

/// `FlagOnly` for a flag-only source.
#[derive(Debug)]
pub(crate) enum ConnectOutcome {
    FlagOnly,
    Installed(InstallReport),
}

#[derive(Debug)]
pub(crate) enum DisconnectOutcome {
    FlagOnly,
    Uninstalled(UninstallReport),
}

/// The step (if any) a user must still take after a successful `connect` —
/// `None` for a target whose hooks take effect on the CLI's next run.
pub(crate) fn post_install_hint(id: &str) -> Option<&'static str> {
    crate::install::target::by_source(id).and_then(|t| t.post_install_hint)
}

/// Whether each registered source is connected: by its hooks when it has an
/// install target, else by its `[sources]` flag.
pub(crate) fn connected(app: &config::AppConfig) -> HashSet<String> {
    connected_with(app, installed_hooks)
}

/// [`connected`] with the hook read injected.
fn connected_with(
    app: &config::AppConfig,
    hooked: impl Fn(&str) -> Option<bool>,
) -> HashSet<String> {
    registry::registered_source_names()
        .filter(|sid| is_on(app, sid, &hooked))
        .map(String::from)
        .collect()
}

/// Whether `sid`'s hooks are installed; `None` for a flag-only source.
fn installed_hooks(sid: &str) -> Option<bool> {
    by_source(sid).map(|t| install::has_hooks(t, None))
}

/// The one fact for `sid`: `hooked(sid)`, else its `[sources]` flag.
fn is_on(app: &config::AppConfig, sid: &str, hooked: impl Fn(&str) -> Option<bool>) -> bool {
    hooked(sid).unwrap_or_else(|| app.sources.get(sid) == Some(&true))
}

/// Onboarding opens when nothing is connected — unless the config failed to
/// load, which means "previously configured": every write its apply makes
/// would be refused by `update_config`.
pub(crate) fn is_first_run(connected: &HashSet<String>, load_degraded: bool) -> bool {
    !load_degraded && connected.is_empty()
}

/// [`connected`] for one source, as the live gate re-reads it after a change.
pub(crate) fn is_connected(cfg: &Path, sid: &str) -> bool {
    is_on(&config::load(cfg, &mut Vec::new()), sid, installed_hooks)
}

/// Rewrite `[sources]` whole, as the flag-only sources that are on once
/// `change` applies, so a key no reader looks at never outlives a change.
/// Runs before any hook write: a config `update_config` refuses stops the op
/// with nothing to undo.
fn write_flags(cfg: &Path, change: Option<(&str, bool)>) -> Result<()> {
    let flag_only: Vec<&str> = registry::registered_source_names()
        .filter(|sid| by_source(sid).is_none())
        .collect();
    config::save_flag_sources(cfg, &flag_only, change)
}

/// Connect a source: install its hooks, or turn on a flag-only source's flag.
///
/// **Honors the explicit id — it does NOT gate on CLI presence.** Unlike the
/// in-TUI panel (which renders an absent CLI as `NoCli` and refuses the toggle),
/// this installs for any registered id even if that CLI isn't installed yet —
/// pre-provisioning for automation/onboarding where the caller stated intent.
///
/// # Errors
///
/// If `id` is not a registered source, rewriting `[sources]` fails, or the hook install fails.
pub(crate) fn connect(cfg: &Path, id: &str) -> Result<ConnectOutcome> {
    let sid = registered_id(id)?;
    connect_target(cfg, sid, by_source(sid))
}

/// The core of [`connect`], with `target` passed EXPLICITLY so tests can inject
/// a deterministic-fail fake.
fn connect_target(
    cfg: &Path,
    sid: &'static str,
    target: Option<&Target>,
) -> Result<ConnectOutcome> {
    write_flags(cfg, target.is_none().then_some((sid, true)))?;
    Ok(match target {
        Some(t) => ConnectOutcome::Installed(install::install_target(t, None, None)?),
        None => ConnectOutcome::FlagOnly,
    })
}

/// Disconnect a source: uninstall its hooks, or turn off a flag-only source's
/// flag.
///
/// # Errors
///
/// If `id` is not a registered source, rewriting `[sources]` fails, or the hook removal fails.
pub(crate) fn disconnect(cfg: &Path, id: &str) -> Result<DisconnectOutcome> {
    let sid = registered_id(id)?;
    disconnect_target(cfg, sid, by_source(sid))
}

fn disconnect_target(
    cfg: &Path,
    sid: &'static str,
    target: Option<&Target>,
) -> Result<DisconnectOutcome> {
    write_flags(cfg, target.is_none().then_some((sid, false)))?;
    Ok(match target {
        Some(t) => DisconnectOutcome::Uninstalled(install::uninstall_target(t, None)?),
        None => DisconnectOutcome::FlagOnly,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
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
/// source (a failed item doesn't abort the batch).
pub(crate) fn reconcile_to(cfg: &Path, desired: &HashSet<String>) -> Vec<(String, ChangeOutcome)> {
    let current = connected(&config::load(cfg, &mut Vec::new()));
    reconcile_from(cfg, &current, desired)
}

/// [`reconcile_to`] from an injected `current`.
fn reconcile_from(
    cfg: &Path,
    current: &HashSet<String>,
    desired: &HashSet<String>,
) -> Vec<(String, ChangeOutcome)> {
    plan_reconcile(current, desired)
        .into_iter()
        .map(|(sid, action)| (sid.to_string(), apply_one(cfg, sid, action)))
        .collect()
}

fn apply_one(cfg: &Path, sid: &'static str, action: Action) -> ChangeOutcome {
    match action {
        Action::Connect => apply_want(cfg, sid, true).into(),
        Action::Disconnect => apply_want(cfg, sid, false).into(),
        Action::NoOp => ChangeOutcome::NoOp,
    }
}

fn apply_want(cfg: &Path, sid: &'static str, want: bool) -> AppliedChange {
    let done = if want {
        connect(cfg, sid).map(|_| AppliedChange::Connected)
    } else {
        disconnect(cfg, sid).map(|_| AppliedChange::Disconnected)
    };
    done.unwrap_or_else(|e| AppliedChange::Failed(format!("{e:#}")))
}

/// How both presenters word a disconnect that couldn't reach `claude` to
/// deregister the plugin, which keeps it registered with no hooks.
pub(crate) const PLUGIN_LEFT_REGISTERED_PHRASE: &str =
    "plugin left registered (claude not on PATH)";

/// Connect each of `ids` — onboarding's confirm and `setup --yes`. Nothing is
/// connected on a first run, so an id left out needs no change.
pub(crate) fn connect_each(cfg: &Path, ids: &[&'static str]) -> Vec<(String, AppliedChange)> {
    ids.iter()
        .map(|&sid| (sid.to_string(), apply_want(cfg, sid, true)))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Connected,
    Disconnected,
    /// A target-bearing CLI that isn't installed on this machine. Carries
    /// whether its hooks are installed, because a connected-but-absent source is
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
pub(crate) struct RowFacts {
    pub present: bool,
    pub config_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) struct RowInput {
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

pub(crate) fn build_rows_from(inputs: Vec<RowInput>) -> Vec<ConnectionRow> {
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
pub(crate) fn build_rows(connected: &HashSet<String>, log: &str) -> Vec<ConnectionRow> {
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
/// NOT [`ConnState::connected`], which stays `true` for a connected-but-absent
/// `NoCli` source. Changing it is a `--json`
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

pub(crate) fn status(cfg: &Path, log: &str) -> Vec<SourceStatus> {
    let connected = connected(&config::load(cfg, &mut Vec::new()));
    build_rows(&connected, log)
        .iter()
        .map(status_from_row)
        .collect()
}

/// Which agent CLIs are installed on this machine (target-bearing + probed
/// present) — the "offer to connect these" set for first-run onboarding.
pub(crate) fn detect() -> Vec<&'static str> {
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

        let hint = post_install_hint("dsh").expect("dsh needs a restart step");
        assert!(
            hint.contains("restart") && hint.contains("web"),
            "non-web profiles compose patches once at boot; web hot-reloads — got {hint:?}"
        );

        for id in pixtuoid_core::source::registry::registered_source_names() {
            if id == "openclaw" || id == "omp" || id == "dsh" {
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

    fn flags(pairs: &[(&str, bool)]) -> config::AppConfig {
        config::AppConfig {
            sources: pairs.iter().map(|&(k, v)| (k.to_string(), v)).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_hook_source_is_connected_by_its_hooks_and_never_by_its_flag() {
        let app = flags(&[("claude-code", true), ("codex", false)]);
        let hooked = |sid: &str| match sid {
            "codex" => Some(true),
            "copilot" | "antigravity" => None,
            _ => Some(false),
        };
        assert_eq!(connected_with(&app, hooked), set(&["codex"]));
        let every: HashSet<String> = registry::registered_source_names()
            .map(String::from)
            .collect();
        assert_eq!(
            connected_with(&app, |_| Some(true)),
            every,
            "every registered source, only those"
        );
    }

    #[test]
    fn first_run_is_nothing_connected_over_a_config_that_loaded() {
        assert!(is_first_run(&HashSet::new(), false));
        assert!(!is_first_run(&set(&["codex"]), false));
        assert!(
            !is_first_run(&HashSet::new(), true),
            "a degraded config was configured"
        );
    }

    #[test]
    fn a_flag_only_source_is_connected_by_its_flag() {
        let app = flags(&[("copilot", true), ("antigravity", false)]);
        let hooked = |sid: &str| match sid {
            "copilot" | "antigravity" => None,
            _ => Some(false),
        };
        assert_eq!(connected_with(&app, hooked), set(&["copilot"]));
    }

    #[test]
    fn a_change_rewrites_sources_as_exactly_the_flag_only_sources_that_are_on() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        std::fs::write(
            &cfg,
            "theme = \"x\"\n[sources]\nclaude-code = true\ncodex = false\nantigravity = true\n",
        )
        .unwrap();
        connect(&cfg, "copilot").unwrap();
        let app = config::load(&cfg, &mut Vec::new());
        assert_eq!(
            app.sources,
            flags(&[("antigravity", true), ("copilot", true)]).sources
        );
        assert_eq!(
            app.theme.as_deref(),
            Some("x"),
            "only [sources] is rewritten"
        );

        disconnect(&cfg, "antigravity").unwrap();
        disconnect(&cfg, "copilot").unwrap();
        let raw = std::fs::read_to_string(&cfg).unwrap();
        assert!(
            !raw.contains("[sources]"),
            "an all-off table is dropped: {raw}"
        );
    }

    #[test]
    fn a_failed_install_leaves_no_flag_behind() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        let err = connect_target(&cfg, "rollbacktest", Some(&FAIL_TARGET)).unwrap_err();
        assert!(err.to_string().contains("forced install failure"), "{err}");
        let app = config::load(&cfg, &mut Vec::new());
        assert!(app.sources.is_empty(), "{:?}", app.sources);
    }

    #[test]
    fn a_failed_hook_removal_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        assert!(disconnect_target(&cfg, "rollbacktest", Some(&FAIL_TARGET)).is_err());
    }

    #[test]
    fn a_change_over_a_malformed_config_touches_no_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "not = [valid").unwrap();
        // FAIL_TARGET's install would name itself; the table write must stop first.
        let err = connect_target(&cfg, "rollbacktest", Some(&FAIL_TARGET)).unwrap_err();
        assert!(!err.to_string().contains("forced install failure"), "{err}");
    }

    #[test]
    fn connect_each_connects_only_the_ids_it_is_given() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        let outcomes = connect_each(&cfg, &["antigravity"]);
        assert_eq!(
            outcomes,
            vec![("antigravity".to_string(), AppliedChange::Connected)]
        );
        let app = config::load(&cfg, &mut Vec::new());
        assert_eq!(app.sources, flags(&[("antigravity", true)]).sources);
    }

    #[test]
    fn connect_then_disconnect_a_no_target_source_persists_the_flag() {
        // Antigravity has no install target → a pure flag flip, so this touches no
        // real agent config and mutates no env.
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
            None,
            "an off flag is not kept"
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
        host: None,
    };

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
        assert_eq!(ChangeOutcome::Connected.wire_outcome().token(), "connected");
        assert_eq!(
            ChangeOutcome::Disconnected.wire_outcome().token(),
            "disconnected"
        );
        assert_eq!(ChangeOutcome::NoOp.wire_outcome().token(), "no_op");
        assert_eq!(
            ChangeOutcome::Failed("boom".into()).wire_outcome().token(),
            "failed"
        );
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

    /// A schema IS its committed contract. `just gen-contract` rewrites it and
    /// the Raycast types generated from it.
    fn assert_contract(schema: &schemars::Schema, file: &str) {
        let generated = serde_json::to_string_pretty(schema).unwrap() + "\n";
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../integrations/raycast/contract")
            .join(file);
        let committed = snapbox::Data::read_from(&path, Some(snapbox::data::DataFormat::Text));
        snapbox::assert_data_eq!(generated, committed.raw());
    }

    #[test]
    fn outcome_row_schema_matches_the_committed_contract() {
        assert_contract(
            &schemars::schema_for!(OutcomeRow),
            "outcome-row.schema.json",
        );
    }

    #[test]
    fn source_status_schema_matches_the_committed_contract() {
        assert_contract(
            &schemars::schema_for!(SourceStatus),
            "source-status.schema.json",
        );
    }

    #[test]
    fn reconcile_disconnects_the_complement_and_noops_the_rest() {
        // The injected current set keeps this off the machine's real hooks.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        connect(&cfg, "antigravity").unwrap();

        let outcomes: std::collections::HashMap<_, _> =
            reconcile_from(&cfg, &set(&["antigravity"]), &HashSet::new())
                .into_iter()
                .collect();

        assert_eq!(outcomes["antigravity"], ChangeOutcome::Disconnected);
        assert_eq!(
            outcomes["codex"],
            ChangeOutcome::NoOp,
            "not connected → no change"
        );
        let app = config::load(&cfg, &mut Vec::new());
        assert!(app.sources.is_empty(), "{:?}", app.sources);
    }
}
