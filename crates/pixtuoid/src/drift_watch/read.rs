//! The set readers: how a document yields upstream's names, one per
//! [`Read`] variant that yields any. Each returns `None` when it cannot find
//! the declaration, which the watch files as probe health, never as a rename.

use std::collections::BTreeSet;

use pixtuoid_core::source::upstream::Read;
use serde_json::Value;

pub(super) type Names = BTreeSet<String>;

/// Upstream's names per `read`, from one document. `Each` yields none: it
/// tests our names instead.
pub(super) fn names(read: &Read, text: &str) -> Option<Names> {
    match *read {
        Read::Each { .. } => None,
        Read::JsonUnion { def, prop } => json_union(text, def, prop),
    }
}

/// Scoped to the union ON PURPOSE: every definition's `type` const also yields
/// nested-content tags (`audio`, `text`) that no envelope carries.
fn json_union(text: &str, def: &str, prop: Option<&str>) -> Option<Names> {
    let root: Value = serde_json::from_str(text).ok()?;
    let defs = root.get("$defs").or_else(|| root.get("definitions"))?;
    let union = defs.get(def)?;
    let members = union
        .get("oneOf")
        .or_else(|| union.get("anyOf"))?
        .as_array()?;
    let found: Names = members
        .iter()
        .filter_map(|m| {
            let node = match m.get("$ref").and_then(Value::as_str) {
                Some(r) => defs.get(r.rsplit('/').next()?)?,
                None => m,
            };
            let node = match prop {
                Some(p) => node.get("properties")?.get(p)?,
                None => node,
            };
            schema_const(node)
        })
        .collect();
    (!found.is_empty()).then_some(found)
}

/// A schema node's pinned string: `const`, or a one-element `enum`.
fn schema_const(node: &Value) -> Option<String> {
    let c = match node.get("const") {
        Some(c) => c,
        None => match node.get("enum")?.as_array()?.as_slice() {
            [only] => only,
            _ => return None,
        },
    };
    c.as_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(xs: &[&str]) -> Option<Names> {
        Some(xs.iter().map(|s| (*s).to_owned()).collect())
    }

    #[test]
    fn a_json_union_resolves_refs_consts_and_single_enums() {
        let schema = r##"{"$defs": {
            "U": {"oneOf": [
                {"$ref": "#/$defs/A"},
                {"properties": {"tag": {"const": "inline"}}},
                {"properties": {"tag": {"enum": ["single"]}}},
                {"properties": {"tag": {"enum": ["one", "two"]}}}
            ]},
            "A": {"properties": {"tag": {"const": "by_ref"}}},
            "S": {"oneOf": [{"const": "done"}, {"const": "failed"}]},
            "Nested": {"properties": {"tag": {"const": "never_in_the_union"}}}
        }}"##;
        let union = Read::JsonUnion {
            def: "U",
            prop: Some("tag"),
        };
        assert_eq!(names(&union, schema), set(&["by_ref", "inline", "single"]));
        let own = Read::JsonUnion {
            def: "S",
            prop: None,
        };
        assert_eq!(names(&own, schema), set(&["done", "failed"]));
        let missing = Read::JsonUnion {
            def: "Gone",
            prop: None,
        };
        assert_eq!(names(&missing, schema), None);
        assert_eq!(names(&union, "not json"), None);
        // The draft-07 spelling: `definitions` and `anyOf`.
        let draft7 = r##"{"definitions": {
            "U": {"anyOf": [{"$ref": "#/definitions/A"}]},
            "A": {"properties": {"tag": {"const": "by_ref"}}}
        }}"##;
        assert_eq!(names(&union, draft7), set(&["by_ref"]));
    }
}
