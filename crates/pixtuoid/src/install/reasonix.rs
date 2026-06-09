//! Reasonix hook install target.
//!
//! Writes the GLOBAL `~/.reasonix/settings.json` — project-scope
//! (`<repo>/.reasonix/settings.json`) hooks only load after the user runs
//! `/hooks trust`, so a project-scope install would silently never fire
//! (`internal/hook/trust.go` @v1.2.0). The schema is Reasonix's own, FLAT
//! shape (`internal/hook/hook.go:88-106` @v1.2.0) — per-event arrays of
//! `{match, command, description, timeout, cwd}` entries, NOT Claude's nested
//! `{matcher, hooks: [{type, command}]}` groups:
//!
//! ```json
//! {"hooks": {"PreToolUse": [{"command": "PIXTUOID_SOURCE=reasonix '/abs/pixtuoid-hook'",
//!                            "timeout": 1000, "description": "pixtuoid visualizer",
//!                            "_pixtuoid": true}]}}
//! ```
//!
//! - `match` is OMITTED: empty = every tool. (Upstream special-cases `"*"` to
//!   every-tool as well; any OTHER value is an ANCHORED regex and a malformed
//!   one never fires — omission is the simplest always-fires form.)
//! - `timeout` is in MILLISECONDS (upstream default 5000 for the gating
//!   PreToolUse, where a TIMEOUT BLOCKS the user's tool call). The shim
//!   self-limits to 200ms and always exits 0, so 1000ms is pure headroom.
//! - `_pixtuoid` is the managed-entry sentinel; Go's `json.Unmarshal` ignores
//!   unknown fields, so Reasonix never sees it.
//! - Hooks are loaded once at session boot — the orchestrator's standard
//!   "start a new session" hint covers activation.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Map, Value};

use crate::install::io;
#[cfg(unix)]
use crate::install::io::shell_single_quote;
use crate::install::target::MergeOutcome;

const SENTINEL_KEY: &str = "_pixtuoid";

/// Events we register == events we decode (`source/reasonix.rs`), enforced by
/// `every_registered_reasonix_event_decodes` below. PostLLMCall / PreCompact /
/// SubagentStop are deliberately absent: per-model-turn noise, compaction
/// internals, and a no-id subagent signal already covered by the parent's
/// `task` PostToolUse.
const REASONIX_EVENTS: &[&str] = &[
    "SessionStart",
    "PreToolUse",
    "PostToolUse",
    "UserPromptSubmit",
    "Stop",
    "Notification",
    "SessionEnd",
];

pub fn default_config_path() -> PathBuf {
    io::home_relative(".reasonix/settings.json")
}

/// Presence probe for auto-detection. The default file-exists check on
/// `default_config_path` would NEVER fire: Reasonix itself never creates
/// `~/.reasonix/settings.json` (it is purely user-authored; `readSettings`
/// just returns nil when missing). What a real install does create is the v2
/// config dir — `os.UserConfigDir()/reasonix` (sessions/config live there) —
/// and hook/trust users additionally have `~/.reasonix`. Probe both.
pub fn detect_installed() -> bool {
    user_config_dir().join("reasonix").exists() || io::home_relative(".reasonix").exists()
}

/// Rust mapping of Go's `os.UserConfigDir()` for the platforms we ship:
/// macOS `$HOME/Library/Application Support`, **Windows `%APPDATA%`** (Roaming —
/// where Reasonix's v2 config dir actually lives; without this arm
/// `detect_installed` probes `~/.config/reasonix` on Windows, which Reasonix never
/// creates, so auto-detection would always miss), else `$XDG_CONFIG_HOME` falling
/// back to `~/.config`.
fn user_config_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        io::home_relative("Library/Application Support")
    } else if cfg!(windows) {
        match std::env::var("APPDATA") {
            Ok(a) if !a.is_empty() => PathBuf::from(a),
            _ => io::home_relative("AppData/Roaming"),
        }
    } else {
        match std::env::var("XDG_CONFIG_HOME") {
            Ok(x) if !x.is_empty() => PathBuf::from(x),
            _ => io::home_relative(".config"),
        }
    }
}

/// Reasonix runs the `command` string under a shell — `sh -c` on Unix, `cmd.exe
/// /c` on Windows (verified: `internal/hook/hook.go:414` `shellInvocation`, an
/// explicit `GOOS=="windows"` branch). Same contract as Codex, so the OS forms
/// mirror codex::hook_command exactly:
/// - **Unix**: env-prefix `PIXTUOID_SOURCE=reasonix '<abs-path>'` (single-quoted).
/// - **Windows**: BARE `<abs-path> --source reasonix` via the shared
///   `io::windows_bare_hook_command` (cmd.exe can't express the env-prefix; the
///   source rides as the shim's `--source` flag). That helper REJECTS a path with
///   a space or cmd metacharacter (#195) — a quoted path can't survive cmd /C.
///
/// Err on non-UTF-8 (prevents the to_string_lossy dead-hook).
pub fn hook_command(resolved: &Path) -> Result<String> {
    let p = resolved
        .to_str()
        .ok_or_else(|| anyhow!("pixtuoid-hook path is non-UTF-8: {}", resolved.display()))?;
    #[cfg(windows)]
    let cmd = io::windows_bare_hook_command(p, "reasonix")?;
    #[cfg(unix)]
    let cmd = format!("PIXTUOID_SOURCE=reasonix {}", shell_single_quote(p));
    Ok(cmd)
}

fn parse_or_empty(content: &str) -> Result<Value> {
    if content.trim().is_empty() {
        return Ok(json!({}));
    }
    // No file path here — the orchestrator wraps the error with the real path.
    serde_json::from_str(content).context("not valid JSON — refusing to overwrite")
}

pub fn merge_install(content: &str, hook_cmd: &str) -> Result<MergeOutcome> {
    let doc = parse_or_empty(content)?;
    let merged = json_merge_install(doc.clone(), hook_cmd);
    let changed = merged != doc;
    Ok(MergeOutcome {
        content: serde_json::to_string_pretty(&merged)?,
        changed,
    })
}

pub fn merge_uninstall(content: &str) -> Result<MergeOutcome> {
    let doc = parse_or_empty(content)?;
    let cleaned = json_merge_uninstall(doc.clone());
    let changed = cleaned != doc;
    Ok(MergeOutcome {
        content: serde_json::to_string_pretty(&cleaned)?,
        changed,
    })
}

fn is_managed_entry(entry: &Value) -> bool {
    entry.get(SENTINEL_KEY).and_then(|v| v.as_bool()) == Some(true)
}

fn managed_entry(hook_command: &str) -> Value {
    json!({
        SENTINEL_KEY: true,
        "command": hook_command,
        "timeout": 1000,
        "description": "pixtuoid visualizer"
    })
}

fn json_merge_install(doc: Value, hook_command: &str) -> Value {
    let mut root: Map<String, Value> = doc.as_object().cloned().unwrap_or_default();
    let hooks = root
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !hooks.is_object() {
        *hooks = Value::Object(Map::new());
    }
    if let Value::Object(hooks_obj) = hooks {
        for ev in REASONIX_EVENTS {
            let list = hooks_obj
                .entry((*ev).to_string())
                .or_insert_with(|| Value::Array(vec![]));
            if !list.is_array() {
                *list = Value::Array(vec![]);
            }
            if let Value::Array(arr) = list {
                arr.retain(|entry| !is_managed_entry(entry));
                arr.push(managed_entry(hook_command));
            }
        }
    }
    Value::Object(root)
}

fn json_merge_uninstall(mut doc: Value) -> Value {
    let Some(root) = doc.as_object_mut() else {
        return doc;
    };
    let Some(Value::Object(hooks_obj)) = root.get_mut("hooks") else {
        return doc;
    };
    for (_ev, list) in hooks_obj.iter_mut() {
        if let Some(arr) = list.as_array_mut() {
            arr.retain(|entry| !is_managed_entry(entry));
        }
    }
    let to_remove: Vec<String> = hooks_obj
        .iter()
        .filter_map(|(k, v)| match v.as_array() {
            Some(a) if a.is_empty() => Some(k.clone()),
            _ => None,
        })
        .collect();
    for k in to_remove {
        hooks_obj.remove(&k);
    }
    if hooks_obj.is_empty() {
        root.remove("hooks");
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_creates_flat_entries_for_all_events() {
        let doc = json_merge_install(json!({}), "PIXTUOID_SOURCE=reasonix '/opt/pixtuoid-hook'");
        let hooks = doc.get("hooks").and_then(|v| v.as_object()).unwrap();
        for ev in REASONIX_EVENTS {
            let arr = hooks.get(*ev).and_then(|v| v.as_array()).unwrap();
            assert_eq!(arr.len(), 1, "event {ev}");
            let entry = &arr[0];
            // FLAT Reasonix shape: command directly on the entry — no nested
            // {hooks:[{type,command}]} group, which Reasonix would ignore
            // (empty `command` entries are skipped upstream).
            assert_eq!(
                entry["command"].as_str().unwrap(),
                "PIXTUOID_SOURCE=reasonix '/opt/pixtuoid-hook'"
            );
            assert!(entry[SENTINEL_KEY].as_bool().unwrap());
            assert_eq!(entry["timeout"].as_i64().unwrap(), 1000);
            assert!(
                entry.get("hooks").is_none() && entry.get("type").is_none(),
                "must not write CC-style nested groups"
            );
            // `match` omitted = every tool (upstream also special-cases "*";
            // omission is the simplest always-fires form).
            assert!(entry.get("match").is_none(), "must not write a match key");
        }
    }

    #[test]
    fn install_is_idempotent_and_replaces_across_paths() {
        let a = json_merge_install(json!({}), "PIXTUOID_SOURCE=reasonix '/opt/a/pixtuoid-hook'");
        let b = json_merge_install(a.clone(), "PIXTUOID_SOURCE=reasonix '/opt/a/pixtuoid-hook'");
        assert_eq!(a, b, "same command re-install is a no-op");
        let c = json_merge_install(a, "PIXTUOID_SOURCE=reasonix '/opt/b/pixtuoid-hook'");
        for ev in REASONIX_EVENTS {
            assert_eq!(
                c["hooks"][*ev].as_array().unwrap().len(),
                1,
                "event {ev} duplicated on path change"
            );
        }
    }

    #[test]
    fn install_preserves_user_entries() {
        let initial = json!({
            "hooks": {
                "PreToolUse": [ { "match": "bash", "command": "my-guard.sh" } ]
            },
            "other": "setting"
        });
        let merged = json_merge_install(initial, "/x");
        let arr = merged["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["command"], json!("my-guard.sh"));
        assert_eq!(merged["other"], json!("setting"));
    }

    #[test]
    fn uninstall_removes_only_managed_entries_and_empty_maps() {
        let installed = json_merge_install(
            json!({"hooks": {"PreToolUse": [ { "match": "bash", "command": "my-guard.sh" } ]}}),
            "/x",
        );
        let cleaned = json_merge_uninstall(installed);
        let arr = cleaned["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["command"], json!("my-guard.sh"));
        for ev in REASONIX_EVENTS.iter().filter(|e| **e != "PreToolUse") {
            assert!(
                cleaned["hooks"].get(*ev).is_none(),
                "event {ev} should be dropped once empty"
            );
        }
    }

    #[test]
    fn uninstall_all_managed_drops_hooks_map() {
        let installed = json_merge_install(json!({}), "/x");
        let cleaned = json_merge_uninstall(installed);
        assert!(cleaned.get("hooks").is_none(), "got {cleaned}");
    }

    #[test]
    fn merge_install_idempotent_reports_unchanged() {
        let first = merge_install("", "/x").unwrap();
        assert!(first.changed);
        let second = merge_install(&first.content, "/x").unwrap();
        assert!(!second.changed, "second install is a semantic no-op");
    }

    #[test]
    fn merge_uninstall_no_pixtuoid_hooks_reports_unchanged() {
        let user = r#"{ "hooks": { "Stop": [ { "command": "notify-send done" } ] } }"#;
        let out = merge_uninstall(user).unwrap();
        assert!(!out.changed, "no managed entries → semantic no-op");
    }

    #[test]
    fn merge_install_rejects_invalid_json() {
        // A malformed settings.json upstream silently disables ALL the user's
        // hooks — refusing to overwrite is the only safe behavior.
        assert!(merge_install("{not json", "/x").is_err());
    }

    #[test]
    fn install_coerces_non_object_hooks_and_non_array_events() {
        let doc = json_merge_install(json!({"hooks": "garbage"}), "/x");
        assert!(doc["hooks"].is_object());
        let doc = json_merge_install(json!({"hooks": {"Stop": 42}}), "/x");
        assert_eq!(doc["hooks"]["Stop"].as_array().unwrap().len(), 1);
    }

    // Unix POSIX-form pin (single-quoted env-prefix). Unix-only: on Windows
    // hook_command emits the bare form and this spaced path would be REJECTED.
    #[cfg(unix)]
    #[test]
    fn hook_command_stamps_source_and_quotes() {
        let cmd = hook_command(Path::new("/Users/Jane Doe/bin/pixtuoid-hook")).unwrap();
        assert_eq!(
            cmd,
            "PIXTUOID_SOURCE=reasonix '/Users/Jane Doe/bin/pixtuoid-hook'"
        );
    }

    // Windows: bare exec form `<path> --source reasonix` (mirrors codex; Reasonix
    // shells hooks via cmd.exe /c, hook.go:414). Pinned by check-windows + windows-test.
    #[test]
    #[cfg(windows)]
    fn hook_command_emits_bare_exec_form_with_source_flag_on_windows() {
        let cmd = hook_command(Path::new(r"C:\tools\pixtuoid-hook.exe")).unwrap();
        assert_eq!(cmd, r"C:\tools\pixtuoid-hook.exe --source reasonix");
    }

    // Windows: a path with a space or cmd metacharacter is rejected at install
    // (shared io::windows_bare_hook_command guard — see #195).
    #[test]
    #[cfg(windows)]
    fn hook_command_rejects_cmd_unsafe_path_on_windows() {
        assert!(hook_command(Path::new(r"C:\Program Files\pixtuoid-hook.exe")).is_err());
        let err = hook_command(Path::new(r"C:\Users\a&b\pixtuoid-hook.exe"))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("cmd.exe") && err.contains("ordinary characters"),
            "must explain the cmd-unsafe path + workaround: {err}"
        );
    }

    // detect_installed probes user_config_dir()/reasonix; on Windows that must be
    // %APPDATA% (Go's os.UserConfigDir), not ~/.config, or auto-detection misses.
    #[cfg(windows)]
    #[test]
    fn user_config_dir_uses_appdata_on_windows() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var_os("APPDATA");
        std::env::set_var("APPDATA", r"C:\Users\ada\AppData\Roaming");
        assert_eq!(
            user_config_dir(),
            PathBuf::from(r"C:\Users\ada\AppData\Roaming")
        );
        match saved {
            Some(v) => std::env::set_var("APPDATA", v),
            None => std::env::remove_var("APPDATA"),
        }
    }

    #[test]
    #[cfg(unix)]
    fn hook_command_errors_on_non_utf8_path() {
        use std::os::unix::ffi::OsStrExt;
        let bad = Path::new(std::ffi::OsStr::from_bytes(b"/x/\xff/pixtuoid-hook"));
        assert!(hook_command(bad).is_err());
    }

    // Internal-consistency guard (mirror of the CC/Codex ones): every hook
    // event we REGISTER with Reasonix must have a decoder arm, else it arrives
    // at the shared socket and `decode_hook_payload` bails — silently dropped.
    #[test]
    fn every_registered_reasonix_event_decodes() {
        use pixtuoid_core::source::decoder::decode_hook_payload;
        for ev in REASONIX_EVENTS {
            // Reasonix envelope: camelCase, `event` discriminator, cwd-only
            // identity, stamped by the shim.
            let payload = serde_json::json!({
                "event": ev,
                "cwd": "/repo",
                "_pixtuoid_source": "reasonix",
            });
            assert!(
                decode_hook_payload(payload).is_ok(),
                "registered Reasonix hook {ev:?} has no decoder arm — it would \
                 bail as unsupported. Add an arm in pixtuoid-core source/reasonix.rs."
            );
        }
    }
}
