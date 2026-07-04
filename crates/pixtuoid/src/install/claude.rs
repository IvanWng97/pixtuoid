use std::path::{Path, PathBuf};

use anyhow::Result;
use pixtuoid_core::source::claude_code::claude_config_dir;
use serde_json::{json, Value};

use crate::install::io;
use crate::install::merge;
use crate::install::target::MergeOutcome;
use crate::install::SENTINEL_KEY;

const EVENTS: &[&str] = &[
    "SessionStart",
    "PreToolUse",
    "PostToolUse",
    "Notification",
    // Subagent lifecycle (#241): instant child registration + the ONLY end
    // signal a Workflow-fleet subagent gets (no per-agent Agent tool_use →
    // no b1 drain; no transcript end marker → stale-sweep otherwise).
    "SubagentStart",
    "SubagentStop",
    "SessionEnd",
];

pub fn default_config_path() -> Result<PathBuf> {
    if let Some(dir) = claude_config_dir() {
        return Ok(dir.join("settings.json"));
    }
    io::home_relative_checked(".claude/settings.json")
}

/// Unix: writes the bare name so CC can PATH-resolve it (portability over absolute
/// paths — a stow-managed binary or cargo install may land in a custom prefix).
/// EXCEPTION: an explicit `--hook-path` (`explicit`) always wins — the user passed
/// it precisely because the binary is off-PATH, so the absolute path is embedded
/// (single-quoted; CC runs shell-form commands through a shell), matching the
/// Codex/Reasonix targets.
///
/// Windows: exec form requires the absolute PE path (CC's shell-form entry goes
/// through cmd.exe/PowerShell — unportable, PATHEXT-dependent). The orchestrator
/// already hard-errors if resolution failed (`BinaryStrategy::EmbedAbsolute` on
/// Windows), so `resolved` is guaranteed to be an absolute path here.
pub fn hook_command(resolved: &Path, explicit: bool) -> Result<String> {
    #[cfg(not(windows))]
    {
        if explicit {
            let p = merge::hook_path_str(resolved)?;
            return Ok(crate::install::hook_cmd::unix::shell_single_quote(p));
        }
        Ok("pixtuoid-hook".to_string())
    }
    #[cfg(windows)]
    {
        let _ = explicit; // exec form always embeds the absolute path
        merge::hook_path_str(resolved).map(str::to_string)
    }
}

/// Build the inner hook object written into the CC settings JSON.
///
/// Unix (exec_form=false): `{"type":"command","command":"pixtuoid-hook"}` —
/// shell-form, CC PATH-resolves the bare name.
///
/// Windows (exec_form=true): `{"type":"command","command":"<abs-path>","args":[]}` —
/// exec form, CC spawns the PE directly without a shell (no cmd.exe /c, no PATHEXT).
/// serde_json escapes the Windows path (`C:\…`) automatically — no hand-built JSON.
///
/// The `_pixtuoid` sentinel and `matcher` live on the OUTER entry object, not here;
/// detection/uninstall key on the sentinel, so the shape of this inner object is
/// irrelevant to the matcher — no uninstall changes needed.
pub fn hook_entry(cmd: &str, exec_form: bool) -> Value {
    if exec_form {
        json!({ "type": "command", "command": cmd, "args": [] })
    } else {
        json!({ "type": "command", "command": cmd })
    }
}

/// Install-schema verification (#309): every registered EVENT still has a managed
/// entry (sentinel-tagged), and the shim command is read back for the on-disk
/// check. The command lives in the OUTER entry's `hooks[0].command` (Unix bare
/// `pixtuoid-hook` → PATH-resolved soft check; explicit/Windows → absolute).
pub fn verify_schema(content: &str) -> crate::install::verify::SchemaParse {
    use crate::install::verify::{assemble, SchemaParse, ShimRef};
    let Ok(doc) = serde_json::from_str::<Value>(content) else {
        return SchemaParse::broken("settings.json no longer parses as JSON");
    };
    let hooks = doc.get("hooks").and_then(|h| h.as_object());
    let mut missing = Vec::new();
    let mut any = false;
    let mut shim = ShimRef::Unknown;
    for ev in EVENTS {
        let managed: Option<&Value> = hooks
            .and_then(|h| h.get(*ev))
            .and_then(|a| a.as_array())
            .and_then(|arr| arr.iter().find(|e| is_managed_entry(e)));
        match managed {
            Some(entry) => {
                any = true;
                if shim == ShimRef::Unknown {
                    shim = claude_shim_ref(entry);
                }
            }
            None => missing.push(*ev),
        }
    }
    assemble(&missing, any, shim, vec![])
}

/// The shim command for a CC managed entry is the inner `hooks[0].command`:
/// bare `pixtuoid-hook` (Unix, PATH-resolved) or an absolute path (Unix explicit
/// single-quoted / Windows exec form).
fn claude_shim_ref(entry: &Value) -> crate::install::verify::ShimRef {
    use crate::install::verify::ShimRef;
    let cmd = entry
        .get("hooks")
        .and_then(|h| h.as_array())
        .and_then(|a| a.first())
        .and_then(|h| h.get("command"))
        .and_then(|c| c.as_str());
    match cmd {
        None => ShimRef::Unknown,
        Some(c) => {
            let c = c.trim();
            if c == "pixtuoid-hook" {
                ShimRef::BareName
            } else if c.starts_with('\'') && c.ends_with('\'') {
                // Unix explicit form: a `shell_single_quote`'d absolute path. Reverse
                // the POSIX escaping via the SHARED `posix_unquote` — a naive
                // `trim_matches('\'')` mangles an embedded `'\''` (an apostrophe in the
                // path), false-flagging the install "broken" (the R0620-364-01
                // mis-decode class that the shared `shell_shim_ref` already avoids).
                ShimRef::Absolute(std::path::PathBuf::from(
                    crate::install::verify::posix_unquote(c),
                ))
            } else {
                // Windows exec form (bare absolute `.exe`) or an unquoted path.
                ShimRef::Absolute(std::path::PathBuf::from(c))
            }
        }
    }
}

pub fn merge_install(content: &str, hook_cmd: &str) -> Result<MergeOutcome> {
    let doc = merge::parse_json_or_empty(content)?;
    // Valid JSON but not an object (a top-level array/string/number) would be
    // silently discarded by the shared merge's `as_object().unwrap_or_default()`
    // — writing only our hooks and dropping the user's document. Refuse, mirroring
    // `parse_or_empty`'s stance and the uninstall twin (which preserves it).
    // Reasonix keeps the IDENTICAL guard.
    if !doc.is_object() && !doc.is_null() {
        anyhow::bail!("settings is valid JSON but not an object — refusing to overwrite");
    }
    // Delegate to the shared flat-JSON merge (the same core Reasonix/Cursor ride).
    // Claude's per-event entry is NESTED (`{matcher, hooks:[…]}`) rather than
    // flat, but that shape rides ENTIRELY in the `managed_entry` closure — the
    // merge only keys managed entries on the `_pixtuoid` sentinel, so the byte
    // output is preserved (`managed_entry` emits the same `json!` as the old inline).
    let merged =
        merge::flat_json_merge_install(doc.clone(), EVENTS, SENTINEL_KEY, managed_entry, hook_cmd);
    let changed = merged != doc;
    Ok(MergeOutcome {
        content: serde_json::to_string_pretty(&merged)?,
        changed,
    })
}

pub fn merge_uninstall(content: &str) -> Result<MergeOutcome> {
    let doc = merge::parse_json_or_empty(content)?;
    let cleaned = merge::flat_json_merge_uninstall(doc.clone(), SENTINEL_KEY);
    let changed = cleaned != doc;
    Ok(MergeOutcome {
        content: serde_json::to_string_pretty(&cleaned)?,
        changed,
    })
}

// We ONLY manage entries carrying our own `SENTINEL_KEY`; a foreign hook entry
// (another tool's, or one with an unrecognized legacy sentinel) is inert — CC
// ignores unknown hooks — and is left untouched on install/uninstall. KEPT
// because `verify_schema` (the claude-specific check whose command is nested at
// `hooks[0].command`) scans for managed entries with it. (The v0.3.0-era
// special-case strip of a foreign legacy sentinel was dropped in 0.12.0 — those
// installs are too old to keep serving.)
fn is_managed_entry(entry: &Value) -> bool {
    entry.get(SENTINEL_KEY).and_then(|v| v.as_bool()) == Some(true)
}

/// The managed per-event entry for the shared `flat_json_merge_install`. Claude's
/// is NESTED (`{_pixtuoid, matcher, hooks:[{type, command}]}`) — unlike the flat
/// Reasonix/Cursor entries — but the merge treats it opaquely (keys only on the
/// sentinel), so this closure is the one place the CC shape lives. Byte-identical
/// to the old inline `json!` push.
fn managed_entry(hook_command: &str) -> Value {
    json!({
        SENTINEL_KEY: true,
        "matcher": ".*",
        "hooks": [ hook_entry(hook_command, cfg!(windows)) ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Thin wrappers over the shared flat-JSON merge so the per-shape tests below
    // exercise Claude's events + nested-entry shape against the common core
    // (mirrors reasonix.rs's test wrappers). Production `merge_install`/
    // `merge_uninstall` call the shared helpers directly.
    fn json_merge_install(doc: Value, hook_command: &str) -> Value {
        merge::flat_json_merge_install(doc, EVENTS, SENTINEL_KEY, managed_entry, hook_command)
    }

    fn json_merge_uninstall(doc: Value) -> Value {
        merge::flat_json_merge_uninstall(doc, SENTINEL_KEY)
    }

    #[test]
    fn default_config_path_honors_claude_config_dir() {
        // std::env is process-global; serialize against the other env-mutating
        // tests in this binary (config.rs, tui/embedded_pack.rs) per repo convention.
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved_config = std::env::var_os("CLAUDE_CONFIG_DIR");
        let fallback_suffix = PathBuf::from(".claude").join("settings.json");

        std::env::remove_var("CLAUDE_CONFIG_DIR");
        let unset_path = default_config_path().unwrap();
        assert!(
            unset_path.ends_with(&fallback_suffix),
            "default config path must end with .claude/settings.json, got {unset_path:?}"
        );

        let custom_dir = std::env::temp_dir().join("pixtuoid-claude-config-dir");
        std::env::set_var("CLAUDE_CONFIG_DIR", &custom_dir);
        assert_eq!(
            default_config_path().unwrap(),
            custom_dir.join("settings.json")
        );

        std::env::set_var("CLAUDE_CONFIG_DIR", "");
        let empty_path = default_config_path().unwrap();
        assert!(
            empty_path.ends_with(&fallback_suffix),
            "empty CLAUDE_CONFIG_DIR must fall back to .claude/settings.json, got {empty_path:?}"
        );

        match saved_config {
            Some(v) => std::env::set_var("CLAUDE_CONFIG_DIR", v),
            None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
        }
    }

    #[test]
    fn install_creates_entries_for_all_events() {
        let doc = json_merge_install(json!({}), "/usr/local/bin/pixtuoid-hook");
        let hooks = doc.get("hooks").and_then(|v| v.as_object()).unwrap();
        for ev in EVENTS {
            let arr = hooks.get(*ev).and_then(|v| v.as_array()).unwrap();
            assert_eq!(arr.len(), 1, "event {ev}");
            assert_eq!(arr[0][SENTINEL_KEY], json!(true));
            assert_eq!(
                arr[0]["hooks"][0]["command"],
                json!("/usr/local/bin/pixtuoid-hook")
            );
        }
    }

    #[test]
    fn install_is_idempotent() {
        let d1 = json_merge_install(json!({}), "/x");
        let d2 = json_merge_install(d1.clone(), "/x");
        assert_eq!(d1, d2);
    }

    #[test]
    fn merge_install_rejects_valid_json_that_is_not_an_object() {
        // A top-level array/string/number is valid JSON but would be silently
        // discarded by the object-coercion — refuse rather than drop the doc.
        assert!(merge_install("[1, 2, 3]", "/x").is_err());
        assert!(merge_install("\"hi\"", "/x").is_err());
        // `null` is allowed (treated as empty → a fresh hooks object).
        assert!(merge_install("null", "/x").is_ok());
        // A normal object still installs.
        assert!(merge_install("{}", "/x").unwrap().changed);
    }

    #[test]
    fn install_preserves_unrelated_entries() {
        let initial = json!({
            "hooks": {
                "PreToolUse": [
                    { "matcher": "Write", "hooks": [{"type":"command","command":"/other"}] }
                ]
            },
            "theme": "dark"
        });
        let merged = json_merge_install(initial, "/x");
        let arr = merged["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(merged["theme"], json!("dark"));
    }

    #[test]
    fn uninstall_removes_sentinel_entries_only() {
        let installed = json_merge_install(
            json!({
                "hooks": { "PreToolUse": [
                    { "matcher": "Write", "hooks": [{"type":"command","command":"/other"}] }
                ]}
            }),
            "/x",
        );
        let cleaned = json_merge_uninstall(installed);
        let arr = cleaned["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0][SENTINEL_KEY], json!(null));
    }

    #[test]
    fn uninstall_drops_empty_hooks_map() {
        let installed = json_merge_install(json!({}), "/x");
        let cleaned = json_merge_uninstall(installed);
        assert!(cleaned.get("hooks").is_none(), "got {cleaned}");
    }

    // The v0.3.0-era special-case strip of a foreign legacy sentinel was removed
    // in 0.12.0 (see is_managed_entry): any entry lacking OUR `SENTINEL_KEY` is an
    // ordinary unmanaged entry — install/uninstall must leave it alone like any
    // other user/third-party entry (inert; CC ignores unknown hooks).
    #[test]
    fn foreign_sentinel_entries_are_no_longer_stripped() {
        let initial = json!({
            "hooks": {
                "PreToolUse": [
                    { "_legacy": true, "matcher": ".*", "hooks": [{"type":"command","command":"/old"}] }
                ]
            }
        });
        let merged = json_merge_install(initial.clone(), "/new");
        let arr = merged["hooks"]["PreToolUse"].as_array().unwrap();
        let commands: Vec<&str> = arr
            .iter()
            .map(|e| e["hooks"][0]["command"].as_str().unwrap())
            .collect();
        assert!(commands.contains(&"/old"), "legacy entry left in place");
        assert!(commands.contains(&"/new"));

        let cleaned = json_merge_uninstall(initial);
        let arr = cleaned["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1, "uninstall keeps the legacy entry too");
        assert_eq!(arr[0]["hooks"][0]["command"], json!("/old"));
    }

    #[test]
    fn uninstall_non_array_hook_value_does_not_panic() {
        let doc = json!({
            "hooks": {
                "PreToolUse": "not-an-array",
                "PostToolUse": 42
            }
        });
        let cleaned = json_merge_uninstall(doc);
        let hooks = cleaned["hooks"].as_object().unwrap();
        assert_eq!(
            hooks["PreToolUse"],
            json!("not-an-array"),
            "non-array values should pass through unchanged"
        );
        assert_eq!(hooks["PostToolUse"], json!(42));
    }

    // Defensive coercion (install side): a non-object `hooks` value is replaced
    // with a fresh object, then all events are populated.
    #[test]
    fn install_coerces_non_object_hooks_to_object() {
        let doc = json_merge_install(json!({ "hooks": "garbage-string" }), "/x");
        let hooks = doc.get("hooks").and_then(|v| v.as_object()).unwrap();
        for ev in EVENTS {
            assert_eq!(
                hooks.get(*ev).and_then(|v| v.as_array()).unwrap().len(),
                1,
                "event {ev} populated after coercion"
            );
        }
    }

    // Defensive coercion (install side): a non-array event value becomes a
    // 1-element array carrying the managed sentinel.
    #[test]
    fn install_coerces_non_array_event_to_array() {
        let doc = json_merge_install(json!({ "hooks": { "PreToolUse": 42 } }), "/x");
        let arr = doc["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert!(arr[0][SENTINEL_KEY].as_bool().unwrap());
    }

    // Uninstall early-return: a top-level non-object document is returned as-is.
    #[test]
    fn uninstall_non_object_doc_returns_unchanged() {
        let input = json!([1, 2, 3]);
        assert_eq!(json_merge_uninstall(input.clone()), input);
    }

    #[test]
    fn merge_install_on_empty_string_produces_valid_populated_config() {
        let out = merge_install("", "pixtuoid-hook").unwrap();
        assert!(out.changed);
        let v: Value = serde_json::from_str(&out.content).unwrap();
        assert!(v["hooks"]["PreToolUse"][0][SENTINEL_KEY].as_bool().unwrap());
    }

    #[test]
    fn merge_uninstall_on_empty_string_is_noop() {
        let out = merge_uninstall("").unwrap();
        assert!(!out.changed, "empty doc has nothing to remove");
        let v: Value = serde_json::from_str(&out.content).unwrap();
        assert!(v.get("hooks").is_none());
    }

    #[test]
    fn merge_install_rejects_invalid_json() {
        assert!(merge_install("{not json", "pixtuoid-hook").is_err());
    }

    // Semantic-change detection: re-installing on an already-current config (even
    // re-serialized differently) reports changed=false → no rewrite, no backup churn.
    #[test]
    fn merge_install_idempotent_reports_unchanged() {
        let first = merge_install("", "pixtuoid-hook").unwrap();
        let second = merge_install(&first.content, "pixtuoid-hook").unwrap();
        assert!(!second.changed, "second install is a semantic no-op");
    }

    // Uninstall on a hand-formatted config with NO pixtuoid hooks must be a no-op
    // (changed=false) so the orchestrator never rewrites it or deletes the backup.
    #[test]
    fn merge_uninstall_no_pixtuoid_hooks_reports_unchanged() {
        let user = "{\n  \"theme\": \"dark\",\n  \"hooks\": {\n    \"PreToolUse\": [ { \"matcher\": \"Write\", \"hooks\": [ {\"type\":\"command\",\"command\":\"/mine\"} ] } ]\n  }\n}";
        let out = merge_uninstall(user).unwrap();
        assert!(!out.changed, "no managed entries → semantic no-op");
    }

    // --- hook_command: explicit --hook-path vs bare PATH name (#19) -----------

    #[cfg(unix)]
    #[test]
    fn hook_command_explicit_path_is_embedded_on_unix() {
        // `--hook-path` always wins: the user passed it precisely because the
        // binary is off-PATH, so the bare name would write a hook that never
        // fires. Single-quoted — CC runs shell-form commands through a shell.
        let cmd = hook_command(Path::new("/opt/custom/pixtuoid-hook"), true).unwrap();
        assert_eq!(cmd, "'/opt/custom/pixtuoid-hook'");
        let spaced = hook_command(Path::new("/Users/Jane Doe/bin/pixtuoid-hook"), true).unwrap();
        assert_eq!(spaced, "'/Users/Jane Doe/bin/pixtuoid-hook'");
    }

    #[cfg(unix)]
    #[test]
    fn hook_command_auto_resolved_stays_bare_on_unix() {
        // The PATH-portability default is load-bearing (binary upgrades apply
        // immediately) — only the explicit flag overrides it.
        let cmd = hook_command(Path::new("/usr/local/bin/pixtuoid-hook"), false).unwrap();
        assert_eq!(cmd, "pixtuoid-hook");
    }

    #[cfg(unix)]
    #[test]
    fn claude_shim_ref_recovers_a_single_quoted_path_with_an_apostrophe() {
        // Claude's Unix explicit hook command IS `shell_single_quote(path)`. A path
        // containing an apostrophe round-trips as `'/U/O'\''B/hook'` — a naive
        // `trim_matches('\'')` leaves the inner `'\''` and mis-decodes the path,
        // false-flagging the install "broken" on `doctor` / the Sources panel (the
        // R0620-364-01 mis-decode class, on the bespoke claude sibling). The decoder
        // must reverse the POSIX escaping via the shared `posix_unquote`.
        use crate::install::hook_cmd::unix::shell_single_quote;
        use crate::install::verify::ShimRef;
        let path = "/U/O'B/pixtuoid-hook";
        let cmd = shell_single_quote(path);
        // sanity: the round-trip really does embed `'\''`, the case the naive trim broke.
        assert!(
            cmd.contains("'\\''"),
            "expected an escaped apostrophe in {cmd:?}"
        );
        let entry = serde_json::json!({ "hooks": [{ "command": cmd }] });
        assert_eq!(
            claude_shim_ref(&entry),
            ShimRef::Absolute(std::path::PathBuf::from(path))
        );
    }

    #[test]
    fn claude_shim_ref_half_quoted_command_is_literal_not_unquoted() {
        // Only a FULLY single-quoted command (`'…'`) is POSIX-unquoted — a
        // half-quoted/malformed string (opening quote, no close) must be taken as a
        // LITERAL path, not unquoted. Pins the `starts_with && ends_with` pairing
        // (an OR would unquote a half-quoted string).
        use crate::install::verify::ShimRef;
        let entry = serde_json::json!({ "hooks": [{ "command": "'/opt/pixtuoid-hook" }] });
        assert_eq!(
            claude_shim_ref(&entry),
            ShimRef::Absolute(std::path::PathBuf::from("'/opt/pixtuoid-hook"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn hook_command_embeds_absolute_path_on_windows_either_way() {
        for explicit in [true, false] {
            let cmd = hook_command(Path::new(r"C:\tools\pixtuoid-hook.exe"), explicit).unwrap();
            assert_eq!(cmd, r"C:\tools\pixtuoid-hook.exe");
        }
    }

    // --- hook_entry shape (runs on every platform) ----------------------------

    /// exec_form=true → `{"type":"command","command":"<path>","args":[]}`.
    /// Simulates Windows entry construction with an absolute path.
    #[test]
    fn windows_entry_is_exec_form_with_absolute_path() {
        let entry = hook_entry(r"C:\Users\user\.cargo\bin\pixtuoid-hook.exe", true);
        assert_eq!(entry["type"], json!("command"));
        assert_eq!(
            entry["command"],
            json!(r"C:\Users\user\.cargo\bin\pixtuoid-hook.exe")
        );
        // exec form MUST have an `args` key (empty array) so CC spawns via
        // exec/CreateProcess instead of a shell.
        assert_eq!(
            entry["args"],
            json!([]),
            "exec form must carry args:[] for shell-free spawn"
        );
    }

    /// exec_form=false → `{"type":"command","command":"pixtuoid-hook"}`, NO `args` key.
    /// Byte-stable Unix shape; the missing `args` key is intentional — CC shell-form
    /// (PATH resolution) requires it absent.
    #[test]
    fn unix_entry_stays_bare_shell_form() {
        let entry = hook_entry("pixtuoid-hook", false);
        assert_eq!(entry["type"], json!("command"));
        assert_eq!(entry["command"], json!("pixtuoid-hook"));
        assert!(
            entry.get("args").is_none(),
            "unix shell-form must NOT carry an args key (was: {entry})"
        );
    }

    // Internal-consistency guard (mirror of the Codex one): every hook event we
    // REGISTER with Claude Code must have a decoder arm, else it bails at the
    // shared socket and is silently dropped.
    #[test]
    fn every_registered_cc_event_decodes() {
        use pixtuoid_core::source::decoder::decode_hook_payload;
        for ev in EVENTS {
            let payload = serde_json::json!({
                "hook_event_name": ev,
                "session_id": "sess",
                "transcript_path": "/p/sess.jsonl",
                "cwd": "/repo",
                // Required by the SubagentStart/Stop arms (claim-fully guard);
                // an inert extra field for every other event.
                "agent_id": "a0000000000000001",
            });
            assert!(
                decode_hook_payload(payload).is_ok(),
                "registered CC hook {ev:?} has no decoder arm — it would bail as \
                 unsupported. Add an arm in pixtuoid-core (decoder.rs shared arms \
                 or claude_code.rs's custom decoder)."
            );
        }
    }

    // The #241 upgrade path: a re-run over a settings.json installed by an
    // older pixtuoid (pre-Subagent events) must ADD the new event arrays —
    // changed=true, all current EVENTS present — and stay idempotent after.
    #[test]
    fn reinstall_adds_newly_registered_events_to_an_older_install() {
        let old_events = [
            "SessionStart",
            "PreToolUse",
            "PostToolUse",
            "Notification",
            "SessionEnd",
        ];
        let mut old = json!({ "hooks": {} });
        for ev in old_events {
            old["hooks"][ev] = json!([{
                SENTINEL_KEY: true,
                "matcher": ".*",
                "hooks": [ hook_entry("pixtuoid-hook", false) ]
            }]);
        }
        let out = merge_install(&old.to_string(), "pixtuoid-hook").unwrap();
        assert!(out.changed, "adding the Subagent events is a real change");
        let v: Value = serde_json::from_str(&out.content).unwrap();
        for ev in EVENTS {
            assert!(
                v["hooks"][*ev][0][SENTINEL_KEY].as_bool().unwrap_or(false),
                "event {ev} must be installed after the upgrade re-run"
            );
        }
        let again = merge_install(&out.content, "pixtuoid-hook").unwrap();
        assert!(!again.changed, "second re-run is a semantic no-op");
    }
}
