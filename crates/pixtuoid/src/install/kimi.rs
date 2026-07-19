//! Kimi Code CLI hook install target.
//!
//! Writes the GLOBAL Kimi config (`<KIMI_CODE_HOME>/config.toml`, default
//! `~/.kimi-code/config.toml`) — Kimi's documented hooks live in a top-level
//! `[[hooks]]` array of tables:
//!
//! ```toml
//! [[hooks]]
//! event = "PreToolUse"
//! command = "PIXTUOID_SOURCE=kimi '/abs/pixtuoid-hook'"
//! timeout = 5
//! ```
//!
//! Load-bearing details (all confirmed against the repo's raw docs):
//! - **NO `_pixtuoid` sentinel.** Kimi's `[[hooks]]` allows EXACTLY four fields
//!   (`event`/`matcher`/`command`/`timeout`) and *"extra fields will cause the
//!   config file to fail to load"* (hooks.md). Every OTHER shared-config target
//!   tags its entries with the `_pixtuoid` key; Kimi is the first that can't. So
//!   managed-entry detection keys on the **command's source marker** instead
//!   (`PIXTUOID_SOURCE=kimi` on Unix / ` --source kimi` on Windows — see
//!   [`is_managed_command`]), which is path-independent so it still matches after
//!   a shim-path change. `verify::shell_shim_ref` extracts the shim from the same
//!   command shape the shared shell targets write.
//! - **One command for all events.** Kimi's shim reads the event off stdin
//!   (`hook_event_name` in the payload), like Claude/Codex/Cursor — NOT baked per
//!   entry like CodeWhale — so every `[[hooks]]` entry carries the identical
//!   command. `matcher` is OMITTED (match every tool).
//! - **Shell execution.** Kimi runs the `command` under a shell (the doc's
//!   `terminal-notifier … 'Task done'` example needs shell quote-parsing, and the
//!   exit-code 0/2 + stdout/stderr contract is the shell-hook model), so the OS
//!   forms mirror Codex/CodeWhale exactly via `hook_cmd::shell_hook_command`: Unix
//!   env-prefix `PIXTUOID_SOURCE=kimi '<path>'`, Windows bare `<path> --source
//!   kimi`. (CAPTURE-GATED, like Cursor: if a live run proves Kimi argv-execs the
//!   command WITHOUT a shell, switch to `exec_hook_command` — the Hermes form.)
//! - **`timeout`.** Bounds worst-case latency for the blocking-capable events
//!   (`PreToolUse`/`Stop`); the shim exits within 200ms by contract, and Kimi
//!   fails OPEN on a non-zero/timeout, so a hung shim never blocks the agent.
//! - Comments/ordering are lost on the `toml::Value` round-trip (a backup is
//!   taken) — same accepted caveat as Codex/CodeWhale.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use toml::value::Table;

use crate::install::io;
use crate::install::target::MergeOutcome;
use pixtuoid_core::source::kimi::SOURCE_NAME;

/// Events we register == events we decode (`pixtuoid_core::source::kimi` + the
/// shared CC-shaped arms), enforced by `every_registered_kimi_event_decodes`.
/// The lifecycle core (`SessionStart`/`PreToolUse`/`PostToolUse`/`Stop`/
/// `SessionEnd`) rides the shared arms; `PermissionRequest` gives the Waiting
/// state; the two `*Failure` variants close a failed tool/turn via the source's
/// custom `Extend` decoder (a failed tool fires `PostToolUseFailure`, which must
/// still end the activity — the Cursor lesson). `UserPromptSubmit`/`Subagent*`/
/// `PreCompact`/`Interrupt`/`Notification`/`PermissionResult` are deliberately
/// unregistered (no lifecycle meaning here, or uncaptured payload shape).
const KIMI_EVENTS: &[&str] = &[
    "SessionStart",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "Stop",
    "StopFailure",
    "SessionEnd",
];

/// Per-entry `timeout` (seconds). The shim's contract is a ≤200ms exit, and Kimi
/// fails OPEN on a non-zero/timeout, so this only bounds a pathologically hung
/// shim on the blocking-capable events (`PreToolUse`/`Stop`). Kimi's range is
/// 1–600 (hooks.md); 5s is a generous ceiling over the 200ms bound.
const KIMI_HOOK_TIMEOUT_SECS: i64 = 5;

/// The data-root dir Kimi actually reads: `$KIMI_CODE_HOME` verbatim (the ONE
/// documented override — env-vars.md; no `KIMI_HOME`/`KIMI_CONFIG_DIR`), else
/// `<home>/.kimi-code` (Node `os.homedir()`, USERPROFILE-first on Windows).
/// `config.toml` lives directly in it (data-locations.md). Env is taken verbatim
/// (no `~`-expand): the documented value is already shell-expanded/absolute, the
/// CodeWhale/Reasonix trim-only posture (#342).
fn kimi_config_dir() -> Option<PathBuf> {
    resolve_config_dir(
        io::nonempty_env("KIMI_CODE_HOME"),
        pixtuoid_core::platform::user_home_opt(),
    )
}

/// Pure core for [`kimi_config_dir`] — the env override and home are injected so
/// both arms unit-test without env/FS mutation.
fn resolve_config_dir(kimi_code_home_env: Option<String>, home: Option<String>) -> Option<PathBuf> {
    if let Some(h) = kimi_code_home_env {
        return Some(PathBuf::from(h));
    }
    home.map(|h| PathBuf::from(h).join(".kimi-code"))
}

pub(crate) fn default_config_path() -> Result<PathBuf> {
    kimi_config_dir()
        .map(|d| d.join("config.toml"))
        .ok_or_else(|| {
            anyhow!(
                "cannot resolve the home directory (HOME/USERPROFILE unset); pass --config <path>"
            )
        })
}

/// Presence probe for auto-detection. Kimi may not create `config.toml` until the
/// user configures a provider, but it creates its data root (`~/.kimi-code`, or
/// `KIMI_CODE_HOME`) on first run — probe that dir, not the file we write (the
/// Reasonix/CodeWhale rule).
pub(crate) fn detect_installed() -> bool {
    kimi_config_dir().is_some_and(|d| d.exists())
}

/// Kimi runs the `command` under a shell (module doc), so the OS forms mirror
/// Codex/CodeWhale: Unix env-prefix `PIXTUOID_SOURCE=kimi '<abs>'`, Windows bare
/// `<abs> --source kimi` (8.3 short-name for a space/metacharacter path, else
/// reject). Err on non-UTF-8 (prevents the to_string_lossy dead-hook).
pub(crate) fn hook_command(resolved: &Path, _explicit: bool) -> Result<String> {
    let p = crate::install::merge::hook_path_str(resolved)?;
    crate::install::hook_cmd::shell_hook_command(p, SOURCE_NAME)
}

pub(crate) fn merge_install(content: &str, hook_cmd: &str) -> Result<MergeOutcome> {
    crate::install::merge::toml_merge_outcome(content, |doc| toml_merge_install(doc, hook_cmd))
}

pub(crate) fn merge_uninstall(content: &str) -> Result<MergeOutcome> {
    crate::install::merge::toml_merge_outcome(content, toml_merge_uninstall)
}

/// The `PIXTUOID_SOURCE=kimi` env-prefix marker (Unix `shell_hook_command` form).
/// Pinned to `shell_hook_command`'s spelling by the install round-trip tests
/// (a managed entry we WRITE must be one we DETECT).
fn env_marker() -> String {
    format!("PIXTUOID_SOURCE={SOURCE_NAME}")
}

/// The ` --source kimi` flag marker (Windows bare form + any `--source` form).
fn flag_marker() -> String {
    format!("{}{SOURCE_NAME}", crate::install::hook_cmd::SOURCE_FLAG)
}

/// A `[[hooks]]` entry is OURS iff its `command` carries the kimi source marker.
/// Kimi's config forbids the `_pixtuoid` sentinel (module doc), so this
/// command-substring test replaces it. Both platform forms are checked so a
/// config synced across OSes still round-trips. Keyed on the SOURCE-specific
/// marker (`=kimi` / ` kimi`), not the shim basename, so it never removes a
/// different pixtuoid source's entry (there is none in Kimi's config, but the
/// specificity is free).
fn is_managed_command(command: &str) -> bool {
    command.contains(&env_marker()) || command.contains(&flag_marker())
}

fn is_managed_entry(entry: &toml::Value) -> bool {
    entry
        .get("command")
        .and_then(|c| c.as_str())
        .is_some_and(is_managed_command)
}

fn managed_entry(event: &str, hook_cmd: &str) -> toml::Value {
    let mut t = Table::new();
    t.insert("event".into(), toml::Value::String(event.into()));
    t.insert("command".into(), toml::Value::String(hook_cmd.into()));
    t.insert(
        "timeout".into(),
        toml::Value::Integer(KIMI_HOOK_TIMEOUT_SECS),
    );
    toml::Value::Table(t)
}

fn toml_merge_install(doc: toml::Value, hook_cmd: &str) -> toml::Value {
    let mut root = doc.as_table().cloned().unwrap_or_default();
    let arr = root
        .entry("hooks".to_string())
        .or_insert_with(|| toml::Value::Array(vec![]));
    // Defensive: coerce a non-array `hooks` (a hand-edited scalar) to an array.
    if !arr.is_array() {
        *arr = toml::Value::Array(vec![]);
    }
    if let Some(arr) = arr.as_array_mut() {
        // Replace prior managed entries (idempotent across path changes), keep
        // the user's own hook entries untouched.
        arr.retain(|e| !is_managed_entry(e));
        for ev in KIMI_EVENTS {
            arr.push(managed_entry(ev, hook_cmd));
        }
    }
    toml::Value::Table(root)
}

fn toml_merge_uninstall(mut doc: toml::Value) -> toml::Value {
    let Some(root) = doc.as_table_mut() else {
        return doc;
    };
    if let Some(arr) = root.get_mut("hooks").and_then(|h| h.as_array_mut()) {
        arr.retain(|e| !is_managed_entry(e));
    }
    // Drop the array once ours were the only entries (a user's own hooks keep it).
    if root
        .get("hooks")
        .and_then(|h| h.as_array())
        .is_some_and(|a| a.is_empty())
    {
        root.remove("hooks");
    }
    doc
}

/// Install-schema verification (#309): every `KIMI_EVENTS` event still has a
/// managed `[[hooks]]` entry (detected by the command marker, not a sentinel),
/// and the shim path extracted for `install::verify_target` to stat.
pub(crate) fn verify_schema(content: &str) -> crate::install::verify::SchemaParse {
    use crate::install::verify::{assemble, shell_shim_ref, SchemaParse, ShimRef};
    let Ok(doc) = toml::from_str::<toml::Value>(content) else {
        return SchemaParse::broken("config.toml no longer parses as TOML");
    };
    let entries: Vec<&toml::Value> = doc
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|a| a.iter().filter(|e| is_managed_entry(e)).collect())
        .unwrap_or_default();
    // No managed entries → give Kimi-accurate wording (the shared `assemble`
    // no-managed message names the `_pixtuoid` sentinel Kimi can't carry).
    if entries.is_empty() {
        return SchemaParse::broken(
            "no managed pixtuoid hook entries in [[hooks]] (the pixtuoid source marker \
             `PIXTUOID_SOURCE=kimi` / `--source kimi` is absent — the config was hand-edited \
             or hooks were never installed)",
        );
    }
    let mut missing = Vec::new();
    let mut shim = ShimRef::Unknown;
    for ev in KIMI_EVENTS {
        match entries
            .iter()
            .find(|e| e.get("event").and_then(|v| v.as_str()) == Some(ev))
        {
            Some(e) => {
                if shim == ShimRef::Unknown {
                    shim = e
                        .get("command")
                        .and_then(|c| c.as_str())
                        .map(shell_shim_ref)
                        .unwrap_or(ShimRef::Unknown);
                }
            }
            None => missing.push(*ev),
        }
    }
    assemble(&missing, true, shim, vec![])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> toml::Value {
        toml::from_str(s).unwrap()
    }

    const CMD: &str = "PIXTUOID_SOURCE=kimi '/opt/bin/pixtuoid-hook'";

    #[test]
    fn config_dir_honors_kimi_code_home_then_default() {
        // KIMI_CODE_HOME wins verbatim (the documented override — env-vars.md).
        assert_eq!(
            resolve_config_dir(Some("/custom/kimi".into()), Some("/home/u".into())),
            Some(PathBuf::from("/custom/kimi"))
        );
        // Else <home>/.kimi-code (data-locations.md default).
        assert_eq!(
            resolve_config_dir(None, Some("/home/u".into())),
            Some(PathBuf::from("/home/u").join(".kimi-code"))
        );
        // No home + no override → None (installer surfaces "pass --config").
        assert_eq!(resolve_config_dir(None, None), None);
    }

    #[test]
    fn default_config_path_honors_kimi_code_home_env() {
        // std::env is process-global; serialize against other env-mutating tests.
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var_os("KIMI_CODE_HOME");

        // KIMI_CODE_HOME set → <dir>/config.toml VERBATIM (no exists-gate, unlike
        // codex). Unconditional (not `if let Ok`) so a mutation making
        // default_config_path always-Err is CAUGHT, not skipped.
        let custom = std::env::temp_dir().join("pixtuoid-kimi-home-cfg-test");
        std::env::set_var("KIMI_CODE_HOME", &custom);
        assert_eq!(default_config_path().unwrap(), custom.join("config.toml"));

        // Empty → treated as unset (nonempty_env trims) → falls back to
        // <home>/.kimi-code; assert only the filename when a home resolves (CI
        // always has one, a stripped env legitimately errs).
        std::env::set_var("KIMI_CODE_HOME", "");
        if let Ok(p) = default_config_path() {
            assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("config.toml"));
        }

        match saved {
            Some(v) => std::env::set_var("KIMI_CODE_HOME", v),
            None => std::env::remove_var("KIMI_CODE_HOME"),
        }
    }

    #[test]
    fn detect_installed_probes_the_data_root_not_the_config_file() {
        // std::env is process-global; serialize against other env-mutating tests.
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var_os("KIMI_CODE_HOME");

        let root = std::env::temp_dir().join("pixtuoid-kimi-detect-test");
        let _ = std::fs::remove_dir_all(&root);
        std::env::set_var("KIMI_CODE_HOME", &root);
        assert!(!detect_installed(), "an absent data root must not detect");

        // Create the data root WITHOUT a config.toml → still detected: Kimi
        // creates the root on first run before any config is written, so we probe
        // the dir, not the file we'd write (the Reasonix/CodeWhale rule).
        std::fs::create_dir_all(&root).unwrap();
        assert!(
            detect_installed(),
            "an existing data root must detect even with no config.toml"
        );

        match saved {
            Some(v) => std::env::set_var("KIMI_CODE_HOME", v),
            None => std::env::remove_var("KIMI_CODE_HOME"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn install_creates_one_entry_per_event_with_command_and_timeout() {
        let out = merge_install("", CMD).unwrap();
        assert!(out.changed);
        let v = parse(&out.content);
        let arr = v["hooks"].as_array().unwrap();
        assert_eq!(arr.len(), KIMI_EVENTS.len());
        for (entry, ev) in arr.iter().zip(KIMI_EVENTS) {
            assert_eq!(entry["event"].as_str().unwrap(), *ev);
            assert_eq!(entry["command"].as_str().unwrap(), CMD);
            assert_eq!(
                entry["timeout"].as_integer().unwrap(),
                KIMI_HOOK_TIMEOUT_SECS
            );
            // No sentinel field — Kimi's [[hooks]] rejects unknown fields.
            assert!(
                entry.get("_pixtuoid").is_none(),
                "must NOT write a sentinel (Kimi rejects extra fields)"
            );
            // The command marker is what makes the entry detectable as ours.
            assert!(is_managed_entry(entry), "our entry must be self-detecting");
        }
    }

    #[test]
    fn install_is_idempotent_and_replaces_across_paths() {
        let a = merge_install("", CMD).unwrap();
        let b = merge_install(&a.content, CMD).unwrap();
        assert!(!b.changed, "same-command re-install is a semantic no-op");
        // A path change replaces (does not duplicate) the managed entries — the
        // marker is path-independent, so the old entries are still detected.
        let c = merge_install(
            &a.content,
            "PIXTUOID_SOURCE=kimi '/usr/local/bin/pixtuoid-hook'",
        )
        .unwrap();
        assert_eq!(
            parse(&c.content)["hooks"].as_array().unwrap().len(),
            KIMI_EVENTS.len(),
            "path change must not duplicate entries"
        );
    }

    #[test]
    fn install_preserves_user_hooks_and_other_keys() {
        let user = r#"
default_model = "kimi-for-coding"

[providers.moonshot]
api_key = "secret"

[[hooks]]
event = "Notification"
command = "terminal-notifier -message done"
"#;
        let out = merge_install(user, CMD).unwrap();
        let v = parse(&out.content);
        assert_eq!(v["default_model"].as_str(), Some("kimi-for-coding"));
        assert_eq!(
            v["providers"]["moonshot"]["api_key"].as_str(),
            Some("secret"),
            "unrelated provider/key config survives"
        );
        let arr = v["hooks"].as_array().unwrap();
        // user's 1 + every managed kimi event
        assert_eq!(arr.len(), 1 + KIMI_EVENTS.len());
        assert!(
            arr.iter()
                .any(|e| e["command"].as_str() == Some("terminal-notifier -message done")),
            "the user's own hook must be preserved"
        );
    }

    #[test]
    fn uninstall_removes_only_managed_entries() {
        let user = "[[hooks]]\nevent = \"Notification\"\ncommand = \"my-own-hook\"\n";
        let installed = merge_install(user, CMD).unwrap();
        let cleaned = merge_uninstall(&installed.content).unwrap();
        assert!(cleaned.changed);
        let v = parse(&cleaned.content);
        let arr = v["hooks"].as_array().unwrap();
        assert_eq!(arr.len(), 1, "only the user's own hook remains");
        assert_eq!(arr[0]["command"].as_str(), Some("my-own-hook"));
    }

    #[test]
    fn uninstall_of_pixtuoid_only_install_drops_the_hooks_array() {
        let installed = merge_install("", CMD).unwrap();
        let cleaned = merge_uninstall(&installed.content).unwrap();
        let v = parse(&cleaned.content);
        assert!(
            v.get("hooks").is_none(),
            "a pixtuoid-only [[hooks]] must be fully removed, got {v}"
        );
    }

    #[test]
    fn uninstall_no_managed_hooks_is_a_noop() {
        let user = "[[hooks]]\nevent = \"Notification\"\ncommand = \"echo hi\"\n";
        let out = merge_uninstall(user).unwrap();
        assert!(!out.changed, "no managed entries → semantic no-op");
    }

    #[test]
    fn merge_install_rejects_invalid_toml() {
        // A malformed config must NOT be overwritten (it'd wipe the user's
        // provider/key/hooks); refuse instead.
        assert!(merge_install("not = valid = toml", CMD).is_err());
    }

    #[test]
    fn install_coerces_a_non_array_hooks_key() {
        let out = merge_install("hooks = \"garbage\"\n", CMD).unwrap();
        let v = parse(&out.content);
        assert!(v["hooks"].is_array());
        assert_eq!(v["hooks"].as_array().unwrap().len(), KIMI_EVENTS.len());
    }

    #[test]
    fn verify_schema_passes_full_install_and_flags_missing_and_absent() {
        use crate::install::verify::ShimRef;
        // (1) A complete managed install verifies clean with a real shim ref.
        let full = merge_install("", CMD).unwrap().content;
        let sound = verify_schema(&full);
        assert!(sound.issues.is_empty(), "{:?}", sound.issues);
        assert_ne!(sound.shim, ShimRef::Unknown);
        assert_eq!(
            sound.shim,
            ShimRef::Absolute(PathBuf::from("/opt/bin/pixtuoid-hook"))
        );

        // (2) A partial install (one event only) reports the rest missing.
        let partial = "[[hooks]]\nevent = \"SessionStart\"\ncommand = \"PIXTUOID_SOURCE=kimi '/x/pixtuoid-hook'\"\n";
        let p = verify_schema(partial);
        let joined = p.issues.join(" | ");
        assert!(
            joined.contains("missing hook entries for") && joined.contains("PreToolUse"),
            "a partial install must list missing events, got {:?}",
            p.issues
        );

        // (3) No managed entries at all → the Kimi-accurate no-marker message.
        let none = verify_schema("[[hooks]]\nevent = \"Notification\"\ncommand = \"echo hi\"\n");
        assert!(
            none.issues.iter().any(|i| i.contains("--source kimi")),
            "the no-managed message must name the command marker, not a sentinel, got {:?}",
            none.issues
        );
    }

    #[test]
    fn verify_schema_reports_broken_on_unparseable_toml() {
        use crate::install::verify::ShimRef;
        let res = verify_schema("not = = toml");
        assert_eq!(res.shim, ShimRef::Unknown);
        assert!(res
            .issues
            .iter()
            .any(|i| i.contains("no longer parses as TOML")));
    }

    // Unix POSIX-form pin. Unix-only: on Windows hook_command emits the bare form
    // and this spaced path would be REJECTED (8.3 unavailable on CI).
    #[cfg(unix)]
    #[test]
    fn hook_command_is_the_env_prefix_shell_form() {
        let cmd = hook_command(Path::new("/opt/bin/pixtuoid-hook"), false).unwrap();
        assert_eq!(cmd, "PIXTUOID_SOURCE=kimi '/opt/bin/pixtuoid-hook'");
        assert!(
            is_managed_command(&cmd),
            "the written command must self-detect"
        );
    }

    #[test]
    #[cfg(windows)]
    fn hook_command_emits_bare_exec_form_with_source_flag_on_windows() {
        let cmd = hook_command(Path::new(r"C:\tools\pixtuoid-hook.exe"), false).unwrap();
        assert_eq!(cmd, r"C:\tools\pixtuoid-hook.exe --source kimi");
        assert!(
            is_managed_command(&cmd),
            "the Windows form must self-detect"
        );
    }

    #[test]
    #[cfg(unix)]
    fn hook_command_errors_on_non_utf8_path() {
        use std::os::unix::ffi::OsStrExt;
        let bad = Path::new(std::ffi::OsStr::from_bytes(b"/x/\xff/pixtuoid-hook"));
        assert!(hook_command(bad, false).is_err());
    }

    #[test]
    fn managed_command_detects_both_platform_forms_and_ignores_foreign() {
        assert!(is_managed_command(
            "PIXTUOID_SOURCE=kimi '/opt/pixtuoid-hook'"
        ));
        assert!(is_managed_command(
            r"C:\bin\pixtuoid-hook.exe --source kimi"
        ));
        // A different pixtuoid source's command is NOT ours (defensive specificity).
        assert!(!is_managed_command(
            "PIXTUOID_SOURCE=codex '/opt/pixtuoid-hook'"
        ));
        assert!(!is_managed_command(
            r"C:\bin\pixtuoid-hook.exe --source cursor"
        ));
        // A user's own hook command is untouched.
        assert!(!is_managed_command("terminal-notifier -message done"));
    }

    // Internal-consistency guard (mirror of CC/Codex/Cursor): every hook event we
    // REGISTER with Kimi must have a decoder arm (shared or the custom Extend),
    // else it arrives at the shared socket and the decoder bails — silently
    // dropped.
    #[test]
    fn every_registered_kimi_event_decodes() {
        use pixtuoid_core::source::decoder::decode_hook_payload;
        for ev in KIMI_EVENTS {
            let payload = serde_json::json!({
                "hook_event_name": ev,
                "session_id": "s",
                "cwd": "/repo",
                "tool_name": "Bash",
                "tool_use_id": "t1",
                "_pixtuoid_source": SOURCE_NAME,
            });
            assert!(
                decode_hook_payload(payload).is_ok(),
                "registered Kimi hook {ev:?} has no decoder arm — it would bail as \
                 unsupported. Add an arm in pixtuoid-core source/kimi.rs (or the shared arms)."
            );
        }
    }

    // MEMBERSHIP pin — the completeness half `every_registered_kimi_event_decodes`
    // can't see (it only proves registered ⊆ decodable, so silently DROPPING an
    // event ships green: no clean reap if `SessionEnd` goes, a failed tool
    // lingering Active if `PostToolUseFailure` goes — and the drift-watch reads
    // this same const one-directionally, so it's blind too; cargo-mutants doesn't
    // mutate `&[&str]` initializers). Pins the exact set so a drop is a LOUD diff.
    #[test]
    fn kimi_events_pins_the_exact_registered_set() {
        use std::collections::BTreeSet;
        assert_eq!(
            KIMI_EVENTS.iter().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "SessionStart",
                "PreToolUse",
                "PostToolUse",
                "PostToolUseFailure",
                "PermissionRequest",
                "Stop",
                "StopFailure",
                "SessionEnd",
            ]),
            "KIMI_EVENTS membership changed — the two `*Failure` variants are the \
             ones ONLY the custom Extend decoder services; update this pin deliberately."
        );
    }
}
