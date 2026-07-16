//! grok (Grok Build) hook install target — a wholly-owned drop-in JSON file.
//!
//! grok discovers hooks from every `*.json` in `{grok_home}/hooks/` (global
//! scope, ALWAYS trusted — no trust-store entry needed), so pixtuoid writes its
//! OWN file `{grok_home}/hooks/pixtuoid.json` and never merges into a shared
//! settings file. Same ownership model as the opencode plugin: `merge_install`
//! renders the whole file, `merge_uninstall` replaces it with a sentinel-free
//! empty stub (the write-only orchestrator can't delete; grok loads the stub to
//! zero hooks). The top-level `_pixtuoid` key is the sentinel — grok's hook
//! config structs carry no `deny_unknown_fields`, so unknown sibling keys are
//! ignored (verified against xai-grok-hooks config.rs @ c68e39f6).
//!
//! **Source attribution rides the handler `env` map** (`PIXTUOID_SOURCE:
//! "grok"`), NOT a command argument: grok's runner injects handler env into the
//! hook process on every platform, and an argument-less absolute path avoids
//! grok's shell heuristic entirely in the common case (a command string with no
//! space/metachar is DIRECT-exec'd — no `sh -c`, no PowerShell). A path that
//! DOES carry a space or metachar takes the shell route, so `hook_command`
//! quotes it per-platform: the shared POSIX single-quote for Unix `sh -c`; on
//! Windows the DOS 8.3 short name first (metachar-free → back to direct exec,
//! shell-agnostic — the #195 trick), falling back to the PowerShell
//! call-operator `& '…'` only when 8.3 is disabled on the volume (then a
//! Git-Bash-detected or `GROK_SHELL=cmd` setup is the accepted residual). A
//! path containing `$` is REJECTED at install: grok env-expands command
//! strings at LOAD time, before quoting can protect anything.
//!
//! grok dispatches hooks SEQUENTIALLY and AWAITED INLINE on the session actor,
//! so every entry pins `timeout: 2` (seconds) — the shim's own bound is 200ms,
//! and 2s keeps a wedged shim from stalling grok the default 5s per event.
//!
//! Both `SubagentStop` AND `SubagentEnd` are registered: the docs name
//! `SubagentStop`, but upstream's subagent-finish file-hook dispatch keys the
//! `SubagentEnd` alias (updates.rs — file-registry lookup is exact-key, no
//! canonicalization), so registering only the documented spelling would never
//! fire. Drift-watched; the decoder claims both wire values.
//! `PreCompact`/`PostCompact` are deliberately unregistered (not agent
//! activity — the CC/Codex precedent).

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{json, Value};

use crate::install::target::MergeOutcome;
use crate::install::verify::{SchemaParse, ShimRef};

/// Top-level sentinel key + note. The KEY is the detection signal
/// (`merge_uninstall` / `verify_schema` / `detect_config` key on it); the value
/// is a human note for anyone opening the file.
const SENTINEL_KEY: &str = "_pixtuoid";
const SENTINEL_NOTE: &str =
    "managed by pixtuoid — disconnect grok in the Sources panel (s) to remove";

/// Written on uninstall: a valid hooks file registering nothing, WITHOUT the
/// sentinel, so a re-uninstall is a clean no-op. grok parses it to zero hooks.
const REMOVED_STUB: &str = "{\n  \"_note\": \"pixtuoid hooks removed by disconnecting grok in pixtuoid's Sources panel (press s).\",\n  \"hooks\": {}\n}\n";

/// Per-entry timeout (SECONDS — grok's settings-file unit, converted to ms
/// upstream). grok awaits hooks inline; the shim exits within its 200ms bound,
/// so 2s is 10× margin while capping a wedged shim at 2s instead of the 5s
/// default per event.
const HOOK_TIMEOUT_SECS: u64 = 2;

/// Registration keys (grok accepts PascalCase; the wire `hookEventName` values
/// are snake_case). Every entry here must decode —
/// `every_registered_grok_event_decodes` pins it.
pub(crate) const GROK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionDenied",
    "Notification",
    "Stop",
    "StopFailure",
    "SubagentStart",
    "SubagentStop",
    "SubagentEnd",
    "SessionEnd",
];

/// `{grok_home}/hooks/pixtuoid.json` — the SAME `grok_home()` resolution the
/// watcher's sessions root and the liveness probe ride (GROK_HOME
/// unconditional, else `~/.grok`), so the three can never disagree. HARD
/// error when neither GROK_HOME nor a home dir resolves: grok's hook
/// DISCOVERY gates on the fallible `user_grok_home()` (scans NOTHING in that
/// environment — util/hooks.rs), so writing into `grok_home()`'s degenerate
/// fallback would land hooks grok never reads (the home-anchored targets'
/// rule: error with "pass --config" instead).
pub(crate) fn default_config_path() -> Result<PathBuf> {
    if !home_resolvable(
        crate::install::io::nonempty_env("GROK_HOME").as_deref(),
        pixtuoid_core::platform::user_home_opt().as_deref(),
    ) {
        anyhow::bail!(
            "cannot resolve the home directory (HOME/USERPROFILE unset) and GROK_HOME is \
             unset — grok would not read hooks written to a fallback path; pass --config <path>"
        );
    }
    Ok(pixtuoid_core::source::grok::grok_home()
        .join("hooks")
        .join("pixtuoid.json"))
}

/// Mirrors upstream `user_grok_home()`'s resolvability gate (`GROK_HOME` set
/// OR a real home dir exists) — the condition under which grok's hook
/// discovery actually scans `{grok_home}/hooks`.
fn home_resolvable(grok_home_env: Option<&str>, home: Option<&str>) -> bool {
    grok_home_env.is_some() || home.is_some()
}

/// Presence probe for auto-detect: grok's OWN root (`grok_home()` — grok
/// `create_dir_all`s it on first run) or its canonical binary path. NOT our
/// hooks file — keying on our own artifact would chicken-and-egg auto-detection
/// (the opencode/Reasonix rule).
pub(crate) fn detect_installed() -> bool {
    let home = pixtuoid_core::source::grok::grok_home();
    home.join("bin").join(grok_binary_name()).exists() || home.join("sessions").exists()
}

fn grok_binary_name() -> &'static str {
    if cfg!(windows) {
        "grok.exe"
    } else {
        "grok"
    }
}

/// grok's own shell heuristic (runner/command.rs): a command containing any of
/// these (or starting `~`) runs via `sh -c` / PowerShell; otherwise it is
/// direct-exec'd. Mirrored so `hook_command` knows when quoting is needed.
fn needs_shell_route(cmd: &str) -> bool {
    cmd.starts_with('~')
        || cmd
            .chars()
            .any(|c| matches!(c, ' ' | '|' | '&' | ';' | '>' | '<' | '$'))
}

/// The command string: the bare absolute shim path when it direct-execs
/// (common case), else quoted for the shell grok will route it through. The
/// `--source` argument is deliberately absent — attribution rides the handler
/// `env` map, keeping the command argument-less (and therefore direct-exec'd)
/// on every platform.
///
/// A path containing `$` is REJECTED outright: grok env-expands `$VAR`/`${VAR}`
/// in the command string at LOAD time (before any quoting applies), so no
/// quoting can protect it — the expansion mangles the path (or refuses the
/// spawn) and the hooks silently never fire. Better a loud install error than
/// an installed-but-no-sprite.
pub(crate) fn hook_command(resolved: &Path, _explicit: bool) -> Result<String> {
    let path = crate::install::merge::hook_path_str(resolved)?;
    if path.contains('$') {
        anyhow::bail!(
            "the hook binary path {path:?} contains '$' — grok env-expands hook command \
             strings at load time, so this path cannot be installed faithfully; move the \
             shim or pass --hook-path"
        );
    }
    if !needs_shell_route(path) {
        return Ok(path.to_string());
    }
    #[cfg(unix)]
    {
        // The sh -c route: POSIX single-quoting (the shared helper — one
        // spelling of the escaping across all shell-quoting targets).
        Ok(crate::install::hook_cmd::unix::shell_single_quote(path))
    }
    #[cfg(windows)]
    {
        // Prefer the DOS 8.3 short name: metachar-free by construction, so
        // the command drops back to the argument-less direct-exec path — no
        // shell at all, immune to grok's shell cascade (pwsh vs Git Bash vs
        // GROK_SHELL=cmd all moot). The #195 trick Codex/Reasonix use.
        if let Some(short) = crate::install::hook_cmd::windows_short_path(path) {
            if !needs_shell_route(&short) {
                return Ok(short);
            }
        }
        // 8.3 unavailable (disabled on the volume): PowerShell call-operator
        // form — correct for grok's DEFAULT shells (pwsh → powershell.exe);
        // a Git-Bash-detected or GROK_SHELL=cmd setup with a spacey shim
        // path on an 8.3-less volume is the accepted residual (module doc).
        Ok(format!("& '{}'", path.replace('\'', "''")))
    }
}

/// Render the whole managed file (it is wholly ours). `changed` is a SEMANTIC
/// diff — parse both sides — so a hand-reformatted but equivalent file is a
/// no-op and never churns a backup.
pub(crate) fn merge_install(content: &str, hook_cmd: &str) -> Result<MergeOutcome> {
    let rendered = render_hooks_file(hook_cmd);
    let existing: Value = serde_json::from_str(content.trim()).unwrap_or_else(|_| json!({}));
    Ok(MergeOutcome {
        changed: existing != rendered,
        content: format!("{}\n", serde_json::to_string_pretty(&rendered)?),
    })
}

/// Replace our file with the sentinel-free stub. A foreign file (no sentinel
/// key), an already-removed stub, or empty content is a semantic no-op.
pub(crate) fn merge_uninstall(content: &str) -> Result<MergeOutcome> {
    let ours = serde_json::from_str::<Value>(content.trim())
        .ok()
        .is_some_and(|v| v.get(SENTINEL_KEY).is_some());
    Ok(MergeOutcome {
        changed: ours,
        content: if ours {
            REMOVED_STUB.to_string()
        } else {
            content.to_string()
        },
    })
}

fn render_hooks_file(hook_cmd: &str) -> Value {
    let mut hooks = serde_json::Map::new();
    for event in GROK_EVENTS {
        // NO `matcher` key on ANY group: upstream rejects a matcher on a
        // lifecycle event (SessionStart, SessionEnd, Stop, UserPromptSubmit)
        // as a per-GROUP load error — that group's hooks silently never fire
        // — and absent == match-all for the tool events.
        hooks.insert(
            (*event).to_string(),
            json!([{
                "hooks": [{
                    "type": "command",
                    "command": hook_cmd,
                    "timeout": HOOK_TIMEOUT_SECS,
                    "env": { "PIXTUOID_SOURCE": pixtuoid_core::source::grok::SOURCE_NAME }
                }]
            }]),
        );
    }
    json!({ SENTINEL_KEY: SENTINEL_NOTE, "hooks": hooks })
}

/// Install-schema verification (#309): sentinel present, EVERY registered
/// event still carries a managed handler (catches an older install missing
/// newly-registered events), the attribution env intact, no stray matcher (a
/// hand-added matcher on a lifecycle event is rejected per-GROUP upstream —
/// that event's hook silently never fires, the half-dead class), and the shim
/// path extracted for the on-disk stat.
pub(crate) fn verify_schema(content: &str) -> SchemaParse {
    let Ok(doc) = serde_json::from_str::<Value>(content.trim()) else {
        return SchemaParse::broken("~/.grok/hooks/pixtuoid.json does not parse as JSON");
    };
    if doc.get(SENTINEL_KEY).is_none() {
        return SchemaParse::broken(
            "the grok hooks file is missing or replaced (sentinel absent) — reconnect grok",
        );
    }
    let mut issues = Vec::new();
    let mut shim = ShimRef::Unknown;
    let hooks = doc.get("hooks").and_then(|h| h.as_object());
    for event in GROK_EVENTS {
        let Some(groups) = hooks.and_then(|h| h.get(*event)).and_then(|g| g.as_array()) else {
            issues.push(format!(
                "registered grok event {event} has no hook entry — an older install; reconnect grok"
            ));
            continue;
        };
        for group in groups {
            if group.get("matcher").is_some() {
                issues.push(format!(
                    "grok event {event} carries a matcher — upstream rejects the group \
                     (that event's hook never fires); reconnect grok"
                ));
            }
            for handler in group
                .get("hooks")
                .and_then(|h| h.as_array())
                .into_iter()
                .flatten()
            {
                if handler
                    .pointer("/env/PIXTUOID_SOURCE")
                    .and_then(|v| v.as_str())
                    != Some(pixtuoid_core::source::grok::SOURCE_NAME)
                {
                    issues.push(format!(
                        "grok event {event}'s handler lost its PIXTUOID_SOURCE env — events \
                         would attribute to claude-code and be dropped; reconnect grok"
                    ));
                }
                if let Some(cmd) = handler.get("command").and_then(|c| c.as_str()) {
                    if let Some(path) = extract_shim_path(cmd) {
                        shim = ShimRef::Absolute(path);
                    }
                }
            }
        }
    }
    if matches!(shim, ShimRef::Unknown) {
        issues.push("could not read the shim path from the grok hooks file".to_string());
    }
    SchemaParse { issues, shim }
}

/// Undo `hook_command`'s three shapes: bare path, `'…'` (Unix shell route),
/// `& '…'` (PowerShell route).
fn extract_shim_path(cmd: &str) -> Option<PathBuf> {
    let cmd = cmd.trim();
    let inner = if let Some(rest) = cmd.strip_prefix("& ") {
        rest.trim()
            .strip_prefix('\'')?
            .strip_suffix('\'')?
            .replace("''", "'")
    } else if cmd.starts_with('\'') {
        cmd.strip_prefix('\'')?
            .strip_suffix('\'')?
            .replace(r"'\''", "'")
    } else {
        cmd.to_string()
    };
    (!inner.is_empty()).then(|| PathBuf::from(inner))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_renders_every_registered_event_with_env_attribution() {
        let out = merge_install("", "/opt/bin/pixtuoid-hook").unwrap();
        assert!(out.changed);
        let doc: Value = serde_json::from_str(&out.content).unwrap();
        assert!(doc.get(SENTINEL_KEY).is_some(), "sentinel present");
        let hooks = doc["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), GROK_EVENTS.len());
        for event in GROK_EVENTS {
            let handler = &hooks[*event][0]["hooks"][0];
            assert_eq!(handler["command"], "/opt/bin/pixtuoid-hook");
            assert_eq!(handler["env"]["PIXTUOID_SOURCE"], "grok");
            assert_eq!(handler["timeout"], HOOK_TIMEOUT_SECS);
            assert!(
                hooks[*event][0].get("matcher").is_none(),
                "{event}: no matcher key — lifecycle events reject matchers upstream"
            );
        }
    }

    #[test]
    fn install_is_idempotent_and_reformat_tolerant() {
        let a = merge_install("", "/opt/bin/pixtuoid-hook").unwrap();
        let b = merge_install(&a.content, "/opt/bin/pixtuoid-hook").unwrap();
        assert!(!b.changed, "same-path re-install is a semantic no-op");
        // A hand-reformatted but EQUIVALENT file is also a no-op (parsed diff).
        let reformatted =
            serde_json::to_string(&serde_json::from_str::<Value>(&a.content).unwrap()).unwrap();
        assert!(
            !merge_install(&reformatted, "/opt/bin/pixtuoid-hook")
                .unwrap()
                .changed
        );
        // A path change re-renders.
        let c = merge_install(&a.content, "/usr/local/bin/pixtuoid-hook").unwrap();
        assert!(c.changed);
    }

    #[test]
    fn uninstall_writes_the_sentinel_free_stub_and_round_trips() {
        let installed = merge_install("", "/opt/bin/pixtuoid-hook").unwrap();
        let removed = merge_uninstall(&installed.content).unwrap();
        assert!(removed.changed);
        let doc: Value = serde_json::from_str(&removed.content).unwrap();
        assert!(doc.get(SENTINEL_KEY).is_none(), "stub drops the sentinel");
        assert_eq!(doc["hooks"], json!({}), "stub registers zero hooks");
        // Re-uninstall, a foreign file, and empty content are no-ops.
        assert!(!merge_uninstall(&removed.content).unwrap().changed);
        assert!(
            !merge_uninstall(r#"{"hooks":{"PreToolUse":[]}}"#)
                .unwrap()
                .changed
        );
        assert!(!merge_uninstall("").unwrap().changed);
    }

    #[test]
    fn hook_command_is_bare_when_direct_execable_and_quoted_otherwise() {
        // The common case: no space/metachar → bare path, direct exec.
        assert_eq!(
            hook_command(Path::new("/opt/bin/pixtuoid-hook"), false).unwrap(),
            "/opt/bin/pixtuoid-hook"
        );
        // A spacey path takes the shell route → per-platform quoting. (On
        // Windows the 8.3 short name is preferred first, but this fixture
        // path doesn't exist → GetShortPathNameW fails → the `& '…'` form.)
        let spacey = hook_command(Path::new("/Users/Foo Bar/bin/pixtuoid-hook"), false).unwrap();
        #[cfg(unix)]
        assert_eq!(spacey, "'/Users/Foo Bar/bin/pixtuoid-hook'");
        #[cfg(windows)]
        assert_eq!(spacey, "& '/Users/Foo Bar/bin/pixtuoid-hook'");
        // Round-trip: verify can extract the path back from every shape.
        for p in ["/opt/bin/pixtuoid-hook", "/Users/Foo Bar/bin/pixtuoid-hook"] {
            let cmd = hook_command(Path::new(p), false).unwrap();
            assert_eq!(extract_shim_path(&cmd), Some(PathBuf::from(p)), "{cmd}");
        }
    }

    #[test]
    fn hook_command_rejects_a_dollar_path_loudly() {
        // grok env-expands $VAR in command strings at LOAD time — quoting
        // can't protect a `$`-carrying path, so installing it would be the
        // silent installed-but-no-sprite class. Loud install error instead.
        assert!(hook_command(Path::new("/opt/$weird/pixtuoid-hook"), false).is_err());
    }

    #[test]
    fn config_path_requires_a_resolvable_home_or_grok_home() {
        // Mirrors upstream user_grok_home(): grok's hook discovery scans
        // NOTHING when neither resolves — an install must error, not write a
        // file grok never reads.
        assert!(home_resolvable(Some("/custom"), None));
        assert!(home_resolvable(None, Some("/home/u")));
        assert!(home_resolvable(Some("/custom"), Some("/home/u")));
        assert!(!home_resolvable(None, None));
    }

    #[test]
    fn verify_flags_missing_events_lost_env_and_foreign_files() {
        let installed = merge_install("", "/opt/bin/pixtuoid-hook").unwrap();
        let sound = verify_schema(&installed.content);
        assert!(sound.issues.is_empty(), "{:?}", sound.issues);
        assert_eq!(
            sound.shim,
            ShimRef::Absolute(PathBuf::from("/opt/bin/pixtuoid-hook"))
        );

        // An OLDER install missing a newly-registered event is half-dead.
        let mut doc: Value = serde_json::from_str(&installed.content).unwrap();
        doc["hooks"].as_object_mut().unwrap().remove("SubagentEnd");
        let half_dead = verify_schema(&doc.to_string());
        assert!(half_dead.issues.iter().any(|i| i.contains("SubagentEnd")));

        // A lost attribution env would mis-tag every event.
        let mut doc: Value = serde_json::from_str(&installed.content).unwrap();
        doc["hooks"]["Stop"][0]["hooks"][0]["env"] = json!({});
        assert!(verify_schema(&doc.to_string())
            .issues
            .iter()
            .any(|i| i.contains("PIXTUOID_SOURCE")));

        // A hand-added matcher would kill the whole file upstream.
        let mut doc: Value = serde_json::from_str(&installed.content).unwrap();
        doc["hooks"]["SessionStart"][0]["matcher"] = json!("*");
        assert!(verify_schema(&doc.to_string())
            .issues
            .iter()
            .any(|i| i.contains("matcher")));

        // Foreign / unparseable files are broken with the sentinel message.
        assert!(!verify_schema(r#"{"hooks":{}}"#).issues.is_empty());
        assert!(!verify_schema("not json").issues.is_empty());
    }

    /// Internal-consistency guard (the CC/Codex/Reasonix/CodeWhale mirror):
    /// every event we REGISTER must decode — a registered-but-undecoded event
    /// is the original SubagentStop bug class. Wire values are the snake_case
    /// serialization of the registration keys.
    #[test]
    fn every_registered_grok_event_decodes() {
        use pixtuoid_core::source::decoder::decode_hook_payload;
        for event in GROK_EVENTS {
            // PascalCase registration key → snake_case wire value.
            let mut wire = String::new();
            for (i, c) in event.chars().enumerate() {
                if c.is_uppercase() {
                    if i > 0 {
                        wire.push('_');
                    }
                    wire.extend(c.to_lowercase());
                } else {
                    wire.push(c);
                }
            }
            let mut payload = serde_json::json!({
                "_pixtuoid_source": "grok",
                "hookEventName": wire,
                "sessionId": "0197fa30-sess",
                "cwd": "/repo",
                "workspaceRoot": "/repo",
                "timestamp": "2026-07-16T12:00:00Z"
            });
            match *event {
                "PreToolUse" | "PostToolUse" | "PostToolUseFailure" | "PermissionDenied" => {
                    payload["toolName"] = serde_json::json!("run_terminal_command");
                    payload["toolUseId"] = serde_json::json!("c1");
                    payload["toolInput"] = serde_json::json!({"command": "ls"});
                }
                "Notification" => {
                    payload["notificationType"] = serde_json::json!("permission_prompt");
                }
                "SubagentStart" | "SubagentStop" | "SubagentEnd" => {
                    payload["subagentId"] = serde_json::json!("0197fa31-child");
                    payload["subagentType"] = serde_json::json!("explore");
                }
                _ => {}
            }
            assert!(
                decode_hook_payload(payload).is_ok(),
                "registered grok event {event} (wire {wire}) failed to decode — \
                 add an arm in source/grok.rs"
            );
        }
    }
}
