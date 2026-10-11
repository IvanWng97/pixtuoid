//! The plugin pixtuoid lays out for a CLI whose own plugin system loads the hooks:
//! a root per host under pixtuoid's config dir, so no two hosts share a file, holding
//! one plugin named [`NAME`] that the host's CLI registers.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::install::io;
use crate::install::target::MergeOutcome;
use crate::install::verify::{SchemaParse, ShimRef, assemble};

/// The marketplace and the plugin share one name, so the plugin id is
/// `pixtuoid@pixtuoid`.
pub(crate) const NAME: &str = "pixtuoid";

/// A hooks file's place in its plugin dir, where Claude Code and Codex both look.
pub(crate) const HOOKS_FILE: &[&str] = &["hooks", "hooks.json"];

pub(crate) fn plugin_id() -> String {
    format!("{NAME}@{NAME}")
}

/// `<pixtuoid config>/<root>/<plugin>/<file…>`.
pub(crate) fn owned_path(root: &str, file: &[&str]) -> Result<PathBuf> {
    let base = crate::config::config_base().context("no home directory resolves")?;
    Ok(file
        .iter()
        .fold(base.join("pixtuoid").join(root).join(NAME), |p, c| {
            p.join(c)
        }))
}

/// The plugin dir and the root of `<root>/<plugin>/hooks/hooks.json`.
pub(crate) fn hooks_layout(config: &Path) -> Result<(&Path, &Path)> {
    let plugin = config.parent().and_then(Path::parent);
    plugin.zip(plugin.and_then(Path::parent)).with_context(|| {
        format!(
            "{} is not <marketplace>/<plugin>/hooks/hooks.json",
            config.display()
        )
    })
}

/// `./<plugin dir name>`, how a marketplace names a plugin dir inside its root.
pub(crate) fn relative_source(plugin: &Path) -> Result<String> {
    let name = plugin.file_name().context("the plugin dir has no name")?;
    Ok(format!("./{}", name.to_string_lossy()))
}

/// Write each manifest, creating its dir.
pub(crate) fn write_manifests(docs: &[(PathBuf, Value)]) -> Result<()> {
    for (path, doc) in docs {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        io::write_config_atomic(path, &serde_json::to_string_pretty(doc)?)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

/// Delete `dir` only when `manifest` names [`NAME`]: a custom config path must not
/// take a stranger's dir.
pub(crate) fn remove_if_ours(dir: &Path, manifest: &Path) -> Result<()> {
    let Ok(text) = std::fs::read_to_string(manifest) else {
        return Ok(());
    };
    let ours = serde_json::from_str::<Value>(&text).is_ok_and(|m| m["name"].as_str() == Some(NAME));
    if ours {
        std::fs::remove_dir_all(dir).with_context(|| format!("removing {}", dir.display()))?;
    }
    Ok(())
}

/// A hooks file in Claude Code's plugin shape, `{"hooks": {<event>: [<group>]}}`,
/// which Codex's plugins read too (`codex-rs/config/src/hook_config.rs` @ rust-v0.162.1).
pub(crate) fn render_hooks(events: &[&str], group: &Value) -> Result<String> {
    let want = json!({
        "hooks": events
            .iter()
            .map(|ev| ((*ev).to_string(), json!([group])))
            .collect::<serde_json::Map<_, _>>()
    });
    Ok(serde_json::to_string_pretty(&want)?)
}

/// Every event in `events` holds a group, and the first group's command gives the
/// shim back through `shim_of`.
pub(crate) fn verify_hooks(
    content: &str,
    events: &[&str],
    shim_of: fn(command: &str) -> ShimRef,
) -> SchemaParse {
    let Ok(doc) = serde_json::from_str::<Value>(content) else {
        return SchemaParse::broken("the plugin's hooks.json no longer parses as JSON");
    };
    let hooks = doc.get("hooks").and_then(Value::as_object);
    let mut missing = Vec::new();
    let mut any = false;
    let mut shim = ShimRef::Unknown;
    for ev in events {
        let group = hooks
            .and_then(|h| h.get(*ev))
            .and_then(Value::as_array)
            .and_then(|groups| groups.first());
        let Some(group) = group else {
            missing.push(*ev);
            continue;
        };
        any = true;
        if shim == ShimRef::Unknown {
            shim = group["hooks"][0]["command"]
                .as_str()
                .map_or(ShimRef::Unknown, shim_of);
        }
    }
    assemble(&missing, any, shim, vec![])
}

/// The hooks file as a disconnect leaves it; `changed` iff it held hooks.
pub(crate) fn uninstall_hooks(content: &str) -> Result<MergeOutcome> {
    let had_hooks = serde_json::from_str::<Value>(content)
        .ok()
        .and_then(|doc| doc.get("hooks")?.as_object().map(|h| !h.is_empty()))
        .unwrap_or(false);
    Ok(MergeOutcome {
        content: "{}\n".to_string(),
        changed: had_hooks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hooks_file_sits_two_levels_inside_its_plugin_root() {
        let base = std::env::temp_dir().join("pixtuoid-xdg");
        temp_env::with_var("XDG_CONFIG_HOME", Some(&base), || {
            let hooks = owned_path("some-plugin", HOOKS_FILE).unwrap();
            let root = base.join("pixtuoid").join("some-plugin");
            assert_eq!(
                hooks,
                root.join("pixtuoid").join("hooks").join("hooks.json")
            );
            let (plugin, at) = hooks_layout(&hooks).unwrap();
            assert_eq!(
                (plugin, at),
                (root.join("pixtuoid").as_path(), root.as_path())
            );
            assert_eq!(relative_source(plugin).unwrap(), "./pixtuoid");
        });
    }

    #[test]
    fn uninstall_empties_the_file_and_reports_a_change_only_with_hooks() {
        let installed = render_hooks(&["Stop"], &json!({ "hooks": [] })).unwrap();
        let out = uninstall_hooks(&installed).unwrap();
        assert!(out.changed);
        assert!(!uninstall_hooks(&out.content).unwrap().changed);
        assert!(!uninstall_hooks("").unwrap().changed);
    }

    #[test]
    fn only_a_manifest_naming_us_lets_its_dir_go() {
        let tmp = tempfile::TempDir::new().unwrap();
        let manifest = tmp.path().join("dir").join("m.json");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        std::fs::write(&manifest, r#"{"name":"someone-else"}"#).unwrap();
        remove_if_ours(manifest.parent().unwrap(), &manifest).unwrap();
        assert!(manifest.exists());
        std::fs::write(&manifest, r#"{"name":"pixtuoid"}"#).unwrap();
        remove_if_ours(manifest.parent().unwrap(), &manifest).unwrap();
        assert!(!manifest.exists());
    }
}
