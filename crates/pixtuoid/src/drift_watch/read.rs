//! The readers: how a document yields upstream's names, one per
//! [`Read`](pixtuoid_core::source::upstream::Read) variant. A reader that
//! cannot find its declaration says what it found instead, which the watch
//! files as probe health, never as a rename.

use std::collections::BTreeSet;

use regex::Regex;
use serde_json::Value;

pub(super) type Names = BTreeSet<String>;

/// Why a reader yields no names.
#[derive(Debug)]
pub(super) enum Unread {
    /// A pattern of ours does not compile.
    Ours(regex::Error),
    /// The document lacks what the reader reads: this is what it found
    /// instead, worded to follow "the reader found".
    Missing(String),
}

impl From<regex::Error> for Unread {
    fn from(e: regex::Error) -> Self {
        Self::Ours(e)
    }
}

fn missing(what: impl Into<String>) -> Unread {
    Unread::Missing(what.into())
}

/// The ones of `ours` that `pattern` finds, `{name}` standing for each.
pub(super) fn each(text: &str, pattern: &str, ours: &BTreeSet<&str>) -> Result<Names, Unread> {
    let mut found = Names::new();
    for name in ours {
        if Regex::new(&pattern.replace("{name}", &regex::escape(name)))?.is_match(text) {
            found.insert((*name).to_owned());
        }
    }
    Ok(found)
}

/// A Rust document is read with its comments stripped, so a commented-out
/// declaration is not one.
pub(super) fn sole(text: &str, path: &str, pattern: &str) -> Result<Names, Unread> {
    let src = if path.ends_with(".rs") {
        strip_rust_comments(text)
    } else {
        text.to_owned()
    };
    let re = Regex::new(pattern)?;
    let mut all = re.captures_iter(&src);
    match (all.next(), all.next()) {
        (Some(one), None) => one
            .get(1)
            .map(|m| [m.as_str().to_owned()].into())
            .ok_or_else(|| missing("a match of its pattern with no name in it")),
        (None, _) => Err(missing("no match of its pattern")),
        (Some(_), Some(_)) => Err(missing("more than one match of its pattern")),
    }
}

/// Off the brace-balanced, comment-stripped body of `fn name`.
pub(super) fn rust_arms(text: &str, name: &str, arm: &str) -> Result<Names, Unread> {
    let opener = Regex::new(&format!(r"fn\s+{}\b[^{{]*\{{", regex::escape(name)))?;
    let heads = Regex::new(&format!(r#"((?:"[^"]*"\s*\|\s*)*"[^"]*")\s*=>\s*{arm}"#))?;
    let quoted = Regex::new(r#""([^"]*)""#)?;
    let body = braced_body(&strip_rust_comments(text), &opener)
        .ok_or_else(|| missing(format!("no `fn {name}`")))?;
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
    if found.is_empty() {
        return Err(missing(format!(
            "no arm of `fn {name}` yielding its target"
        )));
    }
    Ok(found)
}

/// Comments go first, and a struct variant's own fields never name a variant,
/// so only the top level is read. An `alias` is only ever read by serde, so it
/// never counts: a name moved into one is gone from the wire.
pub(super) fn rust_enum(text: &str, name: &str) -> Result<Names, Unread> {
    let src = strip_rust_comments(text);
    let opener = Regex::new(&format!(r"enum\s+{}\b[^{{;]*\{{", regex::escape(name)))?;
    let decl = opener
        .find(&src)
        .ok_or_else(|| missing(format!("no `enum {name}`")))?;
    // The item's own attributes: whatever follows the item before it.
    let head = &src[..decl.start()];
    let attrs = &head[head.rfind(['{', '}', ';']).map_or(0, |i| i + 1)..];
    let snake = Regex::new(r#"#\[serde\([^\]]*\brename_all\s*=\s*"snake_case""#)?;
    if !snake.is_match(attrs) {
        return Err(missing(format!(
            "`enum {name}` without `rename_all = \"snake_case\"`"
        )));
    }
    let body =
        braced_body(&src, &opener).ok_or_else(|| missing(format!("no end to `enum {name}`")))?;
    let variants = top_level(&body, '{', '}');
    let rename = Regex::new(r#"#\[serde\([^\]]*\brename\s*=\s*"([^"]+)""#)?;
    let ident = Regex::new(r"\b([A-Z][A-Za-z0-9]*)\b")?;
    let names: Names = split_top_level(&variants)
        .into_iter()
        .filter_map(|variant| match rename.captures(variant) {
            Some(renamed) => renamed.get(1).map(|m| m.as_str().to_owned()),
            None => ident
                .captures(&top_level(variant, '[', ']'))
                .and_then(|c| c.get(1))
                .map(|m| snake_case(m.as_str())),
        })
        .collect();
    if names.is_empty() {
        return Err(missing(format!("no variant in `enum {name}`")));
    }
    Ok(names)
}

/// serde's own `snake_case` (serde_derive's `internals/case.rs`): an `_`
/// before every uppercase letter but the first, then all of it lowercase.
fn snake_case(ident: &str) -> String {
    let mut out = String::new();
    for (i, c) in ident.char_indices() {
        if i > 0 && c.is_uppercase() {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// `src` cut at each comma outside every bracket.
fn split_top_level(src: &str) -> Vec<&str> {
    let (mut out, mut depth, mut start) = (Vec::new(), 0usize, 0);
    for (i, c) in src.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                out.push(&src[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&src[start..]);
    out
}

/// `src` without anything between `open` and its `close`, nested pairs
/// included.
fn top_level(src: &str, open: char, close: char) -> String {
    let mut depth = 0usize;
    src.chars()
        .filter(|&c| {
            if c == open {
                depth += 1;
            } else if c == close {
                depth = depth.saturating_sub(1);
                return false;
            }
            depth == 0
        })
        .collect()
}

/// A duplicate or restyled declaration is probe health, never a rename, so
/// exactly one must match.
pub(super) fn ts_fields(text: &str, name: &str) -> Result<Names, Unread> {
    let decl = Regex::new(&format!(
        r"(?ms)^export\s+interface\s+{}\b[^{{\n]*\{{(.*?)^\}}",
        regex::escape(name)
    ))?;
    let mut all = decl.captures_iter(text);
    let body = match (all.next(), all.next()) {
        (Some(one), None) => one.get(1).map_or("", |m| m.as_str()),
        _ => return Err(missing(format!("no single `export interface {name}`"))),
    };
    let field =
        Regex::new(r#"(?m)^\s*(?:readonly\s+)?(?:"([^"]+)"|'([^']+)'|([A-Za-z_$][\w$]*))\??\s*:"#)?;
    let names: Names = field
        .captures_iter(body)
        .filter_map(|c| c.get(1).or_else(|| c.get(2)).or_else(|| c.get(3)))
        .map(|m| m.as_str().to_owned())
        .collect();
    if names.is_empty() {
        return Err(missing(format!("no field in `{name}`")));
    }
    Ok(names)
}

pub(super) fn matches(text: &str, pattern: &str) -> Result<Names, Unread> {
    let found: Names = Regex::new(pattern)?
        .captures_iter(text)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_owned()))
        .collect();
    if found.is_empty() {
        return Err(missing("no match of its pattern"));
    }
    Ok(found)
}

/// The first table under that header: a page's later tables repeat its names.
pub(super) fn md_table(text: &str, header: &str) -> Result<Names, Unread> {
    let table = Regex::new(&format!(
        r"(?m)^\|\s*{}\s*\|[^\n]*\n\|[\s:|-]*\n((?:\|[^\n]*\n?)+)",
        regex::escape(header)
    ))?;
    let rows = table
        .captures(text)
        .and_then(|c| c.get(1))
        .ok_or_else(|| missing(format!("no `{header}` table")))?;
    let name = Regex::new(r"(?m)^\|\s*`([^`]+)`\s*\|")?;
    let found: Names = name
        .captures_iter(rows.as_str())
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_owned()))
        .collect();
    if found.is_empty() {
        return Err(missing(format!("no name in the `{header}` table")));
    }
    Ok(found)
}

pub(super) fn md_headings(text: &str) -> Result<Names, Unread> {
    let heading = Regex::new(r"(?m)^#{2,4}[ \t]+([A-Za-z_][A-Za-z0-9_]*)[ \t]*$")?;
    let found: Names = heading
        .captures_iter(text)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_owned()))
        .collect();
    if found.is_empty() {
        return Err(missing("no one-word heading"));
    }
    Ok(found)
}

/// Comments go first: a comment quoting `{"action": "continue"}` once ended
/// the set at its stray `}`. An empty set is an answer, not a miss.
pub(super) fn py_set(text: &str, decl: &str) -> Result<Names, Unread> {
    let src = strip_py_comments(text);
    let opener = Regex::new(&format!(r"(?:{decl})\s*\{{"))?;
    let mut all = opener.find_iter(&src);
    if !matches!((all.next(), all.next()), (Some(_), None)) {
        return Err(missing("no single declaration matching its pattern"));
    }
    let body =
        braced_body(&src, &opener).ok_or_else(|| missing("no end to the set its pattern opens"))?;
    let member = Regex::new(r#""([^"\\\n]+)""#)?;
    Ok(member
        .captures_iter(&body)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_owned()))
        .collect())
}

/// Python source without `#` comments, string literals kept.
fn strip_py_comments(src: &str) -> String {
    src.lines()
        .map(|line| {
            let mut quote = None;
            for (i, c) in line.char_indices() {
                match (quote, c) {
                    (None, '#') => return &line[..i],
                    (None, '"' | '\'') => quote = Some(c),
                    (Some(q), _) if c == q => quote = None,
                    _ => {}
                }
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Only each row's header: a row's body is `key: value` lines whose values
/// carry capitalised tokens of their own.
pub(super) fn rust_macro_rows(text: &str, name: &str) -> Result<Names, Unread> {
    let src = strip_rust_comments(text);
    let opener = Regex::new(&format!(r"(?m)^\s*{}!\s*\{{", regex::escape(name)))?;
    let body = braced_body(&src, &opener).ok_or_else(|| missing(format!("no `{name}!` table")))?;
    let header = Regex::new(r"^\s*([A-Z][A-Za-z0-9_]*)\s*\{")?;
    let found: Names = split_top_level(&body)
        .into_iter()
        .filter_map(|row| header.captures(row)?.get(1).map(|m| m.as_str().to_owned()))
        .collect();
    if found.is_empty() {
        return Err(missing(format!("no row in `{name}!`")));
    }
    Ok(found)
}

/// Scoped to the union ON PURPOSE: every definition's `type` const also yields
/// nested-content tags (`audio`, `text`) that no envelope carries.
pub(super) fn json_union(text: &str, def: &str, prop: Option<&str>) -> Result<Names, Unread> {
    let root: Value = serde_json::from_str(text).map_err(|_| missing("no JSON"))?;
    union_members(&root, def, prop)
        .filter(|found| !found.is_empty())
        .ok_or_else(|| missing(format!("no `{def}` union")))
}

fn union_members(root: &Value, def: &str, prop: Option<&str>) -> Option<Names> {
    let defs = root.get("$defs").or_else(|| root.get("definitions"))?;
    let union = defs.get(def)?;
    let members = union
        .get("oneOf")
        .or_else(|| union.get("anyOf"))?
        .as_array()?;
    Some(
        members
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
            .collect(),
    )
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

pub(super) fn json_keys(text: &str) -> Result<Names, Unread> {
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
    let root: Value = serde_json::from_str(text).map_err(|_| missing("no JSON"))?;
    let mut out = Names::new();
    walk(&root, &mut out);
    if out.is_empty() {
        return Err(missing("no `properties`"));
    }
    Ok(out)
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

    /// The names, or what the reader found instead; a pattern of ours that
    /// fails to compile fails the test.
    fn found(read: Result<Names, Unread>) -> Result<Vec<String>, String> {
        match read {
            Ok(names) => Ok(names.into_iter().collect()),
            Err(Unread::Missing(what)) => Err(what),
            Err(Unread::Ours(e)) => panic!("a test pattern does not compile: {e}"),
        }
    }

    fn names(xs: &[&str]) -> Result<Vec<String>, String> {
        Ok(xs.iter().map(|s| (*s).to_owned()).collect())
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
            found(json_union(schema, "U", Some("tag"))),
            names(&["by_ref", "inline", "single"])
        );
        assert_eq!(
            found(json_union(schema, "S", None)),
            names(&["done", "failed"])
        );
        assert_eq!(
            found(json_union(schema, "Gone", None)),
            Err("no `Gone` union".to_owned())
        );
        assert_eq!(
            found(json_union("not json", "U", Some("tag"))),
            Err("no JSON".to_owned())
        );
        // The draft-07 spelling: `definitions` and `anyOf`.
        let draft7 = r##"{"definitions": {
            "U": {"anyOf": [{"$ref": "#/definitions/A"}]},
            "A": {"properties": {"tag": {"const": "by_ref"}}}
        }}"##;
        assert_eq!(
            found(json_union(draft7, "U", Some("tag"))),
            names(&["by_ref"])
        );
    }

    #[test]
    fn a_sole_read_refuses_a_second_declaration() {
        let p = r#"const M: &str = "([^"]+)""#;
        let read = |text: &str, path: &str| found(sole(text, path, p));
        assert_eq!(read(r#"const M: &str = "a";"#, "x.rs"), names(&["a"]));
        // A `#[cfg(test)]` twin above the real one would make a first match wrong.
        assert_eq!(
            read("const M: &str = \"a\";\nconst M: &str = \"b\";", "x.rs"),
            Err("more than one match of its pattern".to_owned())
        );
        assert_eq!(
            read("const N: &str = \"a\";", "x.rs"),
            Err("no match of its pattern".to_owned())
        );
        // A commented-out declaration is not one, but only in Rust.
        let old = "// const M: &str = \"old\";\nconst M: &str = \"a\";";
        assert_eq!(read(old, "x.rs"), names(&["a"]));
        assert!(read(old, "x.ts").is_err());
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
        let read = |t: &str| found(rust_arms(t, "parse", arm));
        assert_eq!(read(src), names(&["spawn", "start", "wait"]));
        assert_eq!(read("fn parsed() {}"), Err("no `fn parse`".to_owned()));
        assert_eq!(
            read("fn parse() { match a { \"list\" => Ok(Action::List) } }"),
            Err("no arm of `fn parse` yielding its target".to_owned())
        );
    }

    #[test]
    fn a_rust_enum_yields_its_serde_wire_names() {
        let src = r#"
            const X: u8 = 1;
            /// Docs, and a `#[serde(rename_all = "camelCase")]` in a comment.
            #[derive(Serialize)]
            #[serde(tag = "type", rename_all = "snake_case")]
            pub(super) enum Item<'a> {
                /// Fires like `Foo { bar }` does, or `Baz }`.
                EventMsg { payload: Cow<'a, Payload> },
                #[serde(rename = "compaction_summary", alias = "compacted")]
                Compaction(Summary),
                HTTPCall,
                Struct {
                    #[serde(rename = "agentAddress")]
                    agent_address: String,
                    nested: Inner { Deep },
                },
                // Gone,
            }
            enum ItemKind { NotThisOne }"#;
        assert_eq!(
            found(rust_enum(src, "Item")),
            names(&["compaction_summary", "event_msg", "h_t_t_p_call", "struct"])
        );
        assert_eq!(
            found(rust_enum(src, "ItemKind")).map(|_| ()).unwrap_err(),
            "`enum ItemKind` without `rename_all = \"snake_case\"`"
        );
        assert_eq!(
            found(rust_enum(src, "Missing")),
            Err("no `enum Missing`".to_owned())
        );
        // An attribute's own words never name the variant it sits on.
        let attributed = "#[serde(rename_all = \"snake_case\")]\nenum A {\n    #[doc = \"Other\"]\n    Plain,\n}";
        assert_eq!(found(rust_enum(attributed, "A")), names(&["plain"]));
        // Another spelling is never guessed at: its names would all be wrong.
        let camel = "#[serde(rename_all = \"camelCase\")]\nenum E { OneTwo }";
        assert!(found(rust_enum(camel, "E")).is_err());
    }

    #[test]
    fn ts_fields_read_the_one_interface() {
        let src = "export interface Entry extends Base {\n  readonly type: \"x\";\n  title?: string;\n  \"quoted-key\": number;\n  nested: {\n    inner: string;\n  };\n}\nexport interface Other {\n  title: string;\n  only_other: string;\n}\n";
        assert_eq!(
            found(ts_fields(src, "Entry")),
            names(&["inner", "nested", "quoted-key", "title", "type"])
        );
        assert_eq!(
            found(ts_fields(src, "Gone")),
            Err("no single `export interface Gone`".to_owned())
        );
        let twice = format!("{src}export interface Other {{\n  b: string;\n}}\n");
        assert!(found(ts_fields(&twice, "Other")).is_err());
    }

    #[test]
    fn matches_read_every_first_group() {
        let go = "const (\n\tSessionStart Event = \"SessionStart\"\n\tStop Event = \"Stop\"\n)\nvar x = \"Nope\"";
        let p = r#"\w+\s+Event\s*=\s*"(\w+)""#;
        assert_eq!(found(matches(go, p)), names(&["SessionStart", "Stop"]));
        assert_eq!(
            found(matches("const ()", p)),
            Err("no match of its pattern".to_owned())
        );
    }

    #[test]
    fn an_md_table_is_read_by_its_header_and_first_column() {
        let page = "| Event | When it fires |\n|---|---|\n| `PreToolUse` | before `Bash` |\n| `Stop` | done |\n\n| Field | Event |\n|---|---|\n| `x` | `Other` |\n";
        assert_eq!(
            found(md_table(page, "Event")),
            names(&["PreToolUse", "Stop"])
        );
        assert_eq!(
            found(md_table(page, "Hook")),
            Err("no `Hook` table".to_owned())
        );
    }

    #[test]
    fn md_headings_are_the_one_word_sections() {
        let page = "# Hooks\n## Hook events\n### preToolUse\n#### stop\n##### deeper\nprose stop\n";
        assert_eq!(found(md_headings(page)), names(&["preToolUse", "stop"]));
    }

    #[test]
    fn a_py_set_is_the_one_declaration_comments_first() {
        let src = "VALID_HOOKS: set[str] = {\n    \"on_start\",  # was {\"x\"} once\n    \"on_end\",\n}\n";
        let decl = r"VALID_HOOKS\s*:\s*set\[str\]\s*=";
        assert_eq!(found(py_set(src, decl)), names(&["on_end", "on_start"]));
        let frozen = "_BLOCKING_EVENTS = frozenset({\"pre_tool_call\"})";
        assert_eq!(
            found(py_set(frozen, r"_BLOCKING_EVENTS\s*=\s*frozenset\(")),
            names(&["pre_tool_call"])
        );
        for comment in [
            "# returns {\"action\": \"continue\"",
            "# anything else } ends it",
        ] {
            let poisoned = format!(
                "VALID_HOOKS: set[str] = {{\n    \"on_start\",\n    {comment}\n    \"on_end\",\n}}\n"
            );
            assert_eq!(
                found(py_set(&poisoned, decl)),
                names(&["on_end", "on_start"])
            );
        }
        assert_eq!(found(py_set("X = {\n}", r"X\s*=")), names(&[]));
        assert!(found(py_set(&format!("{src}{src}"), decl)).is_err());
    }

    #[test]
    fn macro_rows_are_the_row_headers_only() {
        let src = "macro_rules! hook_events {\n    ($($v:ident { $($r:tt)* }),*) => { pub enum HookEventName { $($v,)* } };\n}\nhook_events! {\n    SessionStart { alias: \"start\", traits: [Blocking] },\n    // Gone { },\n    Stop { alias: \"stop\" },\n}\n";
        assert_eq!(
            found(rust_macro_rows(src, "hook_events")),
            names(&["SessionStart", "Stop"])
        );
        assert_eq!(
            found(rust_macro_rows("fn x() {}", "hook_events")),
            Err("no `hook_events!` table".to_owned())
        );
    }

    #[test]
    fn json_keys_walk_every_depth() {
        let schema = r#"{"properties": {"a": {}, "data": {"properties": {"b": {}}}},
                         "definitions": {"X": {"anyOf": [{"properties": {"c": {}}}]}}}"#;
        assert_eq!(found(json_keys(schema)), names(&["a", "b", "c", "data"]));
        assert_eq!(
            found(json_keys(r#"{"type": "object"}"#)),
            Err("no `properties`".to_owned())
        );
        assert_eq!(found(json_keys("not json")), Err("no JSON".to_owned()));
    }

    #[test]
    fn comment_stripping_keeps_strings_and_lifetimes() {
        let src = "let a = \"// kept\"; // gone\n/* gone /* nested */ gone */fn f<'a>(x: &'a str) {} // gone";
        assert_eq!(
            strip_rust_comments(src),
            "let a = \"// kept\"; \nfn f<'a>(x: &'a str) {} "
        );
        // An escaped quote does not end the string, so what follows it is kept.
        let escaped = r#"let b = "a \" // kept"; // gone"#;
        assert_eq!(strip_rust_comments(escaped), r#"let b = "a \" // kept"; "#);
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
