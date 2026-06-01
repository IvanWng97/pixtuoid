use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};

use crate::install::target::MergeOutcome;

const SENTINEL_KEY: &str = "_pixtuoid";

/// Legacy sentinel keys from previous tool names. Entries tagged with any of
/// these are stripped on install/uninstall so a v0.3.x → v0.4.x upgrade does
/// not leave orphan hooks pointing at missing binaries.
const LEGACY_SENTINEL_KEYS: &[&str] = &["_ascii_agents"];

const EVENTS: &[&str] = &[
    "SessionStart",
    "PreToolUse",
    "PostToolUse",
    "Notification",
    "SessionEnd",
];

pub fn default_config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(format!("{home}/.claude/settings.json"))
}

/// Claude writes the bare name for portability (CC spawns hooks via PATH).
/// Ignores the resolved path entirely (existence is checked by the orchestrator).
pub fn hook_command(_resolved: &Path) -> Result<String> {
    Ok("pixtuoid-hook".to_string())
}

fn parse_or_empty(content: &str) -> Result<Value> {
    if content.trim().is_empty() {
        return Ok(json!({}));
    }
    // No file path here — the orchestrator wraps the error with the real path
    // (which may be a `--config` override, not the default settings.json).
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
    if entry.get(SENTINEL_KEY).and_then(|v| v.as_bool()) == Some(true) {
        return true;
    }
    LEGACY_SENTINEL_KEYS
        .iter()
        .any(|k| entry.get(*k).and_then(|v| v.as_bool()) == Some(true))
}

fn json_merge_install(doc: Value, hook_command: &str) -> Value {
    let mut root: Map<String, Value> = doc.as_object().cloned().unwrap_or_default();
    let hooks = root
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    // Coerce a non-object `hooks` to an empty object, then bind via `if let`
    // (always matches now) — avoids an `.expect()` in production code.
    if !hooks.is_object() {
        *hooks = Value::Object(Map::new());
    }
    if let Value::Object(hooks_obj) = hooks {
        for ev in EVENTS {
            let list = hooks_obj
                .entry((*ev).to_string())
                .or_insert_with(|| Value::Array(vec![]));
            if !list.is_array() {
                *list = Value::Array(vec![]);
            }
            if let Value::Array(arr) = list {
                arr.retain(|entry| !is_managed_entry(entry));
                arr.push(json!({
                    SENTINEL_KEY: true,
                    "matcher": ".*",
                    "hooks": [ { "type": "command", "command": hook_command } ]
                }));
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

    // Regression for the v0.3.x → v0.4.x upgrade path: legacy entries tagged
    // `_ascii_agents` must be stripped on install and uninstall. The previous
    // PR #40 dropped the dual-sentinel cleanup, leaving stale hooks that
    // point at a missing `ascii-agents-hook` binary.
    #[test]
    fn install_strips_legacy_ascii_agents_entries() {
        let initial = json!({
            "hooks": {
                "PreToolUse": [
                    { "_ascii_agents": true, "matcher": ".*", "hooks": [{"type":"command","command":"/old"}] },
                    { "matcher": "Write", "hooks": [{"type":"command","command":"/keep"}] }
                ]
            }
        });
        let merged = json_merge_install(initial, "/new");
        let arr = merged["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(
            arr.len(),
            2,
            "legacy stripped, user entry kept, pixtuoid added"
        );
        let commands: Vec<&str> = arr
            .iter()
            .map(|e| e["hooks"][0]["command"].as_str().unwrap())
            .collect();
        assert!(commands.contains(&"/keep"));
        assert!(commands.contains(&"/new"));
        assert!(!commands.contains(&"/old"));
    }

    #[test]
    fn uninstall_strips_legacy_ascii_agents_entries() {
        let initial = json!({
            "hooks": {
                "PreToolUse": [
                    { "_ascii_agents": true, "matcher": ".*", "hooks": [{"type":"command","command":"/old"}] }
                ]
            }
        });
        let cleaned = json_merge_uninstall(initial);
        assert!(
            cleaned.get("hooks").is_none(),
            "legacy entry should be removed and empty hooks map dropped: {cleaned}"
        );
    }

    #[test]
    fn uninstall_strips_legacy_keeps_user_entries() {
        let initial = json!({
            "hooks": {
                "PreToolUse": [
                    { "_ascii_agents": true, "matcher": ".*", "hooks": [{"type":"command","command":"/old"}] },
                    { "matcher": "Write", "hooks": [{"type":"command","command":"/keep"}] }
                ]
            }
        });
        let cleaned = json_merge_uninstall(initial);
        let arr = cleaned["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["hooks"][0]["command"], json!("/keep"));
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
            });
            assert!(
                decode_hook_payload(payload).is_ok(),
                "registered CC hook {ev:?} has no decoder arm — it would bail as \
                 unsupported. Add an arm in pixtuoid-core source/decoder.rs."
            );
        }
    }
}
