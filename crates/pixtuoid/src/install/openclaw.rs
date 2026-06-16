//! OpenClaw install target — the TWO-OWNERSHIP hybrid.
//!
//! OpenClaw is one always-on gateway DAEMON; pixtuoid renders it as a single
//! presence-gated wandering lobster mascot. Its plugin observes the gateway
//! lifecycle and pipes a STRICT allowlist of timing/id fields (never message
//! content) to the `pixtuoid-hook` shim (`--source openclaw`).
//!
//! Unlike opencode (a single auto-discovered plugin file), OpenClaw needs BOTH:
//!   1. the plugin DIR — `<openclaw-home>/plugins/pixtuoid/{openclaw.plugin.json,
//!      package.json, index.js}` — wholly owned by pixtuoid (the `extra_artifacts`
//!      Target hook writes these verbatim, the shim path baked into `index.js`).
//!   2. a config merge into `<openclaw-home>/openclaw.json` adding
//!      `plugins.load.paths += [<plugin-dir>]` and `plugins.entries.pixtuoid =
//!      { enabled: true, hooks: { allowConversationAccess: true } }`.
//!
//! Capture-confirmed (2026-06-15): `openclaw plugins install --link <dir>` +
//! `enable` writes EXACTLY those config keys to openclaw.json (no separate
//! registry), so the install is a pure `ConfigLock` write — no subprocess. The
//! `allowConversationAccess` grant un-gates `before_agent_run`/`agent_end` (the
//! busy tell); UNINSTALL REVOKES it (removes our `entries.pixtuoid` subtree) so a
//! disconnect leaves no standing conversation-access grant. The plugin files are
//! left in place on uninstall (the config un-merge stops the gateway loading
//! them) — an accepted residual like opencode's stub.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use crate::install::io;
use crate::install::target::MergeOutcome;

/// The plugin id — the key under `plugins.entries` and `plugins.load.paths`'s dir.
const PLUGIN_ID: &str = "pixtuoid";
/// First-line marker in the rendered entry module (provenance; not load-bearing
/// for detection, which keys on OpenClaw's own dirs).
#[allow(dead_code)]
const SENTINEL: &str = "@pixtuoid-openclaw-plugin";
/// Placeholder for the baked shim path in the bundled entry module.
const HOOK_PLACEHOLDER: &str = "{{HOOK_PATH_JSON}}";
const PLUGIN_TEMPLATE: &str = include_str!("openclaw_plugin.js");

/// The OpenClaw gateway hook events pixtuoid depends on — the SINGLE source of
/// truth, pinned to BOTH the plugin's `HOOKS` array (what we register) AND the
/// `decode_openclaw_hook_payload` arms (what we decode) by the consistency test
/// below, and the list `check_upstream_drift.py` reads for the CI upstream watch.
/// A rename upstream makes that hook silently stop firing (the plugin registers
/// by name), so this is the drift surface to watch (defense #4).
pub const OPENCLAW_EVENTS: &[&str] = &[
    "gateway_start",
    "gateway_stop",
    "session_start",
    "session_end",
    "before_agent_run",
    "agent_end",
];

const MANIFEST: &str = r#"{
  "id": "pixtuoid",
  "name": "Pixtuoid",
  "description": "Forwards OpenClaw gateway daemon-presence signals to pixtuoid (the terminal office visualizer).",
  "configSchema": { "type": "object", "additionalProperties": false, "properties": {} },
  "activation": { "onStartup": true }
}
"#;

const PACKAGE: &str = r#"{
  "name": "pixtuoid",
  "version": "0.0.0",
  "type": "module",
  "private": true,
  "openclaw": { "extensions": ["./index.js"], "runtimeExtensions": ["./index.js"] }
}
"#;

/// OpenClaw's state dir (holds `openclaw.json` + `plugins/`), mirroring its own
/// `config/paths.ts::resolveStateDir` + `infra/home-dir.ts::resolveRawOsHomeDir`:
/// the `OPENCLAW_STATE_DIR` override wins; else the state dir is
/// `<effective-home>/.openclaw`, where the effective home is `OPENCLAW_HOME`, then
/// **`$HOME`, then `%USERPROFILE%`** — i.e. **HOME-FIRST** (like CodeWhale), NOT
/// pixtuoid's generic `USERPROFILE`-first `io::home_relative`. A Windows user who
/// exports `HOME` (Git Bash / MSYS2 / Cygwin) has the gateway read
/// `%HOME%\.openclaw\`, so writing our plugin/config to `%USERPROFILE%\.openclaw\`
/// would leave it where the gateway never loads it (no lobster). The HOME-vs-
/// USERPROFILE half is shared with CodeWhale via [`pixtuoid_core::platform::home_first_dir`];
/// the `OPENCLAW_HOME` override layers on top (OpenClaw-specific). The legacy
/// pre-rebrand `.clawdbot` dir is preferred when `.openclaw` is absent and
/// `.clawdbot` exists (OpenClaw's `resolveStateDir` legacy fallback — the same
/// "don't shadow the user's real config" rule as CodeWhale's `.deepseek`).
fn openclaw_state_dir() -> Result<PathBuf> {
    resolve_openclaw_state_dir(
        io::nonempty_env("OPENCLAW_STATE_DIR"),
        io::nonempty_env("OPENCLAW_HOME"),
        pixtuoid_core::platform::home_first_dir(),
        |p| p.exists(),
    )
}

/// Pure core for [`openclaw_state_dir`] — every env input, the resolved OS home,
/// and the existence check are injected so the precedence is unit-testable without
/// env/FS mutation.
fn resolve_openclaw_state_dir(
    state_dir_env: Option<String>,
    openclaw_home_env: Option<String>,
    os_home_first: Option<PathBuf>,
    exists: impl Fn(&Path) -> bool,
) -> Result<PathBuf> {
    if let Some(d) = state_dir_env {
        return Ok(PathBuf::from(d));
    }
    let home = openclaw_home_env
        .map(PathBuf::from)
        .or(os_home_first)
        .ok_or_else(|| {
            anyhow!(
                "cannot resolve OpenClaw's home (OPENCLAW_STATE_DIR/OPENCLAW_HOME/HOME/USERPROFILE \
                 unset); pass --config <path>"
            )
        })?;
    let modern = home.join(".openclaw");
    if exists(&modern) {
        return Ok(modern);
    }
    let legacy = home.join(".clawdbot");
    if exists(&legacy) {
        return Ok(legacy);
    }
    Ok(modern)
}

/// The config file we merge into, mirroring OpenClaw's `resolveConfigPath`: the
/// `OPENCLAW_CONFIG_PATH` override (a FULL config-file path, assumed absolute — see
/// the CodeWhale note on why a relative override can't be reconciled across
/// processes) wins; else prefer an existing `openclaw.json` in the state dir, then
/// the legacy `clawdbot.json`, else `openclaw.json` for a fresh install (never
/// shadow a real `clawdbot.json` the gateway still reads).
pub fn default_config_path() -> Result<PathBuf> {
    Ok(resolve_openclaw_config_path(
        io::nonempty_env("OPENCLAW_CONFIG_PATH"),
        openclaw_state_dir()?,
        |p| p.exists(),
    ))
}

/// Pure core for [`default_config_path`] — the override + resolved state dir +
/// existence check injected.
fn resolve_openclaw_config_path(
    config_path_env: Option<String>,
    state_dir: PathBuf,
    exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    if let Some(p) = config_path_env {
        return PathBuf::from(p);
    }
    let modern = state_dir.join("openclaw.json");
    if exists(&modern) {
        return modern;
    }
    let legacy = state_dir.join("clawdbot.json");
    if exists(&legacy) {
        return legacy;
    }
    modern
}

/// The wholly-owned plugin dir: `<state-dir>/plugins/pixtuoid`.
fn plugin_dir() -> Result<PathBuf> {
    Ok(openclaw_state_dir()?.join("plugins").join(PLUGIN_ID))
}

/// Auto-detect probe: is OpenClaw present (its state dir exists), so the
/// Sources panel OFFERS it? Probe OpenClaw's OWN dir, NOT our plugin/config —
/// keying on our artifact would chicken-and-egg (opencode/Reasonix rationale).
/// With `OPENCLAW_STATE_DIR` set that dir IS the state dir; else probe both the
/// modern `.openclaw` and the legacy `.clawdbot` under the effective home.
pub fn detect_installed() -> bool {
    if let Some(d) = io::nonempty_env("OPENCLAW_STATE_DIR") {
        return PathBuf::from(d).exists();
    }
    let home = io::nonempty_env("OPENCLAW_HOME")
        .map(PathBuf::from)
        .or_else(pixtuoid_core::platform::home_first_dir);
    home.is_some_and(|h| h.join(".openclaw").exists() || h.join(".clawdbot").exists())
}

/// The shim's absolute path, baked into the plugin (the gateway runs it under
/// Node, no PATH reliance). Err on non-UTF-8 like opencode/Codex.
pub fn hook_command(resolved: &Path, _explicit: bool) -> Result<String> {
    resolved
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("pixtuoid-hook path is non-UTF-8: {}", resolved.display()))
}

/// The wholly-owned plugin dir files (manifest + package.json + entry module).
/// `extra_artifacts` Target hook: written verbatim on install, shim path baked in.
pub fn plugin_artifacts(hook_path: &Path) -> Result<Vec<(PathBuf, String)>> {
    let dir = plugin_dir()?;
    let hook = hook_path
        .to_str()
        .ok_or_else(|| anyhow!("pixtuoid-hook path is non-UTF-8: {}", hook_path.display()))?;
    Ok(vec![
        (dir.join("openclaw.plugin.json"), MANIFEST.to_string()),
        (dir.join("package.json"), PACKAGE.to_string()),
        (dir.join("index.js"), render_plugin(hook)?),
    ])
}

fn render_plugin(hook_path: &str) -> Result<String> {
    let json = serde_json::to_string(hook_path)
        .context("serializing the hook path into the openclaw plugin")?;
    Ok(PLUGIN_TEMPLATE.replace(HOOK_PLACEHOLDER, &json))
}

fn parse_or_empty(content: &str) -> Result<Value> {
    if content.trim().is_empty() {
        Ok(json!({}))
    } else {
        serde_json::from_str(content).context("parsing openclaw.json")
    }
}

fn obj_mut<'a>(v: &'a mut Value, key: &str) -> Result<&'a mut serde_json::Map<String, Value>> {
    let map = v
        .as_object_mut()
        .ok_or_else(|| anyhow!("openclaw.json: `{key}` is not a JSON object"))?;
    Ok(map)
}

/// Merge our plugin registration into openclaw.json: add `plugins.load.paths`
/// pointing at the plugin dir + `plugins.entries.pixtuoid = {enabled, hooks:
/// {allowConversationAccess}}`. `changed` is a semantic (parsed) diff, so a
/// same-state re-install is a no-op. `_hook_cmd` is unused — the shim path lives
/// in the plugin file (an `extra_artifact`), not the config.
pub fn merge_install(content: &str, _hook_cmd: &str) -> Result<MergeOutcome> {
    let dir = plugin_dir()?;
    let dir_str = dir
        .to_str()
        .ok_or_else(|| anyhow!("plugin dir path is non-UTF-8: {}", dir.display()))?
        .to_string();
    let mut root = parse_or_empty(content)?;
    let before = root.clone();
    {
        let root_obj = obj_mut(&mut root, "root")?;
        let plugins = root_obj.entry("plugins").or_insert_with(|| json!({}));
        let plugins = obj_mut(plugins, "plugins")?;

        let load = plugins.entry("load").or_insert_with(|| json!({}));
        let load = obj_mut(load, "plugins.load")?;
        let paths = load.entry("paths").or_insert_with(|| json!([]));
        let paths = paths
            .as_array_mut()
            .ok_or_else(|| anyhow!("openclaw.json: `plugins.load.paths` is not an array"))?;
        if !paths.iter().any(|p| p.as_str() == Some(dir_str.as_str())) {
            paths.push(json!(dir_str));
        }

        let entries = plugins.entry("entries").or_insert_with(|| json!({}));
        let entries = obj_mut(entries, "plugins.entries")?;
        entries.insert(
            PLUGIN_ID.to_string(),
            json!({ "enabled": true, "hooks": { "allowConversationAccess": true } }),
        );
    }
    let changed = root != before;
    Ok(MergeOutcome {
        changed,
        content: serde_json::to_string_pretty(&root)? + "\n",
    })
}

/// Remove our registration: drop the plugin-dir path from `plugins.load.paths`
/// and REMOVE the `plugins.entries.pixtuoid` subtree (revoking the
/// conversation-access grant — R-P1). A foreign plugin's entries/paths survive.
pub fn merge_uninstall(content: &str) -> Result<MergeOutcome> {
    let dir = plugin_dir()?;
    let dir_str = dir.to_str().map(str::to_string);
    let mut root = parse_or_empty(content)?;
    let before = root.clone();
    if let Some(plugins) = root.get_mut("plugins").and_then(Value::as_object_mut) {
        if let Some(paths) = plugins
            .get_mut("load")
            .and_then(Value::as_object_mut)
            .and_then(|l| l.get_mut("paths"))
            .and_then(Value::as_array_mut)
        {
            paths.retain(|p| p.as_str().map(str::to_string) != dir_str);
        }
        if let Some(entries) = plugins.get_mut("entries").and_then(Value::as_object_mut) {
            entries.remove(PLUGIN_ID);
        }
    }
    let changed = root != before;
    Ok(MergeOutcome {
        changed,
        content: serde_json::to_string_pretty(&root)? + "\n",
    })
}

/// Install-schema check (#314, the "silent-dead source" detector): verify our
/// `openclaw.json` merge is still sound. The shim path lives in the SEPARATE
/// plugin `index.js` (an `extra_artifact`), NOT this config, so the shim ref is
/// `Unknown` — `verify_target` downgrades that to a soft note, false-positive-
/// free. The HARD checks are the two config-level facts only WE write: the
/// enabled `entries.pixtuoid` entry + its `load.paths` dir registration (a
/// removed/disabled entry = the gateway silently never loads us). Per-source
/// format knowledge stays here (invariant #3).
pub fn verify_schema(content: &str) -> crate::install::verify::SchemaParse {
    use crate::install::verify::{SchemaParse, ShimRef};
    let Ok(root) = serde_json::from_str::<Value>(content) else {
        return SchemaParse::broken("openclaw.json is not valid JSON — reconnect openclaw");
    };
    let entry = &root["plugins"]["entries"][PLUGIN_ID];
    if entry.is_null() {
        return SchemaParse::broken(
            "the pixtuoid plugin entry is missing from openclaw.json — reconnect openclaw",
        );
    }
    let mut issues = Vec::new();
    if entry["enabled"] != json!(true) {
        issues.push("the pixtuoid openclaw plugin is installed but disabled".into());
    }
    // `load.paths` must still point at our plugin dir (`…/plugins/pixtuoid`).
    // Separator-tolerant so a Windows backslash path still matches.
    let registered = root["plugins"]["load"]["paths"]
        .as_array()
        .is_some_and(|paths| {
            paths.iter().any(|p| {
                p.as_str().is_some_and(|s| {
                    s.replace('\\', "/")
                        .ends_with(&format!("plugins/{PLUGIN_ID}"))
                })
            })
        });
    if !registered {
        issues
            .push("openclaw.json `load.paths` no longer registers the pixtuoid plugin dir".into());
    }
    SchemaParse {
        issues,
        shim: ShimRef::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openclaw_state_dir_override_wins_outright() {
        // OPENCLAW_STATE_DIR is OpenClaw's own state-dir override (resolveStateDir)
        // — it points AT the dir (no `.openclaw` join) and beats home + override;
        // `exists` is never consulted.
        let p = resolve_openclaw_state_dir(
            Some("/custom/state".into()),
            Some("/ignored/home".into()),
            Some(PathBuf::from("/ignored/oshome")),
            |_| panic!("exists() must not be consulted when OPENCLAW_STATE_DIR is set"),
        )
        .unwrap();
        assert_eq!(p, PathBuf::from("/custom/state"));
    }

    #[test]
    fn openclaw_state_dir_honors_openclaw_home_then_os_home_first() {
        // No state-dir override → OPENCLAW_HOME wins over the OS home (mirrors
        // resolveEffectiveHomeDir honoring OPENCLAW_HOME before OS homes), and the
        // `.openclaw` state dir is joined onto it (no legacy → modern).
        let p = resolve_openclaw_state_dir(
            None,
            Some(r"D:\claw".into()),
            Some(PathBuf::from(r"C:\Users\me")),
            |_| false,
        )
        .unwrap();
        assert_eq!(p, PathBuf::from(r"D:\claw").join(".openclaw"));
        // No OPENCLAW_HOME → the OS HOME-first home (home_first_dir) is used.
        let p =
            resolve_openclaw_state_dir(None, None, Some(PathBuf::from(r"C:\Users\me")), |_| false)
                .unwrap();
        assert_eq!(p, PathBuf::from(r"C:\Users\me").join(".openclaw"));
    }

    #[test]
    fn openclaw_state_dir_prefers_legacy_clawdbot_only_when_modern_absent() {
        // Mirror resolveStateDir's legacy fallback: .openclaw wins when it exists;
        // .clawdbot is used ONLY when .openclaw is absent and .clawdbot exists; else
        // a fresh install lands in .openclaw (never shadow a real .clawdbot).
        let home = PathBuf::from("/home/u");
        let modern = home.join(".openclaw");
        let legacy = home.join(".clawdbot");
        // .openclaw exists → .openclaw (even if .clawdbot also exists).
        let p =
            resolve_openclaw_state_dir(None, None, Some(home.clone()), |q| q == modern).unwrap();
        assert_eq!(p, modern);
        // only .clawdbot exists → .clawdbot.
        let p =
            resolve_openclaw_state_dir(None, None, Some(home.clone()), |q| q == legacy).unwrap();
        assert_eq!(p, legacy);
        // neither exists → .openclaw (fresh install).
        let p = resolve_openclaw_state_dir(None, None, Some(home), |_| false).unwrap();
        assert_eq!(p, modern);
    }

    #[test]
    fn openclaw_config_path_override_and_legacy_file_preference() {
        let state = PathBuf::from("/home/u/.openclaw");
        let modern = state.join("openclaw.json");
        let legacy = state.join("clawdbot.json");
        // OPENCLAW_CONFIG_PATH wins verbatim — exists() never consulted.
        let p = resolve_openclaw_config_path(Some("/custom/oc.json".into()), state.clone(), |_| {
            panic!("exists() must not be consulted when OPENCLAW_CONFIG_PATH is set")
        });
        assert_eq!(p, PathBuf::from("/custom/oc.json"));
        // No override: prefer existing openclaw.json, then legacy clawdbot.json,
        // else openclaw.json for a fresh install.
        assert_eq!(
            resolve_openclaw_config_path(None, state.clone(), |q| q == modern),
            modern
        );
        assert_eq!(
            resolve_openclaw_config_path(None, state.clone(), |q| q == legacy),
            legacy
        );
        assert_eq!(resolve_openclaw_config_path(None, state, |_| false), modern);
    }

    #[test]
    fn openclaw_state_dir_errors_when_nothing_resolves() {
        // No override, no OPENCLAW_HOME, and home_first_dir returned None (no
        // HOME/USERPROFILE) → the actionable "pass --config" error, like the other
        // home-anchored targets.
        let err = resolve_openclaw_state_dir(None, None, None, |_| false).unwrap_err();
        assert!(
            err.to_string().contains("pass --config"),
            "unresolvable home must surface the actionable error: {err}"
        );
    }

    /// Internal drift defense (#3): the events we REGISTER (the plugin's HOOKS
    /// array) must equal the events we DECODE (`decode_openclaw_hook_payload`
    /// arms) must equal `OPENCLAW_EVENTS`. A registered-but-undecoded (or vice
    /// versa) event — the class that bit Codex's SubagentStop — fails here at
    /// `cargo test`, no network needed.
    #[test]
    fn openclaw_events_plugin_decoder_and_const_agree() {
        use pixtuoid_core::source::openclaw::decode_openclaw_hook_payload;
        // 1) Every const event has a plugin HOOKS registration.
        for ev in OPENCLAW_EVENTS {
            assert!(
                PLUGIN_TEMPLATE.contains(&format!("\"{ev}\"")),
                "plugin HOOKS is missing the registered event `{ev}`"
            );
        }
        // 2) The plugin registers EXACTLY the const set (no extra/stale name).
        let hooks_block = PLUGIN_TEMPLATE
            .split_once("const HOOKS = [")
            .and_then(|(_, rest)| rest.split_once("];"))
            .map(|(inner, _)| inner)
            .expect("plugin defines a HOOKS array");
        let registered: std::collections::HashSet<&str> = hooks_block
            .split(',')
            .map(|s| s.trim().trim_matches('"'))
            .filter(|s| !s.is_empty())
            .collect();
        let expected: std::collections::HashSet<&str> = OPENCLAW_EVENTS.iter().copied().collect();
        assert_eq!(
            registered, expected,
            "plugin HOOKS drifted from OPENCLAW_EVENTS"
        );
        // 3) Every const event has a decoder arm (non-empty presence update).
        for ev in OPENCLAW_EVENTS {
            let payload = json!({ "type": ev });
            let updates = decode_openclaw_hook_payload(&payload).unwrap();
            assert!(
                !updates.is_empty(),
                "decode_openclaw_hook_payload has no arm for registered event `{ev}`"
            );
        }
    }

    #[test]
    fn install_renders_plugin_with_baked_shim_path_and_sentinel() {
        let arts = plugin_artifacts(Path::new("/opt/bin/pixtuoid-hook")).unwrap();
        assert_eq!(arts.len(), 3, "manifest + package.json + index.js");
        let index = &arts
            .iter()
            .find(|(p, _)| p.ends_with("index.js"))
            .unwrap()
            .1;
        assert!(
            index.contains(SENTINEL),
            "entry module carries the sentinel"
        );
        assert!(
            index.contains("\"/opt/bin/pixtuoid-hook\""),
            "shim path baked JSON-escaped"
        );
        assert!(!index.contains(HOOK_PLACEHOLDER), "placeholder replaced");
        assert!(
            index.contains("--source"),
            "spawns the shim with --source openclaw"
        );
    }

    #[test]
    fn merge_install_adds_load_path_enabled_and_the_grant() {
        let out = merge_install("{}", "/opt/bin/pixtuoid-hook").unwrap();
        assert!(out.changed);
        let v: Value = serde_json::from_str(&out.content).unwrap();
        let entry = &v["plugins"]["entries"]["pixtuoid"];
        assert_eq!(entry["enabled"], json!(true));
        assert_eq!(
            entry["hooks"]["allowConversationAccess"],
            json!(true),
            "the busy-tell grant"
        );
        let paths = v["plugins"]["load"]["paths"].as_array().unwrap();
        assert!(
            paths.iter().any(|p| {
                // Separator-tolerant: the dir is built with the OS separator, so on
                // Windows the path ends `plugins\pixtuoid` (the merge writes the
                // native form; verify_schema normalizes it the same way).
                p.as_str()
                    .unwrap()
                    .replace('\\', "/")
                    .ends_with("plugins/pixtuoid")
            }),
            "load.paths points at the plugin dir"
        );
    }

    #[test]
    fn merge_install_is_idempotent() {
        let a = merge_install("{}", "/x").unwrap();
        let b = merge_install(&a.content, "/x").unwrap();
        assert!(!b.changed, "re-install of the same state is a no-op");
    }

    #[test]
    fn merge_install_preserves_foreign_config() {
        let foreign = r#"{"gateway":{"mode":"local"},"plugins":{"entries":{"anthropic":{"enabled":true}},"load":{"paths":["/some/other/plugin"]}}}"#;
        let out = merge_install(foreign, "/x").unwrap();
        let v: Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(v["gateway"]["mode"], json!("local"), "foreign keys survive");
        assert_eq!(v["plugins"]["entries"]["anthropic"]["enabled"], json!(true));
        let paths = v["plugins"]["load"]["paths"].as_array().unwrap();
        assert!(
            paths
                .iter()
                .any(|p| p.as_str() == Some("/some/other/plugin")),
            "foreign path kept"
        );
        assert_eq!(paths.len(), 2, "ours appended, foreign kept");
    }

    #[test]
    fn uninstall_revokes_the_grant_but_keeps_foreign_entries() {
        let installed = merge_install(
            r#"{"plugins":{"entries":{"anthropic":{"enabled":true}}}}"#,
            "/x",
        )
        .unwrap();
        let removed = merge_uninstall(&installed.content).unwrap();
        assert!(removed.changed);
        let v: Value = serde_json::from_str(&removed.content).unwrap();
        assert!(
            v["plugins"]["entries"].get("pixtuoid").is_none(),
            "our entry (incl. the conversation-access grant) is revoked"
        );
        assert_eq!(
            v["plugins"]["entries"]["anthropic"]["enabled"],
            json!(true),
            "a foreign plugin's grant survives"
        );
        let paths = v["plugins"]["load"]["paths"].as_array().unwrap();
        assert!(
            !paths
                .iter()
                .any(|p| p.as_str().unwrap().ends_with("plugins/pixtuoid")),
            "our load.path removed"
        );
    }

    #[test]
    fn uninstall_of_unmanaged_config_is_a_no_op() {
        assert!(!merge_uninstall("{}").unwrap().changed);
        assert!(!merge_uninstall("").unwrap().changed);
        assert!(
            !merge_uninstall(r#"{"gateway":{"mode":"local"}}"#)
                .unwrap()
                .changed
        );
    }

    #[test]
    fn install_then_uninstall_round_trips() {
        let installed = merge_install("{}", "/x").unwrap();
        let removed = merge_uninstall(&installed.content).unwrap();
        let v: Value = serde_json::from_str(&removed.content).unwrap();
        assert!(v["plugins"]["entries"].get("pixtuoid").is_none());
    }

    #[test]
    fn empty_content_is_treated_as_empty_document() {
        let out = merge_install("", "/x").unwrap();
        assert!(out.changed);
        assert!(serde_json::from_str::<Value>(&out.content).is_ok());
    }

    #[test]
    fn hook_command_returns_absolute_path() {
        assert_eq!(
            hook_command(Path::new("/opt/bin/pixtuoid-hook"), false).unwrap(),
            "/opt/bin/pixtuoid-hook"
        );
    }
}
