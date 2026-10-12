//! Reasonix hook install target: a plugin `reasonix plugin install --link` loads in
//! place from pixtuoid's config dir, through its native `reasonix-plugin.json`
//! (`docs/PLUGIN_PACKAGES.md` @ v2.35.0). A global plugin's hooks need no
//! `/hooks trust`, which project-scope hooks do.
//!
//! - `match` is OMITTED: empty = every tool. Any other value is an ANCHORED
//!   regex, and a malformed one never fires.
//! - `timeout` is in MILLISECONDS, and on the gating PreToolUse a TIMEOUT BLOCKS
//!   the user's tool call.
//! - EXEC form (`args` present): a command with neither `args` nor `shellCommand`
//!   that is not an absolute path resolves inside the plugin root
//!   (`internal/ext/hook/windows_posix_script.go` @ v2.35.0), so a shell-form
//!   `PIXTUOID_SOURCE=reasonix '<shim>'` would never run.
//! - No `_pixtuoid` sentinel: a v2 manifest rejects unknown fields.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::install::host_plugin::{self, NAME};
use crate::install::io;
use crate::install::merge;
#[cfg(any(test, feature = "upstream"))]
use crate::install::target::UNFIRED_HOOK;
use crate::install::target::{HostRegistration, LegacyConfig, MergeOutcome, Unregistered};
use crate::install::verify::{SchemaParse, ShimRef};
#[cfg(any(test, feature = "upstream"))]
use pixtuoid_core::source::upstream::{At, Doc, Finding, Read, Watch, When};

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

#[cfg(any(test, feature = "upstream"))]
pub(crate) const UPSTREAM: &[Watch] = &[Watch {
    id: "reasonix.hook_events",
    ours: || REASONIX_EVENTS.to_vec(),
    docs: &[Doc {
        at: At::Path {
            repo: "esengine/DeepSeek-Reasonix",
            branch: "main-v2",
            path: "internal/hook/hook.go",
        },
        anchor: r"type Event string\b",
        owns: "the `Event` type",
    }],
    read: Read::Matches {
        pattern: r#"\w+\s+Event\s*=\s*"(\w+)""#,
    },
    when: When::Vanishes,
    finding: Finding::Breaking(UNFIRED_HOOK),
    siblings: None,
}];

/// Reasonix links the plugin dir in place, so a rewrite of the manifest reaches the
/// next session.
pub(crate) const HOST: HostRegistration = HostRegistration {
    cli: CLI,
    register,
    unregister,
    is_registered,
    copy: None,
    legacy: Some(LegacyConfig {
        path: settings_path,
        strip: merge::flat_json_uninstall,
    }),
};

const CLI: io::HostCli = io::HostCli {
    command: "reasonix",
};

const MANIFEST: &str = "reasonix-plugin.json";

const HOOKS_AT: host_plugin::HooksAt = host_plugin::HooksAt {
    hooks: "/contributes/hooks",
    command: "/command",
};

pub(crate) fn default_config_path() -> Result<PathBuf> {
    host_plugin::owned_path("reasonix-plugin", &[MANIFEST])
}

/// The GLOBAL `settings.json` a pixtuoid from before the plugin merged into.
fn settings_path() -> Result<PathBuf> {
    Ok(home()?.join("settings.json"))
}

/// Where Reasonix records its installed plugins and whether each is enabled
/// (`internal/ext/pluginpkg/pluginpkg.go` @ v2.35.0).
fn packages_path() -> Result<PathBuf> {
    Ok(home()?.join("plugin-packages.json"))
}

/// Reasonix's home is platform-ASYMMETRIC, so a generic USERPROFILE-first path on
/// Windows misses it. Erroring mirrors the sibling home-anchored targets instead of
/// silently reading a CWD-relative file Reasonix never writes.
fn home() -> Result<PathBuf> {
    reasonix_home()
        .ok_or_else(|| io::home_unset("Reasonix's home", "REASONIX_HOME/APPDATA/HOME/USERPROFILE"))
}

/// `REASONIX_HOME` (upstream trims but does NOT `~`-expand it, hence `home: None`)
/// → Windows `%APPDATA%\reasonix` → else `<home>/.reasonix`.
fn reasonix_home() -> Option<PathBuf> {
    resolve_reasonix_home(
        pixtuoid_core::platform::path_env("REASONIX_HOME").map(|v| io::expand_tilde(&v, None)),
        cfg!(windows),
        user_config_dir(),
        pixtuoid_core::platform::user_home_opt(),
    )
}

/// Pure core for [`reasonix_home`] — everything is injected so BOTH platform arms
/// unit-test on any host. `None` only when neither `%APPDATA%` nor a home
/// resolves, and the legacy strip and the registration probe then error rather
/// than read a CWD-relative file Reasonix never writes.
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

/// Reasonix is present when its home dir is — and hook/trust users additionally
/// have a `~/.reasonix` even on Windows, so probe both.
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
/// fall back to a home join and no home resolves.
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

/// The shim's absolute path, exec form; the source rides as `--source` in `args`.
/// Err on non-UTF-8 (prevents the to_string_lossy dead-hook).
pub(crate) fn hook_command(resolved: &Path, _explicit: bool) -> Result<String> {
    // `_explicit` is Claude's bare-name-vs-absolute switch — Reasonix always
    // embeds the absolute path, so the flag changes nothing here.
    merge::hook_path_str(resolved).map(str::to_string)
}

pub(crate) fn render_manifest(hook_cmd: &str) -> Result<String> {
    let entry = json!({
        "command": hook_cmd,
        "args": ["--source", pixtuoid_core::source::reasonix::SOURCE_NAME],
        "timeout": 1000,
        "description": "pixtuoid visualizer",
    });
    let mut doc = manifest();
    doc["contributes"] = json!({ "hooks": host_plugin::hooks_map(REASONIX_EVENTS, &entry) });
    Ok(serde_json::to_string_pretty(&doc)?)
}

/// The manifest with nothing to contribute. No `version`: none is required, and one
/// would put the release in a file a launch compares byte for byte.
fn manifest() -> Value {
    json!({
        "apiVersion": "reasonix.io/plugin/v2",
        "name": NAME,
        "description": "Sends Reasonix's session events to the pixtuoid office",
    })
}

pub(crate) fn verify_schema(content: &str) -> SchemaParse {
    host_plugin::verify_hooks(content, HOOKS_AT, REASONIX_EVENTS, |c| {
        ShimRef::Absolute(PathBuf::from(c))
    })
}

pub(crate) fn installed_shim(content: &str) -> ShimRef {
    verify_schema(content).shim
}

/// The manifest as a disconnect leaves it: still one Reasonix loads, should the
/// plugin stay registered, and still naming us so `unregister` may delete it.
pub(crate) fn uninstall(content: &str) -> Result<MergeOutcome> {
    Ok(MergeOutcome {
        content: serde_json::to_string_pretty(&manifest())?,
        changed: host_plugin::holds_hooks(content, HOOKS_AT),
    })
}

/// `--yes` is the CLI's consent to write, not a trust step; `--replace` makes a
/// repeat connect re-link (`docs/PLUGIN_PACKAGES.md` @ v2.35.0).
fn register(config: &Path) -> Result<()> {
    let (plugin, _) = host_plugin::layout(config, &[MANIFEST])?;
    CLI.run(&[
        "plugin".as_ref(),
        "install".as_ref(),
        plugin.as_os_str(),
        "--link".as_ref(),
        "--replace".as_ref(),
        "--yes".as_ref(),
    ])?;
    Ok(())
}

/// Reads Reasonix's state first, so a disconnect with nothing registered starts no
/// CLI. Removing a linked plugin leaves its dir (`docs/PLUGIN_PACKAGES.md`
/// @ v2.35.0), so this deletes it.
fn unregister(config: &Path) -> Result<Unregistered> {
    let (_, root) = host_plugin::layout(config, &[MANIFEST])?;
    let installed = plugin_entry()?.is_some();
    if installed {
        if CLI.locate().is_none() {
            tracing::warn!("reasonix not on PATH; leaving the pixtuoid plugin registered");
            return Ok(Unregistered::Unreachable);
        }
        CLI.run(&[
            "plugin".as_ref(),
            "remove".as_ref(),
            NAME.as_ref(),
            "--yes".as_ref(),
        ])?;
    }
    host_plugin::remove_if_ours(root, config)?;
    Ok(if installed {
        Unregistered::Removed
    } else {
        Unregistered::Absent
    })
}

fn is_registered() -> Result<bool> {
    Ok(plugin_entry()?.is_some_and(|p| p["enabled"].as_bool() == Some(true)))
}

/// Our row in Reasonix's installed plugins, if any.
fn plugin_entry() -> Result<Option<Value>> {
    let path = packages_path()?;
    let content = io::read_config(&path)?;
    if content.trim().is_empty() {
        return Ok(None);
    }
    let state: Value =
        serde_json::from_str(&content).with_context(|| format!("parsing {}", path.display()))?;
    Ok(state["plugins"]
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["name"].as_str() == Some(NAME)))
        .cloned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::SENTINEL_KEY;

    fn json_merge_uninstall(doc: Value) -> Value {
        merge::flat_json_merge_uninstall(doc, SENTINEL_KEY)
    }

    /// Two events as a pixtuoid from before the plugin merged them into settings.json.
    fn legacy() -> Value {
        let ours = json!({
            SENTINEL_KEY: true,
            "command": "PIXTUOID_SOURCE=reasonix '/opt/pixtuoid-hook'",
            "timeout": 1000,
            "description": "pixtuoid visualizer"
        });
        json!({ "hooks": { "PreToolUse": [ours.clone()], "Stop": [ours] } })
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
    fn uninstall_removes_only_managed_entries_and_empty_maps() {
        let mut installed = legacy();
        installed["hooks"]["PreToolUse"]
            .as_array_mut()
            .unwrap()
            .insert(0, json!({ "match": "bash", "command": "my-guard.sh" }));
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
        let cleaned = json_merge_uninstall(legacy());
        assert!(cleaned.get("hooks").is_none(), "got {cleaned}");
    }

    #[test]
    fn every_event_execs_the_shim_with_its_source_and_reads_it_back() {
        let shim = if cfg!(windows) {
            r"C:\Program Files\pixtuoid-hook.exe"
        } else {
            "/Users/Jane Doe/bin/pixtuoid-hook"
        };
        let cmd = hook_command(Path::new(shim), false).unwrap();
        assert_eq!(cmd, shim, "exec form: the bare path, spaces and all");
        let rendered = render_manifest(&cmd).unwrap();
        let doc: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(doc["apiVersion"], json!("reasonix.io/plugin/v2"));
        let mut keys: Vec<_> = doc.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["apiVersion", "contributes", "description", "name"],
            "a v2 manifest rejects unknown fields"
        );
        let hooks = doc["contributes"]["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), REASONIX_EVENTS.len());
        for ev in REASONIX_EVENTS {
            assert_eq!(
                hooks[*ev],
                json!([{
                    "command": shim,
                    "args": ["--source", "reasonix"],
                    "timeout": 1000,
                    "description": "pixtuoid visualizer",
                }]),
                "{ev}"
            );
        }
        let v = verify_schema(&rendered);
        assert!(v.issues.is_empty(), "{:?}", v.issues);
        assert_eq!(v.shim, ShimRef::Absolute(PathBuf::from(shim)));
    }

    #[test]
    fn a_disconnected_manifest_still_loads_and_names_us() {
        let out = uninstall(&render_manifest("/x/pixtuoid-hook").unwrap()).unwrap();
        assert!(out.changed);
        let doc: Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(doc, manifest());
        assert!(!uninstall(&out.content).unwrap().changed);
        assert!(!uninstall("").unwrap().changed);
    }

    #[test]
    fn merge_uninstall_no_pixtuoid_hooks_reports_unchanged() {
        let user = r#"{ "hooks": { "Stop": [ { "command": "notify-send done" } ] } }"#;
        let out = merge::flat_json_uninstall(user).unwrap();
        assert!(!out.changed, "no managed entries → semantic no-op");
    }

    #[cfg(windows)]
    #[test]
    fn user_config_dir_uses_appdata_on_windows() {
        temp_env::with_var("APPDATA", Some(r"C:\Users\ada\AppData\Roaming"), || {
            assert_eq!(
                user_config_dir(),
                Some(PathBuf::from(r"C:\Users\ada\AppData\Roaming"))
            );
        });
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
