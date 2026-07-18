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

/// The wire-facing outcome token — a CLOSED set, published in the JSON schema
/// as an `enum` so the generated Raycast type is a string-literal UNION (a
/// consumer typo like `"conected"` is a `tsc` error, not a runtime miss).
/// Chosen over a bare `string` in the pre-store-publication free window: the
/// stronger contract costs nothing now; loosening later is additive.
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
    /// The serialized token — the ONE string authority (serde's snake_case
    /// rename and this table are pinned equal by `wire_outcome_serializes_as_its_token`).
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
    /// Stable BARE wire token for `--json` / scripting — a machine-matchable
    /// value, never carrying human text (the detail rides in [`Self::message`]).
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

    /// The serialized token for this outcome (via [`WireOutcome::token`]).
    pub fn wire_token(&self) -> &'static str {
        self.wire_outcome().token()
    }

    /// The human-readable detail alongside the token — `Some` exactly for
    /// `Failed` (the only variant that carries any).
    pub fn message(&self) -> Option<&str> {
        match self {
            ChangeOutcome::Failed(msg) => Some(msg),
            _ => None,
        }
    }
}

/// One row of the `--json` batch envelope `pixtuoid connect|disconnect|sources set`
/// print — the SECOND stable wire contract the Raycast extension parses (alongside
/// `SourceStatus`), with the same treatment: a committed JSON Schema
/// (`integrations/raycast/contract/outcome-row.schema.json`, golden-tested below)
/// the extension's TS type is generated from (`gen:contract`). The wire shape is
/// `{id, outcome, message?}` — a bare machine token plus an optional human-detail
/// field, split from the older folded `failed: <msg>` string BEFORE the
/// extension's store publication (the in-repo extension ships atomically with
/// the binary, so no installed consumer parsed the old form). Once the extension
/// is store-published, changing this shape needs a version handshake — see the
/// sharp edge in `crates/pixtuoid/CLAUDE.md`. Pinned by
/// `outcome_row_json_shape_is_the_raycast_contract` + the envelope test in
/// `sources_cli.rs`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
// Same rationale as `SourceStatus` below: `additionalProperties: false` so the
// generated TS type has no index signature and a consumer typo is a `tsc` error.
#[cfg_attr(test, derive(schemars::JsonSchema), schemars(deny_unknown_fields))]
pub struct OutcomeRow {
    /// The registry source id the outcome applies to (e.g. `codex`).
    pub id: String,
    /// The BARE outcome token: `connected` | `disconnected` | `no_op` |
    /// `failed` — a schema ENUM, so the generated TS side is a string-literal
    /// union (machine-matchable with `===`); human text rides in `message`.
    pub outcome: WireOutcome,
    /// Human-readable detail for the row — present exactly when the outcome
    /// carries any (`failed`), and OMITTED (not `null`) otherwise, so a
    /// success row stays the minimal `{id, outcome}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl OutcomeRow {
    /// Map one applied outcome to its wire row: the bare token plus the
    /// optional human message — the ONE outcome→row authority, so the two
    /// emitting surfaces (`run_change` / `run_sources_set`) can't drift.
    pub fn new(id: String, outcome: &ChangeOutcome) -> Self {
        OutcomeRow {
            id,
            outcome: outcome.wire_outcome(),
            message: outcome.message().map(str::to_string),
        }
    }
}

/// A serializable status row for `pixtuoid sources --json` — the STABLE wire
/// contract the Raycast extension parses (pinned by `source_status_json_shape`).
/// Deliberately a flat DTO, NOT the internal `ConnectionRow` (whose shape is a
/// UI concern free to change).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
// `deny_unknown_fields` stamps `additionalProperties: false` into the emitted
// schema so the generated TS type has NO `[k: string]: unknown` index signature —
// then a renamed/typo'd field in the consumer is a `tsc` error, not silently
// `unknown`. Matches the wire reality: the CLI never emits extra keys.
#[cfg_attr(test, derive(schemars::JsonSchema), schemars(deny_unknown_fields))]
pub struct SourceStatus {
    pub id: String,
    pub display_name: String,
    pub connected: bool,
    pub cli_present: bool,
    /// A health/issue summary (install-broken / decode-drift), or `null` when n/a.
    // Generates `health?: string | null`, kept OPTIONAL on purpose. The wire always
    // emits `health` (no `skip_serializing_if`; pinned by `source_status_json_shape`),
    // so the `?` is a harmless SUPERSET, and the consumer only does `if (s.health)`
    // — identical for optional vs required. The one schemars knob to force `required`
    // (`schemars(required)`) STRIPS the `| null` → the WRONG `health: string` (the
    // wire CAN be null), the very "mis-specified nullability" pitfall
    // PARALLEL-DELIVERY.md names. So nullable is preserved (it matters); optional is
    // kept (it doesn't).
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
    // Capture the PRIOR flag before the optimistic save, so a failed install
    // restores the exact pre-attempt state: a re-connect of an ALREADY-connected
    // source (`connect` re-run, `setup --yes`) can fail while the old, working
    // hooks stay on disk — forcing `false` there would silently disconnect a
    // healthy source on the next launch.
    let prior = config::load(cfg, &mut Vec::new()).sources.get(sid).copied();
    config::save_source_connected(cfg, sid, true)?;
    match target {
        Some(t) => match install::install_target(t, target_config, None) {
            Ok(r) => Ok(ConnectOutcome::Installed(r)),
            Err(e) => {
                // Roll the flag back to the prior state so the next launch
                // doesn't honor a "connected" with no hooks behind it — and an
                // absent flag rolls back to ABSENT (preserving the
                // `is_first_run` empty-table signal), not an explicit `false`.
                let restore = match prior {
                    Some(v) => config::save_source_connected(cfg, sid, v),
                    None => config::remove_source_connected(cfg, sid),
                };
                if let Err(re) = restore {
                    // The write path just succeeded, so this is rare — but a
                    // silently-failed restore leaves flag=true with no hooks
                    // (the shown-but-broken class), so it must leave a trace.
                    tracing::warn!(
                        source = sid,
                        error = %format!("{re:#}"),
                        "connect rollback failed to restore the prior [sources] flag"
                    );
                }
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
pub(crate) fn plan_reconcile(
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
/// each (a failed item doesn't abort the batch). The CURRENT set is computed the
/// same way the boot seed is (`config::resolve_connected` — explicit `true`
/// flags only, pure config read since the 0.12.0 migrate-inference removal).
pub fn reconcile_to(cfg: &Path, desired: &HashSet<String>) -> Vec<(String, ChangeOutcome)> {
    let app = config::load(cfg, &mut Vec::new());
    let current = config::resolve_connected(&app);
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
            Ok(o) => map_disconnect_outcome(o),
            Err(e) => ChangeOutcome::Failed(format!("{e:#}")),
        },
        Action::NoOp => ChangeOutcome::NoOp,
    }
}

/// Map a SUCCESSFUL `disconnect`'s [`DisconnectOutcome`] to the CLI
/// [`ChangeOutcome`]. Split out of `apply_one` so the load-bearing fold is
/// teeth-testable apart from the real install FS path: a folded hook-removal
/// failure MUST surface as `Failed` (with the reason), NEVER a clean
/// `Disconnected` — else a caller hides stale hooks behind a clean "disconnected".
fn map_disconnect_outcome(o: DisconnectOutcome) -> ChangeOutcome {
    match o {
        DisconnectOutcome::HookRemovalFailed(e) => {
            ChangeOutcome::Failed(format!("hooks not removed: {e}"))
        }
        DisconnectOutcome::FlagOnly | DisconnectOutcome::Uninstalled(_) => {
            ChangeOutcome::Disconnected
        }
    }
}

/// Apply an EXPLICIT per-source decision list (the first-run onboarding apply):
/// connect each `true` id, disconnect each `false` id. Unlike `reconcile_to` (which
/// is declarative over EVERY registered source and would disconnect the complement),
/// this touches ONLY the ids passed — a source absent from the list (e.g.
/// `antigravity`, which never appears in `detect()`) keeps its existing flag —
/// or, absent one, the plain disconnected default — never a surprise write.
/// Each write makes `[sources]` non-empty, so the first-run gate
/// (`setup::is_first_run`) closes. Idempotent: connect/disconnect no-op at the
/// install layer when already in state.
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

/// The onboarding SKIP freeze (pure core): map each detected source to its REAL
/// current connection state — connected in the live gate OR already carrying
/// installed hooks (`is_hooked`). Feeding THIS to [`apply_choices`] (rather than
/// the live gate alone, which is EMPTY on a first run) is what makes a skip
/// preserve an existing install: a pre-0.12 upgrader — hooks present but no
/// `[sources]` flag, exactly the population onboarding replays to re-connect —
/// freezes to `true`, so apply re-installs idempotently (a semantic no-op) instead
/// of disconnecting + UNINSTALLING its working hooks. `is_hooked` is injected so
/// this stays pure and unit-testable; production callers go through [`skip_freeze`].
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

/// The production onboarding-SKIP freeze: [`freeze_for_skip`] with the real
/// install-state probe assembled HERE, so the install-layer access
/// (`has_hooks`/`by_source`) stays funneled through this `sources` facade — the
/// same layer that owns connect/disconnect's install calls — rather than reaching
/// out of the TUI event loop. Does per-target config reads (`has_hooks`), which
/// the caller runs inline — a brief one-shot stall, like the rest of the skip I/O
/// (block_in_place was removed in #603; see the tui/CLAUDE.md sharp edge).
pub(crate) fn skip_freeze(
    detected: impl IntoIterator<Item = &'static str>,
    connected: &HashSet<String>,
) -> Vec<(&'static str, bool)> {
    freeze_for_skip(detected, connected, |id| {
        by_source(id).is_some_and(install::has_hooks)
    })
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
    /// A target-bearing CLI that isn't installed on this machine — nothing to bind
    /// to. Carries the persisted `[sources]` intent (`connected`) because `NoCli`
    /// overrides the `Connected`/`Disconnected` display (an absent CLI is worth
    /// surfacing), yet a connected-but-absent source is still disconnectable — its
    /// hooks live in the config, not the missing binary — so the toggle needs the
    /// bit the display hides.
    NoCli { connected: bool },
}

impl ConnState {
    /// Whether this source is in the live connected-set (the persisted `[sources]`
    /// intent): `true` for `Connected`, `false` for `Disconnected`, and the carried
    /// bit for `NoCli` (a connected-but-absent CLI is still disconnectable). The ONE
    /// derivation now that `ConnectionRow` no longer stores the bit separately.
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
    /// The core source id (registry `SourceDescriptor.name`, e.g. "claude-code")
    /// — the unifying key; joined to an install target via `Target.core_source`.
    pub source_id: &'static str,
    /// 2-char badge id (`cc`/`cx`/…), from the source descriptor.
    pub label_prefix: &'static str,
    pub display_name: &'static str,
    /// The connection facet — and, for `NoCli`, the persisted-intent bit the
    /// display hides (read it via `ConnState::connected`). The row no longer
    /// stores `connected` separately: `state` is the single source of truth.
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
        "omp" => "Oh My Pi",
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
///
/// NOTE: the wire `connected` here is deliberately PRESENT-AND-BOUND
/// (`state == Connected`), NOT the persisted `[sources]` intent bit
/// (`ConnState::connected` — which stays `true` for a connected-but-absent `NoCli`
/// source). The two answer different questions; the wire keeps the
/// present-and-bound meaning it always had (changing it is a `--json` contract
/// change needing `gen-contract`).
fn status_from_row(r: &ConnectionRow) -> SourceStatus {
    SourceStatus {
        id: r.source_id.to_string(),
        display_name: r.display_name.to_string(),
        connected: matches!(r.state, ConnState::Connected),
        // A no-target (JSONL-only) source is always "present"; a target-bearing
        // one is present unless probed absent (`NoCli`).
        cli_present: !matches!(r.state, ConnState::NoCli { .. }),
        health: r.health.clone(),
    }
}

/// The status of every registered source — what `pixtuoid sources [--json]` and
/// onboarding read. Resolves the connected-set the same way the boot seed does.
pub fn status(cfg: &Path, log: &str) -> Vec<SourceStatus> {
    let app = config::load(cfg, &mut Vec::new());
    let connected = config::resolve_connected(&app);
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
    fn status_from_row_connected_is_present_and_bound_not_persisted_intent() {
        // The wire `connected` is PRESENT-AND-BOUND (state == Connected), NOT the
        // persisted `[sources]` intent bit. A NoCli{connected:true} (absent CLI whose
        // stored intent is "connected") must serialize connected:false AND
        // cli_present:false — the divergence status_from_row's doc warns about. The
        // empty-HOME golden only ever produces NoCli{connected:false}+connected:false
        // rows, so it can't distinguish `matches!(Connected)` from `state.connected()`.
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
        // A pre-0.12 upgrader: config has NO [sources] table (empty connected set),
        // but claude-code's hooks ARE installed. Skip must freeze it to `true` so
        // apply re-installs idempotently (hooks survive), NOT `false` — which would
        // disconnect → uninstall its working hooks (the bug). A fresh, un-hooked
        // source (codex) freezes to `false`. Teeth: reading only the connected gate
        // (the old behavior) would freeze claude-code false here.
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
        // A source already connected in the live gate freezes to `true` even with
        // no installed hooks (e.g. antigravity, a no-target source).
        let connected = set(&["antigravity"]);
        let freeze = freeze_for_skip(["antigravity", "codex"], &connected, |_| false);
        assert_eq!(freeze, vec![("antigravity", true), ("codex", false)]);
    }

    #[test]
    fn map_disconnect_outcome_surfaces_a_folded_hook_removal_failure() {
        // The safety fold: a disconnect whose flag flipped but whose hooks did NOT
        // get removed must surface as Failed (with the reason), never a clean
        // Disconnected — else a caller hides stale hooks behind "disconnected".
        match map_disconnect_outcome(DisconnectOutcome::HookRemovalFailed("boom".into())) {
            ChangeOutcome::Failed(m) => assert_eq!(m, "hooks not removed: boom"),
            other => panic!("expected Failed, got {other:?}"),
        }
        // A clean disconnect (flag-only) maps to a plain Disconnected.
        assert!(matches!(
            map_disconnect_outcome(DisconnectOutcome::FlagOnly),
            ChangeOutcome::Disconnected
        ));
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
        // must roll back to its PRIOR state. From a fresh config that state is
        // ABSENT (keeping the is_first_run signal), not a forced `false`.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        let err = connect_target(&cfg, "rollbacktest", Some(&FAIL_TARGET), None).unwrap_err();
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
        // The already-connected re-connect case (`pixtuoid connect` re-run,
        // `setup --yes` over a working source): a failed re-install must RESTORE
        // the prior `true`, never force `false` — the old hooks are still on
        // disk and working, so persisting false silently disconnects a healthy
        // source on next launch.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        config::save_source_connected(&cfg, "rollbacktest", true).unwrap();

        let err = connect_target(&cfg, "rollbacktest", Some(&FAIL_TARGET), None).unwrap_err();
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
        // Explicit prior `false` is restored as `false` (not removed — the
        // rollback restores the exact pre-attempt state, and removal would
        // re-open the is_first_run signal for an onboarded user).
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        config::save_source_connected(&cfg, "rollbacktest", false).unwrap();

        connect_target(&cfg, "rollbacktest", Some(&FAIL_TARGET), None).unwrap_err();
        let app = config::load(&cfg, &mut Vec::new());
        assert_eq!(app.sources.get("rollbacktest"), Some(&false));
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
    fn wire_outcome_serializes_as_its_token() {
        // serde's snake_case rename and the token() table are two spellings of
        // one contract — pin them equal for every variant.
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
        // The human detail rides SEPARATELY (the `message` field) — never
        // folded into the token.
        assert_eq!(ChangeOutcome::Failed("boom".into()).message(), Some("boom"));
        assert_eq!(ChangeOutcome::Connected.message(), None);
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
    fn outcome_row_json_shape_is_the_raycast_contract() {
        // Pins the exact `{id, outcome, message?}` JSON row `connect`/
        // `disconnect`/`sources set --json` emit per source: a bare machine
        // token in `outcome`, the human detail in `message` — present exactly
        // on failure, OMITTED (not null) on success.
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
    fn outcome_row_schema_matches_the_committed_contract() {
        // The `OutcomeRow` twin of `source_status_schema_matches_the_committed_contract`
        // below (same regenerate flow: `UPDATE_CONTRACT_SCHEMA=1`, then the raycast
        // `gen:contract`). Shares the `schema_matches_the_committed_contract`
        // name suffix so one test filter regenerates both goldens.
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
        // The Raycast extension GENERATES its SourceStatus type from this committed
        // JSON Schema (`integrations/raycast/contract/source-status.schema.json`),
        // so the two can't hand-drift. This test fails if the struct changes
        // without the schema being regenerated — regenerate with
        // `just gen-contract` (UPDATE_CONTRACT_SCHEMA=1), then the raycast
        // `gen:contract` + tsc catches any consumer break.
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
        // every other source has no flag ⇒ resolves "not connected" (NoOp under an
        // empty desired — resolve_connected reads only explicit flags since the
        // 0.12.0 migrate-inference removal, so no install-state injection needed).
        // Pre-set antigravity connected; desired={} ⇒ antigravity disconnects,
        // all targets NoOp. Deterministic, no real hooks.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        connect(&cfg, "antigravity").unwrap(); // flag → true

        let outcomes: std::collections::HashMap<_, _> =
            reconcile_to(&cfg, &HashSet::new()).into_iter().collect();

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
        // the list is never touched (the "an unlisted source's flag is never
        // written" property that a declarative reconcile_to would break). Drive
        // only the no-target source so there's no agent-config I/O.
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
