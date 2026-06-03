use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use toml::value::Table;

use crate::install::io;
use crate::install::target::MergeOutcome;

const SENTINEL_KEY: &str = "_pixtuoid";

const CODEX_EVENTS: &[&str] = &[
    "SessionStart",
    "PreToolUse",
    "PostToolUse",
    "UserPromptSubmit",
    "SubagentStart",
    "SubagentStop",
    "Stop",
    "PermissionRequest",
];

pub fn default_config_path() -> PathBuf {
    io::home_relative(".codex/config.toml")
}

/// POSIX single-quote a string so a shell treats it as one literal token —
/// embedded single quotes become `'\''`. Codex runs the `command` under a
/// shell, so an unquoted path containing spaces would split into multiple args
/// and the hook would never be found.
fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Codex runs the `command` string under a shell; we write an ABSOLUTE path
/// (robust regardless of PATH), single-quoted (robust to spaces), prefixed with
/// PIXTUOID_SOURCE so the shim can stamp the source. Err on non-UTF-8 (prevents
/// the to_string_lossy dead-hook).
pub fn hook_command(resolved: &Path) -> Result<String> {
    let p = resolved
        .to_str()
        .ok_or_else(|| anyhow!("pixtuoid-hook path is non-UTF-8: {}", resolved.display()))?;
    Ok(format!("PIXTUOID_SOURCE=codex {}", shell_single_quote(p)))
}

fn parse_or_empty(content: &str) -> Result<toml::Value> {
    if content.trim().is_empty() {
        return Ok(toml::Value::Table(Table::new()));
    }
    // No file path here — the orchestrator wraps the error with the real path
    // (which may be a `--config` override, not the default config.toml).
    toml::from_str(content).context("not valid TOML — refusing to overwrite")
}

pub fn merge_install(content: &str, hook_cmd: &str) -> Result<MergeOutcome> {
    let doc = parse_or_empty(content)?;
    let merged = toml_merge_install(doc.clone(), hook_cmd);
    let changed = merged != doc;
    Ok(MergeOutcome {
        content: toml::to_string_pretty(&merged)?,
        changed,
    })
}

pub fn merge_uninstall(content: &str) -> Result<MergeOutcome> {
    let doc = parse_or_empty(content)?;
    let cleaned = toml_merge_uninstall(doc.clone());
    let changed = cleaned != doc;
    Ok(MergeOutcome {
        content: toml::to_string_pretty(&cleaned)?,
        changed,
    })
}

fn command_basename_is_hook(command: &str) -> bool {
    // The command string may be "PIXTUOID_SOURCE=codex /path/pixtuoid-hook";
    // take the last whitespace-separated token, then its file_name.
    let token = command.split_whitespace().last().unwrap_or(command);
    Path::new(token).file_name().and_then(|s| s.to_str()) == Some("pixtuoid-hook")
}

fn handler_is_managed(h: &toml::Value) -> bool {
    if h.get(SENTINEL_KEY).and_then(|v| v.as_bool()) == Some(true) {
        return true;
    }
    // Legacy fallback for pre-sentinel (#59) entries.
    h.get("type").and_then(|v| v.as_str()) == Some("command")
        && h.get("command")
            .and_then(|v| v.as_str())
            .is_some_and(command_basename_is_hook)
}

fn prune_managed_handlers(group: &mut toml::Value) {
    if let Some(hooks) = group.get_mut("hooks").and_then(|h| h.as_array_mut()) {
        hooks.retain(|h| !handler_is_managed(h));
    }
}

fn group_has_no_hooks(group: &toml::Value) -> bool {
    group
        .get("hooks")
        .and_then(|h| h.as_array())
        .is_some_and(|h| h.is_empty())
}

fn managed_group(hook_command: &str) -> toml::Value {
    let mut handler = Table::new();
    handler.insert("type".into(), toml::Value::String("command".into()));
    handler.insert("command".into(), toml::Value::String(hook_command.into()));
    handler.insert("timeout".into(), toml::Value::Integer(5));
    handler.insert(
        "statusMessage".into(),
        toml::Value::String("pixtuoid visualizer".into()),
    );
    handler.insert(SENTINEL_KEY.into(), toml::Value::Boolean(true));

    // No `matcher`: an omitted matcher means "match all" in Codex. We must NOT
    // write `matcher = "*"` — Codex (verified on 0.135) rejects a bare `*` as an
    // invalid regex and silently drops the ENTIRE group, so SessionStart/
    // PreToolUse never fire. Matcher-less groups (the only ones that fired live)
    // match every occurrence, which is exactly what a visualizer wants.
    let mut group = Table::new();
    group.insert(
        "hooks".into(),
        toml::Value::Array(vec![toml::Value::Table(handler)]),
    );
    toml::Value::Table(group)
}

fn toml_merge_install(doc: toml::Value, hook_command: &str) -> toml::Value {
    let mut root = doc.as_table().cloned().unwrap_or_default();
    let hooks = root
        .entry("hooks".to_string())
        .or_insert_with(|| toml::Value::Table(Table::new()));
    if !hooks.is_table() {
        *hooks = toml::Value::Table(Table::new());
    }
    if let Some(hooks) = hooks.as_table_mut() {
        for ev in CODEX_EVENTS {
            let entry = hooks
                .entry((*ev).to_string())
                .or_insert_with(|| toml::Value::Array(vec![]));
            if !entry.is_array() {
                *entry = toml::Value::Array(vec![]);
            }
            if let Some(arr) = entry.as_array_mut() {
                for group in arr.iter_mut() {
                    prune_managed_handlers(group);
                }
                arr.retain(|group| !group_has_no_hooks(group));
                arr.push(managed_group(hook_command));
            }
        }
    }
    toml::Value::Table(root)
}

fn toml_merge_uninstall(mut doc: toml::Value) -> toml::Value {
    let Some(root) = doc.as_table_mut() else {
        return doc;
    };
    let Some(toml::Value::Table(hooks)) = root.get_mut("hooks") else {
        return doc;
    };
    for (_ev, list) in hooks.iter_mut() {
        if let Some(arr) = list.as_array_mut() {
            for group in arr.iter_mut() {
                prune_managed_handlers(group);
            }
            arr.retain(|group| !group_has_no_hooks(group));
        }
    }
    let empty: Vec<String> = hooks
        .iter()
        .filter_map(|(k, v)| match v.as_array() {
            Some(a) if a.is_empty() => Some(k.clone()),
            _ => None,
        })
        .collect();
    for k in empty {
        hooks.remove(&k);
    }
    if hooks.is_empty() {
        root.remove("hooks");
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> toml::Value {
        toml::from_str(s).unwrap()
    }

    #[test]
    fn install_creates_groups_for_all_events_with_sentinel() {
        let out = merge_install("", "PIXTUOID_SOURCE=codex /opt/bin/pixtuoid-hook").unwrap();
        assert!(out.changed);
        let v = parse(&out.content);
        for ev in CODEX_EVENTS {
            let arr = v["hooks"][*ev].as_array().unwrap();
            assert_eq!(arr.len(), 1, "event {ev}");
            let handler = &arr[0]["hooks"][0];
            assert_eq!(
                handler["command"].as_str().unwrap(),
                "PIXTUOID_SOURCE=codex /opt/bin/pixtuoid-hook"
            );
            assert_eq!(handler["timeout"].as_integer().unwrap(), 5);
            assert_eq!(
                handler["statusMessage"].as_str().unwrap(),
                "pixtuoid visualizer"
            );
            assert!(handler["_pixtuoid"].as_bool().unwrap());
        }
    }

    #[test]
    fn install_does_not_write_features_hooks() {
        let out = merge_install("", "/x").unwrap();
        let v = parse(&out.content);
        assert!(
            v.get("features").is_none(),
            "must not write [features] hooks = true"
        );
    }

    #[test]
    fn install_writes_no_matcher() {
        // Codex 0.135 fires matcher-bearing events inconsistently and `matcher
        // = "*"` is a dubious regex; an omitted matcher means "match all". Verify
        // no group carries a matcher.
        let out = merge_install("", "/x/pixtuoid-hook").unwrap();
        let v = parse(&out.content);
        let hooks = v["hooks"].as_table().unwrap();
        for (ev, arr) in hooks {
            for group in arr.as_array().unwrap() {
                assert!(
                    group.get("matcher").is_none(),
                    "event {ev} group must not carry a matcher"
                );
            }
        }
    }

    #[test]
    fn install_is_idempotent_across_different_paths() {
        // Sentinel (not basename/path) drives replacement → re-install with a
        // different resolved path replaces, never duplicates.
        let a = merge_install("", "/opt/a/pixtuoid-hook").unwrap();
        let b = merge_install(&a.content, "/opt/b/pixtuoid-hook").unwrap();
        let v = parse(&b.content);
        for ev in CODEX_EVENTS {
            assert_eq!(
                v["hooks"][*ev].as_array().unwrap().len(),
                1,
                "event {ev} duplicated"
            );
        }
    }

    // Re-install with the SAME command is a semantic no-op (changed=false) →
    // orchestrator won't rewrite the file. Guards the F1/F3 byte-vs-semantic fix.
    #[test]
    fn install_same_command_reports_unchanged() {
        let first = merge_install("", "/opt/a/pixtuoid-hook").unwrap();
        let second = merge_install(&first.content, "/opt/a/pixtuoid-hook").unwrap();
        assert!(!second.changed, "identical re-install is a no-op");
    }

    // Uninstall on a config with user hooks but NO pixtuoid entries must be a
    // no-op so the orchestrator never rewrites it or deletes the backup.
    #[test]
    fn uninstall_no_pixtuoid_hooks_reports_unchanged() {
        let cfg = "model = \"o1\"\n\n[[hooks.PreToolUse]]\nmatcher = \"*\"\n\n[[hooks.PreToolUse.hooks]]\ntype = \"command\"\ncommand = \"/usr/bin/mytool\"\n";
        let out = merge_uninstall(cfg).unwrap();
        assert!(!out.changed, "no managed entries → semantic no-op");
    }

    #[test]
    fn uninstall_keeps_user_handler_in_mixed_group() {
        // A group with one managed + one user handler: uninstall strips only ours.
        let installed = merge_install("", "/x/pixtuoid-hook").unwrap();
        let mut v = parse(&installed.content);
        // inject a user handler into the PreToolUse group
        let group = &mut v["hooks"]["PreToolUse"].as_array_mut().unwrap()[0];
        group["hooks"]
            .as_array_mut()
            .unwrap()
            .push(toml::Value::Table({
                let mut t = toml::value::Table::new();
                t.insert("type".into(), "command".into());
                t.insert("command".into(), "/usr/bin/mytool".into());
                t
            }));
        let cleaned = merge_uninstall(&toml::to_string_pretty(&v).unwrap()).unwrap();
        assert!(cleaned.changed, "the managed handler was removed");
        let cv = parse(&cleaned.content);
        let arr = cv["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1, "group kept (user handler remains)");
        let hooks = arr[0]["hooks"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["command"].as_str().unwrap(), "/usr/bin/mytool");
    }

    #[test]
    fn uninstall_removes_empty_groups_and_events() {
        let installed = merge_install("", "/x/pixtuoid-hook").unwrap();
        let cleaned = merge_uninstall(&installed.content).unwrap();
        let v = parse(&cleaned.content);
        assert!(
            v.get("hooks").is_none(),
            "all managed → hooks table dropped: {}",
            cleaned.content
        );
    }

    #[test]
    fn uninstall_legacy_basename_fallback() {
        // A pre-sentinel #59 entry (no _pixtuoid, command basename pixtuoid-hook) is removed.
        let cfg = r#"
[[hooks.PreToolUse]]
matcher = "*"
[[hooks.PreToolUse.hooks]]
type = "command"
command = "/old/pixtuoid-hook"
"#;
        let cleaned = merge_uninstall(cfg).unwrap();
        let v = parse(&cleaned.content);
        assert!(
            v.get("hooks").is_none(),
            "legacy basename entry removed: {}",
            cleaned.content
        );
    }

    #[test]
    #[cfg(unix)]
    fn hook_command_errors_on_non_utf8_path() {
        use std::os::unix::ffi::OsStrExt;
        let bad = std::path::Path::new(std::ffi::OsStr::from_bytes(b"/x/\xff/pixtuoid-hook"));
        assert!(hook_command(bad).is_err());
    }

    #[test]
    fn hook_command_prefixes_source_for_valid_path() {
        let cmd = hook_command(std::path::Path::new("/opt/bin/pixtuoid-hook")).unwrap();
        assert_eq!(cmd, "PIXTUOID_SOURCE=codex '/opt/bin/pixtuoid-hook'");
    }

    // F9: a hook path containing spaces must be single-quoted so the shell does
    // not split it into multiple args (which would silently never find the hook).
    #[test]
    fn hook_command_quotes_path_with_spaces() {
        let cmd = hook_command(std::path::Path::new("/Users/Jane Doe/bin/pixtuoid-hook")).unwrap();
        assert_eq!(
            cmd,
            "PIXTUOID_SOURCE=codex '/Users/Jane Doe/bin/pixtuoid-hook'"
        );
    }

    // Internal-consistency guard: every hook event we REGISTER with Codex must
    // have a decoder arm — otherwise it arrives at the shared socket and
    // `decode_hook_payload` bails ("unsupported hook_event_name"), silently
    // dropping it. This is exactly the class the SubagentStop bug fell into
    // (registered but not decoded). The external drift-watch covers upstream
    // renames; this covers our own registered-vs-decoded drift.
    #[test]
    fn every_registered_codex_event_decodes() {
        use pixtuoid_core::source::decoder::decode_hook_payload;
        for ev in CODEX_EVENTS {
            // A complete-enough payload: `agent_id` satisfies SubagentStart/Stop;
            // the rest is ignored by events that don't need it.
            let payload = serde_json::json!({
                "hook_event_name": ev,
                "session_id": "sess",
                "agent_id": "child",
                "cwd": "/repo",
                "_pixtuoid_source": "codex",
            });
            assert!(
                decode_hook_payload(payload).is_ok(),
                "registered Codex hook {ev:?} has no decoder arm — it would bail \
                 as unsupported. Add an arm in pixtuoid-core source/decoder.rs."
            );
        }
    }
}
