//! Hermes Agent hook install target.
//!
//! Writes the `hooks:` block into `<hermes-home>/config.yaml` (`~/.hermes/config.yaml`
//! by default; `HERMES_HOME` relocates it verbatim — see
//! `pixtuoid_core::source::hermes::hermes_home`). Hermes runs each hook `command` by
//! ARGV-EXEC (quote-aware word-split, NO shell — capture-verified 2026-07-03: the
//! env-prefix form yields "command not found", and `|`/`>` arrive as literal argv), so
//! the command is the bare `'<abs>' --source hermes` exec form on all platforms (via
//! `hook_cmd::exec_hook_command`), NOT Codex's Unix env-prefix.
//!
//! Config shape (YAML), per-event sequences of `{command, timeout, _pixtuoid}`:
//! ```yaml
//! hooks:
//!   on_session_start:
//!     - command: "'/abs/pixtuoid-hook' --source hermes"
//!       timeout: 5
//!       _pixtuoid: true
//! ```
//! - `_pixtuoid: true` is the managed-entry sentinel; Hermes IGNORES the unknown key
//!   (capture-verified: `hermes hooks list` parses + lists an entry carrying it).
//! - `timeout` is optional to Hermes (it defaults 60s); we set an explicit small bound.
//!
//! **Consent gate — deliberately NOT bypassed.** A freshly-declared Hermes shell hook is
//! "not allowlisted" until the user approves it through Hermes's OWN flow (an interactive
//! prompt, or `hermes --accept-hooks`). pixtuoid writes ONLY config.yaml and does NOT
//! forge an approval in `~/.hermes/shell-hooks-allowlist.json`. This mirrors the Codex
//! `trusted_hash` precedent (we write no trust hash; the user approves in the Codex TUI) —
//! respect the tool's security gate rather than pre-authorize on the user's behalf. So a
//! newly-connected Hermes sprite appears only AFTER that one-time approval.
//!
//! YAML merge preserves the user's OTHER config keys (model/provider/…) but does NOT
//! round-trip comments (a saphyr / YAML-ecosystem limitation) — data survives, comments
//! are dropped on the connect/disconnect rewrite. Idempotent via the sentinel.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use saphyr::{LoadableYamlNode, MappingOwned, ScalarOwned, Yaml, YamlEmitter, YamlOwned};

use crate::install::target::MergeOutcome;
use crate::install::verify::{SchemaParse, ShimRef};
use crate::install::SENTINEL_KEY;

/// Events we register == events the decoder handles (`pixtuoid_core::source::hermes`),
/// enforced by `every_registered_hermes_event_decodes` below. Snake_case wire values
/// from a real capture (session events carry the `on_` prefix, tool events don't).
const HERMES_EVENTS: &[&str] = &[
    "on_session_start",
    "pre_tool_call",
    "post_tool_call",
    "on_session_end",
];

/// The per-hook timeout (seconds) written into config.yaml. The shim returns within its
/// 200ms send bound; 5s is generous headroom (Hermes defaults 60s when the key is
/// omitted). One named source of truth for both the writer and the verify check.
const HOOK_TIMEOUT_SECS: i64 = 5;

pub fn default_config_path() -> Result<PathBuf> {
    pixtuoid_core::source::hermes::hermes_home()
        .map(|d| d.join("config.yaml"))
        .ok_or_else(|| {
            anyhow!("cannot resolve the Hermes home (HERMES_HOME/HOME unset); pass --config <path>")
        })
}

/// Presence probe: Hermes creates `<hermes-home>` on first run and OWNS config.yaml (it
/// holds the user's model/provider), so unlike Cursor's hooks.json the config file isn't
/// purely ours — probing the home DIR is the robust "is Hermes installed" signal
/// (config.yaml may not exist until `hermes setup`). Mirrors the Cursor/Reasonix
/// dir-probe rationale.
pub fn detect_installed() -> bool {
    pixtuoid_core::source::hermes::hermes_home().is_some_and(|d| d.exists())
}

/// Hermes ARGV-execs the command (no shell), so the bare `'<abs>' --source hermes` exec
/// form on all platforms. Err on a non-UTF-8 path (prevents the lossy dead-hook).
pub fn hook_command(resolved: &Path, _explicit: bool) -> Result<String> {
    let p = crate::install::merge::hook_path_str(resolved)?;
    crate::install::hook_cmd::exec_hook_command(p, "hermes")
}

// --- saphyr YAML helpers (owned model: parse → mutate → emit) ---

fn ystr(s: &str) -> YamlOwned {
    YamlOwned::Value(ScalarOwned::String(s.to_string()))
}

fn ybool(b: bool) -> YamlOwned {
    YamlOwned::Value(ScalarOwned::Boolean(b))
}

/// Parse config.yaml into its root mapping. Empty/whitespace content ⇒ the empty
/// document (never an error — the [`MergeOutcome`] empty rule). Errs (refusing to
/// overwrite) on unparseable YAML or a non-mapping root — a Hermes config.yaml root is
/// always a key/value mapping.
fn parse_root_mapping(content: &str) -> Result<MappingOwned> {
    if content.trim().is_empty() {
        return Ok(MappingOwned::new());
    }
    let docs = YamlOwned::load_from_str(content)
        .map_err(|e| anyhow!("config.yaml no longer parses as YAML: {e}"))?;
    match docs.into_iter().next() {
        None => Ok(MappingOwned::new()),
        Some(YamlOwned::Mapping(m)) => Ok(m),
        Some(_) => {
            bail!("config.yaml is valid YAML but its root is not a mapping — refusing to overwrite")
        }
    }
}

/// Re-serialize a root mapping to YAML, stripping saphyr's leading `---` document-start
/// marker for a cleaner diff (Hermes parses it either way; a hand-written config.yaml
/// has none).
fn emit(root: &MappingOwned) -> Result<String> {
    let doc = YamlOwned::Mapping(root.clone());
    let borrowed = Yaml::from(&doc);
    let mut out = String::new();
    YamlEmitter::new(&mut out)
        .dump(&borrowed)
        .map_err(|e| anyhow!("emitting config.yaml: {e}"))?;
    Ok(out.strip_prefix("---\n").map(str::to_string).unwrap_or(out))
}

fn is_managed(entry: &YamlOwned) -> bool {
    matches!(entry, YamlOwned::Mapping(m) if m.get(&ystr(SENTINEL_KEY)) == Some(&ybool(true)))
}

/// A managed entry whose `command` + `timeout` already match ours — an install no-op.
fn managed_matches(entry: &YamlOwned, hook_cmd: &str) -> bool {
    let YamlOwned::Mapping(m) = entry else {
        return false;
    };
    m.get(&ystr("command")) == Some(&ystr(hook_cmd))
        && m.get(&ystr("timeout"))
            == Some(&YamlOwned::Value(ScalarOwned::Integer(HOOK_TIMEOUT_SECS)))
}

fn managed_entry(hook_cmd: &str) -> YamlOwned {
    let mut e = MappingOwned::new();
    e.insert(ystr("command"), ystr(hook_cmd));
    e.insert(
        ystr("timeout"),
        YamlOwned::Value(ScalarOwned::Integer(HOOK_TIMEOUT_SECS)),
    );
    e.insert(ystr(SENTINEL_KEY), ybool(true));
    YamlOwned::Mapping(e)
}

/// Extract the shim path from our exec-form command — `'<abs>' --source hermes` (Unix,
/// single-quoted) or `<abs> --source hermes` (Windows, bare). The shared
/// `verify::shell_shim_ref` handles the shell targets' env-prefix + Windows-bare shapes
/// but NOT a leading QUOTED path before ` --source` (it would keep the quotes), so per
/// invariant #3 the exec form's extraction lives here. `rsplit_once` so a path that
/// literally contains " --source " keeps it and only the genuine trailing flag is cut.
fn exec_shim_ref(command: &str) -> ShimRef {
    let path = command
        .rsplit_once(" --source ")
        .map(|(p, _)| p)
        .unwrap_or(command)
        .trim();
    let unq = if path.len() >= 2 && path.starts_with('\'') && path.ends_with('\'') {
        crate::install::verify::posix_unquote(path)
    } else {
        path.to_string()
    };
    if unq.is_empty() {
        ShimRef::Unknown
    } else {
        ShimRef::Absolute(PathBuf::from(unq))
    }
}

fn managed_command(entry: &YamlOwned) -> Option<String> {
    let YamlOwned::Mapping(m) = entry else {
        return None;
    };
    match m.get(&ystr("command")) {
        Some(YamlOwned::Value(ScalarOwned::String(s))) => Some(s.clone()),
        _ => None,
    }
}

pub fn merge_install(content: &str, hook_cmd: &str) -> Result<MergeOutcome> {
    let mut root = parse_root_mapping(content)?;
    let mut changed = false;

    let hooks_key = ystr("hooks");
    if !matches!(root.get(&hooks_key), Some(YamlOwned::Mapping(_))) {
        // Absent OR present-but-wrong-type — coerce to an empty mapping (mirrors
        // Cursor coercing a non-object `hooks`); the per-event adds below set `changed`.
        root.insert(hooks_key.clone(), YamlOwned::Mapping(MappingOwned::new()));
    }
    let Some(YamlOwned::Mapping(hooks)) = root.get_mut(&hooks_key) else {
        unreachable!("hooks was just ensured to be a mapping")
    };

    for ev in HERMES_EVENTS {
        let ev_key = ystr(ev);
        if !matches!(hooks.get(&ev_key), Some(YamlOwned::Sequence(_))) {
            hooks.insert(ev_key.clone(), YamlOwned::Sequence(Vec::new()));
        }
        let Some(YamlOwned::Sequence(seq)) = hooks.get_mut(&ev_key) else {
            unreachable!("event was just ensured to be a sequence")
        };
        let managed: Vec<&YamlOwned> = seq.iter().filter(|e| is_managed(e)).collect();
        if managed.len() == 1 && managed_matches(managed[0], hook_cmd) {
            continue; // already exactly one correct managed entry — no change
        }
        // Normalize: drop every managed entry, append exactly one. User entries survive.
        seq.retain(|e| !is_managed(e));
        seq.push(managed_entry(hook_cmd));
        changed = true;
    }

    Ok(MergeOutcome {
        content: emit(&root)?,
        changed,
    })
}

pub fn merge_uninstall(content: &str) -> Result<MergeOutcome> {
    let mut root = parse_root_mapping(content)?;
    let hooks_key = ystr("hooks");
    let Some(YamlOwned::Mapping(hooks)) = root.get_mut(&hooks_key) else {
        return Ok(MergeOutcome {
            content: emit(&root)?,
            changed: false,
        });
    };

    let mut changed = false;
    let mut emptied_events: Vec<YamlOwned> = Vec::new();
    for (k, v) in hooks.iter_mut() {
        if let YamlOwned::Sequence(seq) = v {
            let before = seq.len();
            seq.retain(|e| !is_managed(e));
            if seq.len() != before {
                changed = true;
                if seq.is_empty() {
                    emptied_events.push(k.clone());
                }
            }
        }
    }
    for k in emptied_events {
        hooks.remove(&k);
    }
    // Drop the `hooks` mapping entirely if OUR removals emptied it (a user's own hook
    // keeps it). Gated on `changed` so an already-empty user `hooks:` is never touched.
    if changed && matches!(root.get(&hooks_key), Some(YamlOwned::Mapping(m)) if m.is_empty()) {
        root.remove(&hooks_key);
    }

    Ok(MergeOutcome {
        content: emit(&root)?,
        changed,
    })
}

/// Install-schema verification (#309): every `HERMES_EVENTS` entry still has a
/// `_pixtuoid`-sentinel managed hook, and the shim path is extracted for
/// `install::verify_target` to stat. A config.yaml with our hooks stripped (a user/tool
/// rewrite) is the silent-dead class this catches. NOTE: the Hermes allowlist consent is
/// deliberately out of scope here — it's the user's to grant in Hermes (module doc), not
/// an install-soundness property pixtuoid owns.
pub fn verify_schema(content: &str) -> SchemaParse {
    let root = match parse_root_mapping(content) {
        Ok(r) => r,
        Err(_) => return SchemaParse::broken("config.yaml no longer parses as a YAML mapping"),
    };
    let hooks = match root.get(&ystr("hooks")) {
        Some(YamlOwned::Mapping(m)) => Some(m),
        _ => None,
    };
    let mut issues = Vec::new();
    let mut shim = ShimRef::Unknown;
    let mut saw_any = false;
    for ev in HERMES_EVENTS {
        let managed_cmd = hooks
            .and_then(|h| h.get(&ystr(ev)))
            .and_then(|v| match v {
                YamlOwned::Sequence(s) => Some(s),
                _ => None,
            })
            .and_then(|s| s.iter().find(|e| is_managed(e)))
            .and_then(managed_command);
        match managed_cmd {
            Some(cmd) => {
                saw_any = true;
                if shim == ShimRef::Unknown {
                    shim = exec_shim_ref(&cmd);
                }
            }
            None => issues.push(format!(
                "config.yaml hooks.{ev} has no pixtuoid-managed entry (reconnect via the Sources panel)"
            )),
        }
    }
    if !saw_any {
        issues.push(
            "config.yaml has no pixtuoid-managed hooks (the _pixtuoid sentinel is absent)".into(),
        );
    }
    SchemaParse { issues, shim }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install(content: &str, cmd: &str) -> String {
        merge_install(content, cmd).unwrap().content
    }

    #[test]
    fn install_writes_all_events_with_sentinel_and_preserves_user_keys() {
        let src = "model: gpt\nprovider: nous\n";
        let out = install(src, "'/opt/pixtuoid-hook' --source hermes");
        let root = parse_root_mapping(&out).unwrap();
        // User keys survive.
        assert_eq!(root.get(&ystr("model")), Some(&ystr("gpt")));
        assert_eq!(root.get(&ystr("provider")), Some(&ystr("nous")));
        let Some(YamlOwned::Mapping(hooks)) = root.get(&ystr("hooks")) else {
            panic!("hooks not a mapping: {out}");
        };
        for ev in HERMES_EVENTS {
            let Some(YamlOwned::Sequence(seq)) = hooks.get(&ystr(ev)) else {
                panic!("event {ev} missing: {out}");
            };
            let managed: Vec<_> = seq.iter().filter(|e| is_managed(e)).collect();
            assert_eq!(managed.len(), 1, "event {ev}");
            assert_eq!(
                managed_command(managed[0]).as_deref(),
                Some("'/opt/pixtuoid-hook' --source hermes")
            );
        }
    }

    #[test]
    fn install_is_idempotent_and_replaces_on_path_change() {
        let a = install("", "'/opt/a/pixtuoid-hook' --source hermes");
        let second = merge_install(&a, "'/opt/a/pixtuoid-hook' --source hermes").unwrap();
        assert!(!second.changed, "same command re-install must be a no-op");
        // Path change → still exactly one managed entry per event (replaced, not dupED).
        let c = install(&a, "'/opt/b/pixtuoid-hook' --source hermes");
        let root = parse_root_mapping(&c).unwrap();
        let YamlOwned::Mapping(hooks) = root.get(&ystr("hooks")).unwrap() else {
            unreachable!()
        };
        for ev in HERMES_EVENTS {
            let YamlOwned::Sequence(seq) = hooks.get(&ystr(ev)).unwrap() else {
                unreachable!()
            };
            assert_eq!(
                seq.iter().filter(|e| is_managed(e)).count(),
                1,
                "event {ev} duplicated on path change"
            );
            assert_eq!(
                managed_command(seq.iter().find(|e| is_managed(e)).unwrap()).as_deref(),
                Some("'/opt/b/pixtuoid-hook' --source hermes")
            );
        }
    }

    #[test]
    fn install_preserves_a_users_own_hook_entry() {
        let src = "hooks:\n  pre_tool_call:\n    - command: my-guard.sh\n";
        let out = install(src, "'/x' --source hermes");
        let root = parse_root_mapping(&out).unwrap();
        let YamlOwned::Mapping(hooks) = root.get(&ystr("hooks")).unwrap() else {
            unreachable!()
        };
        let YamlOwned::Sequence(seq) = hooks.get(&ystr("pre_tool_call")).unwrap() else {
            unreachable!()
        };
        // Two entries: the user's guard + our managed one.
        assert_eq!(seq.len(), 2);
        assert!(seq
            .iter()
            .any(|e| managed_command(e).as_deref() == Some("my-guard.sh")));
        assert_eq!(seq.iter().filter(|e| is_managed(e)).count(), 1);
    }

    #[test]
    fn uninstall_removes_only_managed_and_keeps_user_hooks() {
        let installed = install(
            "hooks:\n  pre_tool_call:\n    - command: my-guard.sh\n",
            "'/x' --source hermes",
        );
        let out = merge_uninstall(&installed).unwrap();
        assert!(out.changed);
        let root = parse_root_mapping(&out.content).unwrap();
        let YamlOwned::Mapping(hooks) = root.get(&ystr("hooks")).unwrap() else {
            unreachable!()
        };
        // The user's pre_tool_call guard survives; the emptied events are dropped.
        let YamlOwned::Sequence(seq) = hooks.get(&ystr("pre_tool_call")).unwrap() else {
            unreachable!()
        };
        assert_eq!(seq.len(), 1);
        assert_eq!(managed_command(&seq[0]).as_deref(), Some("my-guard.sh"));
        for ev in HERMES_EVENTS.iter().filter(|e| **e != "pre_tool_call") {
            assert!(
                hooks.get(&ystr(ev)).is_none(),
                "empty event {ev} should be dropped"
            );
        }
    }

    #[test]
    fn uninstall_all_managed_drops_the_hooks_mapping_but_keeps_user_config() {
        let installed = install("model: gpt\n", "'/x' --source hermes");
        let out = merge_uninstall(&installed).unwrap();
        let root = parse_root_mapping(&out.content).unwrap();
        assert!(
            root.get(&ystr("hooks")).is_none(),
            "hooks should be dropped: {}",
            out.content
        );
        assert_eq!(
            root.get(&ystr("model")),
            Some(&ystr("gpt")),
            "user config must survive"
        );
    }

    #[test]
    fn uninstall_with_no_managed_hooks_is_a_no_op() {
        let out = merge_uninstall("hooks:\n  stop:\n    - command: notify\n").unwrap();
        assert!(!out.changed);
    }

    #[test]
    fn merge_install_rejects_non_mapping_root() {
        assert!(merge_install("- 1\n- 2\n", "/x").is_err());
        assert!(merge_install("42\n", "/x").is_err());
    }

    #[test]
    fn empty_content_installs_from_scratch() {
        let first = merge_install("", "/x --source hermes").unwrap();
        assert!(first.changed);
        let second = merge_install(&first.content, "/x --source hermes").unwrap();
        assert!(!second.changed);
    }

    #[test]
    fn verify_flags_stripped_hooks_and_passes_full_install() {
        let installed = install("model: gpt\n", "'/opt/pixtuoid-hook' --source hermes");
        let sound = verify_schema(&installed);
        assert!(sound.issues.is_empty(), "{:?}", sound.issues);
        assert_eq!(
            sound.shim,
            ShimRef::Absolute(PathBuf::from("/opt/pixtuoid-hook")),
            "shim path must be extracted from the exec-form command"
        );
        // A user rewrite that drops our hooks → broken (the #309 silent-dead class).
        let stripped = merge_uninstall(&installed).unwrap().content;
        let p = verify_schema(&stripped);
        assert!(!p.issues.is_empty(), "stripped config must verify broken");
    }

    // Unix POSIX-form pin (single-quoted path + bare flag). Unix-only: on Windows the
    // bare form is emitted and this spaced path would be REJECTED by the 8.3 guard.
    #[cfg(unix)]
    #[test]
    fn hook_command_is_the_exec_form_with_source_flag() {
        let cmd = hook_command(Path::new("/Users/Jane Doe/bin/pixtuoid-hook"), false).unwrap();
        assert_eq!(cmd, "'/Users/Jane Doe/bin/pixtuoid-hook' --source hermes");
    }

    #[test]
    #[cfg(unix)]
    fn hook_command_errors_on_non_utf8_path() {
        use std::os::unix::ffi::OsStrExt;
        let bad = Path::new(std::ffi::OsStr::from_bytes(b"/x/\xff/pixtuoid-hook"));
        assert!(hook_command(bad, false).is_err());
    }

    // Internal-consistency guard (mirror of CC/Codex/Cursor): every hook event we
    // REGISTER with Hermes must have a decoder arm, else it reaches the shared socket
    // and the decoder bails — silently dropped.
    #[test]
    fn every_registered_hermes_event_decodes() {
        use pixtuoid_core::source::decoder::decode_hook_payload;
        for ev in HERMES_EVENTS {
            let payload = serde_json::json!({
                "hook_event_name": ev,
                "session_id": "s",
                "cwd": "/repo",
                "_pixtuoid_source": "hermes",
            });
            assert!(
                decode_hook_payload(payload).is_ok(),
                "registered Hermes hook {ev:?} has no decoder arm — add it in \
                 pixtuoid-core source/hermes.rs."
            );
        }
    }
}
