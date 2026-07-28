//! The scriptable sources-CLI presenters over `pixtuoid::sources` (the TUI-free
//! core): `setup` / `sources [set]` / the shared `connect`/`disconnect` runner.
//! Binary-crate module (lifted out of `main.rs`) — a SIBLING of `sources.rs`,
//! kept out of it on purpose: the core stays presenter-free (its other two
//! presenters, the in-TUI panel and onboarding, live in their own modules too).
//! `main.rs` dispatches here; the `--json` row shape is the typed
//! [`pixtuoid::sources::OutcomeRow`] wire contract.

use std::path::Path;

use anyhow::Result;
use pixtuoid::{config, sources};

use crate::logging::log_file_path;

/// `pixtuoid setup [--yes]` — the headless onboarding twin (Raycast / CI /
/// scripting). Detects installed agent CLIs and connects them via the SAME
/// `sources::apply_choices` the in-TUI onboarding uses. Without `--yes` it is a
/// DRY RUN (prints the detected set only) — writing to another tool's config is
/// opt-in. Exits non-zero if any connect fails (a `$?`-checking caller's signal).
pub(crate) fn run_setup(yes: bool) -> Result<()> {
    let detected = sources::detect();
    if detected.is_empty() {
        println!("No agent CLIs detected on this machine \u{2014} nothing to set up.");
        return Ok(());
    }
    if !yes {
        println!("Detected agent CLIs (run `pixtuoid setup --yes` to connect):");
        for sid in &detected {
            println!("  {sid}");
        }
        return Ok(());
    }
    let cfg = config::config_path();
    let choices: Vec<(&'static str, bool)> = detected.iter().map(|&s| (s, true)).collect();
    let mut any_failed = false;
    for (id, oc) in sources::apply_choices(&cfg, &choices) {
        if matches!(oc, sources::ChangeOutcome::Failed(_)) {
            any_failed = true;
        }
        let row = sources::OutcomeRow::new(id, &oc);
        println!("{}", text_line(&row));
        if let Some(hint) = hint_line(&row) {
            println!("{hint}");
        }
    }
    if any_failed {
        anyhow::bail!("one or more sources failed to connect (see the rows above)");
    }
    Ok(())
}

/// `pixtuoid sources [--json]` — print every source's connection state. Read-only.
pub(crate) fn run_sources_list(json: bool) -> Result<()> {
    let cfg = config::config_path();
    // Same NotFound-vs-everything-else split `doctor` makes: an unreadable log
    // leaves `health` under-reported, so say so instead of returning a silent
    // clean bill. It rides tracing (stderr in every non-TUI mode) rather than
    // stdout — `--json` stdout is the frozen Raycast array.
    let (log, log_warning) = pixtuoid::doctor::read_log(&log_file_path());
    if let Some(w) = log_warning {
        tracing::warn!("{w}");
    }
    let rows = sources::status(&cfg, &log);
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for r in &rows {
            let (mark, state) = if r.connected {
                ('\u{25cf}', "connected") // ●
            } else if r.cli_present {
                ('\u{25cb}', "disconnected") // ○
            } else {
                ('\u{00b7}', "not installed") // ·
            };
            println!("{mark} {:<16} {state}", r.id);
            if let Some(h) = &r.health {
                println!("    {h}");
            }
        }
    }
    Ok(())
}

/// `pixtuoid sources set <ids>` — declarative reconcile (connected set = exactly these).
pub(crate) fn run_sources_set(ids: &[String], json: bool) -> Result<()> {
    let cfg = config::config_path();
    // Validate every id up front so a typo can't partially apply.
    let desired: std::collections::HashSet<String> = ids
        .iter()
        .map(|id| sources::registered_id(id).map(String::from))
        .collect::<Result<_>>()?;
    let rows: Vec<sources::OutcomeRow> = sources::reconcile_to(&cfg, &desired)
        .into_iter()
        .map(|(id, oc)| sources::OutcomeRow::new(id, &oc))
        .collect();
    report_batch(&rows, json)
}

/// Shared `connect`/`disconnect` presenter: validate all ids up front, then apply
/// each, reporting per-source. `op` returns the SUCCESS outcome; an `Err` becomes
/// a `failed` row (message = the error) AND makes the whole command exit non-zero
/// (after emitting all rows) so a `$?`-checking shell/CI/onboarding caller gets a
/// real error signal.
pub(crate) fn run_change(
    ids: &[String],
    json: bool,
    op: impl Fn(&Path, &str) -> Result<sources::ChangeOutcome>,
) -> Result<()> {
    let cfg = config::config_path();
    let sids: Vec<&'static str> = ids
        .iter()
        .map(|id| sources::registered_id(id))
        .collect::<Result<_>>()?;
    let rows: Vec<sources::OutcomeRow> = sids
        .into_iter()
        .map(|sid| {
            let oc = op(&cfg, sid).unwrap_or_else(|e| {
                // The same `failed` token + message split `sources set` emits —
                // spelled through the ONE outcome→row authority (`OutcomeRow::new`)
                // so the two command surfaces can't drift.
                sources::ChangeOutcome::Failed(format!("{e:#}"))
            });
            sources::OutcomeRow::new(sid.to_string(), &oc)
        })
        .collect();
    report_batch(&rows, json)
}

/// The shared tail of `run_change` / `run_sources_set`: emit the batch, then fail
/// the command if any row failed. Emits BEFORE bailing — the `--json` rows must
/// reach stdout even on the non-zero exit (the delivery contract the Raycast
/// consumer rides, pinned by
/// `cli_json::a_failing_connect_emits_the_outcome_rows_and_exits_nonzero`).
/// `any_failed` is DERIVED from the rows (their `outcome` token), so the emitted
/// rows and the exit code can't disagree.
fn report_batch(rows: &[sources::OutcomeRow], json: bool) -> Result<()> {
    emit_outcomes(rows, json)?;
    if rows
        .iter()
        .any(|r| r.outcome == sources::WireOutcome::Failed)
    {
        anyhow::bail!("one or more sources failed (see the rows above)");
    }
    Ok(())
}

/// Print an [`sources::OutcomeRow`] batch as a text table or the `--json` array —
/// the schema-backed envelope the Raycast extension parses back from
/// `connect`/`disconnect`/`sources set` (pinned by
/// `outcome_envelope_is_the_id_outcome_raycast_contract` here plus the
/// byte-shape + committed-schema goldens in `sources.rs`).
fn emit_outcomes(rows: &[sources::OutcomeRow], json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(rows)?);
    } else {
        for row in rows {
            println!("{}", text_line(row));
            if let Some(hint) = hint_line(row) {
                println!("{hint}");
            }
        }
    }
    Ok(())
}

/// The ONE human (non-`--json`) row form, shared by `emit_outcomes` and
/// `run_setup`: `id: token`, with the failure detail appended (`id: failed: msg`
/// — the same line shape the pre-split fold printed).
fn text_line(row: &sources::OutcomeRow) -> String {
    match &row.message {
        Some(m) => format!("{}: {}: {m}", row.id, row.outcome),
        None => format!("{}: {}", row.id, row.outcome),
    }
}

/// The optional SECOND human line under a row — the target's post-install step,
/// because connecting is not always the last one (OpenClaw's running gateway must
/// restart before it loads the plugin). `None` unless the row actually connected
/// AND its target declares a step.
///
/// Owned here, not written at each call site, so `connect` and `setup --yes` cannot
/// disagree on the gate or the glyph. HUMAN output only: the `--json` envelope is
/// the frozen `{id, outcome, message?}` Raycast contract where `message` means
/// FAILURE, so an advisory there would change its meaning.
fn hint_line(row: &sources::OutcomeRow) -> Option<String> {
    (row.outcome == sources::WireOutcome::Connected)
        .then(|| sources::post_install_hint(&row.id))
        .flatten()
        .map(|hint| format!("  \u{21b3} {hint}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_envelope_is_the_id_outcome_raycast_contract() {
        // Pins the exact `{id, outcome, message?}` JSON rows `connect`/
        // `disconnect`/`sources set --json` emit — the batch envelope the
        // Raycast extension parses. A key rename must break THIS test, not the
        // consumer. The outcome TOKEN set itself ("connected"/"disconnected"/
        // "no_op"/"failed") is pinned by sources.rs's
        // `change_outcome_wire_tokens_are_stable`, and every emission site
        // routes through `OutcomeRow::new`, so the failed row below exercises
        // the same token+message split the CLI ships.
        //
        // Raycast is not the only consumer: homebrew-core's formula `test do`
        // parses `connect claude-code --json` and asserts the row equals
        // `{"id" => "claude-code", "outcome" => "connected"}`. Reshaping this
        // envelope breaks Homebrew's CI on the next autobump.
        let rows = vec![
            sources::OutcomeRow::new("codex".to_string(), &sources::ChangeOutcome::Connected),
            sources::OutcomeRow::new(
                "cursor".to_string(),
                &sources::ChangeOutcome::Failed("boom".into()),
            ),
        ];
        assert_eq!(
            serde_json::to_string(&rows).unwrap(),
            r#"[{"id":"codex","outcome":"connected"},{"id":"cursor","outcome":"failed","message":"boom"}]"#
        );
    }

    /// The hint line's GATE and its shape. Mutation testing found this feature had
    /// no teeth at all: stubbing `hint_line` to `None` (i.e. silently never telling
    /// the user to restart their gateway) passed the whole suite.
    #[test]
    fn the_post_install_hint_line_rides_a_connected_row_only() {
        let row =
            |id: &str, oc: sources::ChangeOutcome| sources::OutcomeRow::new(id.to_string(), &oc);

        // A source WITH a step, actually connected ⇒ the indented `↳` second line.
        let line = hint_line(&row("openclaw", sources::ChangeOutcome::Connected))
            .expect("openclaw declares a post-install step");
        assert!(
            line.starts_with("  \u{21b3} "),
            "the hint is an indented continuation of its row — got {line:?}"
        );
        assert_eq!(
            line.trim_start_matches([' ', '\u{21b3}']),
            sources::post_install_hint("openclaw").expect("declared"),
            "the line carries the target's own hint verbatim, never a re-worded copy"
        );

        // Every other outcome is NOT a completed install, so the step must not print:
        // `no_op` especially, or a re-run of `setup --yes` would nag about a gateway
        // the user already restarted.
        for oc in [
            sources::ChangeOutcome::NoOp,
            sources::ChangeOutcome::Disconnected,
            sources::ChangeOutcome::Failed("boom".into()),
        ] {
            assert!(
                hint_line(&row("openclaw", oc.clone())).is_none(),
                "{oc:?} is not a completed install — no step to announce"
            );
        }

        // A connected source with no declared step stays a single line.
        assert!(
            hint_line(&row("claude-code", sources::ChangeOutcome::Connected)).is_none(),
            "claude-code's hooks take effect on its next run — nothing to add"
        );
    }
}
