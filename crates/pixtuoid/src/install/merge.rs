//! Install-WRITE shared helpers — the config parse/merge core every JSON/TOML
//! target's `merge_install`/`merge_uninstall` rides. These fns MUTATE;
//! `verify.rs` next door is the READ-only soundness detector. Per-target FORMAT
//! knowledge stays in each `install/<target>.rs` (invariant #3); only the
//! shape-shared machinery lives here.

use crate::install::target::MergeOutcome;
use serde_json::{json, Map, Value};

/// Parse JSON config content, treating empty/whitespace-only as the empty
/// document (`{}`) — the shared rule every JSON target's merge relies on.
pub(crate) fn parse_json_or_empty(content: &str) -> anyhow::Result<Value> {
    if content.trim().is_empty() {
        return Ok(json!({}));
    }
    use anyhow::Context;
    serde_json::from_str(content).context("not valid JSON — refusing to overwrite")
}

/// Bake `hook_path` (JSON-escaped) into a plugin `template` at `placeholder` —
/// the shared renderer for the code-artifact targets (opencode `.ts`, openclaw
/// `.js`). JSON strings are a subset of JS string literals EXCEPT U+2028/U+2029
/// (valid unescaped in JSON, line terminators in JS) — neither occurs in a real
/// filesystem path, so the result is a valid JS literal for any path the
/// resolver hands us.
pub(crate) fn bake_hook_path(
    template: &str,
    placeholder: &str,
    hook_path: &str,
    what: &str,
) -> anyhow::Result<String> {
    use anyhow::Context;
    let json = serde_json::to_string(hook_path)
        .with_context(|| format!("serializing the hook path into the {what} plugin"))?;
    Ok(template.replace(placeholder, &json))
}

/// The shim path as `&str`. A non-UTF-8 path is rejected rather than
/// `to_string_lossy`'d into a silently-dead hook.
pub(crate) fn hook_path_str(p: &std::path::Path) -> anyhow::Result<&str> {
    use anyhow::anyhow;
    p.to_str()
        .ok_or_else(|| anyhow!("pixtuoid-hook path is non-UTF-8: {}", p.display()))
}

pub(crate) fn parse_toml_or_empty(content: &str) -> anyhow::Result<toml::Value> {
    if content.trim().is_empty() {
        return Ok(toml::Value::Table(toml::value::Table::new()));
    }
    use anyhow::Context;
    toml::from_str(content).context("not valid TOML — refusing to overwrite")
}

/// Merge managed hook entries into `doc`: for each `event`, drop any prior
/// managed entry (keyed on `sentinel`) and push a fresh one built by
/// `make_entry`. The entry SHAPE stays the caller's — the merge treats the entry
/// opaquely, keying only on the sentinel, so Claude's nested
/// `{matcher, hooks:[…]}` group rides through unchanged.
pub(crate) fn flat_json_merge_install(
    doc: Value,
    events: &[&str],
    sentinel: &str,
    make_entry: impl Fn(&str) -> Value,
    hook_command: &str,
) -> Value {
    let mut root: Map<String, Value> = doc.as_object().cloned().unwrap_or_default();
    let hooks = root
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !hooks.is_object() {
        *hooks = Value::Object(Map::new());
    }
    if let Value::Object(hooks_obj) = hooks {
        for ev in events {
            let list = hooks_obj
                .entry((*ev).to_string())
                .or_insert_with(|| Value::Array(vec![]));
            if !list.is_array() {
                *list = Value::Array(vec![]);
            }
            if let Value::Array(arr) = list {
                arr.retain(|entry| !is_flat_managed(entry, sentinel));
                arr.push(make_entry(hook_command));
            }
        }
    }
    Value::Object(root)
}

/// Drop `key` from `parent` when it is an EMPTY object/array. Anything with
/// content (a foreign plugin's entry, the user's own allowlist members) is left
/// exactly as found.
pub(crate) fn prune_empty(parent: &mut serde_json::Map<String, Value>, key: &str) {
    let empty = match parent.get(key) {
        Some(Value::Object(m)) => m.is_empty(),
        Some(Value::Array(a)) => a.is_empty(),
        _ => false,
    };
    if empty {
        parent.remove(key);
    }
}

pub(crate) fn prune_empty_root(root: &mut Value, key: &str) {
    if let Some(obj) = root.as_object_mut() {
        prune_empty(obj, key);
    }
}

/// Remove managed hook entries (keyed on `sentinel`) from `doc`, then drop any
/// event key whose array went empty and the `hooks` object if it emptied. A
/// target-specific key the install set (Cursor's `version`) is deliberately
/// preserved — this only touches `hooks`.
pub(crate) fn flat_json_merge_uninstall(mut doc: Value, sentinel: &str) -> Value {
    let Some(root) = doc.as_object_mut() else {
        return doc;
    };
    let Some(Value::Object(hooks_obj)) = root.get_mut("hooks") else {
        return doc;
    };
    for (_ev, list) in hooks_obj.iter_mut() {
        if let Some(arr) = list.as_array_mut() {
            arr.retain(|entry| !is_flat_managed(entry, sentinel));
        }
    }
    let to_remove: Vec<String> = hooks_obj
        .iter()
        .filter_map(|(k, v)| match v.as_array() {
            Some(a) if a.is_empty() => Some(k.clone()),
            _ => None,
        })
        .collect();
    for k in to_remove {
        hooks_obj.remove(&k);
    }
    prune_empty(root, "hooks");
    doc
}

fn is_flat_managed(entry: &Value, sentinel: &str) -> bool {
    entry.get(sentinel).and_then(|v| v.as_bool()) == Some(true)
}

/// Parse flat-JSON `content`, REFUSE a valid-but-non-object root (which
/// `flat_json_merge_install` would silently coerce to `{}`, dropping the user's
/// document), run `mutate`, and package a `MergeOutcome` whose `changed` is a
/// SEMANTIC parsed-doc diff — a byte diff would churn the user's formatting and
/// delete their only backup on the next uninstall. The guard lives HERE, once,
/// so a future flat-JSON target cannot forget it.
pub(crate) fn flat_json_merge_outcome_install(
    content: &str,
    what: &str,
    mutate: impl FnOnce(Value) -> Value,
) -> anyhow::Result<MergeOutcome> {
    let doc = parse_json_or_empty(content)?;
    if !doc.is_object() && !doc.is_null() {
        anyhow::bail!("{what} is valid JSON but not an object — refusing to overwrite");
    }
    let merged = mutate(doc.clone());
    let changed = merged != doc;
    Ok(MergeOutcome {
        content: serde_json::to_string_pretty(&merged)?,
        changed,
    })
}

/// The uninstall twin, deliberately UNGUARDED on a non-object root — uninstall
/// must stay a clean no-op on a foreign non-object document, never error.
pub(crate) fn flat_json_merge_outcome_uninstall(
    content: &str,
    mutate: impl FnOnce(Value) -> Value,
) -> anyhow::Result<MergeOutcome> {
    let doc = parse_json_or_empty(content)?;
    let cleaned = mutate(doc.clone());
    let changed = cleaned != doc;
    Ok(MergeOutcome {
        content: serde_json::to_string_pretty(&cleaned)?,
        changed,
    })
}

/// The TOML analog. No non-object guard — a TOML root is always a table.
pub(crate) fn toml_merge_outcome(
    content: &str,
    mutate: impl FnOnce(toml::Value) -> toml::Value,
) -> anyhow::Result<MergeOutcome> {
    let doc = parse_toml_or_empty(content)?;
    let merged = mutate(doc.clone());
    let changed = merged != doc;
    Ok(MergeOutcome {
        content: toml::to_string_pretty(&merged)?,
        changed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_json_merge_outcome_install_refuses_a_valid_non_object_root() {
        for content in ["[1, 2, 3]", "\"hello\"", "42", "true"] {
            let err = flat_json_merge_outcome_install(content, "settings", |d| d)
                .unwrap_err()
                .to_string();
            assert!(err.contains("not an object"), "{content}: {err}");
        }
        for ok in ["", "null", "{}"] {
            assert!(
                flat_json_merge_outcome_install(ok, "settings", |d| d).is_ok(),
                "{ok}"
            );
        }
    }

    #[test]
    fn flat_json_merge_outcome_uninstall_is_a_clean_noop_on_a_non_object_root() {
        let out = flat_json_merge_outcome_uninstall("[1, 2, 3]", |d| d).unwrap();
        assert!(!out.changed);
    }

    #[test]
    fn toml_merge_outcome_reports_semantic_change_not_byte_change() {
        let noop = toml_merge_outcome("a = 1\n", |d| d).unwrap();
        assert!(!noop.changed, "identity mutate must be a semantic no-op");
        let changed = toml_merge_outcome("a = 1\n", |mut d| {
            if let toml::Value::Table(t) = &mut d {
                t.insert("b".into(), toml::Value::Integer(2));
            }
            d
        })
        .unwrap();
        assert!(changed.changed);
    }

    #[test]
    fn flat_json_merge_uninstall_returns_non_object_unchanged() {
        let arr = json!([1, 2, 3]);
        assert_eq!(flat_json_merge_uninstall(arr.clone(), "_pixtuoid"), arr);
        let scalar = json!(42);
        assert_eq!(
            flat_json_merge_uninstall(scalar.clone(), "_pixtuoid"),
            scalar
        );
    }

    #[test]
    fn hook_path_str_returns_utf8_path() {
        let p = std::path::Path::new("/opt/bin/pixtuoid-hook");
        assert_eq!(hook_path_str(p).unwrap(), "/opt/bin/pixtuoid-hook");
    }

    #[test]
    fn hook_path_str_rejects_non_utf8() {
        let bad = non_utf8_path();
        let err = hook_path_str(&bad).unwrap_err().to_string();
        assert!(err.contains("non-UTF-8"), "{err}");
    }

    #[cfg(unix)]
    fn non_utf8_path() -> std::path::PathBuf {
        use std::os::unix::ffi::OsStrExt;
        std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b"/x/\xff\xfehook"))
    }

    #[cfg(windows)]
    fn non_utf8_path() -> std::path::PathBuf {
        use std::os::windows::ffi::OsStringExt;
        // An unpaired surrogate (0xD800) is valid UTF-16 to the OS but not UTF-8.
        std::ffi::OsString::from_wide(&[0x005C, 0x0078, 0xD800, 0x0068]).into()
    }
}
