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
//! Capture-confirmed: `openclaw plugins install --link <dir>` +
//! `enable` writes EXACTLY those config keys to openclaw.json (no separate
//! registry), so the install is a pure `ConfigLock` write — no subprocess. The
//! `allowConversationAccess` grant un-gates `before_agent_run`/`agent_end` (the
//! busy tell); UNINSTALL REVOKES it (removes our `entries.pixtuoid` subtree) so a
//! disconnect leaves no standing conversation-access grant. The plugin files are
//! left in place on uninstall (the config un-merge stops the gateway loading
//! them) — an accepted residual like opencode's stub.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::install::io;
use crate::install::target::MergeOutcome;

/// The plugin id — the key under `plugins.entries` and `plugins.load.paths`'s dir.
const PLUGIN_ID: &str = "pixtuoid";
/// First-line marker in the rendered entry module (provenance; not load-bearing
/// for detection, which keys on OpenClaw's own dirs). Only the test suite reads
/// it, so it's test-only.
#[cfg(test)]
const SENTINEL: &str = "@pixtuoid-openclaw-plugin";
/// Placeholder for the baked shim path in the bundled entry module.
const HOOK_PLACEHOLDER: &str = "\"{{HOOK_PATH_JSON}}\"";
const PLUGIN_TEMPLATE: &str = include_str!("openclaw_plugin.js");

/// The OpenClaw gateway hook events pixtuoid depends on — the SINGLE source of
/// truth, pinned to BOTH the plugin's `HOOKS` array (what we register) AND the
/// `decode_openclaw_hook_payload` arms (what we decode) by the consistency test
/// below, and the list `check_upstream_drift.py` reads for the CI upstream watch.
/// A rename upstream makes that hook silently stop firing (the plugin registers
/// by name), so this is the drift surface to watch (defense #4).
// Test-gated: no prod-Rust caller — its only readers are the consistency test
// below and `check_upstream_drift.py`'s source-text parse (cfg-agnostic).
#[cfg(test)]
pub(crate) const OPENCLAW_EVENTS: &[&str] = &[
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
    // OpenClaw `~`-expands OPENCLAW_STATE_DIR + OPENCLAW_HOME against its OS home
    // (resolveRawHomeDir/resolveUserPath, #342), so mirror that before the path
    // logic; the same `home_first_dir()` is both the expansion base and the OS-home
    // fallback.
    let home = pixtuoid_core::platform::home_first_dir();
    resolve_openclaw_state_dir(
        io::nonempty_env("OPENCLAW_STATE_DIR").map(|v| io::expand_tilde(&v, home.as_deref())),
        io::nonempty_env("OPENCLAW_HOME").map(|v| io::expand_tilde(&v, home.as_deref())),
        home,
        |p| p.exists(),
    )
}

/// Pure core for [`openclaw_state_dir`] — every env input, the resolved OS home,
/// and the existence check are injected so the precedence is unit-testable without
/// env/FS mutation.
fn resolve_openclaw_state_dir(
    state_dir_env: Option<PathBuf>,
    openclaw_home_env: Option<PathBuf>,
    os_home_first: Option<PathBuf>,
    exists: impl Fn(&Path) -> bool,
) -> Result<PathBuf> {
    if let Some(d) = state_dir_env {
        return Ok(d);
    }
    let home = openclaw_home_env.or(os_home_first).ok_or_else(|| {
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
/// processes) wins; else the first EXISTING of the four modern/legacy
/// dir × file candidates; else `<modern-dir>/openclaw.json` for a fresh install
/// (never shadow a real config the gateway still reads).
pub(crate) fn default_config_path() -> Result<PathBuf> {
    // OPENCLAW_CONFIG_PATH is `~`-expanded too (resolveUserPath, #342).
    let home = pixtuoid_core::platform::home_first_dir();
    let state_env = io::nonempty_env("OPENCLAW_STATE_DIR");
    // The home whose `.openclaw`/`.clawdbot` pair may hold the config. An explicit
    // OPENCLAW_STATE_DIR points AT the dir and bypasses home resolution entirely,
    // so it gets NO sibling search (the operator named the scope); otherwise the
    // effective home is OPENCLAW_HOME-then-OS-home, exactly as the state dir's own
    // resolution derives it.
    let legacy_home = match state_env {
        Some(_) => None,
        None => io::nonempty_env("OPENCLAW_HOME")
            .map(|v| io::expand_tilde(&v, home.as_deref()))
            .or_else(|| home.clone()),
    };
    Ok(resolve_openclaw_config_path(
        io::nonempty_env("OPENCLAW_CONFIG_PATH").map(|v| io::expand_tilde(&v, home.as_deref())),
        openclaw_state_dir()?,
        legacy_home,
        |p| p.exists(),
    ))
}

/// Pure core for [`default_config_path`] — the override, the resolved state dir,
/// the OS home and the existence check injected.
///
/// The candidate list is FLAT across both dirs, not "pick a dir, then pick a file
/// in it" (upstream searches the same way). The nested form had a real hole: with
/// `~/.openclaw` PRESENT (so it wins as the state dir) but the actual config at
/// `~/.clawdbot/openclaw.json`, we resolved a path the gateway never reads —
/// installing hooks into a file nobody loads, with `doctor` reporting green. The
/// `state_dir` stays first in the list so an `OPENCLAW_STATE_DIR`/`OPENCLAW_HOME`
/// override still outranks the legacy dir; `home` only contributes the legacy
/// SIBLING (`None` ⇒ just the state dir's two candidates).
fn resolve_openclaw_config_path(
    config_path_env: Option<PathBuf>,
    state_dir: PathBuf,
    home: Option<PathBuf>,
    exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    if let Some(p) = config_path_env {
        return p;
    }
    // Modern file before legacy file WITHIN a dir; the resolved state dir before
    // the legacy sibling ACROSS dirs.
    let mut dirs = vec![state_dir.clone()];
    if let Some(h) = home {
        for legacy_dir in [h.join(".openclaw"), h.join(".clawdbot")] {
            if legacy_dir != state_dir {
                dirs.push(legacy_dir);
            }
        }
    }
    for dir in &dirs {
        for file in ["openclaw.json", "clawdbot.json"] {
            let cand = dir.join(file);
            if exists(&cand) {
                return cand;
            }
        }
    }
    state_dir.join("openclaw.json")
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
pub(crate) fn detect_installed() -> bool {
    // Normalize the SAME env vars the SAME way `openclaw_state_dir()` does (#342/#344):
    // `~`-expand `OPENCLAW_STATE_DIR`/`OPENCLAW_HOME` against the same home base. Without
    // this, a `~`-prefixed override would install into the EXPANDED dir but probe the
    // literal `~/…` → `false` → the Sources panel never offers the OpenClaw it just
    // installed into (the install/detect asymmetry).
    let home = pixtuoid_core::platform::home_first_dir();
    resolve_openclaw_detect(
        io::nonempty_env("OPENCLAW_STATE_DIR").map(|v| io::expand_tilde(&v, home.as_deref())),
        io::nonempty_env("OPENCLAW_HOME").map(|v| io::expand_tilde(&v, home.as_deref())),
        home,
        |p| p.exists(),
    )
}

/// Pure core for [`detect_installed`] — parallels [`resolve_openclaw_state_dir`] but
/// answers "does ANY OpenClaw state dir exist" (a presence PROBE) instead of picking
/// one: `OPENCLAW_STATE_DIR` points AT the dir; else probe both `.openclaw` and the
/// legacy `.clawdbot` under the effective home (`OPENCLAW_HOME` override else the OS
/// home). Inputs are injected so the precedence is unit-testable without env/FS.
fn resolve_openclaw_detect(
    state_dir_env: Option<PathBuf>,
    openclaw_home_env: Option<PathBuf>,
    os_home_first: Option<PathBuf>,
    exists: impl Fn(&Path) -> bool,
) -> bool {
    if let Some(d) = state_dir_env {
        return exists(&d);
    }
    let Some(home) = openclaw_home_env.or(os_home_first) else {
        return false;
    };
    exists(&home.join(".openclaw")) || exists(&home.join(".clawdbot"))
}

/// The shim's absolute path, baked into the plugin (the gateway runs it under
/// Node, no PATH reliance). Err on non-UTF-8 like opencode/Codex.
pub(crate) fn hook_command(resolved: &Path, _explicit: bool) -> Result<String> {
    crate::install::merge::hook_path_str(resolved).map(str::to_string)
}

/// The wholly-owned plugin dir files (manifest + package.json + entry module).
/// `extra_artifacts` Target hook: written verbatim on install, shim path baked in.
pub(crate) fn plugin_artifacts(hook_path: &Path) -> Result<Vec<(PathBuf, String)>> {
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
    crate::install::merge::bake_hook_path(PLUGIN_TEMPLATE, HOOK_PLACEHOLDER, hook_path, "openclaw")
}

/// The one-line advice both the JSON5 refusal and the `plugins.allow` note point
/// at — OpenClaw's OWN commands write exactly the keys our merge does.
const OWNER_CLI_ADVICE: &str = "register it with OpenClaw's own CLI instead: \
     `openclaw plugins install --link <dir>` + `openclaw plugins enable pixtuoid`, \
     then set plugins.entries.pixtuoid.hooks.allowConversationAccess = true";

/// The key OpenClaw's config loader uses to pull in another file
/// (`config/includes.ts`) — its presence means the EFFECTIVE `plugins` block may
/// not be the one in this document.
const INCLUDE_KEY: &str = "$include";

/// Parse `openclaw.json` for a MERGE — strict JSON only, with the honest reason.
///
/// OpenClaw reads its own config with **JSON5** (unconditionally — `config/
/// io.load.ts`), so comments, trailing commas, single quotes and unquoted keys are
/// all LEGAL on disk even though OpenClaw's own writer emits strict JSON. Our
/// read→merge→write round-trip re-serializes through `serde_json`, which cannot
/// represent any of that: parsing it would mean silently DELETING the user's
/// comments on their next `connect`. So a non-strict document is refused with the
/// owner-CLI path instead of being rewritten — the same "never destroy the user's
/// config" rule as CodeWhale's untouched `enabled = false`.
fn parse_for_merge(content: &str) -> Result<Value> {
    if content.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(content).map_err(|e| {
        anyhow!(
            "openclaw.json is not strict JSON ({e}). OpenClaw parses it as JSON5, so \
             comments/trailing commas/single quotes are legal there and pixtuoid will not \
             rewrite the file rather than drop them — {OWNER_CLI_ADVICE}"
        )
    })
}

/// Drop `key` from `parent` when it is an EMPTY object/array — the uninstall's
/// husk sweeper. Anything with content (a foreign plugin's entry, another load
/// path, the user's own allowlist members) is left exactly as found.
fn prune_empty(parent: &mut serde_json::Map<String, Value>, key: &str) {
    let empty = match parent.get(key) {
        Some(Value::Object(m)) => m.is_empty(),
        Some(Value::Array(a)) => a.is_empty(),
        _ => false,
    };
    if empty {
        parent.remove(key);
    }
}

/// [`prune_empty`] for a top-level key of the document root.
fn prune_empty_root(root: &mut Value, key: &str) {
    if let Some(obj) = root.as_object_mut() {
        prune_empty(obj, key);
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
pub(crate) fn merge_install(content: &str, _hook_cmd: &str) -> Result<MergeOutcome> {
    let dir = plugin_dir()?;
    let dir_str = dir
        .to_str()
        .ok_or_else(|| anyhow!("plugin dir path is non-UTF-8: {}", dir.display()))?
        .to_string();
    let mut root = parse_for_merge(content)?;
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

        // `plugins.allow` is FAIL-CLOSED upstream: when the user curates an
        // allowlist, a plugin absent from it never loads however enabled its entry
        // is. Join a NON-EMPTY list so the install isn't silently inert. An EMPTY
        // `allow: []` is the user's own "no plugins at all" switch and is left
        // untouched (CodeWhale's `enabled = false` precedent) — `verify_schema`
        // reports that as the reason nothing loads instead.
        if let Some(allow) = plugins.get_mut("allow").and_then(Value::as_array_mut) {
            if !allow.is_empty() && !allow.iter().any(|v| v.as_str() == Some(PLUGIN_ID)) {
                allow.push(json!(PLUGIN_ID));
            }
        }
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
pub(crate) fn merge_uninstall(content: &str) -> Result<MergeOutcome> {
    let dir = plugin_dir()?;
    let dir_str = dir.to_str().map(str::to_string);
    let mut root = parse_for_merge(content)?;
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
        if let Some(allow) = plugins.get_mut("allow").and_then(Value::as_array_mut) {
            allow.retain(|v| v.as_str() != Some(PLUGIN_ID));
        }
        // PRUNE the containers that are now OURS-ONLY-AND-EMPTY, so a disconnect
        // leaves OpenClaw's config as it found it instead of a husk
        // `plugins: { entries: {}, load: { paths: [] } }` (the flat-JSON targets
        // already prune — `merge::flat_json_merge_uninstall`). A container the user
        // has anything else in is never touched.
        prune_empty(plugins, "entries");
        if let Some(load) = plugins.get_mut("load").and_then(Value::as_object_mut) {
            prune_empty(load, "paths");
        }
        prune_empty(plugins, "load");
        prune_empty(plugins, "allow");
    }
    prune_empty_root(&mut root, "plugins");
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
pub(crate) fn verify_schema(content: &str) -> crate::install::verify::SchemaParse {
    use crate::install::verify::{SchemaParse, ShimRef};
    let Ok(root) = serde_json::from_str::<Value>(content) else {
        // NOT "broken": OpenClaw reads this file as JSON5, so a document our strict
        // parser rejects can be perfectly valid and LOADING FINE (comments, trailing
        // commas). Reporting a hard break here told the user to "reconnect openclaw"
        // — advice that cannot succeed, because the merge refuses the same document
        // by design. Report the truth (we cannot verify) and the path that works.
        return SchemaParse {
            issues: vec![],
            notes: vec![format!(
                "openclaw.json is not strict JSON (OpenClaw reads it as JSON5), so pixtuoid \
                 cannot verify the plugin registration — {OWNER_CLI_ADVICE}"
            )],
            shim: ShimRef::Unknown,
        };
    };
    let entry = &root["plugins"]["entries"][PLUGIN_ID];
    if entry.is_null() {
        return SchemaParse::broken(
            "the pixtuoid plugin entry is missing from openclaw.json — reconnect openclaw",
        );
    }
    let mut issues = Vec::new();
    let mut notes = Vec::new();
    if entry["enabled"] != json!(true) {
        issues.push("the pixtuoid openclaw plugin is installed but disabled".into());
    }
    // `plugins.allow` is FAIL-CLOSED upstream: with a curated allowlist, a plugin
    // absent from it never loads however enabled its entry is — the silent-dead
    // class again, invisible to the entry/paths checks above.
    match root["plugins"]["allow"].as_array() {
        Some(allow) if allow.is_empty() => notes.push(
            "openclaw.json `plugins.allow` is empty — OpenClaw loads NO plugin (your own \
             switch; pixtuoid leaves it untouched)"
                .into(),
        ),
        Some(allow) if !allow.iter().any(|v| v.as_str() == Some(PLUGIN_ID)) => issues.push(
            "openclaw.json `plugins.allow` does not list pixtuoid — the allowlist is \
             fail-closed, so the plugin never loads"
                .into(),
        ),
        _ => {}
    }
    // An `$include` means the EFFECTIVE plugins block may come from another file,
    // so a sound-looking document here is not proof the gateway sees it.
    if root.get(INCLUDE_KEY).is_some() {
        notes.push(format!(
            "openclaw.json uses `{INCLUDE_KEY}` — the effective plugins config may come from \
             an included file, which pixtuoid does not read"
        ));
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
        notes,
        // The shim path lives in the SEPARATE plugin entry module (an
        // `extra_artifact`), so it is read + stat'd by `verify_target` instead —
        // see its baked-shim check.
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
        let home = PathBuf::from("/home/u");
        let state = home.join(".openclaw");
        let modern = state.join("openclaw.json");
        let legacy = state.join("clawdbot.json");
        let h = || Some(home.clone());
        // OPENCLAW_CONFIG_PATH wins verbatim — exists() never consulted.
        let p = resolve_openclaw_config_path(
            Some("/custom/oc.json".into()),
            state.clone(),
            h(),
            |_| panic!("exists() must not be consulted when OPENCLAW_CONFIG_PATH is set"),
        );
        assert_eq!(p, PathBuf::from("/custom/oc.json"));
        // No override: prefer existing openclaw.json, then legacy clawdbot.json,
        // else openclaw.json for a fresh install.
        assert_eq!(
            resolve_openclaw_config_path(None, state.clone(), h(), |q| q == modern),
            modern
        );
        assert_eq!(
            resolve_openclaw_config_path(None, state.clone(), h(), |q| q == legacy),
            legacy
        );
        assert_eq!(
            resolve_openclaw_config_path(None, state.clone(), h(), |_| false),
            modern
        );
    }

    #[test]
    fn openclaw_config_path_finds_a_config_in_the_legacy_dir_sibling() {
        // The hole the flat candidate list closes: `~/.openclaw` EXISTS (so it wins
        // as the state dir) but the real config lives in the legacy dir. Resolving
        // `<state>/openclaw.json` there installs hooks into a file the gateway never
        // reads — silently no lobster, with a GREEN doctor.
        let home = PathBuf::from("/home/u");
        let state = home.join(".openclaw"); // present, but holds no config
        let real = home.join(".clawdbot").join("openclaw.json");
        assert_eq!(
            resolve_openclaw_config_path(None, state.clone(), Some(home.clone()), |q| q == real),
            real,
            "the legacy dir's config must win over a config-less modern dir"
        );
        // …and the modern dir still wins when BOTH exist (never demote a real one).
        let modern = state.join("openclaw.json");
        assert_eq!(
            resolve_openclaw_config_path(None, state.clone(), Some(home.clone()), |q| q == real
                || q == modern),
            modern
        );
        // An explicit state-dir override searches NO sibling (`legacy_home: None`,
        // which is what `default_config_path` passes for OPENCLAW_STATE_DIR): the
        // operator named the scope, so the fresh-install path stays inside it.
        let overridden = PathBuf::from("/custom/state");
        assert_eq!(
            resolve_openclaw_config_path(None, overridden.clone(), None, |q| q == real),
            overridden.join("openclaw.json"),
            "an overridden state dir outranks the legacy home sibling"
        );
    }

    #[test]
    fn openclaw_detect_probes_the_same_resolved_dirs_as_install() {
        // The detect probe must agree with openclaw_state_dir()'s resolution (#344):
        // an env override (already `~`-expanded at the call site, like the write path)
        // is probed at the EXPANDED location, never the literal `~/…`.
        let home = PathBuf::from("/home/u");
        // OPENCLAW_STATE_DIR points AT the dir → probed directly, home ignored.
        assert!(resolve_openclaw_detect(
            Some(home.join("claw")),
            None,
            None,
            |q| q == home.join("claw"),
        ));
        assert!(!resolve_openclaw_detect(
            Some(home.join("claw")),
            None,
            None,
            |_| false
        ));
        // No state-dir override: OPENCLAW_HOME wins over the OS home, and BOTH the
        // modern `.openclaw` and the legacy `.clawdbot` are probed under it.
        let claw_home = PathBuf::from("/expanded/claw");
        assert!(resolve_openclaw_detect(
            None,
            Some(claw_home.clone()),
            Some(home.clone()),
            |q| q == claw_home.join(".clawdbot"),
        ));
        // OPENCLAW_HOME unset → the OS HOME-first home is probed.
        assert!(resolve_openclaw_detect(
            None,
            None,
            Some(home.clone()),
            |q| q == home.join(".openclaw")
        ));
        // Nothing resolves (no home at all) → not present, and `exists` is never
        // consulted (no panic).
        assert!(!resolve_openclaw_detect(None, None, None, |_| panic!(
            "exists() must not be consulted when no home resolves"
        )));
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
        // 3) Every const event has a decoder arm (non-empty presence update), and
        // carries the gateway identity the plugin stamps on every forwarded hook.
        for ev in OPENCLAW_EVENTS {
            let payload = json!({ "type": ev, "gatewayPort": 18789 });
            let decoded = decode_openclaw_hook_payload(&payload).unwrap();
            assert!(
                !decoded.updates.is_empty(),
                "decode_openclaw_hook_payload has no arm for registered event `{ev}`"
            );
            assert_eq!(
                decoded.instance.as_str(),
                "18789",
                "`{ev}` must resolve its sending gateway, not fall back"
            );
        }
    }

    #[test]
    fn install_renders_plugin_with_baked_shim_path_and_sentinel() {
        // Resolves the OpenClaw state dir from HOME/USERPROFILE — serialize
        // against config.rs's env-mutating tests (they null both in a window that
        // would else make home_first_dir() return None → unwrap panic under plain
        // `cargo test`; nextest's per-process isolation masks it).
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
    fn rendered_hook_binding_round_trips_escaped_paths() {
        for path in [
            r"C:\Program Files\Pixtuoid\pixtuoid-hook.exe",
            r#"/tmp/"quoted"/pixtuoid-hook"#,
        ] {
            let rendered = render_plugin(path).unwrap();
            let binding = rendered
                .lines()
                .find(|line| line.starts_with("const HOOK_PATH = "))
                .unwrap();
            let encoded = binding
                .strip_prefix("const HOOK_PATH = ")
                .unwrap()
                .strip_suffix(';')
                .unwrap();
            let expected_json = serde_json::to_string(path).unwrap();
            assert_eq!(binding, format!("const HOOK_PATH = {expected_json};"));
            assert_eq!(serde_json::from_str::<String>(encoded).unwrap(), path);
        }
    }

    #[test]
    fn merge_install_adds_load_path_enabled_and_the_grant() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let a = merge_install("{}", "/x").unwrap();
        let b = merge_install(&a.content, "/x").unwrap();
        assert!(!b.changed, "re-install of the same state is a no-op");
    }

    #[test]
    fn merge_install_preserves_foreign_config() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
    fn install_joins_a_curated_allowlist_but_never_an_empty_one() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // A CURATED allowlist is fail-closed upstream: absent from it, our plugin
        // never loads however enabled its entry is. Join it.
        let curated = merge_install(r#"{"plugins":{"allow":["anthropic"]}}"#, "").unwrap();
        let v: Value = serde_json::from_str(&curated.content).unwrap();
        let allow = v["plugins"]["allow"].as_array().unwrap();
        assert!(
            allow.iter().any(|x| x == "pixtuoid") && allow.iter().any(|x| x == "anthropic"),
            "join the allowlist without evicting the user's own ids: {allow:?}"
        );
        // Idempotent — a re-install neither duplicates nor reports a change.
        let again = merge_install(&curated.content, "").unwrap();
        assert!(!again.changed, "a re-install is a semantic no-op");

        // An EMPTY `allow: []` is the user's own "no plugins at all" switch (the
        // CodeWhale `enabled = false` precedent) — untouched, and reported instead.
        let empty = merge_install(r#"{"plugins":{"allow":[]}}"#, "").unwrap();
        let v: Value = serde_json::from_str(&empty.content).unwrap();
        assert_eq!(
            v["plugins"]["allow"].as_array().map(Vec::len),
            Some(0),
            "an explicit allow-nothing switch must not be flipped for us"
        );
        let verdict = verify_schema(&empty.content);
        assert!(
            verdict.issues.is_empty(),
            "the user's switch is not OUR break"
        );
        assert!(
            verdict.notes.iter().any(|n| n.contains("loads NO plugin")),
            "…but it IS why nothing loads: {:?}",
            verdict.notes
        );
    }

    #[test]
    fn verify_flags_an_allowlist_that_omits_us_and_notes_json5_and_include() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let installed = merge_install("{}", "").unwrap();
        assert!(verify_schema(&installed.content).issues.is_empty());

        // A curated allowlist that does NOT list us: enabled entry, registered path,
        // and the gateway still never loads the plugin → HARD.
        let mut v: Value = serde_json::from_str(&installed.content).unwrap();
        v["plugins"]["allow"] = json!(["anthropic"]);
        let verdict = verify_schema(&v.to_string());
        assert!(
            verdict
                .issues
                .iter()
                .any(|i| i.contains("`plugins.allow` does not list pixtuoid")),
            "a fail-closed allowlist must be a HARD issue: {:?}",
            verdict.issues
        );

        // JSON5 (what OpenClaw actually parses) is NOT a break — we simply cannot
        // verify it, and "reconnect openclaw" would be advice that cannot succeed.
        let json5 = "{\n  // the user's note\n  \"plugins\": {},\n}\n";
        let verdict = verify_schema(json5);
        assert!(
            verdict.issues.is_empty(),
            "a JSON5 config is legal upstream — never a hard break: {:?}",
            verdict.issues
        );
        assert!(
            verdict.notes.iter().any(|n| n.contains("JSON5")),
            "…but it must say why it could not be verified: {:?}",
            verdict.notes
        );

        // `$include` means the effective plugins block may live in another file.
        let mut v: Value = serde_json::from_str(&installed.content).unwrap();
        v["$include"] = json!("./extra.json");
        let verdict = verify_schema(&v.to_string());
        assert!(
            verdict.notes.iter().any(|n| n.contains("$include")),
            "an include must be surfaced: {:?}",
            verdict.notes
        );
    }

    #[test]
    fn uninstall_prunes_its_own_husk_but_keeps_anything_foreign() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Install into an EMPTY config, then uninstall: OpenClaw's config must come
        // back to `{}` rather than keeping a husk `plugins:{entries:{},load:{paths:[]}}`
        // that we alone created.
        let installed = merge_install("{}", "").unwrap();
        let removed = merge_uninstall(&installed.content).unwrap();
        assert!(removed.changed);
        let v: Value = serde_json::from_str(&removed.content).unwrap();
        assert_eq!(v, json!({}), "no husk left behind, got {v}");

        // With a foreign plugin present, its containers survive untouched.
        let shared = merge_install(
            r#"{"plugins":{"entries":{"anthropic":{"enabled":true}}}}"#,
            "",
        )
        .unwrap();
        let removed = merge_uninstall(&shared.content).unwrap();
        let v: Value = serde_json::from_str(&removed.content).unwrap();
        assert_eq!(v["plugins"]["entries"]["anthropic"]["enabled"], json!(true));
        assert!(v["plugins"]["entries"].get("pixtuoid").is_none());
    }

    #[test]
    fn uninstall_revokes_the_grant_but_keeps_foreign_entries() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
        // Our path is gone. `load.paths` here held ONLY ours, so the uninstall
        // prunes the emptied array (and its `load` container) rather than leaving a
        // husk in OpenClaw's config — either shape satisfies "our path removed".
        let paths = v["plugins"]["load"]["paths"].as_array();
        assert!(
            paths.is_none_or(|ps| !ps
                .iter()
                .any(|p| p.as_str().is_some_and(|s| s.ends_with("plugins/pixtuoid")))),
            "our load.path removed, got {paths:?}"
        );
    }

    #[test]
    fn uninstall_of_unmanaged_config_is_a_no_op() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let installed = merge_install("{}", "/x").unwrap();
        let removed = merge_uninstall(&installed.content).unwrap();
        let v: Value = serde_json::from_str(&removed.content).unwrap();
        assert!(v["plugins"]["entries"].get("pixtuoid").is_none());
    }

    #[test]
    fn empty_content_is_treated_as_empty_document() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
