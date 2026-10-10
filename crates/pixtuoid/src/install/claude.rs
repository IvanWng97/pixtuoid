use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use pixtuoid_core::source::claude_code::claude_config_dir;
use serde_json::{Value, json};

use crate::install::SENTINEL_KEY;
use crate::install::io;
use crate::install::merge;
use crate::install::target::{HostRegistration, MergeOutcome, Unregistered};

pub(crate) const EVENTS: &[&str] = &[
    "SessionStart",
    "PreToolUse",
    "PostToolUse",
    "Notification",
    // A tool gate arrives as `PermissionRequest`; the only `Notification` a
    // gated run fires is the idle "Claude is waiting for your input" (captured
    // in fixtures/claude-code/permission-recorded). The decoder has read this
    // event since Codex needed it — unregistered, the bytes never came.
    "PermissionRequest",
    // The ONLY end signal a Workflow-fleet subagent gets: no per-agent Agent
    // tool_use, no transcript end marker.
    "SubagentStart",
    "SubagentStop",
    "SessionEnd",
];

/// The marketplace and the plugin share one name, so the plugin id is
/// `pixtuoid@pixtuoid`.
const PLUGIN_NAME: &str = "pixtuoid";

/// Claude Code loads the hooks through its plugin system
/// (code.claude.com/docs/en/plugins): a local marketplace whose one plugin loads
/// in place, so a re-install's rewrite reaches the next session.
pub(crate) const HOST: HostRegistration = HostRegistration {
    register,
    unregister,
    is_registered,
    leftover_hooks,
};

/// `<marketplace>/<plugin>/hooks/hooks.json`, under pixtuoid's own config dir.
pub(crate) fn default_config_path() -> Result<PathBuf> {
    let base = crate::config::config_base().context("no home directory resolves")?;
    Ok(base
        .join("pixtuoid")
        .join("claude-plugin")
        .join(PLUGIN_NAME)
        .join("hooks")
        .join("hooks.json"))
}

/// Claude Code's user settings, where it records the plugin as enabled.
fn settings_path() -> Result<PathBuf> {
    if let Some(dir) = claude_config_dir() {
        return Ok(dir.join("settings.json"));
    }
    io::home_relative_checked(".claude/settings.json")
}

/// Claude Code is present when its config dir exists; the hooks file is ours, so
/// its absence says nothing.
pub(crate) fn detect_installed() -> bool {
    settings_path()
        .ok()
        .and_then(|p| p.parent().map(Path::is_dir))
        .unwrap_or(false)
}

/// The plugin dir and the marketplace root of `<root>/<plugin>/hooks/hooks.json`.
fn plugin_layout(config: &Path) -> Result<(&Path, &Path)> {
    let plugin = config.parent().and_then(Path::parent);
    plugin.zip(plugin.and_then(Path::parent)).with_context(|| {
        format!(
            "{} is not <marketplace>/<plugin>/hooks/hooks.json",
            config.display()
        )
    })
}

fn plugin_id() -> String {
    format!("{PLUGIN_NAME}@{PLUGIN_NAME}")
}

fn register(config: &Path) -> Result<()> {
    let (plugin, root) = plugin_layout(config)?;
    let source = Path::new(".").join(plugin.file_name().context("the plugin dir has no name")?);
    let marketplace = json!({
        "name": PLUGIN_NAME,
        "owner": { "name": PLUGIN_NAME },
        "plugins": [{
            "name": PLUGIN_NAME,
            "source": source.to_string_lossy().replace('\\', "/"),
            "description": "Sends Claude Code's session events to the pixtuoid office",
        }],
    });
    let manifest = json!({
        "name": PLUGIN_NAME,
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Sends Claude Code's session events to the pixtuoid office",
        "homepage": env!("CARGO_PKG_HOMEPAGE"),
    });
    for (path, doc) in [
        (
            root.join(".claude-plugin").join("marketplace.json"),
            marketplace,
        ),
        (plugin.join(".claude-plugin").join("plugin.json"), manifest),
    ] {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        io::write_config_atomic(&path, &serde_json::to_string_pretty(&doc)?)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    // Both are idempotent: a repeat add reports the marketplace already on disk,
    // a repeat install that the plugin already loads in place.
    run_claude(&[
        "plugin".as_ref(),
        "marketplace".as_ref(),
        "add".as_ref(),
        root.as_os_str(),
    ])?;
    run_claude(&["plugin".as_ref(), "install".as_ref(), plugin_id().as_ref()])?;
    Ok(())
}

/// Removing the marketplace uninstalls its plugins too. `remove` exits non-zero on
/// a marketplace it doesn't know, hence the list first.
fn unregister(config: &Path) -> Result<Unregistered> {
    if claude_cli().is_none() {
        // Disabled is still installed: its marketplace must outlive it.
        if plugin_entry()?.is_some() {
            tracing::warn!("claude not on PATH; leaving the pixtuoid plugin registered");
            return Ok(Unregistered::Unreachable);
        }
        remove_marketplace(config)?;
        return Ok(Unregistered::Absent);
    }
    let listed = run_claude(&[
        "plugin".as_ref(),
        "marketplace".as_ref(),
        "list".as_ref(),
        "--json".as_ref(),
    ])?;
    let found = marketplace_listed(&listed)?;
    if found {
        run_claude(&[
            "plugin".as_ref(),
            "marketplace".as_ref(),
            "remove".as_ref(),
            PLUGIN_NAME.as_ref(),
        ])?;
    }
    remove_marketplace(config)?;
    Ok(if found {
        Unregistered::Removed
    } else {
        Unregistered::Absent
    })
}

/// Delete the marketplace `register` wrote around `config`, only when its
/// manifest is ours: a custom `config` path must not take a stranger's dir.
fn remove_marketplace(config: &Path) -> Result<()> {
    let (_, root) = plugin_layout(config)?;
    let manifest = root.join(".claude-plugin").join("marketplace.json");
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        return Ok(());
    };
    let ours =
        serde_json::from_str::<Value>(&text).is_ok_and(|m| m["name"].as_str() == Some(PLUGIN_NAME));
    if ours {
        std::fs::remove_dir_all(root).with_context(|| format!("removing {}", root.display()))?;
    }
    Ok(())
}

/// Read where Claude Code records a user-scope install as enabled
/// (code.claude.com/docs/en/plugins/install#choose-an-install-scope) — the settings
/// file, not `claude plugin list`, which costs a CLI start per check.
fn is_registered() -> Result<bool> {
    Ok(plugin_entry()? == Some(true))
}

/// The plugin's `enabledPlugins` value: `Some(false)` is installed but disabled.
fn plugin_entry() -> Result<Option<bool>> {
    Ok(read_settings(&settings_path()?)?["enabledPlugins"][plugin_id()].as_bool())
}

/// `Null` for a missing or empty file.
fn read_settings(settings: &Path) -> Result<Value> {
    let content = io::read_config(settings)?;
    if content.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&content).with_context(|| format!("parsing {}", settings.display()))
}

fn leftover_hooks() -> Option<String> {
    leftover_hooks_in(&settings_path().ok()?)
}

/// Releases before the plugin merged their hooks into this settings file, keyed
/// on [`SENTINEL_KEY`]; Claude Code runs a plugin's copy of a handler beside a
/// settings file's (code.claude.com/docs/en/hooks), hence "twice". A file that
/// doesn't parse reports none: Claude Code skips it too
/// (code.claude.com/docs/en/settings).
fn leftover_hooks_in(settings: &Path) -> Option<String> {
    let doc = read_settings(settings).ok()?;
    let events: Vec<&str> = doc
        .get("hooks")?
        .as_object()?
        .iter()
        .filter(|(_, list)| {
            list.as_array()
                .is_some_and(|l| l.iter().any(|e| merge::is_flat_managed(e, SENTINEL_KEY)))
        })
        .map(|(event, _)| event.as_str())
        .collect();
    (!events.is_empty()).then(|| {
        format!(
            "{} still holds the hooks an older pixtuoid wrote for {}: Claude Code runs \
             them while disconnected, and twice beside the plugin — delete each entry \
             marked \"{SENTINEL_KEY}\": true",
            crate::display_path(settings),
            crate::strip_control_chars(&events.join(", "))
        )
    })
}

fn marketplace_listed(json_out: &str) -> Result<bool> {
    let rows: Vec<Value> = serde_json::from_str(json_out)
        .context("parsing `claude plugin marketplace list --json`")?;
    Ok(rows
        .iter()
        .any(|r| r.get("name").and_then(Value::as_str) == Some(PLUGIN_NAME)))
}

/// `which` applies PATHEXT, so an npm install's `claude.cmd` resolves on Windows.
/// `None` when no `claude` is on PATH: an uninstall then can't deregister.
fn claude_cli() -> Option<PathBuf> {
    which::which("claude").ok()
}

fn run_claude(args: &[&std::ffi::OsStr]) -> Result<String> {
    let cli = claude_cli().context(
        "Claude Code's `claude` command is not on PATH; pixtuoid installs its hooks as a Claude Code plugin through it — put it on PATH and reconnect",
    )?;
    // The real CLI would register and remove plugins in the developer's own Claude
    // Code — a test run once did.
    #[cfg(test)]
    assert!(
        cli.starts_with(std::env::temp_dir()),
        "a test is about to run the real {} — put a fake `claude` first on PATH",
        cli.display()
    );
    let out = std::process::Command::new(&cli)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| format!("running {}", cli.display()))?;
    if !out.status.success() {
        bail!(
            "`claude {}` failed: {}",
            args.iter()
                .map(|a| a.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" "),
            String::from_utf8_lossy(if out.stderr.is_empty() {
                &out.stdout
            } else {
                &out.stderr
            })
            .trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Unix: the bare name behind the `PIXTUOID_SOURCE=` prefix every other source
/// carries, so CC PATH-resolves it and a binary upgrade applies without a
/// rewrite. [`io::HOOK_OVERRIDE_ENV`] overrides that — the user set it precisely because
/// the binary is off-PATH — and is single-quoted, since CC runs shell-form
/// commands through a shell.
///
/// Windows: exec form requires the absolute PE path (shell-form goes through
/// cmd.exe/PowerShell — unportable, PATHEXT-dependent). The orchestrator hard-errors if
/// resolution failed, so `resolved` is guaranteed absolute here.
pub(crate) fn hook_command(resolved: &Path, explicit: bool) -> Result<String> {
    #[cfg(not(windows))]
    {
        // CC's Unix entry carries no `args` key, so it runs through a SHELL and
        // the env prefix every other source uses works here too. It was bare for
        // years, which made "no `_pixtuoid_source`" mean "probably CC" — the
        // guess `decoder` still has to make for installs written before this, and
        // the reason an unstamped cursor invocation landed on CC's arms at all.
        let p = if explicit {
            merge::hook_path_str(resolved)?
        } else {
            "pixtuoid-hook"
        };
        crate::install::hook_cmd::shell_hook_command(
            p,
            pixtuoid_core::source::claude_code::SOURCE_NAME,
        )
    }
    #[cfg(windows)]
    {
        let _ = explicit; // exec form always embeds the absolute path
        merge::hook_path_str(resolved).map(str::to_string)
    }
}

/// The inner hook object of a hooks.json entry. `exec_form` adds the empty `args` key
/// that makes CC spawn the PE directly instead of through a shell; `matcher` lives
/// on the OUTER entry, not here.
pub(crate) fn hook_entry(cmd: &str, exec_form: bool) -> Value {
    if exec_form {
        json!({ "type": "command", "command": cmd, "args": [] })
    } else {
        json!({ "type": "command", "command": cmd })
    }
}

pub(crate) fn verify_schema(content: &str) -> crate::install::verify::SchemaParse {
    use crate::install::verify::{SchemaParse, ShimRef, assemble};
    let Ok(doc) = serde_json::from_str::<Value>(content) else {
        return SchemaParse::broken("the plugin's hooks.json no longer parses as JSON");
    };
    let hooks = doc.get("hooks").and_then(|h| h.as_object());
    let mut missing = Vec::new();
    let mut any = false;
    let mut shim = ShimRef::Unknown;
    for ev in EVENTS {
        let managed: Option<&Value> = hooks
            .and_then(|h| h.get(*ev))
            .and_then(|a| a.as_array())
            .and_then(|arr| arr.first());
        match managed {
            Some(entry) => {
                any = true;
                if shim == ShimRef::Unknown {
                    shim = claude_shim_ref(entry);
                }
            }
            None => missing.push(*ev),
        }
    }
    assemble(&missing, any, shim, vec![])
}

fn claude_shim_ref(entry: &Value) -> crate::install::verify::ShimRef {
    use crate::install::verify::ShimRef;
    let cmd = entry
        .get("hooks")
        .and_then(|h| h.as_array())
        .and_then(|a| a.first())
        .and_then(|h| h.get("command"))
        .and_then(|c| c.as_str());
    match cmd {
        None => ShimRef::Unknown,
        // The SHELL form is `shell_shim_ref`'s own wire format, so it parses it —
        // including any future ` --event` suffix, which a private copy here would
        // bake into the path. CC alone can answer `BareName`, so that is the one
        // thing mapped on top.
        #[cfg(not(windows))]
        Some(c) => match crate::install::verify::shell_shim_ref(c.trim()) {
            ShimRef::Absolute(p) if p.as_os_str() == "pixtuoid-hook" => ShimRef::BareName,
            other => other,
        },
        // NOT that parser: CC's Windows entry is exec form (`args: []`), a bare
        // absolute path with neither quoting nor `--source`, so it would fall to
        // that parser's last-whitespace-token arm and truncate `C:\Program Files\…`.
        #[cfg(windows)]
        Some(c) => ShimRef::Absolute(std::path::PathBuf::from(c.trim())),
    }
}

/// The plugin's hooks file is ours whole, so install writes the full document and
/// replaces whatever was there.
pub(crate) fn merge_install(content: &str, hook_cmd: &str) -> Result<MergeOutcome> {
    let want = json!({
        "hooks": EVENTS
            .iter()
            .map(|ev| ((*ev).to_string(), json!([managed_entry(hook_cmd)])))
            .collect::<serde_json::Map<_, _>>()
    });
    Ok(MergeOutcome {
        changed: serde_json::from_str::<Value>(content).ok().as_ref() != Some(&want),
        content: serde_json::to_string_pretty(&want)?,
    })
}

pub(crate) fn merge_uninstall(content: &str) -> Result<MergeOutcome> {
    let had_hooks = serde_json::from_str::<Value>(content)
        .ok()
        .and_then(|doc| doc.get("hooks")?.as_object().map(|h| !h.is_empty()))
        .unwrap_or(false);
    Ok(MergeOutcome {
        content: "{}\n".to_string(),
        changed: had_hooks,
    })
}

/// The one place Claude's nested per-event shape lives.
fn managed_entry(hook_command: &str) -> Value {
    json!({
        "matcher": ".*",
        "hooks": [ hook_entry(hook_command, cfg!(windows)) ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_path_honors_claude_config_dir() {
        let fallback_suffix = PathBuf::from(".claude").join("settings.json");

        temp_env::with_var_unset("CLAUDE_CONFIG_DIR", || {
            let unset_path = settings_path().unwrap();
            assert!(
                unset_path.ends_with(&fallback_suffix),
                "default config path must end with .claude/settings.json, got {unset_path:?}"
            );
        });

        let custom_dir = std::env::temp_dir().join("pixtuoid-claude-config-dir");
        temp_env::with_var("CLAUDE_CONFIG_DIR", Some(&custom_dir), || {
            assert_eq!(settings_path().unwrap(), custom_dir.join("settings.json"));
        });

        temp_env::with_var("CLAUDE_CONFIG_DIR", Some(""), || {
            let empty_path = settings_path().unwrap();
            assert!(
                empty_path.ends_with(&fallback_suffix),
                "empty CLAUDE_CONFIG_DIR must fall back to .claude/settings.json, got {empty_path:?}"
            );
        });
    }

    fn leftovers_in(settings: Option<&str>) -> Option<String> {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        if let Some(s) = settings {
            std::fs::write(&path, s).unwrap();
        }
        leftover_hooks_in(&path)
    }

    #[test]
    fn leftover_hooks_names_each_event_a_pre_plugin_install_still_holds() {
        let theirs =
            json!({ "matcher": "Bash", "hooks": [{ "type": "command", "command": "lint" }] });
        let ours = json!({
            SENTINEL_KEY: true,
            "matcher": ".*",
            "hooks": [{ "type": "command", "command": "PIXTUOID_SOURCE=claude-code 'pixtuoid-hook'" }],
        });
        let settings = json!({
            "enabledPlugins": { "pixtuoid@pixtuoid": true },
            "hooks": {
                "PreToolUse": [theirs, ours],
                "Stop": [ours],
                "PostToolUse": [theirs],
            },
        });
        let note = leftovers_in(Some(&settings.to_string())).expect("two events still hold ours");
        assert!(note.contains("PreToolUse, Stop"), "{note}");
        assert!(
            !note.contains("PostToolUse"),
            "only the user's hook there: {note}"
        );
        assert!(
            note.contains(r#""_pixtuoid": true"#),
            "names the removal step: {note}"
        );
        assert!(note.contains("settings.json"), "names the file: {note}");
    }

    #[test]
    fn leftover_hooks_reports_none_without_a_sentinel_entry() {
        for settings in [
            None,
            Some(""),
            Some("{not json"),
            Some("[]"),
            Some(r#"{"enabledPlugins":{"pixtuoid@pixtuoid":true}}"#),
            Some(r#"{"hooks":{}}"#),
            Some(r#"{"hooks":[]}"#),
            Some(r#"{"hooks":{"Stop":[]}}"#),
            Some(r#"{"hooks":{"Stop":{"_pixtuoid":true}}}"#),
            Some(r#"{"hooks":{"Stop":[{"_pixtuoid":false,"hooks":[]}]}}"#),
            Some(
                r#"{"hooks":{"Stop":[{"matcher":"","hooks":[{"type":"command","command":"pixtuoid-hook"}]}]}}"#,
            ),
        ] {
            assert_eq!(leftovers_in(settings), None, "{settings:?}");
        }
    }

    #[test]
    fn the_hooks_file_sits_in_pixtuoids_own_marketplace() {
        let base = std::env::temp_dir().join("pixtuoid-xdg");
        temp_env::with_var("XDG_CONFIG_HOME", Some(&base), || {
            let hooks = default_config_path().unwrap();
            assert_eq!(
                hooks,
                base.join("pixtuoid/claude-plugin/pixtuoid/hooks/hooks.json")
            );
            let (plugin, root) = plugin_layout(&hooks).unwrap();
            assert_eq!(plugin, base.join("pixtuoid/claude-plugin/pixtuoid"));
            assert_eq!(root, base.join("pixtuoid/claude-plugin"));
        });
    }

    #[test]
    fn the_marketplace_listing_is_read_by_name() {
        assert!(marketplace_listed(r#"[{"name":"other"},{"name":"pixtuoid"}]"#).unwrap());
        assert!(!marketplace_listed(r#"[{"name":"other"}]"#).unwrap());
        assert!(marketplace_listed("not json").is_err());
    }

    #[test]
    fn install_writes_every_event_and_is_idempotent() {
        let first = merge_install("", "/usr/local/bin/pixtuoid-hook").unwrap();
        assert!(first.changed);
        let doc: Value = serde_json::from_str(&first.content).unwrap();
        let hooks = doc["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), EVENTS.len());
        for ev in EVENTS {
            assert_eq!(
                hooks[*ev][0]["hooks"][0]["command"],
                json!("/usr/local/bin/pixtuoid-hook")
            );
            assert!(
                hooks[*ev][0].get(SENTINEL_KEY).is_none(),
                "Claude Code rejects unknown keys in a plugin's hooks.json"
            );
        }
        assert!(
            !merge_install(&first.content, "/usr/local/bin/pixtuoid-hook")
                .unwrap()
                .changed
        );
    }

    #[test]
    fn install_replaces_whatever_the_owned_file_held() {
        for prior in ["{not json", "[1, 2]", r#"{"hooks":{"Stop":[]}}"#] {
            let out = merge_install(prior, "pixtuoid-hook").unwrap();
            assert!(out.changed, "{prior}");
            let doc: Value = serde_json::from_str(&out.content).unwrap();
            assert!(doc["hooks"].get("Stop").is_none(), "{prior}");
        }
    }

    #[test]
    fn uninstall_empties_the_file_and_reports_a_change_only_with_hooks() {
        let installed = merge_install("", "pixtuoid-hook").unwrap().content;
        let out = merge_uninstall(&installed).unwrap();
        assert!(out.changed);
        assert!(!merge_uninstall(&out.content).unwrap().changed);
        assert!(!merge_uninstall("").unwrap().changed);
    }

    #[cfg(unix)]
    #[test]
    fn hook_command_explicit_path_is_embedded_and_stamped_on_unix() {
        let cmd = hook_command(Path::new("/opt/custom/pixtuoid-hook"), true).unwrap();
        assert_eq!(
            cmd,
            "PIXTUOID_SOURCE=claude-code '/opt/custom/pixtuoid-hook'"
        );
        let spaced = hook_command(Path::new("/Users/Jane Doe/bin/pixtuoid-hook"), true).unwrap();
        assert_eq!(
            spaced,
            "PIXTUOID_SOURCE=claude-code '/Users/Jane Doe/bin/pixtuoid-hook'"
        );
    }

    #[cfg(unix)]
    #[test]
    fn hook_command_auto_resolved_carries_the_source_on_unix() {
        let cmd = hook_command(Path::new("/usr/local/bin/pixtuoid-hook"), false).unwrap();
        assert_eq!(cmd, "PIXTUOID_SOURCE=claude-code 'pixtuoid-hook'");
    }

    #[cfg(unix)]
    #[test]
    fn claude_shim_ref_reads_the_auto_resolved_command_back_as_a_bare_name() {
        use crate::install::verify::ShimRef;
        let cmd = hook_command(Path::new("/usr/local/bin/pixtuoid-hook"), false).unwrap();
        let entry = serde_json::json!({ "hooks": [{ "command": cmd }] });
        assert_eq!(claude_shim_ref(&entry), ShimRef::BareName);
    }

    #[cfg(unix)]
    #[test]
    fn claude_shim_ref_survives_an_event_suffix_it_does_not_write_today() {
        use crate::install::verify::ShimRef;
        let entry = serde_json::json!({
            "hooks": [{ "command": "PIXTUOID_SOURCE=claude-code '/opt/pixtuoid-hook' --event PreToolUse" }]
        });
        assert_eq!(
            claude_shim_ref(&entry),
            ShimRef::Absolute(std::path::PathBuf::from("/opt/pixtuoid-hook"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn claude_shim_ref_recovers_a_single_quoted_path_with_an_apostrophe() {
        use crate::install::hook_cmd::unix::shell_single_quote;
        use crate::install::verify::ShimRef;
        let path = "/U/O'B/pixtuoid-hook";
        let cmd = shell_single_quote(path);
        assert!(
            cmd.contains("'\\''"),
            "expected an escaped apostrophe in {cmd:?}"
        );
        let entry = serde_json::json!({ "hooks": [{ "command": cmd }] });
        assert_eq!(
            claude_shim_ref(&entry),
            ShimRef::Absolute(std::path::PathBuf::from(path))
        );
    }

    #[test]
    fn claude_shim_ref_half_quoted_command_is_literal_not_unquoted() {
        use crate::install::verify::ShimRef;
        let entry = serde_json::json!({ "hooks": [{ "command": "'/opt/pixtuoid-hook" }] });
        assert_eq!(
            claude_shim_ref(&entry),
            ShimRef::Absolute(std::path::PathBuf::from("'/opt/pixtuoid-hook"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn hook_command_embeds_absolute_path_on_windows_either_way() {
        for explicit in [true, false] {
            let cmd = hook_command(Path::new(r"C:\tools\pixtuoid-hook.exe"), explicit).unwrap();
            assert_eq!(cmd, r"C:\tools\pixtuoid-hook.exe");
        }
    }

    #[test]
    fn windows_entry_is_exec_form_with_absolute_path() {
        let entry = hook_entry(r"C:\Users\user\.cargo\bin\pixtuoid-hook.exe", true);
        assert_eq!(entry["type"], json!("command"));
        assert_eq!(
            entry["command"],
            json!(r"C:\Users\user\.cargo\bin\pixtuoid-hook.exe")
        );
        assert_eq!(
            entry["args"],
            json!([]),
            "exec form must carry args:[] for shell-free spawn"
        );
    }

    #[test]
    fn unix_entry_stays_bare_shell_form() {
        let entry = hook_entry("pixtuoid-hook", false);
        assert_eq!(entry["type"], json!("command"));
        assert_eq!(entry["command"], json!("pixtuoid-hook"));
        assert!(
            entry.get("args").is_none(),
            "unix shell-form must NOT carry an args key (was: {entry})"
        );
    }

    #[test]
    fn every_registered_cc_event_decodes() {
        use pixtuoid_core::source::decoder::decode_hook_payload;
        for ev in EVENTS {
            let payload = serde_json::json!({
                "hook_event_name": ev,
                "session_id": "sess",
                "transcript_path": "/p/sess.jsonl",
                "cwd": "/repo",
                // Required by the SubagentStart/Stop arms; inert for every other event.
                "agent_id": "a0000000000000001",
            });
            assert!(
                decode_hook_payload(payload).is_ok(),
                "registered CC hook {ev:?} has no decoder arm — it would bail as \
                 unsupported. Add an arm in pixtuoid-core (decoder.rs shared arms \
                 or claude_code.rs's custom decoder)."
            );
        }
    }

    #[test]
    fn reinstall_adds_newly_registered_events_to_an_older_install() {
        let old_events = [
            "SessionStart",
            "PreToolUse",
            "PostToolUse",
            "Notification",
            "SessionEnd",
        ];
        let mut old = json!({ "hooks": {} });
        for ev in old_events {
            old["hooks"][ev] = json!([managed_entry("pixtuoid-hook")]);
        }
        let out = merge_install(&old.to_string(), "pixtuoid-hook").unwrap();
        assert!(out.changed, "adding the Subagent events is a real change");
        let v: Value = serde_json::from_str(&out.content).unwrap();
        for ev in EVENTS {
            assert!(
                v["hooks"][*ev][0]["hooks"][0]["command"].is_string(),
                "event {ev} must be installed after the upgrade re-run"
            );
        }
        let again = merge_install(&out.content, "pixtuoid-hook").unwrap();
        assert!(!again.changed, "second re-run is a semantic no-op");
    }

    #[test]
    fn claude_events_pins_the_exact_registered_set() {
        crate::install::assert_event_roster(
            "EVENTS",
            EVENTS,
            &[
                "SessionStart",
                "PreToolUse",
                "PostToolUse",
                "Notification",
                "PermissionRequest",
                "SubagentStart",
                "SubagentStop",
                "SessionEnd",
            ],
        );
    }
}
