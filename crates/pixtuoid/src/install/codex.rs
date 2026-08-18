use std::path::{Path, PathBuf};

use anyhow::Result;
use toml::value::Table;

use crate::install::target::MergeOutcome;
use crate::install::SENTINEL_KEY;

const CODEX_EVENTS: &[&str] = &[
    "SessionStart",
    "PreToolUse",
    "PostToolUse",
    "UserPromptSubmit",
    "SubagentStart",
    "SubagentStop",
    "Stop",
    "PermissionRequest",
    // Fires at GRACEFUL teardown only → an immediate clean exit walk; abrupt
    // exits still ride the probe ladder + short-idle reap.
    "SessionEnd",
];

pub(crate) fn default_config_path() -> Result<PathBuf> {
    // The SAME codex_home() the watcher uses, so the installed-hook config and the
    // watched sessions root can't disagree. Infallibly `Ok` — the `Result` is the
    // shared `Target` signature, which home-anchored targets genuinely need.
    Ok(pixtuoid_core::source::codex::codex_home().join("config.toml"))
}

/// The Codex hook `command`. Codex runs it under a shell (`/bin/sh -lc` on Unix,
/// `cmd.exe /C` on Windows) and reads the plain `command` field on every OS, so
/// the OS-correct form is written here rather than a `commandWindows` override.
///
/// - **Unix**: env-prefix form `PIXTUOID_SOURCE=codex '<path>'`.
/// - **Windows**: BARE exec form `<path> --source codex`. Do NOT quote the path:
///   codex passes the string through `Command::arg`, whose Windows quoting escapes
///   an embedded `"` to `\"`, which `cmd.exe /C` then mangles — the path comes out
///   corrupted and the hook silently never fires. The env-prefix form is invalid
///   under cmd.exe, so the source rides as the shim's `--source` flag. A path with
///   a SPACE or cmd metacharacter (`& | < > ( ) ^ %`) is substituted by its DOS
///   8.3 SHORT name, and REJECTED only if 8.3 generation is off on the volume (#195).
pub(crate) fn hook_command(resolved: &Path, _explicit: bool) -> Result<String> {
    // `_explicit` is Claude's bare-name-vs-absolute switch — Codex always
    // embeds the absolute path, so the flag changes nothing here.
    let p = crate::install::merge::hook_path_str(resolved)?;
    crate::install::hook_cmd::shell_hook_command(p, "codex")
}

pub(crate) fn merge_install(content: &str, hook_cmd: &str) -> Result<MergeOutcome> {
    crate::install::merge::toml_merge_outcome(content, |doc| toml_merge_install(doc, hook_cmd))
}

pub(crate) fn merge_uninstall(content: &str) -> Result<MergeOutcome> {
    crate::install::merge::toml_merge_outcome(content, toml_merge_uninstall)
}

fn handler_is_managed(h: &toml::Value) -> bool {
    // Sentinel-only, no basename fallback: no released version ever wrote a
    // sentinel-less entry, so a basename match could only ever hit a USER
    // hand-written entry pointing at the shim, which uninstall must not touch.
    h.get(SENTINEL_KEY).and_then(|v| v.as_bool()) == Some(true)
}

fn prune_managed_handlers(group: &mut toml::Value) {
    if let Some(hooks) = group.get_mut("hooks").and_then(|h| h.as_array_mut()) {
        hooks.retain(|h| !handler_is_managed(h));
    }
}

/// Install-schema verification: every `CODEX_EVENTS` group still holds a
/// sentinel-tagged handler, and the shim command is read back for the on-disk check.
pub(crate) fn verify_schema(content: &str) -> crate::install::verify::SchemaParse {
    use crate::install::verify::{assemble, shell_shim_ref, SchemaParse, ShimRef};
    let Ok(doc) = toml::from_str::<toml::Value>(content) else {
        return SchemaParse::broken("config.toml no longer parses as TOML");
    };
    let hooks = doc.get("hooks").and_then(|h| h.as_table());
    let mut missing = Vec::new();
    let mut any = false;
    let mut shim = ShimRef::Unknown;
    for ev in CODEX_EVENTS {
        let handler = hooks
            .and_then(|h| h.get(*ev))
            .and_then(|a| a.as_array())
            .and_then(|groups| {
                groups.iter().find_map(|g| {
                    g.get("hooks")
                        .and_then(|hs| hs.as_array())
                        .and_then(|hs| hs.iter().find(|h| handler_is_managed(h)))
                })
            });
        match handler {
            Some(h) => {
                any = true;
                if shim == ShimRef::Unknown {
                    shim = h
                        .get("command")
                        .and_then(|c| c.as_str())
                        .map(shell_shim_ref)
                        .unwrap_or(ShimRef::Unknown);
                }
            }
            None => missing.push(*ev),
        }
    }
    assemble(&missing, any, shim, vec![])
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

    // No `matcher`: an omitted matcher means "match all" in Codex. Do NOT write
    // `matcher = "*"` — codex rejects a bare `*` as an invalid regex and silently
    // drops the ENTIRE group, so SessionStart/PreToolUse never fire.
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

    #[test]
    fn install_same_command_reports_unchanged() {
        let first = merge_install("", "/opt/a/pixtuoid-hook").unwrap();
        let second = merge_install(&first.content, "/opt/a/pixtuoid-hook").unwrap();
        assert!(!second.changed, "identical re-install is a no-op");
    }

    #[test]
    fn uninstall_no_pixtuoid_hooks_reports_unchanged() {
        let cfg = "model = \"o1\"\n\n[[hooks.PreToolUse]]\nmatcher = \"*\"\n\n[[hooks.PreToolUse.hooks]]\ntype = \"command\"\ncommand = \"/usr/bin/mytool\"\n";
        let out = merge_uninstall(cfg).unwrap();
        assert!(!out.changed, "no managed entries → semantic no-op");
    }

    #[test]
    fn uninstall_keeps_user_handler_in_mixed_group() {
        let installed = merge_install("", "/x/pixtuoid-hook").unwrap();
        let mut v = parse(&installed.content);
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
    fn uninstall_leaves_a_hand_written_shim_entry_alone() {
        let cfg = r#"
[[hooks.PreToolUse]]
matcher = "*"
[[hooks.PreToolUse.hooks]]
type = "command"
command = "/hand/written/pixtuoid-hook"
"#;
        let cleaned = merge_uninstall(cfg).unwrap();
        let v = parse(&cleaned.content);
        assert!(
            v.get("hooks").is_some(),
            "hand-written entry survives uninstall: {}",
            cleaned.content
        );
        assert!(!cleaned.changed, "nothing managed to remove");
    }

    #[test]
    #[cfg(windows)]
    fn hook_command_emits_bare_exec_form_with_source_flag_on_windows() {
        let cmd = hook_command(std::path::Path::new(r"C:\tools\pixtuoid-hook.exe"), false).unwrap();
        assert_eq!(cmd, r"C:\tools\pixtuoid-hook.exe --source codex");
    }

    // These paths don't exist on the runner, so GetShortPathNameW fails and the
    // 8.3 substitution falls through to the reject arm (the 8.3-success path is
    // covered in hook_cmd/windows.rs).
    #[test]
    #[cfg(windows)]
    fn hook_command_rejects_cmd_unsafe_path_on_windows() {
        assert!(hook_command(
            std::path::Path::new(r"C:\Program Files\pixtuoid-hook.exe"),
            false
        )
        .is_err());
        let err = hook_command(
            std::path::Path::new(r"C:\Users\a&b\pixtuoid-hook.exe"),
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("cmd.exe") && err.contains("ordinary characters"),
            "must explain the cmd-unsafe path + workaround: {err}"
        );
        for bad in [
            r"C:\p|x\h.exe",
            r"C:\p>x\h.exe",
            r"C:\p(x)\h.exe",
            r"C:\p%x\h.exe",
        ] {
            assert!(
                hook_command(std::path::Path::new(bad), false).is_err(),
                "must reject cmd-unsafe path {bad}"
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn hook_command_errors_on_non_utf8_path() {
        use std::os::unix::ffi::OsStrExt;
        let bad = std::path::Path::new(std::ffi::OsStr::from_bytes(b"/x/\xff/pixtuoid-hook"));
        assert!(hook_command(bad, false).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn hook_command_prefixes_source_for_valid_path() {
        let cmd = hook_command(std::path::Path::new("/opt/bin/pixtuoid-hook"), false).unwrap();
        assert_eq!(cmd, "PIXTUOID_SOURCE=codex '/opt/bin/pixtuoid-hook'");
    }

    #[cfg(unix)]
    #[test]
    fn hook_command_quotes_path_with_spaces() {
        let cmd = hook_command(
            std::path::Path::new("/Users/Jane Doe/bin/pixtuoid-hook"),
            false,
        )
        .unwrap();
        assert_eq!(
            cmd,
            "PIXTUOID_SOURCE=codex '/Users/Jane Doe/bin/pixtuoid-hook'"
        );
    }

    #[test]
    fn install_coerces_non_table_hooks_to_table() {
        let out = merge_install("hooks = 5", "/x/pixtuoid-hook").unwrap();
        let v = parse(&out.content);
        let hooks = v["hooks"].as_table().unwrap();
        for ev in CODEX_EVENTS {
            assert_eq!(
                hooks.get(*ev).and_then(|e| e.as_array()).unwrap().len(),
                1,
                "event {ev} populated after coercion"
            );
        }
    }

    #[test]
    fn install_coerces_non_array_event_to_array() {
        let out = merge_install("[hooks]\nPreToolUse = 5", "/x/pixtuoid-hook").unwrap();
        let v = parse(&out.content);
        let arr = v["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert!(arr[0]["hooks"][0]["_pixtuoid"].as_bool().unwrap());
    }

    #[test]
    fn uninstall_non_table_doc_returns_unchanged() {
        let input = toml::Value::Integer(3);
        assert_eq!(toml_merge_uninstall(input.clone()), input);
    }

    #[test]
    fn uninstall_doc_without_hooks_returns_unchanged() {
        let out = merge_uninstall("model = \"o1\"\n").unwrap();
        assert!(!out.changed, "no [hooks] → nothing to remove");
    }

    #[test]
    fn default_config_path_honors_codex_home_env() {
        // std::env is process-global; serialize against other env-mutating tests.
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var_os("CODEX_HOME");
        let fallback_suffix = PathBuf::from(".codex").join("config.toml");

        std::env::remove_var("CODEX_HOME");
        assert!(
            default_config_path().unwrap().ends_with(&fallback_suffix),
            "unset CODEX_HOME must end with .codex/config.toml, got {:?}",
            default_config_path().unwrap()
        );

        let custom = std::env::temp_dir().join("pixtuoid-codex-home-cfg-test");
        std::fs::create_dir_all(&custom).unwrap();
        std::env::set_var("CODEX_HOME", &custom);
        assert_eq!(default_config_path().unwrap(), custom.join("config.toml"));

        // A non-existent dir falls back, matching upstream codex's own gate.
        let missing = std::env::temp_dir().join("pixtuoid-codex-home-cfg-missing");
        let _ = std::fs::remove_dir_all(&missing);
        std::env::set_var("CODEX_HOME", &missing);
        assert!(
            default_config_path().unwrap().ends_with(&fallback_suffix),
            "non-existent CODEX_HOME must fall back to .codex/config.toml"
        );

        std::env::set_var("CODEX_HOME", "");
        assert!(default_config_path().unwrap().ends_with(&fallback_suffix));

        match saved {
            Some(v) => std::env::set_var("CODEX_HOME", v),
            None => std::env::remove_var("CODEX_HOME"),
        }
        let _ = std::fs::remove_dir_all(&custom);
    }

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

    #[test]
    fn codex_events_pins_the_exact_registered_set() {
        // Deleting a registered event ships GREEN — cargo-mutants does not mutate
        // `&[&str]` initializers and nothing else asserts the SET, which is how
        // both of #929's headline registration fixes could be silently removed.
        // Update this pin deliberately when the roster changes.
        use std::collections::BTreeSet;
        assert_eq!(
            CODEX_EVENTS.iter().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "SessionStart",
                "PreToolUse",
                "PostToolUse",
                "UserPromptSubmit",
                "SubagentStart",
                "SubagentStop",
                "Stop",
                "PermissionRequest",
                "SessionEnd",
            ]),
            "CODEX_EVENTS membership changed — a registered event that vanishes is a \
             shipping bug no other test can see."
        );
    }
}
