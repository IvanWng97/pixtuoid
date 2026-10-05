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
    legacy_config: legacy_config_path,
    legacy_uninstall,
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

/// The settings.json earlier pixtuoid releases merged the hooks into.
fn legacy_config_path() -> Result<PathBuf> {
    if let Some(dir) = claude_config_dir() {
        return Ok(dir.join("settings.json"));
    }
    io::home_relative_checked(".claude/settings.json")
}

/// Claude Code is present when its config dir exists; the hooks file is ours, so
/// its absence says nothing.
pub(crate) fn detect_installed() -> bool {
    legacy_config_path()
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
        if is_registered()? {
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
    let settings = legacy_config_path()?;
    let content = io::read_config(&settings)?;
    if content.trim().is_empty() {
        return Ok(false);
    }
    let doc: Value = serde_json::from_str(&content)
        .with_context(|| format!("parsing {}", settings.display()))?;
    Ok(doc["enabledPlugins"][plugin_id()].as_bool() == Some(true))
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
/// rewrite. An explicit `--hook-path` overrides that — the user passed it precisely
/// because the binary is off-PATH — and is single-quoted, since CC runs shell-form
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

/// The inner hook object of a CC settings entry. `exec_form` adds the empty `args` key
/// that makes CC spawn the PE directly instead of through a shell; the `_pixtuoid`
/// sentinel and `matcher` live on the OUTER entry, not here.
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
        return SchemaParse::broken("settings.json no longer parses as JSON");
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
    hook_shim_ref(
        entry
            .get("hooks")
            .and_then(|h| h.as_array())
            .and_then(|a| a.first()),
    )
}

fn hook_shim_ref(hook: Option<&Value>) -> crate::install::verify::ShimRef {
    use crate::install::verify::ShimRef;
    match hook.and_then(|h| h.get("command")).and_then(|c| c.as_str()) {
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

/// Strips our entries from the settings.json an earlier pixtuoid merged into,
/// keeping everything else.
fn legacy_uninstall(content: &str) -> Result<MergeOutcome> {
    merge::flat_json_merge_outcome_uninstall(content, |mut doc| {
        let mut emptied = Vec::new();
        if let Some(Value::Object(hooks)) = doc.get_mut("hooks") {
            for (event, list) in hooks.iter_mut() {
                let Some(entries) = list.as_array_mut() else {
                    continue;
                };
                let before = entries.len();
                entries.retain_mut(|entry| {
                    let sentinel = entry.get(SENTINEL_KEY).and_then(Value::as_bool) == Some(true);
                    let Some(hs) = entry.get_mut("hooks").and_then(Value::as_array_mut) else {
                        return true;
                    };
                    let before = hs.len();
                    // A user's own hook sharing the group stays.
                    hs.retain(|h| !(hook_is_ours(h) || sentinel && is_our_exec_form(h)));
                    hs.len() == before || !hs.is_empty()
                });
                if entries.len() < before && entries.is_empty() {
                    emptied.push(event.clone());
                }
            }
            // Only what this strip emptied goes: a user's own empty list stays.
            for event in &emptied {
                hooks.remove(event);
            }
        }
        if !emptied.is_empty()
            && let Some(root) = doc.as_object_mut()
        {
            merge::prune_empty(root, "hooks");
        }
        doc
    })
}

/// A legacy hook is ours by its source stamp or the shim it runs. The sentinel an
/// entry carried isn't enough: Claude Code drops the unknown `_pixtuoid` key
/// whenever it rewrites settings.json, after which each re-install appended a
/// duplicate.
fn hook_is_ours(hook: &Value) -> bool {
    use crate::install::verify::ShimRef;
    let stamp = format!(
        "{}={}",
        crate::install::hook_cmd::SOURCE_ENV,
        pixtuoid_core::source::claude_code::SOURCE_NAME
    );
    hook["command"].as_str().is_some_and(|c| c.contains(&stamp))
        || match hook_shim_ref(Some(hook)) {
            ShimRef::BareName => true,
            ShimRef::Absolute(p) => p.file_stem().is_some_and(|s| s == "pixtuoid-hook"),
            ShimRef::Unknown => false,
        }
}

/// Windows' exec form carries no source stamp and may embed a renamed shim
/// (`--hook-path`), so inside a sentinel group its exact shape is what marks it.
fn is_our_exec_form(hook: &Value) -> bool {
    hook["command"]
        .as_str()
        .is_some_and(|c| *hook == hook_entry(c, true))
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
    fn legacy_config_path_honors_claude_config_dir() {
        let fallback_suffix = PathBuf::from(".claude").join("settings.json");

        temp_env::with_var_unset("CLAUDE_CONFIG_DIR", || {
            let unset_path = legacy_config_path().unwrap();
            assert!(
                unset_path.ends_with(&fallback_suffix),
                "default config path must end with .claude/settings.json, got {unset_path:?}"
            );
        });

        let custom_dir = std::env::temp_dir().join("pixtuoid-claude-config-dir");
        temp_env::with_var("CLAUDE_CONFIG_DIR", Some(&custom_dir), || {
            assert_eq!(
                legacy_config_path().unwrap(),
                custom_dir.join("settings.json")
            );
        });

        temp_env::with_var("CLAUDE_CONFIG_DIR", Some(""), || {
            let empty_path = legacy_config_path().unwrap();
            assert!(
                empty_path.ends_with(&fallback_suffix),
                "empty CLAUDE_CONFIG_DIR must fall back to .claude/settings.json, got {empty_path:?}"
            );
        });
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
    fn legacy_cleanup_finds_entries_claude_code_stripped_of_the_sentinel() {
        // Claude Code drops `_pixtuoid` when it rewrites settings.json; three
        // re-installs then left three copies, as one real config had.
        let ours = json!({ "matcher": ".*", "hooks": [{ "type": "command", "command": "pixtuoid-hook" }] });
        let prefixed = json!({ "hooks": [{ "type": "command", "command": "PIXTUOID_SOURCE=claude-code pixtuoid-hook" }] });
        let theirs = json!({ "hooks": [{ "type": "command", "command": "/usr/bin/say done" }] });
        let doc = json!({ "hooks": { "Stop": [ours.clone(), ours, prefixed, theirs.clone()] }, "theme": "dark" });
        let out = legacy_uninstall(&doc.to_string()).unwrap();
        assert!(out.changed);
        let cleaned: Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(
            cleaned,
            json!({ "hooks": { "Stop": [theirs] }, "theme": "dark" })
        );
    }

    #[test]
    fn legacy_cleanup_leaves_a_users_own_empty_hooks_alone() {
        for doc in [
            json!({ "hooks": { "Stop": [] }, "theme": "dark" }),
            json!({ "hooks": {} }),
        ] {
            assert!(
                !legacy_uninstall(&doc.to_string()).unwrap().changed,
                "{doc}"
            );
        }
    }

    #[test]
    fn legacy_cleanup_keeps_a_user_hook_sharing_our_group() {
        let mine = json!({ "type": "command", "command": "/usr/bin/say done" });
        let ours =
            json!({ "type": "command", "command": "PIXTUOID_SOURCE=claude-code 'pixtuoid-hook'" });
        // Also when the user hand-appended into a group that kept our sentinel.
        for group in [
            json!({ "matcher": ".*", "hooks": [ours.clone(), mine.clone()] }),
            json!({ SENTINEL_KEY: true, "matcher": ".*", "hooks": [ours.clone(), mine.clone()] }),
        ] {
            let mut kept = group.clone();
            kept["hooks"] = json!([mine]);
            let out =
                legacy_uninstall(&json!({ "hooks": { "Stop": [group] } }).to_string()).unwrap();
            assert!(out.changed);
            let cleaned: Value = serde_json::from_str(&out.content).unwrap();
            assert_eq!(cleaned, json!({ "hooks": { "Stop": [kept] } }));
        }
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

    #[test]
    fn legacy_cleanup_keeps_foreign_entries_and_settings() {
        let theirs =
            json!({ "matcher": "Write", "hooks": [{ "type": "command", "command": "/mine" }] });
        let ours = json!({ SENTINEL_KEY: true, "hooks": [{ "type": "command", "command": "/opt/pixtuoid-hook" }] });
        // The sentinel alone doesn't make a shell-form hook ours…
        let renamed = json!({ SENTINEL_KEY: true, "hooks": [{ "type": "command", "command": "/renamed/shim" }] });
        // …but the exec form Windows wrote, renamed shim and all, is.
        let windows =
            json!({ SENTINEL_KEY: true, "hooks": [hook_entry(r"C:\tools\pxhook.exe", true)] });
        // The same exec form outside our group is someone else's.
        let their_exec = json!({ "hooks": [hook_entry(r"C:\tools\other.exe", true)] });
        let doc = json!({ "hooks": { "PreToolUse": [theirs.clone(), ours, renamed.clone(), windows, their_exec.clone()], "Stop": "not-an-array" }, "theme": "dark" });
        let out = legacy_uninstall(&doc.to_string()).unwrap();
        assert!(out.changed);
        let cleaned: Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(
            cleaned,
            json!({ "hooks": { "PreToolUse": [theirs, renamed, their_exec], "Stop": "not-an-array" }, "theme": "dark" })
        );
        assert!(!legacy_uninstall(&out.content).unwrap().changed);
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
