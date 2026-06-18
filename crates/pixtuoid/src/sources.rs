//! The source-control CORE: detect / connect / disconnect / reconcile, TUI-free.
//!
//! ONE home for "which agent CLIs exist, their connection state, and how to
//! change it." Three thin presenters sit on top: the in-TUI Sources panel
//! (`tui::connection` + `tui::mod::{connect_source,disconnect_source}`), the
//! scriptable CLI (`pixtuoid sources|connect|disconnect`, Raycast-facing), and
//! first-run onboarding (`crate::setup`). The mutating ops here are the
//! PERSISTED half — they write the `[sources]` flag + install/uninstall hooks,
//! exactly as the panel does, but DON'T touch a running instance's live
//! `ConnectedSources` (a separate CLI process has none; a running `run`/`floating`
//! reflects the change on its next launch). The panel adds the one live line on
//! top.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use pixtuoid_core::source::REGISTERED_SOURCES;

use crate::config;
use crate::install::{
    self,
    target::{by_source, is_present, Target},
    InstallReport, UninstallReport,
};

/// Outcome of a single connect/disconnect, so a batch (`reconcile_to`) can
/// report per-source without aborting the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeOutcome {
    Connected,
    Disconnected,
    /// Already in the desired state — nothing written.
    NoOp,
    /// The change failed (e.g. a hook install error); the message is human-readable.
    Failed(String),
}

impl ChangeOutcome {
    /// Stable wire token for `--json` / scripting. Kept separate from the
    /// enum's `Debug` so the JSON contract can't drift if a variant is renamed.
    pub fn as_wire(&self) -> String {
        match self {
            ChangeOutcome::Connected => "connected".into(),
            ChangeOutcome::Disconnected => "disconnected".into(),
            ChangeOutcome::NoOp => "no_op".into(),
            ChangeOutcome::Failed(msg) => format!("failed: {msg}"),
        }
    }
}

/// A serializable status row for `pixtuoid sources --json` — the STABLE wire
/// contract the Raycast extension parses (pinned by `source_status_json_shape`).
/// Deliberately a flat DTO, NOT the internal `ConnectionRow` (whose shape is a
/// UI concern free to change).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SourceStatus {
    pub id: String,
    pub display_name: String,
    pub connected: bool,
    pub cli_present: bool,
    /// A health/issue summary (install-broken / decode-drift), or `null` when n/a.
    pub health: Option<String>,
}

/// Resolve a user-supplied id to the `'static` registry id, or a clear error
/// (the CLI surface takes arbitrary input; `config::save_source_connected`
/// needs `&'static str`). Mirrors how the panel only ever feeds registry ids.
pub fn registered_id(id: &str) -> Result<&'static str> {
    REGISTERED_SOURCES
        .iter()
        .copied()
        .find(|s| *s == id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "unknown source '{id}' (known: {})",
                REGISTERED_SOURCES.join(", ")
            )
        })
}

/// Result of a successful single connect — carries the `InstallReport` for a
/// target-bearing source so the panel can render its rich notes (backup / PATH
/// warning); `FlagOnly` for a no-target (JSONL-only) source.
#[derive(Debug)]
pub enum ConnectOutcome {
    FlagOnly,
    Installed(InstallReport),
}

/// Result of a single disconnect whose FLAG was persisted false (the user IS
/// disconnected). `Err` from `disconnect` is reserved for the persist-failure
/// abort; a failed hook removal is folded in here (harmless stale hooks remain
/// behind the now-closed gate), so the gate still closes — mirroring the original
/// panel asymmetry (connect rolls back on install failure; disconnect does not).
#[derive(Debug)]
pub enum DisconnectOutcome {
    FlagOnly,
    Uninstalled(UninstallReport),
    /// Flag persisted false, but removing the hooks errored. Carries the message.
    HookRemovalFailed(String),
}

/// Connect a source: PERSIST the `[sources]` flag FIRST (so it survives restart),
/// then — only for a target-bearing source — install its hooks, rolling the flag
/// back if the install fails (a persisted "connected" with no integration behind
/// it would show connected yet never produce an agent). The panel's
/// `connect_source` adds `connected.set(..)` on top for the live gate; the CLI
/// and onboarding don't (a separate process has no live set).
///
/// **Honors the explicit id — it does NOT gate on CLI presence.** Unlike the
/// in-TUI panel (which renders an absent CLI as `NoCli` and refuses the toggle),
/// `connect`/`reconcile_to` install for any registered id even if that CLI isn't
/// installed yet — pre-provisioning for automation/onboarding where the caller
/// stated intent. (`detect()` returns only PRESENT CLIs, so onboarding offers
/// only installed ones; a `connect <absent-cli>` is a deliberate user/script
/// choice that materializes that CLI's config dir.)
pub fn connect(cfg: &Path, id: &str) -> Result<ConnectOutcome> {
    let sid = registered_id(id)?;
    connect_target(cfg, sid, by_source(sid), None)
}

/// The persist + install + rollback core, with the `target` passed EXPLICITLY so
/// tests can inject a deterministic-fail fake (`connect` resolves it from the
/// registry). `target_config` overrides the target's config path.
fn connect_target(
    cfg: &Path,
    sid: &'static str,
    target: Option<&Target>,
    target_config: Option<PathBuf>,
) -> Result<ConnectOutcome> {
    config::save_source_connected(cfg, sid, true)?;
    match target {
        Some(t) => match install::install_target(t, target_config, None) {
            Ok(r) => Ok(ConnectOutcome::Installed(r)),
            Err(e) => {
                // Roll the flag back so the next launch doesn't honor a
                // "connected" with no hooks behind it (the same path the first
                // save just succeeded on, so it's reliable).
                let _ = config::save_source_connected(cfg, sid, false);
                Err(e)
            }
        },
        None => Ok(ConnectOutcome::FlagOnly),
    }
}

/// Disconnect a source: persist the flag false FIRST, then remove its hooks
/// (target-bearing only). No rollback — a failed uninstall still leaves the user
/// disconnected (the safer direction).
pub fn disconnect(cfg: &Path, id: &str) -> Result<DisconnectOutcome> {
    let sid = registered_id(id)?;
    disconnect_target(cfg, sid, by_source(sid), None)
}

fn disconnect_target(
    cfg: &Path,
    sid: &'static str,
    target: Option<&Target>,
    target_config: Option<PathBuf>,
) -> Result<DisconnectOutcome> {
    // `?` here = the persist-failure abort (flip nothing). Past it, the flag is
    // false, so a hook-removal error folds into the outcome rather than erroring.
    config::save_source_connected(cfg, sid, false)?;
    Ok(match target {
        Some(t) => match install::uninstall_target(t, target_config) {
            Ok(r) => DisconnectOutcome::Uninstalled(r),
            Err(e) => DisconnectOutcome::HookRemovalFailed(format!("{e:#}")),
        },
        None => DisconnectOutcome::FlagOnly,
    })
}

/// What `reconcile_to` should do to one source. Pure — see `plan_reconcile`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Connect,
    Disconnect,
    NoOp,
}

/// PURE diff: given the CURRENT connected-set and the DESIRED set, decide each
/// registered source's action. The declarative "connected set = exactly these"
/// semantics the Raycast checkbox-form / `sources set` needs: a source in
/// `desired` but not `current` → Connect; in `current` but not `desired` →
/// Disconnect; otherwise NoOp. Ids outside `REGISTERED_SOURCES` are ignored
/// here (the I/O wrapper validates them up front so an unknown id is a loud
/// error, not a silent drop).
pub fn plan_reconcile(
    current: &HashSet<String>,
    desired: &HashSet<String>,
) -> Vec<(&'static str, Action)> {
    REGISTERED_SOURCES
        .iter()
        .copied()
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

/// Declarative apply: make the connected set EXACTLY `desired` (the Raycast
/// checkbox-form / `sources set` semantics). For each registered source: connect
/// the newly-desired, disconnect the no-longer-desired, NoOp the rest — reporting
/// each (a failed item doesn't abort the batch). `has_hooks` is injected so the
/// CURRENT set is computed the same way the boot seed is (`config::resolve_connected`).
pub fn reconcile_to(cfg: &Path, desired: &HashSet<String>) -> Vec<(String, ChangeOutcome)> {
    reconcile_to_with(cfg, desired, |src| by_source(src).map(install::has_hooks))
}

pub fn reconcile_to_with(
    cfg: &Path,
    desired: &HashSet<String>,
    has_hooks: impl Fn(&'static str) -> Option<bool>,
) -> Vec<(String, ChangeOutcome)> {
    let app = config::load(cfg, &mut Vec::new());
    let current = config::resolve_connected(&app, has_hooks);
    plan_reconcile(&current, desired)
        .into_iter()
        .map(|(sid, action)| (sid.to_string(), apply_one(cfg, sid, action)))
        .collect()
}

/// Apply ONE planned action and map it to a reportable `ChangeOutcome`. The single
/// connect/disconnect→outcome mapping shared by `reconcile_to` (declarative) and
/// `apply_choices` (the explicit onboarding list) so the folded-hook-removal-
/// failure surfacing can't drift between them.
fn apply_one(cfg: &Path, sid: &'static str, action: Action) -> ChangeOutcome {
    match action {
        Action::Connect => match connect(cfg, sid) {
            Ok(_) => ChangeOutcome::Connected,
            Err(e) => ChangeOutcome::Failed(format!("{e:#}")),
        },
        Action::Disconnect => match disconnect(cfg, sid) {
            // The flag IS disconnected; surface a folded hook-removal failure so a
            // caller doesn't hide stale hooks behind a clean "disconnected".
            Ok(DisconnectOutcome::HookRemovalFailed(e)) => {
                ChangeOutcome::Failed(format!("hooks not removed: {e}"))
            }
            Ok(_) => ChangeOutcome::Disconnected,
            Err(e) => ChangeOutcome::Failed(format!("{e:#}")),
        },
        Action::NoOp => ChangeOutcome::NoOp,
    }
}

/// Apply an EXPLICIT per-source decision list (the first-run onboarding apply):
/// connect each `true` id, disconnect each `false` id. Unlike `reconcile_to` (which
/// is declarative over EVERY registered source and would disconnect the complement),
/// this touches ONLY the ids passed — so a migrate-defaulted source absent from the
/// list (e.g. `antigravity`, which is connected-by-default and never appears in
/// `detect()`) is left exactly as it was, never surprise-disconnected. Each write
/// makes `[sources]` non-empty, so the first-run gate (`setup::is_first_run`) closes.
/// Idempotent: connect/disconnect no-op at the install layer when already in state.
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

// ---- Source status MODEL (moved here from `tui::connection`, which re-exports
//      it so the panel/harness are unchanged). Pure: no ratatui, no SceneState. ----

/// Connection state for one CLI row — the single facet the toggle acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    /// Bound: this source's events flow and its characters show. Toggle disconnects.
    Connected,
    /// Unbound: the gate is closed; no characters. Toggle connects.
    Disconnected,
    /// A target-bearing CLI that isn't installed on this machine — nothing to bind to.
    NoCli,
}

/// One row = one agent CLI (the union of registry sources + install targets).
#[derive(Debug, Clone)]
pub struct ConnectionRow {
    /// The core source id (registry `SourceDescriptor.name`, e.g. "claude-code")
    /// — the unifying key; joined to an install target via `Target.core_source`.
    pub source_id: &'static str,
    /// 2-char badge id (`cc`/`cx`/…), from the source descriptor.
    pub label_prefix: &'static str,
    pub display_name: &'static str,
    pub state: ConnState,
    /// The config the hooks live in; `None` for no-target (JSONL-only) rows.
    pub config_path: Option<PathBuf>,
    /// The install target backing this row; `None` ⇒ connect/disconnect is a
    /// flag-only flip (Antigravity — no hooks to write).
    pub target: Option<&'static Target>,
    /// Cached HEALTH summary — a one-line `⚠ …` verdict from
    /// `doctor::diagnose(..).summary()`, computed for CONNECTED rows only.
    pub health: Option<String>,
}

/// Per-target filesystem facts, injected so `build_rows_from` is pure (the FS
/// reads happen in `build_rows`). `Some` exactly when the row has an install target.
#[derive(Debug, Clone)]
pub struct RowFacts {
    pub present: bool,
    pub config_path: Option<PathBuf>,
}

/// One pure input row for `build_rows_from`.
#[derive(Debug, Clone)]
pub struct RowInput {
    pub source_id: &'static str,
    pub label_prefix: &'static str,
    pub target: Option<&'static Target>,
    pub facts: Option<RowFacts>,
    /// Whether this source is in the live connected-set (the persisted intent).
    pub connected: bool,
    /// Cached health summary (injected so `build_rows_from` stays pure).
    pub health: Option<String>,
}

/// Title-case the no-target sources (the registry omits their display names).
fn display_name_for(source_id: &'static str) -> &'static str {
    match source_id {
        "antigravity" => "Antigravity",
        "copilot" => "Copilot CLI",
        other => other,
    }
}

/// Pure row builder over injected facts — the testable core of `build_rows`.
/// A target-bearing CLI that isn't present is `NoCli`; otherwise the connected-set
/// is authoritative.
pub fn build_rows_from(inputs: Vec<RowInput>) -> Vec<ConnectionRow> {
    inputs
        .into_iter()
        .map(|input| {
            let absent_cli = matches!(
                (&input.target, &input.facts),
                (Some(_), Some(f)) if !f.present
            );
            let state = if absent_cli {
                ConnState::NoCli
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

/// Build the status rows from the registry + install targets + the connected-set.
/// Performs FS reads (`is_present`/`default_config_path`) AND, for connected rows,
/// the health rollup (`doctor::diagnose`). `log` is the warn-floor log text.
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
                    .then(|| crate::doctor::diagnose(d.name, log).summary())
                    .flatten(),
            }
        })
        .collect();
    build_rows_from(inputs)
}

/// Map a status row to the serializable `SourceStatus` DTO (the CLI/Raycast wire shape).
fn status_from_row(r: &ConnectionRow) -> SourceStatus {
    SourceStatus {
        id: r.source_id.to_string(),
        display_name: r.display_name.to_string(),
        connected: matches!(r.state, ConnState::Connected),
        // A no-target (JSONL-only) source is always "present"; a target-bearing
        // one is present unless probed absent (`NoCli`).
        cli_present: !matches!(r.state, ConnState::NoCli),
        health: r.health.clone(),
    }
}

/// The status of every registered source — what `pixtuoid sources [--json]` and
/// onboarding read. Resolves the connected-set the same way the boot seed does.
pub fn status(cfg: &Path, log: &str) -> Vec<SourceStatus> {
    let app = config::load(cfg, &mut Vec::new());
    let connected = config::resolve_connected(&app, |src| by_source(src).map(install::has_hooks));
    build_rows(&connected, log)
        .iter()
        .map(status_from_row)
        .collect()
}

/// Which agent CLIs are installed on this machine (target-bearing + probed present)
/// — the "offer to connect these" set for first-run onboarding.
pub fn detect() -> Vec<&'static str> {
    REGISTERED_SOURCES
        .iter()
        .copied()
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
    fn registered_id_accepts_known_rejects_unknown() {
        assert_eq!(registered_id("antigravity").unwrap(), "antigravity");
        let err = registered_id("not-a-source").unwrap_err().to_string();
        assert!(err.contains("unknown source 'not-a-source'"), "{err}");
        assert!(err.contains("antigravity"), "lists known sources: {err}");
    }

    #[test]
    fn connect_then_disconnect_a_no_target_source_persists_the_flag() {
        // Antigravity has no install target → connect/disconnect is a pure flag
        // flip (no agent-config I/O), so we can exercise the persist round in a
        // tempdir without touching any real ~/.claude-style file. No env mutation
        // (the cfg path is explicit), so no TEST_ENV_LOCK needed.
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

    // A target whose install ALWAYS fails (its `default_config_path` errs, so
    // `install_target` bails before any FS) — exercises `connect_target`'s
    // install-failure rollback deterministically + cross-platform.
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
    };

    #[test]
    fn connect_target_rolls_the_flag_back_when_install_fails() {
        // Persist succeeds (writable cfg), THEN install_target fails → the flag
        // must roll back to false (no shown-but-broken source after a restart).
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        let err = connect_target(&cfg, "rollbacktest", Some(&FAIL_TARGET), None).unwrap_err();
        assert!(err.to_string().contains("forced install failure"), "{err}");
        let app = config::load(&cfg, &mut Vec::new());
        assert_eq!(
            app.sources.get("rollbacktest"),
            Some(&false),
            "the flag was rolled back to false"
        );
    }

    #[test]
    fn disconnect_target_folds_a_hook_removal_failure_into_the_outcome() {
        // FAIL_TARGET's uninstall errs (default_config_path errs) → the flag is
        // STILL persisted false (disconnect's primary semantics hold), and the
        // error is FOLDED into HookRemovalFailed (Err is reserved for the
        // persist-abort) so the gate still closes + the CLI/panel can surface it.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        let outcome = disconnect_target(&cfg, "rollbacktest", Some(&FAIL_TARGET), None).unwrap();
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
        // A source in neither is a NoOp (not touched).
        assert_eq!(plan["antigravity"], Action::NoOp);

        // Idempotent: reconciling an already-matching state is all NoOp.
        let steady = plan_reconcile(&desired, &desired);
        assert!(
            steady.iter().all(|(_, a)| *a == Action::NoOp),
            "matching state ⇒ no changes"
        );
    }

    #[test]
    fn change_outcome_wire_tokens_are_stable() {
        assert_eq!(ChangeOutcome::Connected.as_wire(), "connected");
        assert_eq!(ChangeOutcome::Disconnected.as_wire(), "disconnected");
        assert_eq!(ChangeOutcome::NoOp.as_wire(), "no_op");
        assert_eq!(
            ChangeOutcome::Failed("boom".into()).as_wire(),
            "failed: boom"
        );
    }

    #[test]
    fn source_status_json_shape_is_the_raycast_contract() {
        // Pins the exact JSON the Raycast extension parses. Changing a key here
        // is a breaking change to that contract — update both sides deliberately.
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
    fn reconcile_to_disconnects_the_complement_and_noops_the_rest() {
        // Drive only the no-target source (antigravity) to avoid agent-config I/O,
        // and inject has_hooks=Some(false) so every target resolves "not connected"
        // (NoOp under an empty desired). Pre-set antigravity connected; desired={}
        // ⇒ antigravity disconnects, all targets NoOp. Deterministic, no real hooks.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        connect(&cfg, "antigravity").unwrap(); // flag → true

        let outcomes: std::collections::HashMap<_, _> =
            reconcile_to_with(&cfg, &HashSet::new(), |_| Some(false))
                .into_iter()
                .collect();

        assert_eq!(outcomes["antigravity"], ChangeOutcome::Disconnected);
        assert_eq!(
            outcomes["codex"],
            ChangeOutcome::NoOp,
            "not connected → no change"
        );
        // The flag was actually written.
        let app = config::load(&cfg, &mut Vec::new());
        assert_eq!(app.sources.get("antigravity"), Some(&false));
    }

    #[test]
    fn apply_choices_writes_only_the_listed_sources() {
        // The onboarding apply is SCOPED to the ids passed — a source absent from
        // the list is never touched (the "antigravity stays at its migrate default"
        // property that a declarative reconcile_to would break). Drive only the
        // no-target source so there's no agent-config I/O.
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

        // Unchecked (the uncheck / skip-freeze path) persists false.
        apply_choices(&cfg, &[("antigravity", false)]);
        let app = config::load(&cfg, &mut Vec::new());
        assert_eq!(app.sources.get("antigravity"), Some(&false));
    }
}
