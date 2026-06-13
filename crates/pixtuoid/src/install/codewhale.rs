//! CodeWhale hook install target.
//!
//! Writes the GLOBAL CodeWhale config (`~/.codewhale/config.toml`, or the
//! legacy `~/.deepseek/config.toml` when that is the file CodeWhale actually
//! reads — mirroring its own `default_config_path` resolution @0.8.59). The
//! `[hooks]` table holds a single `hooks` ARRAY of `{event, command}` entries
//! (NOT Codex's per-event group keys, NOT Claude's nested `{matcher, hooks}`):
//!
//! ```toml
//! [hooks]
//! enabled = true
//!
//! [[hooks.hooks]]
//! event = "tool_call_before"
//! command = "PIXTUOID_SOURCE=codewhale '/abs/pixtuoid-hook' --event tool_call_before"
//! _pixtuoid = true
//! ```
//!
//! Load-bearing details:
//! - **Per-event command.** Unlike Codex/Reasonix (one command for all events),
//!   CodeWhale sets no event env var, so the event name is BAKED into each
//!   entry's command as ` --event <name>`. The shim's env-mode reads it (see
//!   `pixtuoid-hook` + `source/codewhale.rs`). `hook_command` returns the BASE
//!   command; `merge_install` appends the per-event suffix.
//! - **`enabled = true`.** CodeWhale gates ALL hooks on `[hooks].enabled`
//!   (default true, `hooks.rs::default_enabled`). We set it explicitly so a
//!   user who had previously disabled hooks still gets the visualizer — running
//!   `install-hooks` is an explicit opt-in (the silent-non-fire trap is worse
//!   than re-enabling; cf. Reasonix's project-scope trust gate).
//! - **`_pixtuoid` sentinel.** CodeWhale's `Hook` serde has no
//!   `deny_unknown_fields` (verified @0.8.59), so the marker is ignored by
//!   CodeWhale and round-trips; managed-entry detection keys on it (the
//!   per-event command's last token is the event name, not the binary, so a
//!   Codex-style command-basename fallback wouldn't apply).
//! - Comments/ordering are lost on the `toml::Value` round-trip (a backup is
//!   taken) — same caveat as Codex; surfaced via `post_install_note`.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use toml::value::Table;

use crate::install::io;
use crate::install::target::MergeOutcome;

const SENTINEL_KEY: &str = "_pixtuoid";

/// Events we register == events we decode (`source/codewhale.rs`), enforced by
/// `every_registered_codewhale_event_decodes` below. The `bool` is `env_mode`:
/// `true` events carry identity via `DEEPSEEK_*` env vars, so their command bakes
/// `--event <name>` and the shim builds the envelope from env; `false` events
/// (the subagent observer hooks) are forwarded RAW on stdin — CodeWhale pipes a
/// complete JSON payload (with the child `agent_id`), so the command is the plain
/// stdin-forward form (no `--event`), exactly like the CC/Codex hooks.
///
/// turn_end / mode_change / on_error / shell_env are deliberately absent
/// (per-turn noise / no lifecycle meaning). CodeWhale has NO approval hook in the
/// TUI path (`ApprovalRequired` shows UI + writes the audit log, fires no hook),
/// so there is no Waiting event to register — not a scope cut, no signal exists.
const CODEWHALE_EVENTS: &[(&str, bool)] = &[
    ("session_start", true),
    ("message_submit", true),
    ("tool_call_before", true),
    ("tool_call_after", true),
    ("session_end", true),
    ("subagent_spawn", false),
    ("subagent_complete", false),
];

/// The config CodeWhale actually reads: prefer `~/.codewhale/config.toml`, else
/// the legacy `~/.deepseek/config.toml` when only that exists, else the modern
/// path for a fresh install. Mirrors CodeWhale's own `config::default_config_path`
/// so the installed hooks land in the file the CLI loads (writing a fresh
/// `~/.codewhale/config.toml` when the real config is `~/.deepseek/config.toml`
/// would make CodeWhale PREFER our near-empty file and drop the user's
/// provider/key config).
pub fn default_config_path() -> Result<PathBuf> {
    let modern = io::home_relative_checked(".codewhale/config.toml")?;
    if modern.exists() {
        return Ok(modern);
    }
    let legacy = io::home_relative_checked(".deepseek/config.toml")?;
    if legacy.exists() {
        return Ok(legacy);
    }
    Ok(modern)
}

/// Presence probe for auto-detection. CodeWhale's config FILE may be absent on
/// a fresh install while the product-state dir exists, and the legacy
/// `~/.deepseek` layout puts config elsewhere — so probe the state dirs
/// (created by CodeWhale on first launch) rather than the file we write.
pub fn detect_installed() -> bool {
    io::home_relative(".codewhale").exists() || io::home_relative(".deepseek").exists()
}

/// The BASE hook command (no `--event` — `merge_install` appends the per-event
/// suffix). CodeWhale runs the `command` under a shell — `sh -c` on Unix,
/// `cmd /C` on Windows (verified `hooks.rs::build_shell_command` @0.8.59), the
/// same contract as Codex/Reasonix, so the OS forms mirror them exactly:
/// - **Unix**: env-prefix `PIXTUOID_SOURCE=codewhale '<abs-path>'`.
/// - **Windows**: BARE `<abs-path> --source codewhale` (the source rides the
///   `--source` flag; 8.3 short-name substitution for cmd-unsafe paths via the
///   shared `hook_cmd::windows`). Err on non-UTF-8 (prevents the
///   to_string_lossy dead-hook).
pub fn hook_command(resolved: &Path, _explicit: bool) -> Result<String> {
    // `_explicit` is Claude's bare-name-vs-absolute switch — CodeWhale always
    // embeds the absolute path, so the flag changes nothing here.
    let p = resolved
        .to_str()
        .ok_or_else(|| anyhow!("pixtuoid-hook path is non-UTF-8: {}", resolved.display()))?;
    crate::install::hook_cmd::shell_hook_command(p, "codewhale")
}

fn parse_or_empty(content: &str) -> Result<toml::Value> {
    if content.trim().is_empty() {
        return Ok(toml::Value::Table(Table::new()));
    }
    // No file path here — the orchestrator wraps the error with the real path.
    toml::from_str(content).context("not valid TOML — refusing to overwrite")
}

pub fn merge_install(content: &str, base_cmd: &str) -> Result<MergeOutcome> {
    let doc = parse_or_empty(content)?;
    let merged = toml_merge_install(doc.clone(), base_cmd);
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

fn is_managed_entry(entry: &toml::Value) -> bool {
    entry.get(SENTINEL_KEY).and_then(|v| v.as_bool()) == Some(true)
}

fn managed_entry(event: &str, env_mode: bool, base_cmd: &str) -> toml::Value {
    let mut entry = Table::new();
    entry.insert("event".into(), toml::Value::String(event.into()));
    // env-mode events bake `--event <name>` (the shim reads DEEPSEEK_* env);
    // the subagent observer events forward the raw stdin JSON, so the command is
    // the plain base form (no `--event`) — the shim reads stdin like CC/Codex.
    let command = if env_mode {
        format!("{base_cmd} --event {event}")
    } else {
        base_cmd.to_string()
    };
    entry.insert("command".into(), toml::Value::String(command));
    entry.insert(SENTINEL_KEY.into(), toml::Value::Boolean(true));
    toml::Value::Table(entry)
}

fn toml_merge_install(doc: toml::Value, base_cmd: &str) -> toml::Value {
    let mut root = doc.as_table().cloned().unwrap_or_default();
    let hooks = root
        .entry("hooks".to_string())
        .or_insert_with(|| toml::Value::Table(Table::new()));
    if !hooks.is_table() {
        *hooks = toml::Value::Table(Table::new());
    }
    if let Some(hooks) = hooks.as_table_mut() {
        // Hooks are gated on this flag (default true). Set it so a previously
        // disabled config still fires the visualizer the user just opted into.
        hooks.insert("enabled".into(), toml::Value::Boolean(true));
        let arr = hooks
            .entry("hooks".to_string())
            .or_insert_with(|| toml::Value::Array(vec![]));
        if !arr.is_array() {
            *arr = toml::Value::Array(vec![]);
        }
        if let Some(arr) = arr.as_array_mut() {
            arr.retain(|e| !is_managed_entry(e));
            for (ev, env_mode) in CODEWHALE_EVENTS {
                arr.push(managed_entry(ev, *env_mode, base_cmd));
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
    if let Some(arr) = hooks.get_mut("hooks").and_then(|h| h.as_array_mut()) {
        arr.retain(|e| !is_managed_entry(e));
    }
    // Drop the hooks array once it holds no entries (ours were the only ones).
    if hooks
        .get("hooks")
        .and_then(|h| h.as_array())
        .is_some_and(|a| a.is_empty())
    {
        hooks.remove("hooks");
    }
    // If the [hooks] table is now empty or holds ONLY the `enabled` flag we set,
    // it was ours — drop it so an uninstall fully reverses a pixtuoid-only
    // install. A user's own hooks / extra keys keep it alive. ACCEPTED residual:
    // a user who had a LONE `enabled = false` (no hooks) before install does not
    // get it restored here — install force-set it true (an explicit opt-in to
    // visualization), and with no hooks defined the flag is moot, so the only
    // loss is a no-op config line. Faithfully restoring it would need install to
    // record that it flipped the value; not worth the state for a nil effect.
    let ours_only = hooks.is_empty() || hooks.keys().all(|k| k == "enabled");
    if ours_only {
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

    const BASE: &str = "PIXTUOID_SOURCE=codewhale '/opt/bin/pixtuoid-hook'";

    #[test]
    fn install_creates_one_entry_per_event_with_baked_event_and_sentinel() {
        let out = merge_install("", BASE).unwrap();
        assert!(out.changed);
        // Round-trip MUST survive: enabled (a scalar) sits beside the hooks
        // array-of-tables in the same [hooks] table — pin that toml serializes
        // the scalar before the array headers (else `enabled` would bind to the
        // last entry and corrupt it).
        let v = parse(&out.content);
        assert_eq!(
            v["hooks"]["enabled"].as_bool(),
            Some(true),
            "enabled must round-trip as a [hooks]-level scalar, not absorbed into an entry"
        );
        let arr = v["hooks"]["hooks"].as_array().unwrap();
        assert_eq!(arr.len(), CODEWHALE_EVENTS.len());
        for (entry, (ev, env_mode)) in arr.iter().zip(CODEWHALE_EVENTS) {
            assert_eq!(entry["event"].as_str().unwrap(), *ev);
            let expected = if *env_mode {
                // env-mode events bake `--event <name>`.
                format!("{BASE} --event {ev}")
            } else {
                // subagent observer events forward raw stdin — plain command.
                BASE.to_string()
            };
            assert_eq!(
                entry["command"].as_str().unwrap(),
                expected,
                "env-mode events bake --event; subagent events use the plain stdin-forward command"
            );
            assert!(entry[SENTINEL_KEY].as_bool().unwrap());
        }
    }

    #[test]
    fn install_is_idempotent_and_replaces_across_paths() {
        let a = merge_install("", BASE).unwrap();
        let b = merge_install(&a.content, BASE).unwrap();
        assert!(!b.changed, "same-command re-install is a semantic no-op");
        // A path change replaces (does not duplicate) the managed entries.
        let c = merge_install(
            &a.content,
            "PIXTUOID_SOURCE=codewhale '/usr/local/bin/pixtuoid-hook'",
        )
        .unwrap();
        let v = parse(&c.content);
        assert_eq!(
            v["hooks"]["hooks"].as_array().unwrap().len(),
            CODEWHALE_EVENTS.len(),
            "path change must not duplicate entries"
        );
    }

    #[test]
    fn install_sets_enabled_true_even_when_user_disabled_hooks() {
        let user = "[hooks]\nenabled = false\n";
        let out = merge_install(user, BASE).unwrap();
        let v = parse(&out.content);
        assert_eq!(
            v["hooks"]["enabled"].as_bool(),
            Some(true),
            "install must (re-)enable hooks so the visualizer fires"
        );
    }

    #[test]
    fn install_preserves_user_hooks_and_other_keys() {
        let user = r#"
provider = "deepseek"
api_key = "secret"

[hooks]
enabled = true

[[hooks.hooks]]
event = "session_start"
command = "echo hi"
"#;
        let out = merge_install(user, BASE).unwrap();
        let v = parse(&out.content);
        assert_eq!(v["provider"].as_str(), Some("deepseek"));
        assert_eq!(
            v["api_key"].as_str(),
            Some("secret"),
            "unrelated keys survive"
        );
        let arr = v["hooks"]["hooks"].as_array().unwrap();
        // user's 1 + every managed CodeWhale event
        assert_eq!(arr.len(), 1 + CODEWHALE_EVENTS.len());
        assert!(
            arr.iter().any(|e| e["command"].as_str() == Some("echo hi")),
            "the user's own hook must be preserved"
        );
    }

    #[test]
    fn uninstall_removes_only_managed_entries() {
        let user = r#"
[hooks]
enabled = true

[[hooks.hooks]]
event = "session_start"
command = "echo hi"
"#;
        let installed = merge_install(user, BASE).unwrap();
        let cleaned = merge_uninstall(&installed.content).unwrap();
        assert!(cleaned.changed);
        let v = parse(&cleaned.content);
        let arr = v["hooks"]["hooks"].as_array().unwrap();
        assert_eq!(arr.len(), 1, "only the user's own hook remains");
        assert_eq!(arr[0]["command"].as_str(), Some("echo hi"));
    }

    #[test]
    fn uninstall_of_pixtuoid_only_install_drops_the_hooks_table() {
        let installed = merge_install("", BASE).unwrap();
        let cleaned = merge_uninstall(&installed.content).unwrap();
        let v = parse(&cleaned.content);
        assert!(
            v.get("hooks").is_none(),
            "a pixtuoid-only [hooks] (just enabled + our entries) must be fully removed, got {v}"
        );
    }

    #[test]
    fn uninstall_no_managed_hooks_is_a_no_op() {
        let user = "[hooks]\nenabled = true\n\n[[hooks.hooks]]\nevent = \"session_start\"\ncommand = \"echo hi\"\n";
        let out = merge_uninstall(user).unwrap();
        assert!(!out.changed, "no managed entries → semantic no-op");
    }

    #[test]
    fn merge_install_rejects_invalid_toml() {
        // A malformed config must NOT be overwritten (it'd wipe the user's
        // provider/key/hooks); refuse instead.
        assert!(merge_install("not = valid = toml", BASE).is_err());
    }

    #[test]
    fn install_coerces_non_table_hooks_and_non_array_entries() {
        let out = merge_install("hooks = \"garbage\"", BASE).unwrap();
        let v = parse(&out.content);
        assert!(v["hooks"].is_table());
        assert_eq!(
            v["hooks"]["hooks"].as_array().unwrap().len(),
            CODEWHALE_EVENTS.len()
        );
    }

    // Unix POSIX-form pin. Unix-only: on Windows hook_command emits the bare
    // form and this spaced path would be REJECTED (8.3 unavailable on CI).
    #[cfg(unix)]
    #[test]
    fn hook_command_is_the_base_env_prefix_form_without_event() {
        let cmd = hook_command(Path::new("/opt/bin/pixtuoid-hook"), false).unwrap();
        assert_eq!(cmd, "PIXTUOID_SOURCE=codewhale '/opt/bin/pixtuoid-hook'");
        assert!(
            !cmd.contains("--event"),
            "the event is appended by merge_install"
        );
    }

    #[test]
    #[cfg(windows)]
    fn hook_command_emits_bare_exec_form_with_source_flag_on_windows() {
        let cmd = hook_command(Path::new(r"C:\tools\pixtuoid-hook.exe"), false).unwrap();
        assert_eq!(cmd, r"C:\tools\pixtuoid-hook.exe --source codewhale");
    }

    #[test]
    #[cfg(unix)]
    fn hook_command_errors_on_non_utf8_path() {
        use std::os::unix::ffi::OsStrExt;
        let bad = Path::new(std::ffi::OsStr::from_bytes(b"/x/\xff/pixtuoid-hook"));
        assert!(hook_command(bad, false).is_err());
    }

    // Internal-consistency guard (mirror of the CC/Codex/Reasonix ones): every
    // hook event we REGISTER with CodeWhale must have a decoder arm, else it
    // arrives at the shared socket and the decoder bails — silently dropped.
    #[test]
    fn every_registered_codewhale_event_decodes() {
        use pixtuoid_core::source::decoder::decode_hook_payload;
        for (ev, _env_mode) in CODEWHALE_EVENTS {
            // Carry every identity field a decoder arm might need: `cwd` for the
            // env-mode events, `agent_id`/`workspace` for the subagent events.
            let payload = serde_json::json!({
                "event": ev,
                "cwd": "/repo",
                "agent_id": "agent-1",
                "workspace": "/repo",
                "_pixtuoid_source": "codewhale",
            });
            assert!(
                decode_hook_payload(payload).is_ok(),
                "registered CodeWhale hook {ev:?} has no decoder arm — it would \
                 bail as unsupported. Add an arm in pixtuoid-core source/codewhale.rs."
            );
        }
    }
}
