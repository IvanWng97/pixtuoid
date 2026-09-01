//! DeepSeek Harness install target: one zero-import plugin file plus one
//! mount row in the HOME-level patch file, which applies to every profile
//! (`$DSH_HOME/cordis.patch.yml` is the last user layer before `--patch`
//! overlays). The plugin is a single `.mjs` deliberately free of
//! `@deepseek-ai` imports, so mounting it by ABSOLUTE path needs no package
//! install, no pnpm, and cannot summon a second cordis instance — probed live
//! against a stock npm dsh (0.1.1-rc.2): the loader `import()`s a bare
//! absolute path and the `web` profile even hot-reloads the patch file.
//! ACCEPTED residual: uninstall removes the mount ROW only — the plugin file
//! stays under `$DSH_HOME/pixtuoid/` (`write_config_atomic` cannot delete),
//! and unmounted it is inert bytes.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use saphyr::{LoadableYamlNode, MappingOwned, ScalarOwned, Yaml, YamlEmitter, YamlOwned};

use crate::install::target::MergeOutcome;
use crate::install::verify::{SchemaParse, ShimRef};

/// The mount row's `id` — uninstall and the one-row verify both key on it.
const PLUGIN_ID: &str = "pixtuoid";

const HOOK_PLACEHOLDER: &str = "\"{{HOOK_PATH_JSON}}\"";

pub(crate) const PLUGIN_TEMPLATE: &str = include_str!("dsh_plugin.mjs");

/// `$DSH_HOME` else `~/.dsh` — upstream `resolveDshHome`
/// (`packages/util/home-paths/src/index.ts`, fetched 2026-09-01) resolves
/// configured-path > `$DSH_HOME` > `~/.dsh` with no XDG arm and treats a
/// blank `$DSH_HOME` as unset; it also expands a `~/` value and resolves a
/// relative one against CWD (`resolve(expandHomePath(..))`) — we refuse both
/// instead of mirroring, because a cwd-dependent mount row would point
/// somewhere new each launch. No configured path reaches the mount, so the
/// env axis is the whole surface here.
fn dsh_home() -> Result<PathBuf> {
    if let Some(p) = crate::install::io::nonempty_env("DSH_HOME") {
        if !p.is_absolute() {
            bail!(
                "DSH_HOME={} is not absolute — dsh would expand `~` and \
                 resolve the rest against its own CWD, so a pixtuoid-written \
                 mount row would drift per launch; set an absolute path",
                p.display()
            );
        }
        return Ok(p);
    }
    pixtuoid_core::platform::user_home_opt()
        .map(|h| h.join(".dsh"))
        .context("no home directory resolves — cannot locate ~/.dsh")
}

/// The mergeable config: the home-level patch file every profile layers in.
pub(crate) fn default_config_path() -> Result<PathBuf> {
    Ok(dsh_home()?.join("cordis.patch.yml"))
}

/// Where the plugin file itself lives (the patch row's `name` points here).
pub(crate) fn plugin_path() -> Result<PathBuf> {
    Ok(dsh_home()?.join(PLUGIN_ID).join("pixtuoid-dsh.mjs"))
}

/// Presence probe: dsh's own home, never our artifacts (chicken-and-egg).
pub(crate) fn detect_installed() -> bool {
    dsh_home().map(|h| h.exists()).unwrap_or(false)
}

/// dsh runs plugins under Node and the plugin spawns the shim by embedded
/// path — always absolute, `_explicit` is irrelevant.
pub(crate) fn hook_command(resolved: &Path, _explicit: bool) -> Result<String> {
    crate::install::merge::hook_path_str(resolved).map(str::to_string)
}

/// The `extra_artifacts` hook: the plugin file, shim path baked, rewritten
/// verbatim on every (re)install.
pub(crate) fn plugin_artifacts(hook_path: &Path) -> Result<Vec<(PathBuf, String)>> {
    let hook = crate::install::merge::hook_path_str(hook_path)?;
    let rendered =
        crate::install::merge::bake_hook_path(PLUGIN_TEMPLATE, HOOK_PLACEHOLDER, hook, "dsh")?;
    Ok(vec![(plugin_path()?, rendered)])
}

fn ystr(s: &str) -> YamlOwned {
    YamlOwned::Value(ScalarOwned::String(s.to_string()))
}

/// Parse the patch file as its top-level array; empty content is the empty
/// document. A non-array top level is a config we must not touch.
fn parse_rows(content: &str) -> Result<Vec<YamlOwned>> {
    if content.trim().is_empty() {
        return Ok(Vec::new());
    }
    let docs = YamlOwned::load_from_str(content)
        .map_err(|e| anyhow::anyhow!("cordis.patch.yml does not parse as YAML: {e}"))?;
    match docs.into_iter().next() {
        None => Ok(Vec::new()),
        Some(YamlOwned::Sequence(rows)) => Ok(rows),
        Some(_) => bail!(
            "cordis.patch.yml's top level is not a patch list — refusing to rewrite a \
             config pixtuoid does not understand"
        ),
    }
}

fn emit_rows(rows: &[YamlOwned]) -> Result<String> {
    if rows.is_empty() {
        return Ok(String::new());
    }
    let doc = YamlOwned::Sequence(rows.to_vec());
    let borrowed = Yaml::from(&doc);
    let mut out = String::new();
    YamlEmitter::new(&mut out)
        .dump(&borrowed)
        .map_err(|e| anyhow::anyhow!("emitting cordis.patch.yml: {e}"))?;
    let mut out = out.strip_prefix("---\n").map(str::to_string).unwrap_or(out);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

/// Our whole patch row: `- insert: [{id: pixtuoid, name: <plugin path>}]`.
fn mount_row(plugin: &str) -> YamlOwned {
    let mut entry = MappingOwned::new();
    entry.insert(ystr("id"), ystr(PLUGIN_ID));
    entry.insert(ystr("name"), ystr(plugin));
    let mut row = MappingOwned::new();
    row.insert(
        ystr("insert"),
        YamlOwned::Sequence(vec![YamlOwned::Mapping(entry)]),
    );
    YamlOwned::Mapping(row)
}

/// Is this row OUR mount row (an `insert` op whose single entry id is ours)?
fn is_our_row(row: &YamlOwned) -> bool {
    let YamlOwned::Mapping(m) = row else {
        return false;
    };
    let Some(YamlOwned::Sequence(entries)) = m.get(&ystr("insert")) else {
        return false;
    };
    entries.iter().any(
        |e| matches!(e, YamlOwned::Mapping(em) if em.get(&ystr("id")) == Some(&ystr(PLUGIN_ID))),
    )
}

/// `changed` is a semantic diff: same plugin path → no rewrite.
pub(crate) fn merge_install(content: &str, _hook_cmd: &str) -> Result<MergeOutcome> {
    let plugin = plugin_path()?;
    let plugin = plugin.to_str().context("plugin path is not UTF-8")?;
    let mut rows = parse_rows(content)?;
    let wanted = mount_row(plugin);
    if let Some(existing) = rows.iter_mut().find(|r| is_our_row(r)) {
        if *existing == wanted {
            return Ok(MergeOutcome {
                content: content.to_string(),
                changed: false,
            });
        }
        *existing = wanted;
    } else {
        rows.push(wanted);
    }
    Ok(MergeOutcome {
        content: emit_rows(&rows)?,
        changed: true,
    })
}

/// Remove only our row; every foreign patch op survives byte-preserved as data
/// (comments do not survive saphyr — the hermes target's documented trade).
pub(crate) fn merge_uninstall(content: &str) -> Result<MergeOutcome> {
    let rows = parse_rows(content)?;
    let kept: Vec<YamlOwned> = rows.iter().filter(|r| !is_our_row(r)).cloned().collect();
    if kept.len() == rows.len() {
        return Ok(MergeOutcome {
            content: content.to_string(),
            changed: false,
        });
    }
    Ok(MergeOutcome {
        content: emit_rows(&kept)?,
        changed: true,
    })
}

/// The patch file carries the MOUNT, not the shim (that is baked inside the
/// plugin file) — so `shim` stays `Unknown` here; a stale or missing plugin
/// file is `verify_extra_artifacts`' whole-file compare (nothing re-installs
/// on a pixtuoid upgrade — that check is what catches an old bake).
pub(crate) fn verify_schema(content: &str) -> SchemaParse {
    let rows = match parse_rows(content) {
        Ok(rows) => rows,
        Err(e) => return SchemaParse::broken(e.to_string()),
    };
    let ours: Vec<&YamlOwned> = rows.iter().filter(|r| is_our_row(r)).collect();
    let mut parse = SchemaParse {
        shim: ShimRef::Unknown,
        ..Default::default()
    };
    match ours.len() {
        0 => parse
            .issues
            .push("no pixtuoid mount row — dsh never loads the plugin".to_string()),
        1 => {
            let YamlOwned::Mapping(m) = ours[0] else {
                unreachable!("is_our_row admits mappings only");
            };
            let Some(YamlOwned::Sequence(entries)) = m.get(&ystr("insert")) else {
                unreachable!("is_our_row admits insert rows only");
            };
            let name = entries.iter().find_map(|e| match e {
                YamlOwned::Mapping(em) if em.get(&ystr("id")) == Some(&ystr(PLUGIN_ID)) => {
                    em.get(&ystr("name")).and_then(|n| match n {
                        YamlOwned::Value(ScalarOwned::String(s)) => Some(s.clone()),
                        _ => None,
                    })
                }
                _ => None,
            });
            match name {
                // PURE over the content (`target.rs` schema contract): file
                // existence/staleness is `verify_extra_artifacts`' stat +
                // whole-file compare, never checked here.
                Some(n) if Path::new(&n).is_absolute() => {}
                Some(n) => parse.issues.push(format!(
                    "the mount row's plugin path {n} is not absolute — dsh's loader only \
                     imports bare absolute paths"
                )),
                None => parse
                    .issues
                    .push("the pixtuoid mount row carries no plugin path".to_string()),
            }
        }
        n => parse.issues.push(format!(
            "{n} pixtuoid mount rows — duplicates are ours; reconnect dsh to collapse them"
        )),
    }
    parse
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The template's leading marker — uninstall keys on the patch ROW (not
    /// this), so the sentinel is a recognizer for humans and this test only.
    const SENTINEL: &str = "@pixtuoid-dsh-plugin";

    /// Every `type: "<wire name>"` the plugin sends, extracted from the template.
    fn plugin_wire_types() -> std::collections::BTreeSet<String> {
        let mut set = std::collections::BTreeSet::new();
        for part in PLUGIN_TEMPLATE.split("type: \"").skip(1) {
            set.insert(part.split('"').next().expect("closed quote").to_string());
        }
        set
    }

    #[test]
    fn the_plugin_sends_exactly_what_the_decoder_reads() {
        // The registered-events↔decoder-arms guard: a wire name only one side
        // knows is a silently dead event (sent-and-dropped, or read-and-never-sent).
        let decoder_reads: std::collections::BTreeSet<String> = [
            "session_start",
            "session_end",
            "tool_call",
            "tool_result",
            "approval_asked",
            "approval_decided",
            "model",
            "usage",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        assert_eq!(plugin_wire_types(), decoder_reads);
    }

    #[test]
    fn every_plugin_payload_type_decodes_to_events() {
        use pixtuoid_core::source::dsh::decode_dsh_payload;
        let sid = "01b00000-0000-7000-8000-000000000001";
        for ty in plugin_wire_types() {
            let payload = serde_json::json!({
                "type": ty, "sessionId": sid, "cwd": "/r",
                "callId": "c1", "toolName": "bash", "approvalId": "a1",
                "outcome": "allowed-once", "model": "m", "provider": "p",
                "inputTokens": 1, "outputTokens": 1,
            });
            let evs = decode_dsh_payload(&payload).expect("decodes");
            assert!(
                !evs.is_empty(),
                "{ty} decodes to nothing — a dead wire name"
            );
        }
    }

    #[test]
    fn the_plugin_subscribes_only_verified_emit_channels_and_never_awaits() {
        // Allowlist, not denylist: upstream carries ~21 non-emit (waterfall/
        // serial/parallel) events and the set churns, so naming bad ones can
        // only sample the invariant. The enforceable form: every `ctx.on`
        // call site names a channel verified `@mode emit` upstream
        // (`runtime-types.ts` for the two agent channels, `session/src/
        // index.ts` for `session/event`; fetched 2026-09-01).
        let subscribed: std::collections::BTreeSet<&str> = PLUGIN_TEMPLATE
            .split(r#"ctx.on("#)
            .skip(1)
            .map(|p| {
                p.trim_start_matches('"')
                    .split('"')
                    .next()
                    .expect("closed quote")
            })
            .collect();
        let allowed = std::collections::BTreeSet::from([
            "agent/session-start",
            "agent/disposed",
            "session/event",
        ]);
        assert_eq!(subscribed, allowed);
        assert!(
            !PLUGIN_TEMPLATE.contains("await"),
            "an await inside a listener is a stall waiting to be awaited"
        );
    }

    #[test]
    fn the_plugin_forwards_parent_session_only_for_subagent_headers() {
        // `parentSession` alone is upstream's seed-lineage field — an ordinary
        // user branch (`ctx.sessions.fork()`) stamps it too; forwarded ungated,
        // a branched conversation renders as a delegation child.
        assert!(
            PLUGIN_TEMPLATE.contains(r#"header.origin === "subagent" && header.parentSession"#),
            "base() lost the subagent-discriminator gate on parentSession"
        );
    }

    #[test]
    fn install_mounts_once_idempotently_and_preserves_foreign_rows() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("DSH_HOME", home.path());

        let foreign =
            "- insert:\n    - id: timer\n      name: '@deepseek-ai/cordis-plugin-timer'\n";
        let merged = merge_install(foreign, "/unused").unwrap();
        assert!(merged.changed);
        assert!(merged.content.contains("timer"), "foreign row preserved");
        assert!(merged.content.contains(PLUGIN_ID));
        assert!(merged.content.contains("pixtuoid-dsh.mjs"));

        let again = merge_install(&merged.content, "/unused").unwrap();
        assert!(!again.changed, "same path re-install is a no-op");

        let removed = merge_uninstall(&merged.content).unwrap();
        assert!(removed.changed);
        assert!(removed.content.contains("timer"));
        assert!(!removed.content.contains(PLUGIN_ID));
        assert!(!merge_uninstall(foreign).unwrap().changed);

        std::env::remove_var("DSH_HOME");
    }

    #[test]
    fn a_moved_home_re_mounts_the_new_plugin_path() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::env::set_var("DSH_HOME", a.path());
        let first = merge_install("", "/unused").unwrap();
        std::env::set_var("DSH_HOME", b.path());
        let second = merge_install(&first.content, "/unused").unwrap();
        assert!(second.changed, "a stale mount path must be rewritten");
        // Structural, not string: saphyr quote-escapes a Windows path in the
        // emitted YAML, so a lossy-string `contains` passes only on Unix. A
        // re-merge under home B is a no-op exactly when the row already
        // carries B's plugin path.
        assert!(!merge_install(&second.content, "/unused").unwrap().changed);
        std::env::remove_var("DSH_HOME");
    }

    #[test]
    fn merge_refuses_a_non_list_config_rather_than_rewriting_it() {
        assert!(merge_install("just: a mapping\n", "/unused").is_err());
        assert!(merge_uninstall("just: a mapping\n").is_err());
    }

    #[test]
    fn verify_reports_the_missing_row_and_the_relative_path() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("DSH_HOME", home.path());

        let none = verify_schema("");
        assert!(none
            .issues
            .iter()
            .any(|i| i.contains("no pixtuoid mount row")));

        // The plugin file does not exist on disk, yet the schema reads
        // clean: verify_schema is PURE over the content — a missing file is
        // `verify_extra_artifacts`' finding, pinned by the shared
        // missing-code-artifact test in `install/tests.rs`.
        let merged = merge_install("", "/unused").unwrap();
        let ok = verify_schema(&merged.content);
        assert!(ok.issues.is_empty(), "{ok:?}");

        let relative = "- insert:\n    - id: pixtuoid\n      name: relative/plugin.mjs\n";
        let rel = verify_schema(relative);
        assert!(rel.issues.iter().any(|i| i.contains("not absolute")));

        std::env::remove_var("DSH_HOME");
    }

    #[test]
    fn plugin_artifacts_bake_the_hook_path_and_carry_the_sentinel() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("DSH_HOME", home.path());
        let arts = plugin_artifacts(Path::new("/opt/bin/pixtuoid-hook")).unwrap();
        assert_eq!(arts.len(), 1);
        let (path, content) = &arts[0];
        assert!(path.starts_with(home.path()));
        assert!(content.contains(SENTINEL));
        assert!(content.contains("\"/opt/bin/pixtuoid-hook\""));
        assert!(!content.contains(HOOK_PLACEHOLDER));
        std::env::remove_var("DSH_HOME");
    }

    #[test]
    fn dsh_home_refuses_a_relative_override() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("DSH_HOME", "relative/home");
        assert!(dsh_home().is_err());
        std::env::remove_var("DSH_HOME");
    }
}
