//! Structured **decode-drift breadcrumbs**: every site where the upstream wire
//! format surprises us emits ONE `tracing` event with a stable
//! `target`/`kind`/`source`, so the warn-floor log `pixtuoid doctor` reads
//! captures it with no decoder signature change.
//!
//! **Alarm only on what the vendor PROMISED; defend everything else by not
//! depending on it.** An alarm states a fact about US ("this decoder met X and
//! dropped it"), never a guess about THEM ("upstream renamed X").
//!
//! **Only a surface we READ may raise one, and never from a name list.** These
//! warn per LINE with no dedup, so breadcrumbing every unrecognised name needed a
//! hand-kept mirror of upstream's vocabulary to stay quiet — and a harmless
//! upstream ADDITION then alarmed every user. Breadcrumb the condition that costs
//! events instead: a payload we decode having gone, or an unknown discriminator
//! arriving WITH that payload. An unrecognised `hook_event_name` VALUE is safe to
//! name in the `install/` sources, whose shim only receives hooks they registered
//! — it catches a re-SPELLING, never a rename, since registration is name-keyed
//! and a renamed hook never fires at all.
//!
//! A decoder ending in a bare `_ => vec![]` is silent even when the line does
//! arrive; grep the decoders for that shape rather than keeping a list here.
//!
//! `source` is a static registry name (safe). The free-form values
//! (`name`/`field`/`tool`/`detail`) are untrusted wire content, made display-safe
//! HERE at emission: the non-TUI `tracing` stream writes to RAW stderr, which no
//! cell buffer clips and no presenter sanitizes, and is on by default at `warn`.

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
