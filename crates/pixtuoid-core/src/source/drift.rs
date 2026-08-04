//! Structured **decode-drift breadcrumbs** — the single source of truth for the
//! source self-diagnosis layer. Every site where the upstream wire format
//! surprises us emits ONE `tracing` event with a stable `target` + `kind` +
//! `source`, so the persistent warn-floor log (read by `pixtuoid doctor`)
//! captures it without any decoder signature change.
//!
//! **The flood-safe axis rule (`unknown_event` has NO dedup — it warns per
//! line).** A transcript decoder's tail may breadcrumb an unrecognized shape
//! ONLY on a LOW-cardinality STRUCTURAL axis — a brand-new line SHAPE the source
//! emits a bounded number of (codex's `RolloutItem` OUTER, copilot's `type`
//! NAMESPACE, omp's entry `type`). It MUST stay SILENT on the HIGH-cardinality
//! EVENT axis — a new value under a shape we already ignore dozens of per turn
//! (codex's `EventMsg`/`ResponseItem` inners, copilot's `assistant.*_delta`,
//! omp's `customType`) — because one warn per line there floods the warn-floor.
//! Each decoder pins its axis in a `KNOWN_*` const, drift-watched by
//! `check_upstream_drift.py` where the schema is fetchable. The axis lands in a
//! different position in each source, so the principle is codified HERE and
//! applied inline rather than abstracted.
//!
//! `source` is a static registry source name (safe). The free-form values
//! (`name`/`field`/`tool`/`detail`) are untrusted wire content, so they are made
//! display-safe HERE, at emission: the non-TUI `tracing` stream writes to RAW
//! stderr, which no cell buffer clips and no presenter sanitizes, and it is on
//! by default at `warn`.

/// The `tracing` target every drift breadcrumb shares; consumers key on it.
pub const TARGET: &str = "pixtuoid::drift";

use crate::source::decoder::display_safe;

/// A hook/transcript event we don't handle and that isn't a registered custom
/// event — for a renamed event WE depend on, this is the signal.
pub fn unknown_event(source: &str, name: &str) {
    tracing::warn!(target: TARGET, source = %source, kind = "unknown_event", name = %display_safe(name));
}

/// A REQUIRED field of an event we DO handle is absent — the decode degrades to
/// a graceful default, but attribution is wrong. Call ONLY on events we've
/// committed to decoding: on a type-discriminator read a missing value just
/// means "a line we ignore", and breadcrumbing those would flood.
pub fn missing_field(source: &str, event: &str, field: &str) {
    tracing::warn!(target: TARGET, source = %source, kind = "missing_field", event = %display_safe(event), field = %display_safe(field));
}

/// The subagent-dispatch tool ran under a name we don't recognise — semantic
/// `subagent_type` detection still handled it, but upstream renamed the tool.
pub fn unknown_dispatch(source: &str, tool: &str) {
    tracing::warn!(target: TARGET, source = %source, kind = "unknown_dispatch", tool = %display_safe(tool));
}

/// A consumed upstream data SHAPE drifted — a registry/transcript field that
/// still parses but lost a key we read. `detail` carries the specifics.
pub fn shape_drift(source: &str, detail: &str) {
    tracing::warn!(target: TARGET, source = %source, kind = "shape_drift", detail = %display_safe(detail));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_capture::capture_logs as capture;

    #[test]
    fn breadcrumb_values_are_display_safe_and_capped() {
        // Cf bidi overrides are checked alongside Cc because
        // `char::is_control` is Cc-only, and Trojan Source (CVE-2021-42574)
        // rode exactly that gap.
        let out = capture(|| {
            unknown_event("codex", "ev\u{1b}]0;PWNED\u{7}il\u{202e}Z");
            unknown_dispatch("claude-code", "De\u{1b}[31mlegateZ");
            missing_field("copilot", "to\u{1b}olZ", "na\u{202e}meZ");
            shape_drift("claude-code", &"x".repeat(1000));
        });
        for bad in ['\u{1b}', '\u{7}', '\u{202e}'] {
            assert!(
                !out.contains(bad),
                "U+{:04X} reached the terminal sink:\n{out}",
                bad as u32
            );
        }
        // Sanitizing is not dropping.
        for needle in ["ev]0;PWNEDilZ", "De[31mlegateZ", "toolZ", "nameZ"] {
            assert!(out.contains(needle), "missing {needle:?} in:\n{out}");
        }
        assert!(
            !out.contains(&"x".repeat(200)),
            "an uncapped value became an uncapped log line:\n{out}"
        );
    }

    #[test]
    fn breadcrumbs_emit_the_structured_drift_target_and_fields() {
        let out = capture(|| {
            unknown_event("codex", "MysteryHookZ");
            missing_field("copilot", "tool.execution_start", "toolNameZ");
            unknown_dispatch("claude-code", "DelegateZ");
            shape_drift("claude-code", "registry-missing-pidZ");
        });
        for needle in [
            TARGET,
            "unknown_event",
            "MysteryHookZ",
            "codex",
            "missing_field",
            "toolNameZ",
            "copilot",
            "unknown_dispatch",
            "DelegateZ",
            "shape_drift",
            "registry-missing-pidZ",
            "claude-code",
        ] {
            assert!(
                out.contains(needle),
                "missing {needle:?} in captured log:\n{out}"
            );
        }
    }
}
