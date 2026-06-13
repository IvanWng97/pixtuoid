//! opencode hook install target — a TS PLUGIN, not a config block.
//!
//! opencode has no config-level shell hook (and SQLite-only sessions, no
//! tailable transcript), so pixtuoid integrates as an opencode plugin: opencode
//! auto-discovers `<config>/plugins/*.{ts,js}` (the canonical docs' dir; the
//! anomalyco fork's `config/plugin.ts::load` globs `{plugin,plugins}` so both
//! work there, but PLURAL `plugins/` is the documented dir canonical opencode
//! scans), so we DROP a plugin file at `<opencode-config>/plugins/pixtuoid.ts` —
//! no edit to the user's `opencode.jsonc` (no comment-clobber risk). This is the
//! FIRST install target that ships a CODE artifact rather than a config block.
//!
//! The plugin's `event` hook receives the same EventV2 stream the server SSE
//! endpoint serves (dir-scoped, base `type` — `event-v2-bridge.ts`), and pipes
//! the lifecycle/tool/permission events into the `pixtuoid-hook` shim on stdin
//! (`--source opencode`, plain mode). The shim's absolute path is baked into the
//! plugin (JSON-escaped) at install time; the template lives in
//! `opencode_plugin.ts`.
//!
//! The plugin FILE is wholly owned by pixtuoid (not a shared config we merge
//! into), so `merge_install` renders the whole file and `merge_uninstall`
//! replaces it with a sentinel-free no-op stub (`export {}`). ACCEPTED residual:
//! uninstall leaves that ~1-line stub rather than deleting the file — the
//! orchestrator's `write_atomic` can't delete, and the stub is a harmless empty
//! module opencode loads to nothing. `merge_uninstall` keys on the
//! `@pixtuoid-opencode-plugin` sentinel (absent from the stub) to decide
//! `changed`, so a re-install/uninstall round-trip is exact. (`detect_installed`
//! — the auto-detect probe — keys on the opencode CLI's dirs, NOT this sentinel;
//! see its doc.)
//!
//! Config dir resolution mirrors opencode's own (`global.ts`): `OPENCODE_CONFIG_DIR`
//! else `$XDG_CONFIG_HOME/opencode` else `~/.config/opencode`.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use crate::install::io;
use crate::install::target::MergeOutcome;

/// First-line marker in the rendered plugin — `merge_uninstall` keys on it to
/// detect our managed plugin (absent from the removed-stub, so an uninstall of
/// a foreign/removed file is a clean no-op).
const SENTINEL: &str = "@pixtuoid-opencode-plugin";

/// The placeholder the bundled template carries for the baked shim path.
const HOOK_PLACEHOLDER: &str = "{{HOOK_PATH_JSON}}";

/// The bundled plugin source (with the `{{HOOK_PATH_JSON}}` placeholder).
const PLUGIN_TEMPLATE: &str = include_str!("opencode_plugin.ts");

/// Written on uninstall: a valid empty ES module (opencode loads it to zero
/// hooks) WITHOUT the sentinel, so a re-uninstall is a clean no-op.
const REMOVED_STUB: &str = "// pixtuoid opencode plugin removed by `pixtuoid uninstall-hooks --target opencode`.\nexport {}\n";

/// opencode's config dir: `OPENCODE_CONFIG_DIR`, else `$XDG_CONFIG_HOME/opencode`,
/// else `~/.config/opencode` — mirroring opencode `global.ts` so we write into
/// the dir it actually scans for plugins.
fn opencode_config_dir() -> Result<PathBuf> {
    config_dir_from(
        io::nonempty_env("OPENCODE_CONFIG_DIR").as_deref(),
        io::nonempty_env("XDG_CONFIG_HOME").as_deref(),
        io::user_home().as_deref(),
    )
}

/// Pure precedence resolver (testable without env mutation): `OPENCODE_CONFIG_DIR`,
/// then `$XDG_CONFIG_HOME/opencode`, then `<home>/.config/opencode`. Errs only when
/// none resolve (no home) — same contract as the home-anchored targets.
fn config_dir_from(oc: Option<&str>, xdg: Option<&str>, home: Option<&str>) -> Result<PathBuf> {
    if let Some(dir) = oc.filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    if let Some(xdg) = xdg.filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(xdg).join("opencode"));
    }
    home.filter(|s| !s.is_empty())
        .map(|h| PathBuf::from(h).join(".config").join("opencode"))
        .ok_or_else(|| {
            anyhow!(
                "cannot resolve the home directory (HOME/USERPROFILE unset); pass --config <path>"
            )
        })
}

/// The managed plugin file: `<opencode-config>/plugins/pixtuoid.ts`. The dir is
/// `plugins` (PLURAL) — the canonical opencode docs auto-discover
/// `<config>/plugins/*.{ts,js}`; the anomalyco fork globs `{plugin,plugins}` (so
/// both work there), but plural is the documented form and the only one canonical
/// opencode scans, so it's correct for every install.
pub fn default_config_path() -> Result<PathBuf> {
    Ok(opencode_config_dir()?.join("plugins").join("pixtuoid.ts"))
}

/// Presence probe for auto-detect (`is_present`): is the opencode CLI present,
/// so a bare `install-hooks` OFFERS it? Probe opencode's OWN dirs — the config
/// dir we write into (created on first run) and the XDG data dir (the SQLite
/// store) — NOT our plugin file: keying on our own artifact would chicken-and-egg
/// (opencode could never be auto-detected until AFTER we'd installed into it).
/// Mirrors CodeWhale's CLI-dir probe. Uninstall keys on the plugin file existing
/// (`config_present`, file-existence) and `merge_uninstall` on the
/// `@pixtuoid-opencode-plugin` sentinel, so removal stays exact regardless.
pub fn detect_installed() -> bool {
    opencode_config_dir().map(|d| d.exists()).unwrap_or(false)
        || io::home_relative(".local/share/opencode").exists()
}

/// The "command" for opencode is the shim's absolute path, baked into the
/// plugin. opencode runs the plugin under Bun and spawns the shim with that
/// path, so it must be embedded (no PATH reliance) — Err on non-UTF-8 like
/// Codex/Reasonix/CodeWhale. `_explicit` (Claude's bare-vs-absolute switch) is
/// irrelevant: opencode always needs the absolute path.
pub fn hook_command(resolved: &Path, _explicit: bool) -> Result<String> {
    resolved
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("pixtuoid-hook path is non-UTF-8: {}", resolved.display()))
}

/// Render the plugin with the shim path baked in (JSON-encoded → a valid,
/// escaped JS string literal, so a path with quotes/backslashes can't break the
/// module). `changed` is a content diff: a same-path re-install is a no-op.
pub fn merge_install(content: &str, hook_path: &str) -> Result<MergeOutcome> {
    let baked = render_plugin(hook_path)?;
    Ok(MergeOutcome {
        changed: content != baked,
        content: baked,
    })
}

/// Replace our plugin with the sentinel-free no-op stub. `changed` only when the
/// current content is actually ours (carries the sentinel) — a foreign file, an
/// already-removed stub, or empty content is a semantic no-op (left untouched).
pub fn merge_uninstall(content: &str) -> Result<MergeOutcome> {
    let ours = content.contains(SENTINEL);
    Ok(MergeOutcome {
        changed: ours,
        content: if ours {
            REMOVED_STUB.to_string()
        } else {
            content.to_string()
        },
    })
}

fn render_plugin(hook_path: &str) -> Result<String> {
    // serde_json emits a double-quoted, escaped JSON string. JSON strings are a
    // subset of JS string literals EXCEPT U+2028/U+2029 (valid unescaped in JSON,
    // line terminators in JS) — neither occurs in a real filesystem path, so this
    // is a valid JS literal for any path the resolver hands us. Serializing a
    // `&str` is infallible in practice, but propagate the error rather than
    // default to a broken `HOOK_PATH = ""` if it ever weren't.
    let json = serde_json::to_string(hook_path)
        .context("serializing the hook path into the opencode plugin")?;
    Ok(PLUGIN_TEMPLATE.replace(HOOK_PLACEHOLDER, &json))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_bakes_the_hook_path_and_carries_the_sentinel() {
        let out = merge_install("", "/opt/bin/pixtuoid-hook").unwrap();
        assert!(out.changed);
        assert!(
            out.content.contains(SENTINEL),
            "rendered plugin must carry the sentinel"
        );
        // The path is baked as a JSON string literal (quoted), and the
        // placeholder is fully substituted.
        assert!(out.content.contains("\"/opt/bin/pixtuoid-hook\""));
        assert!(
            !out.content.contains(HOOK_PLACEHOLDER),
            "placeholder must be replaced"
        );
        assert!(
            out.content.contains("--source"),
            "spawns the shim with --source opencode"
        );
    }

    #[test]
    fn install_is_idempotent_for_the_same_path() {
        let a = merge_install("", "/opt/bin/pixtuoid-hook").unwrap();
        let b = merge_install(&a.content, "/opt/bin/pixtuoid-hook").unwrap();
        assert!(!b.changed, "same-path re-install is a content no-op");
    }

    #[test]
    fn install_re_renders_on_a_path_change() {
        let a = merge_install("", "/opt/bin/pixtuoid-hook").unwrap();
        let b = merge_install(&a.content, "/usr/local/bin/pixtuoid-hook").unwrap();
        assert!(b.changed);
        assert!(b.content.contains("\"/usr/local/bin/pixtuoid-hook\""));
    }

    #[test]
    fn a_path_with_special_chars_bakes_as_a_valid_escaped_literal() {
        // A backslash / quote in the path must not break the JS string literal.
        let out = merge_install("", r#"/weird/pi"x\hook"#).unwrap();
        // serde_json escapes the quote and backslash.
        assert!(out.content.contains(r#""/weird/pi\"x\\hook""#));
    }

    #[test]
    fn uninstall_replaces_our_plugin_with_a_sentinel_free_stub() {
        let installed = merge_install("", "/opt/bin/pixtuoid-hook").unwrap();
        let removed = merge_uninstall(&installed.content).unwrap();
        assert!(removed.changed);
        assert!(
            !removed.content.contains(SENTINEL),
            "stub must drop the sentinel so detection flips"
        );
        assert!(
            removed.content.contains("export {}"),
            "stub is a valid empty module"
        );
    }

    #[test]
    fn uninstall_of_a_foreign_or_removed_file_is_a_no_op() {
        // A user's own plugin (no sentinel) must not be clobbered.
        let foreign = "export const myPlugin = async () => ({})\n";
        assert!(!merge_uninstall(foreign).unwrap().changed);
        // An already-removed stub is also a no-op (no sentinel).
        assert!(!merge_uninstall(REMOVED_STUB).unwrap().changed);
        // Empty content is a no-op.
        assert!(!merge_uninstall("").unwrap().changed);
    }

    #[test]
    fn install_then_uninstall_round_trips_the_content_sentinel() {
        // After install the content carries the sentinel; after uninstall it
        // doesn't — so merge_uninstall's changed-detection round-trips cleanly.
        let installed = merge_install("", "/opt/bin/pixtuoid-hook").unwrap();
        assert!(installed.content.contains(SENTINEL));
        let removed = merge_uninstall(&installed.content).unwrap();
        assert!(!removed.content.contains(SENTINEL));
    }

    #[test]
    fn config_dir_precedence_is_env_then_xdg_then_home() {
        // OPENCODE_CONFIG_DIR wins outright.
        assert_eq!(
            config_dir_from(Some("/custom/oc"), Some("/xdg"), Some("/home/u")).unwrap(),
            PathBuf::from("/custom/oc")
        );
        // Else $XDG_CONFIG_HOME/opencode.
        assert_eq!(
            config_dir_from(None, Some("/xdg"), Some("/home/u")).unwrap(),
            PathBuf::from("/xdg/opencode")
        );
        // Else ~/.config/opencode.
        assert_eq!(
            config_dir_from(None, None, Some("/home/u")).unwrap(),
            PathBuf::from("/home/u/.config/opencode")
        );
        // Empty env values are treated as unset (basedir-spec semantics).
        assert_eq!(
            config_dir_from(Some(""), Some(""), Some("/home/u")).unwrap(),
            PathBuf::from("/home/u/.config/opencode")
        );
        // No home anywhere → a hard error (never a CWD-relative file).
        assert!(config_dir_from(None, None, None).is_err());
    }

    #[test]
    fn default_path_is_the_plugin_file_under_the_plural_plugins_dir() {
        // PLURAL `plugins/` — the canonical opencode auto-discovery dir (the
        // fork globs both, but canonical scans only `plugins/`).
        assert_eq!(
            config_dir_from(None, Some("/xdg"), None)
                .unwrap()
                .join("plugins")
                .join("pixtuoid.ts"),
            PathBuf::from("/xdg/opencode/plugins/pixtuoid.ts")
        );
    }

    #[test]
    fn hook_command_returns_the_absolute_path() {
        assert_eq!(
            hook_command(Path::new("/opt/bin/pixtuoid-hook"), false).unwrap(),
            "/opt/bin/pixtuoid-hook"
        );
    }

    #[test]
    #[cfg(unix)]
    fn hook_command_errors_on_non_utf8_path() {
        use std::os::unix::ffi::OsStrExt;
        let bad = Path::new(std::ffi::OsStr::from_bytes(b"/x/\xff/pixtuoid-hook"));
        assert!(hook_command(bad, false).is_err());
    }

    // Internal-consistency guard (mirror of the CC/Codex/Reasonix/CodeWhale
    // ones): every opencode event TYPE the bundled plugin forwards must have a
    // decoder arm (or be a deliberate skip), so a registered-but-undecoded type
    // can't silently drop. The plugin's FORWARD set + the tool-part gate are the
    // source of truth; we assert each decodes without error.
    #[test]
    fn every_forwarded_opencode_event_decodes() {
        use pixtuoid_core::source::decoder::decode_hook_payload;
        // The plugin forwards these `type`s (see opencode_plugin.ts FORWARD set
        // + the message.part.updated tool gate). Each must decode (map or skip),
        // never error.
        let payloads = [
            serde_json::json!({"type": "session.created",
                "properties": {"info": {"id": "ses_1", "directory": "/r"}}, "_pixtuoid_source": "opencode"}),
            serde_json::json!({"type": "session.deleted",
                "properties": {"info": {"id": "ses_1", "directory": "/r"}}, "_pixtuoid_source": "opencode"}),
            serde_json::json!({"type": "permission.asked",
                "properties": {"sessionID": "ses_1"}, "_pixtuoid_source": "opencode"}),
            serde_json::json!({"type": "permission.v2.asked",
                "properties": {"sessionID": "ses_1"}, "_pixtuoid_source": "opencode"}),
            serde_json::json!({"type": "message.part.updated",
                "properties": {"sessionID": "ses_1", "part": {"type": "tool", "callID": "c",
                    "tool": "bash", "state": {"status": "running"}}}, "_pixtuoid_source": "opencode"}),
        ];
        for p in payloads {
            let ty = p["type"].clone();
            assert!(
                decode_hook_payload(p).is_ok(),
                "forwarded opencode event {ty} failed to decode — add an arm in source/opencode.rs"
            );
        }
    }
}
