//! omp bridge install target — a TS EXTENSION, not a config block (#951).
//!
//! omp has no config-level shell hook (its hooks are in-process TS extension
//! modules), so pixtuoid drops an extension at
//! `<omp-agent-dir>/extensions/pixtuoid.ts`, which omp auto-discovers at
//! startup (`--no-extensions` opts out and leaves the transcript-only path).
//! The extension forwards the lifecycle/approval allowlist into the
//! `pixtuoid-hook` shim; the shim's absolute path is baked in (JSON-escaped)
//! at install time from the `omp_extension.ts` template.
//!
//! The extension FILE is wholly owned by pixtuoid, so `merge_install` renders
//! the whole file and `merge_uninstall` replaces it with a sentinel-free no-op
//! stub. ACCEPTED residual: uninstall leaves that stub rather than deleting
//! the file — the orchestrator's `write_atomic` can't delete, and the stub is
//! a harmless empty module.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::install::target::MergeOutcome;

/// Marks the extension as ours — absent from the removed-stub, so an uninstall
/// of a foreign/removed file is a clean no-op.
const SENTINEL: &str = "@pixtuoid-omp-extension";

const HOOK_PLACEHOLDER: &str = "\"{{HOOK_PATH_JSON}}\"";

const EXTENSION_TEMPLATE: &str = include_str!("omp_extension.ts");

/// A valid empty ES module WITHOUT the sentinel, so a re-uninstall is a no-op.
const REMOVED_STUB: &str = "// pixtuoid omp extension removed by disconnecting omp in pixtuoid's Sources panel (press s).\nexport default function () {}\n";

/// The extensions root is the ACTIVE agent dir (profile-, override-, and
/// `.env`-aware; deliberately NOT XDG-flattened) — core's resolver owns every
/// axis, a second copy here would be the #880 drift class.
pub(crate) fn default_config_path() -> Result<PathBuf> {
    Ok(pixtuoid_core::source::omp::omp_agent_dir()
        .join("extensions")
        .join("pixtuoid.ts"))
}

/// Presence probe for auto-detect: probe omp's OWN agent dir, NOT our
/// extension file — keying on our own artifact would chicken-and-egg (omp
/// could never be auto-detected until AFTER we'd installed into it).
pub(crate) fn detect_installed() -> bool {
    pixtuoid_core::source::omp::omp_agent_dir().exists()
}

/// omp runs extensions under Bun and spawns the shim by embedded path (no
/// PATH reliance), so `_explicit` — Claude's bare-vs-absolute switch — is
/// irrelevant here: omp always needs the absolute path.
pub(crate) fn hook_command(resolved: &Path, _explicit: bool) -> Result<String> {
    crate::install::merge::hook_path_str(resolved).map(str::to_string)
}

/// `changed` is a content diff: a same-path re-install is a no-op.
pub(crate) fn merge_install(content: &str, hook_path: &str) -> Result<MergeOutcome> {
    let baked = render_extension(hook_path)?;
    Ok(MergeOutcome {
        changed: content != baked,
        content: baked,
    })
}

/// `changed` only when the content is actually ours (carries the sentinel) — a
/// foreign file, an already-removed stub, or empty content is left untouched.
pub(crate) fn merge_uninstall(content: &str) -> Result<MergeOutcome> {
    let ours = content.contains(SENTINEL);
    Ok(MergeOutcome {
        changed: ours,
        content: if ours {
            REMOVED_STUB.to_string()
        } else {
            content.to_string()
        },
    })
}

/// The managed extension is a CODE artifact, so there is no per-event config
/// to check — only that the sentinel is present, the shim-path placeholder was
/// substituted, and the baked `HOOK_PATH` is readable for the on-disk stat.
pub(crate) fn verify_schema(content: &str) -> crate::install::verify::SchemaParse {
    use crate::install::verify::{SchemaParse, ShimRef};
    if !content.contains(SENTINEL) {
        return SchemaParse::broken(
            "the omp extension is missing or replaced (sentinel absent) — reconnect omp",
        );
    }
    if content.contains(HOOK_PLACEHOLDER) {
        return SchemaParse::broken(
            "the omp extension's shim-path placeholder was never substituted",
        );
    }
    let Some(p) = crate::install::verify::baked_hook_path(content) else {
        return SchemaParse::broken("could not read HOOK_PATH from the omp extension");
    };
    // Nothing re-installs on a pixtuoid upgrade, so without this an upgrader
    // keeps their old FORWARD set forever and doctor says fine (the opencode
    // precedent; `omp_extension_forward_set_is_pinned` makes a change
    // deliberate on the AUTHORING side while leaving the installed base
    // silent).
    let stale = render_extension(&p.to_string_lossy())
        .map(|want| want.trim() != content.trim())
        .unwrap_or(false);
    SchemaParse {
        shim: ShimRef::Absolute(p),
        issues: if stale {
            vec![
                "the installed omp extension differs from this pixtuoid's — it \
                 predates an upgrade, so events added since are not forwarded. \
                 Reconnect omp via the Sources panel."
                    .to_string(),
            ]
        } else {
            Vec::new()
        },
        ..Default::default()
    }
}

fn render_extension(hook_path: &str) -> Result<String> {
    crate::install::merge::bake_hook_path(EXTENSION_TEMPLATE, HOOK_PLACEHOLDER, hook_path, "omp")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The events the extension actually registers, read OUT of the template
    /// rather than hand-copied beside it.
    fn extension_forward_set() -> std::collections::BTreeSet<&'static str> {
        let block = EXTENSION_TEMPLATE
            .split_once("const FORWARD = new Set<string>([")
            .and_then(|(_, rest)| rest.split_once("])"))
            .map(|(inner, _)| inner)
            .expect("extension defines a FORWARD set");
        block
            .split(',')
            .map(|s| s.trim().trim_matches('"'))
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// The extension registers EXACTLY what the decoder reads. Two lists in
    /// two languages — a TS `FORWARD` set here, `match ty` arms in
    /// `pixtuoid-core` — and no compiler can see across that gap, so they are
    /// bound through the emitted drift surface instead. Register one the
    /// decoder ignores and we spawn a shim per event for nothing; miss one it
    /// reads and that signal never arrives.
    #[test]
    fn the_extension_forwards_exactly_what_the_decoder_reads() {
        let forwarded: Vec<String> = extension_forward_set()
            .into_iter()
            .map(str::to_string)
            .collect();

        let surface: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../pixtuoid-core/drift-surface.json"),
            )
            .expect("the core drift surface is committed"),
        )
        .expect("the drift surface parses");
        let mut decoded: Vec<String> = surface["decoded"]["omp.hook_events"]
            .as_array()
            .expect("omp.hook_events is a set")
            .iter()
            .map(|v| v.as_str().expect("a name").to_string())
            .collect();
        decoded.sort();

        assert_eq!(forwarded, decoded);
    }

    /// The registered set is a TS `Set`, not a Rust const, so the `*_EVENTS`
    /// membership pins cannot reach it — an event dropped here never reaches
    /// the shim, and no other test can see it (the opencode
    /// `permission.v2.asked` hole).
    #[test]
    fn omp_extension_forward_set_is_pinned() {
        use std::collections::BTreeSet;
        assert_eq!(
            extension_forward_set(),
            BTreeSet::from([
                "session_start",
                "session_switch",
                "session_branch",
                "session_shutdown",
                "tool_approval_requested",
                "tool_approval_resolved",
            ]),
            "omp_extension.ts FORWARD changed — update the decoder and this pin together"
        );
    }

    /// Every event the EXTENSION registers must decode to something — driven
    /// off the template's own set, so a new `FORWARD` entry with no decoder
    /// arm fails here rather than silently arriving as an unmapped event.
    #[test]
    fn every_forwarded_omp_event_decodes() {
        use pixtuoid_core::source::decoder::decode_hook_payload;
        const FILE: &str = "/h/.omp/agent/sessions/-r/2026-08-31T06-00-52-863Z_01a05668-057f-7559-8fed-f28ff062e3ca.jsonl";
        let payloads = [
            serde_json::json!({"type": "session_start", "sessionFile": FILE,
                "sessionId": "u", "cwd": "/r", "_pixtuoid_source": "omp"}),
            serde_json::json!({"type": "session_switch", "sessionFile": FILE,
                "previousSessionFile": "/h/.omp/agent/sessions/-r/2026-08-30T01-00-00-000Z_01a00000-0000-7000-8000-000000000009.jsonl",
                "sessionId": "u", "cwd": "/r", "_pixtuoid_source": "omp"}),
            serde_json::json!({"type": "session_branch", "sessionFile": FILE,
                "previousSessionFile": FILE,
                "sessionId": "u", "cwd": "/r", "_pixtuoid_source": "omp"}),
            serde_json::json!({"type": "session_shutdown", "sessionFile": FILE,
                "sessionId": "u", "cwd": "/r", "_pixtuoid_source": "omp"}),
            serde_json::json!({"type": "tool_approval_requested", "sessionFile": FILE,
                "sessionId": "u", "cwd": "/r", "toolCallId": "c1", "toolName": "bash",
                "_pixtuoid_source": "omp"}),
            serde_json::json!({"type": "tool_approval_resolved", "sessionFile": FILE,
                "sessionId": "u", "cwd": "/r", "toolCallId": "c1", "toolName": "bash",
                "approved": true, "_pixtuoid_source": "omp"}),
        ];
        let covered: std::collections::BTreeSet<&str> = payloads
            .iter()
            .map(|p| p["type"].as_str().unwrap())
            .collect();
        for ev in extension_forward_set() {
            assert!(
                covered.contains(ev),
                "extension forwards `{ev}` but this test has no payload for it — the \
                 hand-listed fixtures drifted from omp_extension.ts"
            );
        }
        for p in payloads {
            let ty = p["type"].clone();
            let evs = decode_hook_payload(p).unwrap();
            assert!(
                !evs.is_empty(),
                "forwarded omp event {ty} decoded to NOTHING — add an arm in source/omp.rs"
            );
        }
    }

    /// The bridge must never hold a gate: omp's `tool_call` handlers fail
    /// CLOSED (a throw/timeout blocks the user's tool), so the extension may
    /// only ever observe. Anchored on `pi.on(name, …)` being the SOLE
    /// registration shape — a literal `pi.on("tool_call"` (or any name outside
    /// FORWARD) cannot appear.
    #[test]
    fn the_extension_registers_no_gating_handlers_and_awaits_nothing() {
        assert!(
            !EXTENSION_TEMPLATE.contains("pi.on(\""),
            "every registration must go through the FORWARD loop, so the pin \
             on FORWARD covers the whole registered set"
        );
        // `await ` (the expression form) — prose in comments says "awaited".
        assert!(
            !EXTENSION_TEMPLATE.contains("await ") && !EXTENSION_TEMPLATE.contains("async "),
            "nothing may be awaited — session_shutdown runs during process \
             exit and a slow shim must never hold omp's handler budget"
        );
        for gate in ["tool_call", "tool_result", "input", "session_before"] {
            assert!(
                !extension_forward_set().iter().any(|e| e.starts_with(gate)),
                "`{gate}*` is a blocking/gating handler class — the bridge is \
                 observe-only"
            );
        }
    }

    #[test]
    fn verify_schema_reports_a_bridge_that_predates_this_pixtuoid() {
        let current = render_extension("/opt/pixtuoid-hook").expect("render");
        assert!(
            verify_schema(&current).issues.is_empty(),
            "the extension this binary would install must verify clean"
        );

        // An older pixtuoid's extension: same sentinel, same baked path, one
        // event short — exactly the shape an upgrader keeps.
        let stale = current.replacen("\"tool_approval_resolved\",\n", "", 1);
        assert_ne!(stale, current, "the mutation must land");
        let issues = verify_schema(&stale).issues;
        assert!(
            issues
                .iter()
                .any(|i| i.contains("predates an upgrade") && i.contains("Reconnect omp")),
            "a stale extension must be reported with its remedy, got {issues:?}"
        );
    }

    #[test]
    fn verify_schema_reports_the_baked_shim_and_every_way_the_bridge_can_be_dead() {
        use crate::install::verify::ShimRef;

        let installed = merge_install("", "/opt/bin/pixtuoid-hook").unwrap().content;
        let sound = verify_schema(&installed);
        assert!(
            sound.issues.is_empty(),
            "a freshly rendered extension is sound — got {:?}",
            sound.issues
        );
        assert_eq!(
            sound.shim,
            ShimRef::Absolute(std::path::PathBuf::from("/opt/bin/pixtuoid-hook")),
            "the baked HOOK_PATH must be reported, or the shim-on-disk check is skipped"
        );

        let foreign = verify_schema("export default function () {}\n");
        assert_eq!(foreign.shim, ShimRef::Unknown);
        assert!(
            foreign.issues.iter().any(|i| i.contains("reconnect omp")),
            "a sentinel-less file must be a HARD issue naming the remedy — got {:?}",
            foreign.issues
        );

        let unsubstituted = format!("// {SENTINEL}\nconst HOOK_PATH = {HOOK_PLACEHOLDER};\n");
        assert!(
            verify_schema(&unsubstituted)
                .issues
                .iter()
                .any(|i| i.contains("placeholder")),
            "an unsubstituted placeholder must be reported"
        );

        let no_binding = format!("// {SENTINEL}\nexport default function () {{}}\n");
        assert!(
            verify_schema(&no_binding)
                .issues
                .iter()
                .any(|i| i.contains("HOOK_PATH")),
            "an unreadable HOOK_PATH must be reported, not silently accepted"
        );
    }

    #[test]
    fn install_bakes_the_hook_path_and_carries_the_sentinel() {
        let out = merge_install("", "/opt/bin/pixtuoid-hook").unwrap();
        assert!(out.changed);
        assert!(out.content.contains(SENTINEL));
        assert!(out.content.contains("\"/opt/bin/pixtuoid-hook\""));
        assert!(!out.content.contains(HOOK_PLACEHOLDER));
        assert!(
            out.content.contains("--source"),
            "spawns the shim with --source omp"
        );
    }

    #[test]
    fn install_is_idempotent_and_re_renders_on_a_path_change() {
        let a = merge_install("", "/opt/bin/pixtuoid-hook").unwrap();
        let b = merge_install(&a.content, "/opt/bin/pixtuoid-hook").unwrap();
        assert!(!b.changed, "same-path re-install is a content no-op");
        let c = merge_install(&a.content, "/usr/local/bin/pixtuoid-hook").unwrap();
        assert!(c.changed);
        assert!(c.content.contains("\"/usr/local/bin/pixtuoid-hook\""));
    }

    #[test]
    fn a_path_with_special_chars_bakes_as_a_valid_escaped_literal() {
        let out = merge_install("", r#"/weird/pi"x\hook"#).unwrap();
        assert!(out.content.contains(r#""/weird/pi\"x\\hook""#));
    }

    #[test]
    fn uninstall_replaces_our_extension_with_a_sentinel_free_stub() {
        let installed = merge_install("", "/opt/bin/pixtuoid-hook").unwrap();
        let removed = merge_uninstall(&installed.content).unwrap();
        assert!(removed.changed);
        assert!(
            !removed.content.contains(SENTINEL),
            "stub must drop the sentinel so detection flips"
        );
        assert!(
            removed.content.contains("export default function () {}"),
            "stub is a valid inert module — omp still imports and calls it"
        );
    }

    #[test]
    fn uninstall_of_a_foreign_or_removed_file_is_a_no_op() {
        let foreign = "export default function (pi) {}\n";
        assert!(!merge_uninstall(foreign).unwrap().changed);
        assert!(!merge_uninstall(REMOVED_STUB).unwrap().changed);
        assert!(!merge_uninstall("").unwrap().changed);
    }

    #[test]
    fn hook_command_returns_the_absolute_path() {
        assert_eq!(
            hook_command(Path::new("/opt/bin/pixtuoid-hook"), false).unwrap(),
            "/opt/bin/pixtuoid-hook"
        );
    }
}
