//! Reasonix hook install target.
//!
//! Writes the GLOBAL `<reasonix-home>/settings.json` (`~/.reasonix/settings.json`
//! on macOS/Linux, **`%APPDATA%\reasonix\settings.json` on Windows** — see
//! `default_config_path`/`reasonix_home`) — project-scope
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

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::install::io;
use crate::install::target::MergeOutcome;
use crate::install::verify;

const SENTINEL_KEY: &str = "_pixtuoid";

/// Events we register == events we decode (`source/reasonix.rs`), enforced by
/// `every_registered_reasonix_event_decodes` below. PostLLMCall / PreCompact /
/// SubagentStop are deliberately absent: per-model-turn noise, compaction
/// internals, and a no-id subagent signal already covered by the parent's
/// `task` PostToolUse. `PermissionRequest` (#302) is the structured approval
/// gate → Waiting, fired alongside `Notification` (idempotent).
const REASONIX_EVENTS: &[&str] = &[
    "SessionStart",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "UserPromptSubmit",
    "Stop",
    "Notification",
    "SessionEnd",
];

/// The GLOBAL `settings.json` Reasonix actually reads, under its `ReasonixHomeDir`.
/// Reasonix's home is **platform-ASYMMETRIC** (`docs/CONFIG_PATHS.md` +
/// `internal/config/config.go::reasonixHomeDir`): `REASONIX_HOME` (verbatim) wins;
/// else macOS/Linux = `~/.reasonix`, but **Windows = `%APPDATA%\reasonix`**
/// (Go's `os.UserConfigDir()/reasonix`, NOT `%USERPROFILE%\.reasonix`). Global
/// hooks live at `<reasonix-home>/settings.json`. Writing `~/.reasonix/settings.json`
/// on Windows (pixtuoid's generic USERPROFILE-first path) lands the hooks where
/// Reasonix never reads → installed, but no sprite — the same class as
/// CodeWhale/OpenClaw but on the %APPDATA% (config-dir) axis, not HOME-vs-USERPROFILE.
pub fn default_config_path() -> Result<PathBuf> {
    reasonix_home()
        .map(|h| h.join("settings.json"))
        .ok_or_else(|| {
            // Reachable ONLY on non-Windows with no `HOME` (the Windows arm always
            // resolves via `user_config_dir()`), so `USERPROFILE` is not named here.
            anyhow!(
                "cannot resolve Reasonix's home (REASONIX_HOME/HOME unset); pass --config <path>"
            )
        })
}

/// Reasonix's `ReasonixHomeDir`: `REASONIX_HOME` (verbatim) → Windows
/// `%APPDATA%\reasonix` (`user_config_dir()/reasonix`) → else `<home>/.reasonix`.
fn reasonix_home() -> Option<PathBuf> {
    resolve_reasonix_home(
        io::nonempty_env("REASONIX_HOME"),
        cfg!(windows),
        user_config_dir(),
        io::user_home(),
    )
}

/// Pure core for [`reasonix_home`] — the `REASONIX_HOME` override, the platform
/// flag, the resolved Windows config dir (`%APPDATA%`), and the OS home are all
/// injected so BOTH platform arms unit-test on any host. The Windows arm ALWAYS
/// returns `Some` (`user_config_dir()` falls `%APPDATA%`→`<home>/AppData/Roaming`,
/// so it never fails) — deliberately MORE lenient than Go's `os.UserConfigDir`,
/// which ERRORS when `%APPDATA%` is unset. Harmless: that case means Reasonix
/// itself resolves no home (it can't read the file either), and the computed
/// `<home>/AppData/Roaming/reasonix` is exactly the canonical `%APPDATA%` default.
fn resolve_reasonix_home(
    reasonix_home_env: Option<String>,
    windows: bool,
    windows_config_dir: PathBuf,
    unix_home: Option<String>,
) -> Option<PathBuf> {
    if let Some(h) = reasonix_home_env {
        return Some(PathBuf::from(h));
    }
    if windows {
        return Some(windows_config_dir.join("reasonix"));
    }
    unix_home.map(|h| PathBuf::from(h).join(".reasonix"))
}

/// Presence probe for auto-detection. The default file-exists check on
/// `default_config_path` would NEVER fire: Reasonix itself never creates
/// `settings.json` (it is purely user-authored; `readSettings` just returns nil
/// when missing). What a real install does create is the Reasonix home dir
/// (`reasonix_home` — `%APPDATA%\reasonix` on Windows, `~/.reasonix` elsewhere,
/// honoring `REASONIX_HOME`); hook/trust users additionally have a `~/.reasonix`
/// even on Windows. Probe both.
pub fn detect_installed() -> bool {
    reasonix_home().is_some_and(|d| d.exists()) || io::home_relative(".reasonix").exists()
}

/// Rust mapping of Go's `os.UserConfigDir()` for the platforms we ship:
/// macOS `$HOME/Library/Application Support`, **Windows `%APPDATA%`** (Roaming —
/// where Reasonix's v2 config dir actually lives; without this arm
/// `detect_installed` probes `~/.config/reasonix` on Windows, which Reasonix never
/// creates, so auto-detection would always miss), else `$XDG_CONFIG_HOME` falling
/// back to `~/.config`.
///
/// The OS->dir decision is a PURE core fn (`platform::resolve_user_config_dir`)
/// so every arm is unit-testable on any host; this site just injects the live
/// OS + env + home values once.
fn user_config_dir() -> PathBuf {
    pixtuoid_core::platform::resolve_user_config_dir(
        std::env::consts::OS,
        std::env::var("APPDATA").ok(),
        std::env::var("XDG_CONFIG_HOME").ok(),
        &io::home_relative(""),
    )
}

/// Reasonix runs the `command` string under a shell — `sh -c` on Unix, `cmd.exe
/// /c` on Windows (verified: `internal/hook/hook.go:414` `shellInvocation`, an
/// explicit `GOOS=="windows"` branch). Same contract as Codex, so the OS forms
/// mirror codex::hook_command exactly:
/// - **Unix**: env-prefix `PIXTUOID_SOURCE=reasonix '<abs-path>'` (single-quoted).
/// - **Windows**: BARE `<abs-path> --source reasonix` via the shared
///   `hook_cmd::windows::windows_bare_hook_command` (cmd.exe can't express the env-prefix; the
///   source rides as the shim's `--source` flag). That helper substitutes the 8.3
///   short name for a space/metacharacter path, rejecting only if 8.3 is disabled
///   (#195) — a quoted path can't survive cmd /C.
///
/// Err on non-UTF-8 (prevents the to_string_lossy dead-hook).
pub fn hook_command(resolved: &Path, _explicit: bool) -> Result<String> {
    // `_explicit` is Claude's bare-name-vs-absolute switch — Reasonix always
    // embeds the absolute path, so the flag changes nothing here.
    let p = resolved
        .to_str()
        .ok_or_else(|| anyhow!("pixtuoid-hook path is non-UTF-8: {}", resolved.display()))?;
    // Same OS fork as Codex, in one place (hook_cmd::shell_hook_command): Unix
    // env-prefix form / Windows bare `<path> --source reasonix`.
    crate::install::hook_cmd::shell_hook_command(p, "reasonix")
}

pub fn merge_install(content: &str, hook_cmd: &str) -> Result<MergeOutcome> {
    let doc = verify::parse_json_or_empty(content)?;
    // See claude.rs: a valid-JSON-but-non-object doc would be silently dropped
    // by `flat_json_merge_install`. Refuse rather than overwrite the user's content.
    if !doc.is_object() && !doc.is_null() {
        anyhow::bail!("settings is valid JSON but not an object — refusing to overwrite");
    }
    let merged = verify::flat_json_merge_install(
        doc.clone(),
        REASONIX_EVENTS,
        SENTINEL_KEY,
        managed_entry,
        hook_cmd,
    );
    let changed = merged != doc;
    Ok(MergeOutcome {
        content: serde_json::to_string_pretty(&merged)?,
        changed,
    })
}

pub fn merge_uninstall(content: &str) -> Result<MergeOutcome> {
    let doc = verify::parse_json_or_empty(content)?;
    let cleaned = verify::flat_json_merge_uninstall(doc.clone(), SENTINEL_KEY);
    let changed = cleaned != doc;
    Ok(MergeOutcome {
        content: serde_json::to_string_pretty(&cleaned)?,
        changed,
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

/// Install-schema verification (#309) — Reasonix's flat-JSON shape (shared with
/// Cursor): `hooks.<event>` arrays of `{_pixtuoid, command}`.
pub fn verify_schema(content: &str) -> crate::install::verify::SchemaParse {
    crate::install::verify::flat_json_verify(content, REASONIX_EVENTS, SENTINEL_KEY)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Thin wrappers over the shared flat-JSON merge so the existing per-shape
    // tests below exercise Reasonix's events + entry shape against the common core.
    fn json_merge_install(doc: Value, hook_command: &str) -> Value {
        verify::flat_json_merge_install(
            doc,
            REASONIX_EVENTS,
            SENTINEL_KEY,
            managed_entry,
            hook_command,
        )
    }

    fn json_merge_uninstall(doc: Value) -> Value {
        verify::flat_json_merge_uninstall(doc, SENTINEL_KEY)
    }

    #[test]
    fn reasonix_home_is_appdata_on_windows_but_dot_reasonix_elsewhere() {
        // The platform asymmetry (docs/CONFIG_PATHS.md): Windows = %APPDATA%\reasonix,
        // macOS/Linux = ~/.reasonix. The Windows arm was the bug — pixtuoid wrote
        // %USERPROFILE%\.reasonix while Reasonix reads %APPDATA%\reasonix.
        let appdata = PathBuf::from(r"C:\Users\me\AppData\Roaming");
        // Windows → <%APPDATA%>/reasonix (the injected config dir), NOT <home>/.reasonix.
        assert_eq!(
            resolve_reasonix_home(None, true, appdata.clone(), Some(r"C:\Users\me".into())),
            Some(appdata.join("reasonix"))
        );
        // Non-Windows → <home>/.reasonix (config dir ignored).
        assert_eq!(
            resolve_reasonix_home(None, false, appdata, Some("/home/u".into())),
            Some(PathBuf::from("/home/u").join(".reasonix"))
        );
        // Non-Windows with no home → None (installer surfaces "pass --config").
        assert_eq!(
            resolve_reasonix_home(None, false, PathBuf::from("/ignored"), None),
            None
        );
    }

    #[test]
    fn reasonix_home_env_override_wins_verbatim_on_both_platforms() {
        // REASONIX_HOME is the home dir VERBATIM (docs: "override Reasonix home"),
        // beating the platform default on either OS; settings.json joins onto it.
        for windows in [true, false] {
            assert_eq!(
                resolve_reasonix_home(
                    Some("/custom/rx".into()),
                    windows,
                    PathBuf::from(r"C:\AppData"),
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
    fn merge_install_rejects_valid_json_that_is_not_an_object() {
        // Mirrors claude.rs: a valid-JSON-but-non-object doc must be refused,
        // not silently coerced to {} (which drops the user's content).
        assert!(merge_install("[1, 2, 3]", "/x").is_err());
        assert!(merge_install("42", "/x").is_err());
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
        let cmd = hook_command(Path::new("/Users/Jane Doe/bin/pixtuoid-hook"), false).unwrap();
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
        let cmd = hook_command(Path::new(r"C:\tools\pixtuoid-hook.exe"), false).unwrap();
        assert_eq!(cmd, r"C:\tools\pixtuoid-hook.exe --source reasonix");
    }

    // Windows: a space/metacharacter path uses its 8.3 short name when available,
    // else rejects (shared hook_cmd::shell_hook_command — see #195). These test
    // paths don't exist on the runner, so the reject fallback fires.
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
        assert!(hook_command(bad, false).is_err());
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
