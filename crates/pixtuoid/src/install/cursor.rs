//! Cursor CLI hook install target.
//!
//! Writes the GLOBAL `<cursor-config-dir>/hooks.json` (`~/.cursor/hooks.json` by
//! default; `CURSOR_CONFIG_DIR` / Linux-BSD `XDG_CONFIG_HOME` relocate it — see
//! `default_config_path`) — Cursor's CC-style hook config.
//! The `cursor-agent` CLI reads both the user-global file and a project
//! `<repo>/.cursor/hooks.json`; we install user-global so it covers every
//! project. Schema (`cursor.com/docs/hooks`) is a `version` + per-event arrays
//! of FLAT `{command}` entries — NOT Claude's nested `{matcher, hooks:[...]}`
//! groups (the group shape reportedly does not fire in the Cursor CLI):
//!
//! ```json
//! {"version": 1,
//!  "hooks": {"preToolUse": [{"command": "PIXTUOID_SOURCE=cursor '/abs/pixtuoid-hook'",
//!                            "_pixtuoid": true}]}}
//! ```
//!
//! - `version` is required by Cursor; set to 1 on install if absent (a user's
//!   existing value is preserved).
//! - `_pixtuoid` is the managed-entry sentinel; Cursor's loader ignores unknown
//!   object fields (the same assumption Reasonix's Go loader makes).
//! - Cursor runs the `command` under a shell (its hook model mirrors Claude
//!   Code's), so the OS forms mirror `codex`/`reasonix` exactly via
//!   `hook_cmd::shell_hook_command`: Unix env-prefix `PIXTUOID_SOURCE=cursor
//!   '<path>'`, Windows bare `<path> --source cursor`. (Capture-gated: if the
//!   CLI exec's the command WITHOUT a shell, the Unix env-prefix won't take and
//!   this switches to the bare `--source` form — verified by the one-shot live
//!   capture before this target flips to "supported".)

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};

use crate::install::io;
use crate::install::merge;
use crate::install::target::MergeOutcome;
use crate::install::SENTINEL_KEY;

/// Events we register == events we decode (`source/cursor.rs`), enforced by
/// `every_registered_cursor_event_decodes` below. The camelCase names are
/// Cursor's. `subagentStart`/`subagentStop` are deliberately absent (not firing
/// in the CLI; session-only) and the shell/file-specific `before*`/`after*`
/// hooks are omitted in favor of the generic `preToolUse`/`postToolUse` pair.
/// `postToolUseFailure` FIRES instead of `postToolUse` on a failed tool, so it
/// is registered too — else a failed tool's ActivityStart never closes under
/// `-p` (where `stop` doesn't fire). The live capture refines this firing set,
/// tracked by the upstream-drift watch.
const CURSOR_EVENTS: &[&str] = &[
    "sessionStart",
    "preToolUse",
    "postToolUse",
    "postToolUseFailure",
    "stop",
    "sessionEnd",
];

/// The Cursor config dir Cursor actually reads, mirroring its documented
/// resolution (`cursor.com/docs/cli/reference/configuration`): `CURSOR_CONFIG_DIR`
/// (a custom dir, all platforms) wins; else on **Linux/BSD** a set `XDG_CONFIG_HOME`
/// gives `$XDG_CONFIG_HOME/cursor`; else `~/.cursor`. `hooks.json` lives in that dir
/// (`cursor.com/docs/hooks`), so it follows the same precedence — without honoring
/// `CURSOR_CONFIG_DIR`/`XDG_CONFIG_HOME`, a user who sets them has Cursor read the
/// overridden dir while pixtuoid wrote `~/.cursor` (installed, but no sprite). The
/// home base is `USERPROFILE`-first (Node `os.homedir`; `~/.cursor`, NOT the
/// Electron IDE's `%APPDATA%\Cursor`).
pub(crate) fn default_config_path() -> Result<PathBuf> {
    cursor_config_dir()
        .map(|d| d.join("hooks.json"))
        .ok_or_else(|| {
            anyhow!(
                "cannot resolve the home directory (HOME/USERPROFILE unset); pass --config <path>"
            )
        })
}

/// Cursor's config dir per `default_config_path`'s precedence. `xdg_applies` is
/// Linux/BSD only (Cursor consults `XDG_CONFIG_HOME` only there); macOS/Windows go
/// straight to `~/.cursor`.
fn cursor_config_dir() -> Option<PathBuf> {
    resolve_config_dir(
        io::nonempty_env("CURSOR_CONFIG_DIR"),
        io::nonempty_env("XDG_CONFIG_HOME"),
        cfg!(all(unix, not(target_os = "macos"))),
        pixtuoid_core::platform::user_home_opt(),
    )
}

/// Pure core for [`cursor_config_dir`] — env overrides, the Linux/BSD XDG flag, and
/// the home are injected so every arm unit-tests on any host.
fn resolve_config_dir(
    cursor_config_dir_env: Option<String>,
    xdg_config_home_env: Option<String>,
    xdg_applies: bool,
    home: Option<String>,
) -> Option<PathBuf> {
    if let Some(d) = cursor_config_dir_env {
        return Some(PathBuf::from(d));
    }
    if let Some(xdg) = xdg_config_home_env.filter(|_| xdg_applies) {
        return Some(PathBuf::from(xdg).join("cursor"));
    }
    home.map(|h| PathBuf::from(h).join(".cursor"))
}

/// Presence probe for auto-detection. Cursor never creates `hooks.json` itself (it
/// is purely user-authored), so a default file-exists check on it would never fire
/// — probe Cursor's own config dir (created on first run) instead, the same reason
/// Reasonix/CodeWhale/opencode probe their CLI dirs. Honors `CURSOR_CONFIG_DIR`/
/// `XDG_CONFIG_HOME` like the write path, plus the plain `~/.cursor` as a lenient
/// fallback.
pub(crate) fn detect_installed() -> bool {
    cursor_config_dir().is_some_and(|d| d.exists()) || io::home_relative(".cursor").exists()
}

/// Cursor runs the `command` under a shell (its hook system mirrors Claude
/// Code's). Same contract as Codex/Reasonix, so the OS forms mirror them:
/// - **Unix**: env-prefix `PIXTUOID_SOURCE=cursor '<abs-path>'` (single-quoted).
/// - **Windows**: BARE `<abs-path> --source cursor` via the shared
///   `hook_cmd::windows::windows_bare_hook_command` (8.3 short name for a
///   space/metacharacter path, else reject — #195).
///
/// Err on non-UTF-8 (prevents the to_string_lossy dead-hook).
pub(crate) fn hook_command(resolved: &Path, _explicit: bool) -> Result<String> {
    let p = merge::hook_path_str(resolved)?;
    crate::install::hook_cmd::shell_hook_command(p, "cursor")
}

pub(crate) fn merge_install(content: &str, hook_cmd: &str) -> Result<MergeOutcome> {
    // Shared parse + non-object guard + semantic-`changed` + serialize wrapper
    // (the guard's one copy lives in merge.rs). Cursor's `version` field injection
    // rides inside `json_merge_install`.
    merge::flat_json_merge_outcome_install(content, "hooks.json", |doc| {
        json_merge_install(doc, hook_cmd)
    })
}

pub(crate) fn merge_uninstall(content: &str) -> Result<MergeOutcome> {
    // Shared flat-JSON uninstall: removes managed entries + drops empty event
    // keys / the `hooks` object. `version` is untouched (preserved by design).
    merge::flat_json_merge_outcome_uninstall(content, |doc| {
        merge::flat_json_merge_uninstall(doc, SENTINEL_KEY)
    })
}

fn managed_entry(hook_command: &str) -> Value {
    json!({
        SENTINEL_KEY: true,
        "command": hook_command
    })
}

/// Install-schema verification (#309) — Cursor's flat-JSON shape (shared with
/// Reasonix): `hooks.<event>` arrays of `{_pixtuoid, command}`, PLUS the
/// Cursor-specific whole-file gate: the top-level `version` key. Cursor requires
/// it (module doc) — a hooks.json with intact managed entries but no numeric
/// `version` loads NO hooks at all, the exact silent-dead class #309 exists to
/// catch (the CodeWhale `[hooks].enabled = false` twin).
pub(crate) fn verify_schema(content: &str) -> crate::install::verify::SchemaParse {
    let mut parse = crate::install::verify::flat_json_verify(content, CURSOR_EVENTS, SENTINEL_KEY);
    // Only reachable on parseable JSON — an unparseable file is already a HARD
    // "no longer parses" issue from flat_json_verify.
    if let Ok(doc) = serde_json::from_str::<Value>(content) {
        if !doc.get("version").is_some_and(|v| v.is_number()) {
            parse.issues.push(
                "hooks.json has no numeric top-level `version` key — Cursor requires it, \
                 so no hooks load (reconnect via the Sources panel)"
                    .to_string(),
            );
        }
    }
    parse
}

/// Cursor's flat-JSON install: set `version` (Cursor-specific, set-if-absent,
/// preserve a user's value) then delegate the per-event managed-entry merge to
/// the shared `flat_json_merge_install` (the IDENTICAL shape Reasonix uses).
fn json_merge_install(doc: Value, hook_command: &str) -> Value {
    // Cursor requires a `version`; set it if absent, preserve a user's value.
    let mut root: Map<String, Value> = doc.as_object().cloned().unwrap_or_default();
    root.entry("version".to_string())
        .or_insert_with(|| json!(1));
    merge::flat_json_merge_install(
        Value::Object(root),
        CURSOR_EVENTS,
        SENTINEL_KEY,
        managed_entry,
        hook_command,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_honors_cursor_config_dir_then_xdg_on_linux_else_dot_cursor() {
        // CURSOR_CONFIG_DIR (custom dir, ALL platforms) wins verbatim — home/xdg
        // never consulted.
        assert_eq!(
            resolve_config_dir(
                Some("/custom/cur".into()),
                Some("/xdg".into()),
                true,
                Some("/h".into())
            ),
            Some(PathBuf::from("/custom/cur"))
        );
        // Linux/BSD (xdg_applies) + XDG_CONFIG_HOME set → $XDG_CONFIG_HOME/cursor.
        assert_eq!(
            resolve_config_dir(None, Some("/xdg".into()), true, Some("/home/u".into())),
            Some(PathBuf::from("/xdg").join("cursor"))
        );
        // Linux/BSD but XDG unset → ~/.cursor (Cursor only consults XDG when set).
        assert_eq!(
            resolve_config_dir(None, None, true, Some("/home/u".into())),
            Some(PathBuf::from("/home/u").join(".cursor"))
        );
        // macOS/Windows (xdg does NOT apply) → ~/.cursor even if XDG_CONFIG_HOME set.
        assert_eq!(
            resolve_config_dir(
                None,
                Some("/xdg".into()),
                false,
                Some(r"C:\Users\me".into())
            ),
            Some(PathBuf::from(r"C:\Users\me").join(".cursor"))
        );
        // No home + no override → None (installer surfaces "pass --config").
        assert_eq!(resolve_config_dir(None, None, false, None), None);
    }

    // Thin wrapper over the shared flat-JSON uninstall so the version-preservation
    // tests below still drive Cursor's path. Deliberately does NOT remove `version`:
    // we can't tell our set-if-absent `1` from a user's own value, and stripping it
    // would DELETE a user's `{"version": N}` (no hooks) on uninstall. A leftover
    // `{"version": 1}` after a from-scratch install→uninstall is a harmless
    // valid-Cursor residual (accepted, like opencode's no-op stub) — the shared
    // helper only touches `hooks`, so `version` survives.
    fn json_merge_uninstall(doc: Value) -> Value {
        merge::flat_json_merge_uninstall(doc, SENTINEL_KEY)
    }

    #[test]
    fn install_creates_flat_entries_for_all_events_with_version() {
        let doc = json_merge_install(json!({}), "PIXTUOID_SOURCE=cursor '/opt/pixtuoid-hook'");
        assert_eq!(doc["version"], json!(1), "Cursor requires a version field");
        let hooks = doc.get("hooks").and_then(|v| v.as_object()).unwrap();
        for ev in CURSOR_EVENTS {
            let arr = hooks.get(*ev).and_then(|v| v.as_array()).unwrap();
            assert_eq!(arr.len(), 1, "event {ev}");
            let entry = &arr[0];
            assert_eq!(
                entry["command"].as_str().unwrap(),
                "PIXTUOID_SOURCE=cursor '/opt/pixtuoid-hook'"
            );
            assert!(entry[SENTINEL_KEY].as_bool().unwrap());
            // Flat shape: command directly on the entry — no CC-style nested group.
            assert!(
                entry.get("hooks").is_none() && entry.get("type").is_none(),
                "must not write CC-style nested groups"
            );
        }
    }

    #[test]
    fn install_preserves_existing_version() {
        let doc = json_merge_install(json!({"version": 2}), "/x");
        assert_eq!(
            doc["version"],
            json!(2),
            "must not clobber a user's version"
        );
    }

    #[test]
    fn install_is_idempotent_and_replaces_across_paths() {
        let a = json_merge_install(json!({}), "PIXTUOID_SOURCE=cursor '/opt/a/pixtuoid-hook'");
        let b = json_merge_install(a.clone(), "PIXTUOID_SOURCE=cursor '/opt/a/pixtuoid-hook'");
        assert_eq!(a, b, "same command re-install is a no-op");
        let c = json_merge_install(a, "PIXTUOID_SOURCE=cursor '/opt/b/pixtuoid-hook'");
        for ev in CURSOR_EVENTS {
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
            "version": 1,
            "hooks": {"preToolUse": [ { "command": "my-guard.sh" } ]},
            "other": "setting"
        });
        let merged = json_merge_install(initial, "/x");
        let arr = merged["hooks"]["preToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["command"], json!("my-guard.sh"));
        assert_eq!(merged["other"], json!("setting"));
    }

    #[test]
    fn uninstall_removes_only_managed_entries_and_empty_maps() {
        let installed = json_merge_install(
            json!({"hooks": {"preToolUse": [ { "command": "my-guard.sh" } ]}}),
            "/x",
        );
        let cleaned = json_merge_uninstall(installed);
        let arr = cleaned["hooks"]["preToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["command"], json!("my-guard.sh"));
        for ev in CURSOR_EVENTS.iter().filter(|e| **e != "preToolUse") {
            assert!(
                cleaned["hooks"].get(*ev).is_none(),
                "event {ev} should be dropped once empty"
            );
        }
        // A user hook survived → the file is not empty → version stays.
        assert_eq!(cleaned["version"], json!(1));
    }

    #[test]
    fn uninstall_all_managed_drops_hooks_but_keeps_version() {
        let installed = json_merge_install(json!({}), "/x");
        let cleaned = json_merge_uninstall(installed);
        assert!(cleaned.get("hooks").is_none(), "got {cleaned}");
        // `version` is preserved (we can't distinguish our `1` from a user's) —
        // a harmless residual, and the only safe choice (see below).
        assert_eq!(cleaned["version"], json!(1), "got {cleaned}");
    }

    #[test]
    fn uninstall_preserves_a_users_version_only_file() {
        // The data-loss case the review caught: a user's {"version": N} with NO
        // hooks must survive install→uninstall, not be stripped to {}.
        let installed = json_merge_install(json!({"version": 3}), "/x");
        let cleaned = json_merge_uninstall(installed);
        assert_eq!(
            cleaned,
            json!({"version": 3}),
            "a user's version must not be lost on uninstall: {cleaned}"
        );
    }

    // Twin of codewhale's verify_schema_flags_enabled_false_and_passes_full_install:
    // Cursor gates ALL hook loading on the top-level `version` key (the module
    // doc's upstream note), so a hooks.json with intact managed entries but no
    // (or a non-numeric) `version` is the #309 silent-dead class and must verify
    // BROKEN — while a fresh full install verifies sound.
    #[test]
    fn verify_schema_flags_a_missing_version_and_passes_full_install() {
        let installed =
            json_merge_install(json!({}), "PIXTUOID_SOURCE=cursor '/opt/pixtuoid-hook'");
        let sound = verify_schema(&installed.to_string());
        assert!(sound.issues.is_empty(), "{:?}", sound.issues);

        // Same managed entries, `version` dropped (a user/tool rewrite).
        let mut versionless = installed.clone();
        versionless.as_object_mut().unwrap().remove("version");
        let p = verify_schema(&versionless.to_string());
        assert!(
            p.issues.iter().any(|i| i.contains("version")),
            "a version-less hooks.json must be flagged: {:?}",
            p.issues
        );

        // A non-numeric version is equally dead to Cursor's loader.
        let mut stringly = installed;
        stringly["version"] = json!("1");
        let p = verify_schema(&stringly.to_string());
        assert!(
            p.issues.iter().any(|i| i.contains("version")),
            "a non-numeric version must be flagged: {:?}",
            p.issues
        );
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
        let user = r#"{ "version": 1, "hooks": { "stop": [ { "command": "notify done" } ] } }"#;
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
        let doc = json_merge_install(json!({"hooks": {"stop": 42}}), "/x");
        assert_eq!(doc["hooks"]["stop"].as_array().unwrap().len(), 1);
    }

    // Unix POSIX-form pin (single-quoted env-prefix). Unix-only: on Windows the
    // bare form is emitted and this spaced path would be REJECTED.
    #[cfg(unix)]
    #[test]
    fn hook_command_stamps_source_and_quotes() {
        let cmd = hook_command(Path::new("/Users/Jane Doe/bin/pixtuoid-hook"), false).unwrap();
        assert_eq!(
            cmd,
            "PIXTUOID_SOURCE=cursor '/Users/Jane Doe/bin/pixtuoid-hook'"
        );
    }

    // Windows: bare exec form `<path> --source cursor` (mirrors codex/reasonix).
    #[test]
    #[cfg(windows)]
    fn hook_command_emits_bare_exec_form_with_source_flag_on_windows() {
        let cmd = hook_command(Path::new(r"C:\tools\pixtuoid-hook.exe"), false).unwrap();
        assert_eq!(cmd, r"C:\tools\pixtuoid-hook.exe --source cursor");
    }

    #[test]
    #[cfg(windows)]
    fn hook_command_rejects_cmd_unsafe_path_on_windows() {
        assert!(hook_command(Path::new(r"C:\Program Files\pixtuoid-hook.exe"), false).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn hook_command_errors_on_non_utf8_path() {
        use std::os::unix::ffi::OsStrExt;
        let bad = Path::new(std::ffi::OsStr::from_bytes(b"/x/\xff/pixtuoid-hook"));
        assert!(hook_command(bad, false).is_err());
    }

    // Internal-consistency guard (mirror of CC/Codex/Reasonix): every hook event
    // we REGISTER with Cursor must have a decoder arm, else it arrives at the
    // shared socket and `decode_hook_payload` bails — silently dropped.
    #[test]
    fn every_registered_cursor_event_decodes() {
        use pixtuoid_core::source::decoder::decode_hook_payload;
        for ev in CURSOR_EVENTS {
            let payload = serde_json::json!({
                "hook_event_name": ev,
                "cwd": "/repo",
                "_pixtuoid_source": "cursor",
            });
            assert!(
                decode_hook_payload(payload).is_ok(),
                "registered Cursor hook {ev:?} has no decoder arm — it would bail \
                 as unsupported. Add an arm in pixtuoid-core source/cursor.rs."
            );
        }
    }
}
