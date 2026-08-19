//! Reasonix hook install target.
//!
//! Writes the GLOBAL `<reasonix-home>/settings.json`: project-scope
//! (`<repo>/.reasonix/settings.json`) hooks only load after the user runs
//! `/hooks trust`, so a project-scope install would silently never fire. The
//! schema is Reasonix's own FLAT shape — per-event arrays of
//! `{match, command, description, timeout, cwd}` entries, NOT Claude's nested
//! `{matcher, hooks: [{type, command}]}` groups.
//!
//! - `match` is OMITTED: empty = every tool. Any other value is an ANCHORED
//!   regex, and a malformed one never fires.
//! - `timeout` is in MILLISECONDS, and on the gating PreToolUse a TIMEOUT BLOCKS
//!   the user's tool call.
//! - `_pixtuoid` is the managed-entry sentinel; Go's `json.Unmarshal` ignores
//!   unknown fields, so Reasonix never sees it.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::install::io;
use crate::install::merge;
use crate::install::target::MergeOutcome;
use crate::install::SENTINEL_KEY;

/// Events we register == events we decode, enforced by
/// `every_registered_reasonix_event_decodes` below. PostToolUseFailure /
/// StopFailure are deliberately ABSENT: the runner re-fires failures to NATIVE
/// hooks registered under PostToolUse / Stop with the event re-labeled, and ours
/// are native-format — registering both spellings would double-fire every failed
/// tool/turn for the same decoded ActivityEnd.
pub(crate) const REASONIX_EVENTS: &[&str] = &[
    "SessionStart",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "UserPromptSubmit",
    "Stop",
    "Notification",
    "SessionEnd",
];

/// The GLOBAL `settings.json` Reasonix actually reads. Reasonix's home is
/// platform-ASYMMETRIC: `REASONIX_HOME` (verbatim) wins; else macOS/Linux =
/// `~/.reasonix`, but **Windows = `%APPDATA%\reasonix`**, NOT
/// `%USERPROFILE%\.reasonix`. Writing pixtuoid's generic USERPROFILE-first path
/// on Windows lands the hooks where Reasonix never reads → installed, no sprite.
pub(crate) fn default_config_path() -> Result<PathBuf> {
    reasonix_home()
        .map(|h| h.join("settings.json"))
        .ok_or_else(|| {
            // Erroring mirrors the sibling home-anchored targets instead of
            // silently writing a CWD-relative config Reasonix never reads.
            anyhow!(
                "cannot resolve Reasonix's home (REASONIX_HOME and the platform \
                 home/config dir unset); pass --config <path>"
            )
        })
}

/// `REASONIX_HOME` (upstream trims but does NOT `~`-expand it, hence `home: None`)
/// → Windows `%APPDATA%\reasonix` → else `<home>/.reasonix`.
fn reasonix_home() -> Option<PathBuf> {
    resolve_reasonix_home(
        io::nonempty_env("REASONIX_HOME").map(|v| io::expand_tilde(&v, None)),
        cfg!(windows),
        user_config_dir(),
        pixtuoid_core::platform::user_home_opt(),
    )
}

/// Pure core for [`reasonix_home`] — everything is injected so BOTH platform arms
/// unit-test on any host. `None` only when neither `%APPDATA%` nor a home
/// resolves; installing then would write a CWD-relative file Reasonix never reads.
fn resolve_reasonix_home(
    reasonix_home_env: Option<PathBuf>,
    windows: bool,
    windows_config_dir: Option<PathBuf>,
    unix_home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(h) = reasonix_home_env {
        return Some(h);
    }
    if windows {
        return windows_config_dir.map(|d| d.join("reasonix"));
    }
    unix_home.map(|h| h.join(".reasonix"))
}

/// Presence probe for auto-detection. The default file-exists check on
/// `default_config_path` would NEVER fire: Reasonix itself never creates
/// `settings.json`, it is purely user-authored. What a real install does create
/// is the Reasonix home dir — and hook/trust users additionally have a
/// `~/.reasonix` even on Windows, so probe both.
pub(crate) fn detect_installed() -> bool {
    reasonix_home().is_some_and(|d| d.exists()) || io::home_relative(".reasonix").exists()
}

/// Rust mapping of Go's `os.UserConfigDir()`: macOS `$HOME/Library/Application
/// Support`, **Windows `%APPDATA%`** (Roaming — without this arm `detect_installed`
/// probes `~/.config/reasonix` on Windows, which Reasonix never creates, so
/// auto-detection would always miss), else `$XDG_CONFIG_HOME` or `~/.config`.
/// `None` when the selected arm needs a home and none resolves, rather than the
/// CWD-relative path `home_relative("")` would fabricate.
fn user_config_dir() -> Option<PathBuf> {
    user_config_dir_checked(
        std::env::consts::OS,
        pixtuoid_core::platform::path_env("APPDATA"),
        pixtuoid_core::platform::path_env("XDG_CONFIG_HOME"),
        pixtuoid_core::platform::user_home_opt(),
    )
}

/// Pure core for [`user_config_dir`]: `None` when the arm the OS/env select would
/// fall back to a home join and no home resolves. `io::nonempty` mirrors the core
/// fn's own empty-as-unset filter so the two can't disagree on when that fires.
fn user_config_dir_checked(
    os: &str,
    appdata: Option<PathBuf>,
    xdg: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    // Blank values were already filtered at the read (`platform::path_env`).
    let env_decides = match os {
        "windows" => appdata.is_some(),
        "macos" => false,
        _ => xdg.is_some(),
    };
    if !env_decides && home.is_none() {
        return None;
    }
    // The home base is only read by the fallback arms, which the gate above
    // guarantees have a real home when reached.
    let home_base = home.unwrap_or_default();
    Some(pixtuoid_core::platform::resolve_user_config_dir(
        os, appdata, xdg, &home_base,
    ))
}

/// Reasonix runs the `command` string under a shell — `sh -c` on Unix, `cmd.exe
/// /c` on Windows:
/// - **Unix**: env-prefix `PIXTUOID_SOURCE=reasonix '<abs-path>'` (single-quoted).
/// - **Windows**: BARE `<abs-path> --source reasonix` — cmd.exe can't express the
///   env-prefix, so the source rides as the shim's `--source` flag, and a
///   space/metacharacter path uses its 8.3 short name because a quoted path
///   can't survive cmd /C.
///
/// Err on non-UTF-8 (prevents the to_string_lossy dead-hook).
pub(crate) fn hook_command(resolved: &Path, _explicit: bool) -> Result<String> {
    // `_explicit` is Claude's bare-name-vs-absolute switch — Reasonix always
    // embeds the absolute path, so the flag changes nothing here.
    let p = merge::hook_path_str(resolved)?;
    crate::install::hook_cmd::shell_hook_command(p, "reasonix")
}

pub(crate) fn merge_install(content: &str, hook_cmd: &str) -> Result<MergeOutcome> {
    merge::flat_json_merge_outcome_install(content, "settings", |doc| {
        merge::flat_json_merge_install(doc, REASONIX_EVENTS, SENTINEL_KEY, managed_entry, hook_cmd)
    })
}

pub(crate) fn merge_uninstall(content: &str) -> Result<MergeOutcome> {
    merge::flat_json_merge_outcome_uninstall(content, |doc| {
        merge::flat_json_merge_uninstall(doc, SENTINEL_KEY)
    })
}

fn managed_entry(hook_command: &str) -> Value {
    json!({
        SENTINEL_KEY: true,
        "command": hook_command,
        "timeout": 1000,
        "description": "pixtuoid visualizer"
    })
}

/// Install-schema verification: `hooks.<event>` arrays of `{_pixtuoid, command}`.
pub(crate) fn verify_schema(content: &str) -> crate::install::verify::SchemaParse {
    crate::install::verify::flat_json_verify(content, REASONIX_EVENTS, SENTINEL_KEY)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json_merge_install(doc: Value, hook_command: &str) -> Value {
        merge::flat_json_merge_install(
            doc,
            REASONIX_EVENTS,
            SENTINEL_KEY,
            managed_entry,
            hook_command,
        )
    }

    fn json_merge_uninstall(doc: Value) -> Value {
        merge::flat_json_merge_uninstall(doc, SENTINEL_KEY)
    }

    #[test]
    fn reasonix_home_is_appdata_on_windows_but_dot_reasonix_elsewhere() {
        let appdata = PathBuf::from(r"C:\Users\me\AppData\Roaming");
        assert_eq!(
            resolve_reasonix_home(
                None,
                true,
                Some(appdata.clone()),
                Some(r"C:\Users\me".into())
            ),
            Some(appdata.join("reasonix"))
        );
        assert_eq!(
            resolve_reasonix_home(None, false, Some(appdata), Some("/home/u".into())),
            Some(PathBuf::from("/home/u").join(".reasonix"))
        );
        assert_eq!(
            resolve_reasonix_home(None, false, Some(PathBuf::from("/ignored")), None),
            None
        );
        assert_eq!(resolve_reasonix_home(None, true, None, None), None);
    }

    #[test]
    fn user_config_dir_checked_refuses_a_homeless_fallback_arm() {
        assert_eq!(
            user_config_dir_checked("windows", Some(r"C:\AppData".into()), None, None),
            Some(PathBuf::from(r"C:\AppData"))
        );
        assert_eq!(
            user_config_dir_checked("linux", None, Some("/xdg".into()), None),
            Some(PathBuf::from("/xdg"))
        );
        // A blank env value counts as unset — filtered at the READ, so this core
        // only ever sees real paths.
        assert_eq!(user_config_dir_checked("windows", None, None, None), None);
        assert_eq!(user_config_dir_checked("macos", None, None, None), None);
        assert_eq!(user_config_dir_checked("linux", None, None, None), None);
        assert_eq!(
            user_config_dir_checked("windows", None, None, Some(r"C:\Users\me".into())),
            Some(PathBuf::from(r"C:\Users\me").join("AppData/Roaming"))
        );
    }

    #[test]
    fn reasonix_home_env_override_wins_verbatim_on_both_platforms() {
        for windows in [true, false] {
            assert_eq!(
                resolve_reasonix_home(
                    Some("/custom/rx".into()),
                    windows,
                    Some(PathBuf::from(r"C:\AppData")),
                    Some("/home/u".into()),
                ),
                Some(PathBuf::from("/custom/rx"))
            );
        }
    }

    #[test]
    fn install_creates_flat_entries_for_all_events() {
        let doc = json_merge_install(json!({}), "PIXTUOID_SOURCE=reasonix '/opt/pixtuoid-hook'");
        let hooks = doc.get("hooks").and_then(|v| v.as_object()).unwrap();
        for ev in REASONIX_EVENTS {
            let arr = hooks.get(*ev).and_then(|v| v.as_array()).unwrap();
            assert_eq!(arr.len(), 1, "event {ev}");
            let entry = &arr[0];
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
    fn merge_install_rejects_valid_json_that_is_not_an_object() {
        assert!(merge_install("[1, 2, 3]", "/x").is_err());
        assert!(merge_install("42", "/x").is_err());
    }

    #[test]
    fn merge_install_rejects_invalid_json() {
        assert!(merge_install("{not json", "/x").is_err());
    }

    #[test]
    fn install_coerces_non_object_hooks_and_non_array_events() {
        let doc = json_merge_install(json!({"hooks": "garbage"}), "/x");
        assert!(doc["hooks"].is_object());
        let doc = json_merge_install(json!({"hooks": {"Stop": 42}}), "/x");
        assert_eq!(doc["hooks"]["Stop"].as_array().unwrap().len(), 1);
    }

    // Unix-only: on Windows `hook_command` emits the bare form and this spaced
    // path would be REJECTED.
    #[cfg(unix)]
    #[test]
    fn hook_command_stamps_source_and_quotes() {
        let cmd = hook_command(Path::new("/Users/Jane Doe/bin/pixtuoid-hook"), false).unwrap();
        assert_eq!(
            cmd,
            "PIXTUOID_SOURCE=reasonix '/Users/Jane Doe/bin/pixtuoid-hook'"
        );
    }

    #[test]
    #[cfg(windows)]
    fn hook_command_emits_bare_exec_form_with_source_flag_on_windows() {
        let cmd = hook_command(Path::new(r"C:\tools\pixtuoid-hook.exe"), false).unwrap();
        assert_eq!(cmd, r"C:\tools\pixtuoid-hook.exe --source reasonix");
    }

    // These fixture paths don't exist on the runner, so the 8.3 short-name lookup
    // fails and the reject fallback is what fires.
    #[test]
    #[cfg(windows)]
    fn hook_command_rejects_cmd_unsafe_path_on_windows() {
        assert!(hook_command(Path::new(r"C:\Program Files\pixtuoid-hook.exe"), false).is_err());
        let err = hook_command(Path::new(r"C:\Users\a&b\pixtuoid-hook.exe"), false)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("cmd.exe") && err.contains("ordinary characters"),
            "must explain the cmd-unsafe path + workaround: {err}"
        );
    }

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
            Some(PathBuf::from(r"C:\Users\ada\AppData\Roaming"))
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
        assert!(hook_command(bad, false).is_err());
    }

    #[test]
    fn every_registered_reasonix_event_decodes() {
        use pixtuoid_core::source::decoder::decode_hook_payload;
        for ev in REASONIX_EVENTS {
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

    #[test]
    fn reasonix_events_pins_the_exact_registered_set() {
        crate::install::assert_event_roster(
            "REASONIX_EVENTS",
            REASONIX_EVENTS,
            &[
                "SessionStart",
                "PreToolUse",
                "PostToolUse",
                "PermissionRequest",
                "UserPromptSubmit",
                "Stop",
                "Notification",
                "SessionEnd",
            ],
        );
    }
}
