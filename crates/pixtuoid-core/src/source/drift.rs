//! Structured **decode-drift breadcrumbs** — the single source of truth for the
//! source self-diagnosis layer. Every site where the upstream wire format
//! surprises us emits ONE `tracing` event with a stable `target` + `kind` +
//! `source`, so:
//!   - the persistent warn-floor log captures it (read by `pixtuoid doctor`), and
//!   - a future counting `tracing::Layer` can tally it for the live TUI footer,
//!
//! WITHOUT any decoder signature change — the emit is an ambient side effect, so
//! the per-source `fn(&Value) -> Result<Vec<AgentEvent>>` seam (invariant #3)
//! is untouched. This is layer 2 of the upstream-drift defense ("self-monitoring
//! from the real stream", `pixtuoid-core/CLAUDE.md`) finally made VISIBLE instead
//! of stranded in a log nobody reads — the gap the Task→Agent rename exposed.
//!
//! **The flood-safe axis rule (`unknown_event` has NO dedup — it warns per
//! line).** A transcript decoder's tail may breadcrumb an unrecognized shape
//! ONLY on a LOW-cardinality STRUCTURAL axis — a brand-new line SHAPE the source
//! emits a bounded number of (codex's `RolloutItem` OUTER, copilot's `type`
//! NAMESPACE, omp's entry `type`). It MUST stay SILENT on the HIGH-cardinality
//! EVENT axis — a new value under a shape we already ignore dozens of per turn
//! (codex's `EventMsg`/`ResponseItem` inners, copilot's `assistant.*_delta`,
//! omp's `customType`) — because one warn per line there floods the warn-floor.
//! Each decoder pins its axis in a `KNOWN_*` const whose COMPLETE set is verified
//! against live upstream and (where the schema is fetchable) drift-watched by
//! `check_upstream_drift.py`, so a new upstream shape is a CI review ping BEFORE
//! it can flood. This is a per-decoder DOMAIN judgment (the axis lands in a
//! different position each source — a match arm, a namespace projection, a
//! two-tier dispatch), not a shared abstraction: the guard is 3 lines, the axis
//! choice is irreducible, so the principle is codified HERE and applied inline.
//!
//! `source` is a static registry source name (safe). The free-form values
//! (`name`/`field`/`tool`/`detail`) are untrusted wire content — every consumer
//! sanitizes (the headless path's `sanitize_line`, the footer's cell buffer).

/// The `tracing` target every drift breadcrumb shares. Consumers (the log scan
/// in `pixtuoid doctor`, the future counting Layer, the footer) key on it.
pub const TARGET: &str = "pixtuoid::drift";

/// A hook event / transcript event we don't handle (and which isn't a registered
/// custom event) — upstream likely added or renamed it. Emitted just before the
/// shared decoder `bail!`s; for a renamed event WE depend on, this is the signal.
pub fn unknown_event(source: &str, name: &str) {
    tracing::warn!(target: TARGET, source = %source, kind = "unknown_event", name = %name);
}

/// A REQUIRED field of an event we DO handle is absent — upstream likely renamed
/// it. The decode degrades to a graceful default (no panic), but attribution is
/// wrong; this is the most COMMON real drift and was previously silent.
/// Call ONLY on events we've committed to decoding (not on type-discriminator
/// reads, where a missing value just means "a line we ignore" — that would flood).
pub fn missing_field(source: &str, event: &str, field: &str) {
    tracing::warn!(target: TARGET, source = %source, kind = "missing_field", event = %event, field = %field);
}

/// The subagent-dispatch tool ran under a name we don't recognise — semantic
/// `subagent_type` detection still handled it, but upstream renamed the tool
/// (the Task→Agent class). Surfaces the new name so the known set / docs update.
pub fn unknown_dispatch(source: &str, tool: &str) {
    tracing::warn!(target: TARGET, source = %source, kind = "unknown_dispatch", tool = %tool);
}

/// A consumed upstream data SHAPE drifted — a registry/transcript field that
/// still parses but lost a key we read (#247). `detail` carries the specifics.
pub fn shape_drift(source: &str, detail: &str) {
    tracing::warn!(target: TARGET, source = %source, kind = "shape_drift", detail = %detail);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_capture::capture_logs as capture;

    // Every breadcrumb must carry the stable `target` + `kind` + `source` + its
    // distinctive value — that contract is what the log scan (`pixtuoid doctor`)
    // and the future counting Layer key on. Loose `contains` so the field-quoting
    // style of the fmt formatter can't make the test brittle.
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
            "codex", // source for unknown_event
            "missing_field",
            "toolNameZ",
            "copilot", // source for missing_field
            "unknown_dispatch",
            "DelegateZ",
            "shape_drift",
            "registry-missing-pidZ",
            "claude-code", // source for unknown_dispatch + shape_drift
        ] {
            assert!(
                out.contains(needle),
                "missing {needle:?} in captured log:\n{out}"
            );
        }
    }
}
