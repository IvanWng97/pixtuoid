//! The set readers: how a document yields upstream's names, one per
//! [`Read`](pixtuoid_core::source::upstream::Read) variant that yields any. A
//! reader returns `None` when it cannot find the declaration, which the watch
//! files as probe health, never as a rename.

use std::collections::BTreeSet;

use regex::Regex;
use serde_json::Value;

pub(super) type Names = BTreeSet<String>;

/// The first group of the ONE match of `pattern`; a Rust document is read with
/// its comments stripped, so a commented-out declaration is not one.
pub(super) fn sole(text: &str, path: &str, pattern: &str) -> Result<Option<Names>, regex::Error> {
    let src = if path.ends_with(".rs") {
        strip_rust_comments(text)
    } else {
        text.to_owned()
    };
    let re = Regex::new(pattern)?;
    let mut all = re.captures_iter(&src);
    Ok(match (all.next(), all.next()) {
        (Some(one), None) => one.get(1).map(|m| [m.as_str().to_owned()].into()),
        _ => None,
    })
}

/// The string patterns of `fn name`'s match arms whose result matches `arm`.
pub(super) fn rust_arms(text: &str, name: &str, arm: &str) -> Result<Option<Names>, regex::Error> {
    let opener = Regex::new(&format!(r"fn\s+{}\b[^{{]*\{{", regex::escape(name)))?;
    let heads = Regex::new(&format!(r#"((?:"[^"]*"\s*\|\s*)*"[^"]*")\s*=>\s*{arm}"#))?;
    let quoted = Regex::new(r#""([^"]*)""#)?;
    Ok(
        braced_body(&strip_rust_comments(text), &opener).and_then(|body| {
            let found: Names = heads
                .captures_iter(&body)
                .flat_map(|h| {
                    quoted
                        .captures_iter(h.get(1).map_or("", |m| m.as_str()))
                        .filter_map(|q| q.get(1).map(|m| m.as_str().to_owned()))
                        .collect::<Vec<_>>()
                })
                .filter(|s| !s.is_empty())
                .collect();
            (!found.is_empty()).then_some(found)
        }),
    )
}

/// Scoped to the union ON PURPOSE: every definition's `type` const also yields
/// nested-content tags (`audio`, `text`) that no envelope carries.
pub(super) fn json_union(text: &str, def: &str, prop: Option<&str>) -> Option<Names> {
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

pub(super) fn json_keys(text: &str) -> Option<Names> {
    fn walk(v: &Value, out: &mut Names) {
        match v {
            Value::Object(o) => {
                if let Some(Value::Object(props)) = o.get("properties") {
                    out.extend(props.keys().cloned());
                }
                o.values().for_each(|x| walk(x, out));
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let root: Value = serde_json::from_str(text).ok()?;
    let mut out = Names::new();
    walk(&root, &mut out);
    (!out.is_empty()).then_some(out)
}

/// Rust source without `//` and `/* */` comments, string literals kept. A
/// scanner, not a regex: `/\*.*?\*/` stops at the first `*/` and re-admits a
/// nested comment's tail. Only `"` opens a string, since an apostrophe is a
/// lifetime and an odd count would swallow the following comment.
fn strip_rust_comments(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let (mut i, mut depth) = (0, 0usize);
    while i < b.len() {
        let pair = b.get(i..i + 2);
        if depth > 0 {
            match pair {
                Some(b"/*") => (depth, i) = (depth + 1, i + 2),
                Some(b"*/") => (depth, i) = (depth - 1, i + 2),
                _ => i += 1,
            }
            continue;
        }
        match pair {
            Some(b"/*") => (depth, i) = (1, i + 2),
            Some(b"//") => {
                i = b[i..]
                    .iter()
                    .position(|&c| c == b'\n')
                    .map_or(b.len(), |n| i + n)
            }
            _ if b[i] == b'"' => {
                let mut j = i + 1;
                while j < b.len() {
                    match b[j] {
                        b'\\' => j += 2,
                        b'"' => {
                            j += 1;
                            break;
                        }
                        _ => j += 1,
                    }
                }
                let j = j.min(b.len());
                out.extend_from_slice(&b[i..j]);
                i = j;
            }
            _ => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The brace-balanced body after `opener`, whose match ends at its `{`. Never a
/// regex: `(.*?)\}` stops at the first `}`, so one nested block truncates it.
fn braced_body(src: &str, opener: &Regex) -> Option<String> {
    let start = opener.find(src)?.end().checked_sub(1)?;
    let mut depth = 0usize;
    for (i, c) in src[start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(src[start + 1..start + i].to_owned());
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sole_of(pattern: &str, text: &str, path: &str) -> Option<Names> {
        sole(text, path, pattern).expect("the test pattern compiles")
    }

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
        assert_eq!(
            json_union(schema, "U", Some("tag")),
            set(&["by_ref", "inline", "single"])
        );
        assert_eq!(json_union(schema, "S", None), set(&["done", "failed"]));
        assert_eq!(json_union(schema, "Gone", None), None);
        assert_eq!(json_union("not json", "U", Some("tag")), None);
        // The draft-07 spelling: `definitions` and `anyOf`.
        let draft7 = r##"{"definitions": {
            "U": {"anyOf": [{"$ref": "#/definitions/A"}]},
            "A": {"properties": {"tag": {"const": "by_ref"}}}
        }}"##;
        assert_eq!(json_union(draft7, "U", Some("tag")), set(&["by_ref"]));
    }

    #[test]
    fn a_sole_read_refuses_a_second_declaration() {
        let p = r#"const M: &str = "([^"]+)""#;
        assert_eq!(sole_of(p, r#"const M: &str = "a";"#, "x.rs"), set(&["a"]));
        // A `#[cfg(test)]` twin above the real one would make a first match wrong.
        assert_eq!(
            sole_of(p, "const M: &str = \"a\";\nconst M: &str = \"b\";", "x.rs"),
            None
        );
        // A commented-out declaration is not one, but only in Rust.
        let old = "// const M: &str = \"old\";\nconst M: &str = \"a\";";
        assert_eq!(sole_of(p, old, "x.rs"), set(&["a"]));
        assert_eq!(sole_of(p, old, "x.ts"), None);
    }

    #[test]
    fn rust_arms_read_only_the_arms_yielding_the_target() {
        let arm = r"Ok\(\s*Action::(?:Start|Wait)\s*\)";
        let src = r#"
            fn parse(input: &str) -> Result<Action> {
                match input {
                    // "commented" => Ok(Action::Start),
                    "start" | "spawn" => Ok(Action::Start),
                    "wait" => Ok( Action::Wait ),
                    "list" => Ok(Action::List),
                    _ => { Err(()) }
                }
            }
            fn other() { "x" => Ok(Action::Start) }"#;
        let read = |t: &str| rust_arms(t, "parse", arm).expect("the test pattern compiles");
        assert_eq!(read(src), set(&["spawn", "start", "wait"]));
        assert_eq!(read("fn parsed() {}"), None);
    }

    #[test]
    fn json_keys_walk_every_depth() {
        let schema = r#"{"properties": {"a": {}, "data": {"properties": {"b": {}}}},
                         "definitions": {"X": {"anyOf": [{"properties": {"c": {}}}]}}}"#;
        assert_eq!(json_keys(schema), set(&["a", "b", "c", "data"]));
    }

    #[test]
    fn comment_stripping_keeps_strings_and_lifetimes() {
        let src = "let a = \"// kept\"; // gone\n/* gone /* nested */ gone */fn f<'a>(x: &'a str) {} // gone";
        assert_eq!(
            strip_rust_comments(src),
            "let a = \"// kept\"; \nfn f<'a>(x: &'a str) {} "
        );
    }

    #[test]
    fn a_braced_body_survives_nested_blocks() {
        let opener = Regex::new(r"enum E\s*\{").expect("compiles");
        let src = "enum E {\n    A { x: u8 },\n    B,\n}\nimpl E {}";
        assert_eq!(
            braced_body(src, &opener).as_deref(),
            Some("\n    A { x: u8 },\n    B,\n")
        );
        assert_eq!(braced_body("enum E { A", &opener), None);
    }
}
