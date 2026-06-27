//! Install-schema verification — the "silent-dead source" detector (#309).
//!
//! A source the user CONNECTED (hooks installed) renders ZERO sprites when its
//! install is structurally broken — the most confusing silent failure. We can't
//! tell "broken" from "the CLI isn't running" by counting events (that was
//! red-teamed out of #308), but we CAN deterministically verify the install we
//! ourselves wrote is still sound. This module is that check: pure, read-only,
//! false-positive-free.
//!
//! Three (four for CodeWhale) checks, all over the on-disk config:
//!   1. our `_pixtuoid` sentinel is still present,
//!   2. EVERY registered event still has a managed entry (catches an OLDER
//!      pixtuoid install missing newly-registered events — e.g. SubagentStart/
//!      Stop — which `has_hooks` passes but is silently half-dead),
//!   3. the embedded shim binary exists + is executable,
//!   4. (CodeWhale) `[hooks].enabled == true` (it gates ALL hooks on this).
//!
//! Per-source FORMAT knowledge stays in each `install/<target>.rs` `verify_schema`
//! (invariant #3) — this module holds only the shared result types, the shell
//! shim-path extractor (4 targets share `shell_hook_command`), and the
//! filesystem layer (`verify_target` in `mod.rs` stats the shim).

use std::path::PathBuf;

use serde_json::{json, Map, Value};

/// Parse JSON config content, treating empty/whitespace-only as the empty
/// document (`{}`) — the shared rule every JSON target's merge relies on (never
/// error on empty). The caller wraps the parse error with the real config path.
pub fn parse_json_or_empty(content: &str) -> anyhow::Result<Value> {
    if content.trim().is_empty() {
        return Ok(json!({}));
    }
    use anyhow::Context;
    serde_json::from_str(content).context("not valid JSON — refusing to overwrite")
}

/// Bake `hook_path` (JSON-escaped) into a plugin `template` at `placeholder` —
/// the shared renderer for the code-artifact targets (opencode `.ts`, openclaw
/// `.js`). serde_json emits a double-quoted, escaped JSON string; JSON strings
/// are a subset of JS string literals EXCEPT U+2028/U+2029 (valid unescaped in
/// JSON, line terminators in JS) — neither occurs in a real filesystem path, so
/// the result is a valid JS literal for any path the resolver hands us.
/// Serializing a `&str` is infallible in practice, but propagate the error
/// rather than default to a broken empty path if it ever weren't. `what` names
/// the target for the error context.
pub fn bake_hook_path(
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

/// The shim path as `&str`, or a uniform non-UTF-8 error — the ONE helper every
/// target's `hook_command` shares so the error wording can't drift (the shell
/// targets feed the `&str` into `hook_cmd::shell_hook_command`; the embed
/// targets `.map(str::to_string)` it). A non-UTF-8 path is rejected rather than
/// `to_string_lossy`'d into a silently-dead hook.
pub fn hook_path_str(p: &std::path::Path) -> anyhow::Result<&str> {
    use anyhow::anyhow;
    p.to_str()
        .ok_or_else(|| anyhow!("pixtuoid-hook path is non-UTF-8: {}", p.display()))
}

/// Parse TOML config content, treating empty/whitespace-only as the empty
/// document. Shared by the TOML targets (Codex/CodeWhale); same empty rule.
pub fn parse_toml_or_empty(content: &str) -> anyhow::Result<toml::Value> {
    if content.trim().is_empty() {
        return Ok(toml::Value::Table(toml::value::Table::new()));
    }
    use anyhow::Context;
    toml::from_str(content).context("not valid TOML — refusing to overwrite")
}

/// Merge managed flat-JSON hook entries into `doc` (Reasonix/Cursor share the
/// IDENTICAL shape): for each `event`, drop any prior managed entry (keyed on
/// `sentinel`) and push a fresh one built by `make_entry`. The per-target entry
/// SHAPE (Reasonix carries `timeout`/`description`, Cursor is bare) stays the
/// caller's `make_entry`, so this is shape-sharing, not a shared decoder. A
/// non-object `hooks` / non-array event is coerced (defensive), matching the two
/// callers' prior inline behavior. Caller-set extras (Cursor's `version`) are
/// applied before the call and pass through untouched.
pub fn flat_json_merge_install(
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

/// Remove managed flat-JSON hook entries (keyed on `sentinel`) from `doc`, then
/// drop any event key whose array went empty and the `hooks` object if it
/// emptied. The inverse of `flat_json_merge_install`, shared by Reasonix/Cursor.
/// A target-specific key the install set (Cursor's `version`) is deliberately
/// preserved — this only touches `hooks`.
pub fn flat_json_merge_uninstall(mut doc: Value, sentinel: &str) -> Value {
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
    if hooks_obj.is_empty() {
        root.remove("hooks");
    }
    doc
}

/// A flat-JSON entry is managed iff `entry[sentinel] == true`.
fn is_flat_managed(entry: &Value, sentinel: &str) -> bool {
    entry.get(sentinel).and_then(|v| v.as_bool()) == Some(true)
}

/// How a managed hook command references the shim binary — extracted PURELY from
/// the config content by a target's `verify_schema`; the filesystem check
/// (exists/executable, or PATH lookup) is layered on by `install::verify_target`,
/// which is the only part with I/O.
#[derive(Debug, PartialEq, Eq)]
pub enum ShimRef {
    /// An embedded absolute path → stat for exists + executable (hard signal).
    Absolute(PathBuf),
    /// A bare name relying on PATH resolution (Claude/Unix) → soft PATH check
    /// (a doctor-process PATH miss is NOT proof the CLI can't resolve it).
    BareName,
    /// No command/path could be extracted (parse failure / unexpected shape).
    Unknown,
}

/// The PURE, config-content-only half of a verification: everything a target can
/// decide from the parsed config alone. `issues` are HARD problems (definitely
/// broken); the shim filesystem check is added later by `verify_target`.
#[derive(Debug)]
pub struct SchemaParse {
    pub issues: Vec<String>,
    pub shim: ShimRef,
}

impl SchemaParse {
    /// A target whose config failed to parse / had no managed entries at all.
    pub fn broken(issue: impl Into<String>) -> Self {
        SchemaParse {
            issues: vec![issue.into()],
            shim: ShimRef::Unknown,
        }
    }
}

/// The full install-schema verdict for one target: config-level HARD issues plus
/// the shim-on-disk check, and any SOFT notes (environment-dependent, never a
/// "broken" verdict on their own — e.g. the Claude/Unix bare-name PATH miss).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SchemaVerifyResult {
    /// Hard problems — the install is definitively broken (hooks can't fire).
    pub issues: Vec<String>,
    /// Soft, possibly-environmental notes — shown by `doctor`, never the boot
    /// warning, and they do NOT make the install count as broken.
    pub notes: Vec<String>,
}

impl SchemaVerifyResult {
    /// Sound iff there are no HARD issues (soft notes don't count).
    pub fn is_sound(&self) -> bool {
        self.issues.is_empty()
    }
}

/// Control-char-strip a path for display in a HARD issue string that may reach a
/// REAL terminal (`pixtuoid doctor`'s stdout, the boot `eprintln!`). The shim
/// path is extracted from the user's HAND-EDITABLE hook command, so a crafted
/// path could carry ANSI/OSC escapes. Sanitize at the SOURCE (here, where the
/// untrusted value enters the issue Vec) so EVERY surface is covered at once —
/// per-output-site sanitize already missed the `doctor` stdout path once (the
/// online review). Mirrors `doctor`'s R0615-06 sanitize discipline. The
/// Sources panel is already safe (ratatui renders control bytes as literals),
/// but source-sanitizing it too is harmless + future-proof.
pub fn display_safe(p: &std::path::Path) -> String {
    crate::strip_control_chars(&p.display().to_string())
}

/// Assemble a `SchemaParse` from a per-target scan: the registered events that
/// LACK a managed entry, whether ANY managed entry was found at all, the shim
/// ref extracted from a managed command, and any target-specific extra issues
/// (e.g. CodeWhale `enabled=false`). Centralizes the issue wording so every
/// target reports consistently.
pub fn assemble(
    missing_events: &[&str],
    any_managed: bool,
    shim: ShimRef,
    extra: Vec<String>,
) -> SchemaParse {
    let mut issues = extra;
    if !any_managed {
        issues.push(
            "no managed pixtuoid hook entries (the `_pixtuoid` sentinel is absent — the config \
             was hand-edited or hooks were never installed)"
                .into(),
        );
    } else if !missing_events.is_empty() {
        issues.push(format!(
            "missing hook entries for: {} (an older pixtuoid install, or an upstream config-schema \
             change, orphaned them — reconnect via the Sources panel)",
            missing_events.join(", ")
        ));
    }
    SchemaParse { issues, shim }
}

/// Verify a FLAT-JSON hook config (Reasonix, Cursor): `hooks.<event>` is an
/// array of `{_pixtuoid: true, command, …}` entries. Shared because the two
/// targets use the IDENTICAL shape; each passes its own `events` + `sentinel`
/// (the per-source knowledge), so this is shape-sharing, not a shared decoder.
pub fn flat_json_verify(content: &str, events: &[&str], sentinel: &str) -> SchemaParse {
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(content) else {
        return SchemaParse::broken("hooks config no longer parses as JSON");
    };
    let hooks = doc.get("hooks").and_then(|h| h.as_object());
    let mut missing = Vec::new();
    let mut any = false;
    let mut shim = ShimRef::Unknown;
    for ev in events {
        let managed = hooks
            .and_then(|h| h.get(*ev))
            .and_then(|a| a.as_array())
            .and_then(|arr| {
                arr.iter()
                    .find(|e| e.get(sentinel).and_then(|v| v.as_bool()) == Some(true))
            });
        match managed {
            Some(entry) => {
                any = true;
                if shim == ShimRef::Unknown {
                    shim = entry
                        .get("command")
                        .and_then(|c| c.as_str())
                        .map(shell_shim_ref)
                        .unwrap_or(ShimRef::Unknown);
                }
            }
            None => missing.push(*ev),
        }
    }
    assemble(&missing, any, shim, vec![])
}

/// Extract the shim path from a shell-form managed command — the form
/// `hook_cmd::shell_hook_command` writes for Codex/Reasonix/CodeWhale/Cursor:
/// Unix `PIXTUOID_SOURCE=<source> '<abs>'` (single-quoted) and Windows
/// `<abs> --source <source>` (bare), either with an optional trailing
/// ` --event <name>` (CodeWhale). Returns `Absolute` for a real path, `Unknown`
/// if nothing path-like can be peeled out. Mirrors the read-back
/// `codex::command_basename_is_hook` already does.
pub fn shell_shim_ref(command: &str) -> ShimRef {
    // Strip the trailing ` --event <name>` (CodeWhale bakes one per entry). Use
    // rsplit_once so a single-quoted PATH that literally contains " --event "
    // keeps that occurrence and only the genuinely-appended tail is removed —
    // otherwise the front-split cuts inside the path and `head` no longer ends
    // with the closing `'`, so the quote arm below is skipped and a bogus partial
    // token leaks out (the R0620-WCR-02/03 path-mis-split twin, for ` --event `).
    let head = match command.rsplit_once(" --event ") {
        Some((before, _)) => before,
        None => command,
    };
    // Unix env-prefix form `PIXTUOID_SOURCE=<src> '<path>'`: the path is POSIX
    // single-quoted by `hook_cmd::unix::shell_single_quote`, so `head` ENDS with the
    // closing `'`. A Windows bare path (`<abs> --source <src>`) never ends in a quote
    // even when it CONTAINS apostrophes (`C:\O'Brien\…`) — so the trailing-quote test
    // discriminates the two forms unambiguously: it neither mis-splits a Unix path
    // that contains " --source " NOR mis-quotes a Windows path with 2+ apostrophes.
    // Decode the span by REVERSING the escaping (`'\''` → `'`) rather than splitting
    // on whitespace, which would truncate a spaced path (the R0615-09/#311 twin).
    if head.ends_with('\'') {
        if let Some(start) = head.find('\'') {
            let end = head.len() - 1; // the closing `'` (a 1-byte apostrophe)
            if start < end {
                let p = posix_unquote(&head[start..=end]);
                return if p.is_empty() {
                    ShimRef::Unknown
                } else {
                    ShimRef::Absolute(PathBuf::from(p))
                };
            }
        }
    }
    // Windows bare form: `<abs> --source <source>` (unquoted).
    if let Some((path, _)) = head.split_once(" --source ") {
        let p = path.trim();
        return if p.is_empty() {
            ShimRef::Unknown
        } else {
            ShimRef::Absolute(PathBuf::from(p))
        };
    }
    // Unquoted fallback (legacy / hand-edited): the last whitespace token.
    // `split_whitespace` never yields an empty token, so no emptiness guard.
    match head.split_whitespace().next_back() {
        Some(tok) => ShimRef::Absolute(PathBuf::from(tok)),
        None => ShimRef::Unknown,
    }
}

/// Reverse `hook_cmd::unix::shell_single_quote` over a span that STARTS and ENDS
/// on a `'`: walk it tracking quote state, so a literal `'\''` (close, escaped
/// quote, reopen) decodes to a single `'` and an in-quote space stays literal.
pub(crate) fn posix_unquote(span: &str) -> String {
    let mut out = String::new();
    let mut in_quote = false;
    let mut chars = span.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => in_quote = !in_quote,
            '\\' if !in_quote => {
                if let Some(escaped) = chars.next() {
                    out.push(escaped);
                }
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_shim_ref_unix_env_prefix_form() {
        assert_eq!(
            shell_shim_ref("PIXTUOID_SOURCE=codex '/opt/pixtuoid-hook'"),
            ShimRef::Absolute(PathBuf::from("/opt/pixtuoid-hook"))
        );
    }

    #[test]
    fn shell_shim_ref_unix_quoted_path_containing_source_marker_is_not_missplit() {
        // A Unix hook path that literally contains " --source " must NOT be mis-parsed
        // by the Windows ` --source ` split: the single-quoted arm wins (the quotes
        // unambiguously bound the path). Before the arm-order fix this returned a
        // truncated bogus path that fails the on-disk check — a verify-only false
        // "install broken".
        assert_eq!(
            shell_shim_ref("PIXTUOID_SOURCE=codex '/opt/my --source dir/pixtuoid-hook'"),
            ShimRef::Absolute(PathBuf::from("/opt/my --source dir/pixtuoid-hook"))
        );
    }

    #[test]
    fn shell_shim_ref_windows_bare_path_with_apostrophes_is_not_mis_quoted() {
        // A Windows bare path may CONTAIN apostrophes (`C:\Bob's O'Brien\…`) but never
        // ENDS in one, so the trailing-quote discriminator routes it to the ` --source `
        // split, NOT the Unix single-quote arm. (The arm-order reorder regressed this;
        // the ends-with-quote test fixes both this and the Unix " --source " case.)
        assert_eq!(
            shell_shim_ref(r"C:\Bob's O'Brien\pixtuoid-hook.exe --source reasonix"),
            ShimRef::Absolute(PathBuf::from(r"C:\Bob's O'Brien\pixtuoid-hook.exe"))
        );
        // …and with CodeWhale's trailing ` --event` tail.
        assert_eq!(
            shell_shim_ref(r"C:\O'Brien\hook.exe --source codewhale --event session_start"),
            ShimRef::Absolute(PathBuf::from(r"C:\O'Brien\hook.exe"))
        );
    }

    #[test]
    fn shell_shim_ref_lone_trailing_quote_is_not_a_quoted_span() {
        // A command ending in a SINGLE `'` with no opening quote (`start == end`) is
        // NOT a valid single-quoted span — it must fall through to the unquoted
        // fallback (treated as a literal token), not decode an empty span to Unknown.
        // Pins the `start < end` boundary.
        assert_eq!(
            shell_shim_ref("/opt/hook'"),
            ShimRef::Absolute(PathBuf::from("/opt/hook'"))
        );
    }

    #[test]
    fn shell_shim_ref_unix_recovers_spaced_single_quoted_path() {
        // The writer single-quotes the path (`shell_single_quote`) so a home dir
        // with a space round-trips through the shell. A whitespace-split reader
        // would truncate `'/Users/Jane Doe/…'` to `Doe/…'` → a bogus relative
        // path that never `.exists()` → a FALSE "shim binary missing" on every
        // boot preflight / `doctor` / Sources panel. The R0615-09 (#311)
        // doctor::field truncation twin.
        assert_eq!(
            shell_shim_ref("PIXTUOID_SOURCE=codex '/Users/Jane Doe/bin/pixtuoid-hook'"),
            ShimRef::Absolute(PathBuf::from("/Users/Jane Doe/bin/pixtuoid-hook"))
        );
    }

    #[test]
    fn shell_shim_ref_unix_recovers_spaced_path_with_event_tail() {
        // CodeWhale's ` --event` tail is stripped first, then the spaced path
        // recovers — the two fixes compose.
        assert_eq!(
            shell_shim_ref(
                "PIXTUOID_SOURCE=codewhale '/Users/Jane Doe/hook' --event session_start"
            ),
            ShimRef::Absolute(PathBuf::from("/Users/Jane Doe/hook"))
        );
    }

    #[test]
    fn shell_shim_ref_unix_recovers_path_with_embedded_single_quote() {
        // `shell_single_quote("/U/O'B/hook")` == `'/U/O'\''B/hook'`; the reader
        // reverses the POSIX `'\''` escaping, not just spaces.
        assert_eq!(
            shell_shim_ref(r#"PIXTUOID_SOURCE=codex '/U/O'\''B/hook'"#),
            ShimRef::Absolute(PathBuf::from("/U/O'B/hook"))
        );
    }

    #[test]
    fn shell_shim_ref_windows_bare_form() {
        assert_eq!(
            shell_shim_ref(r"C:\bin\pixtuoid-hook.exe --source reasonix"),
            ShimRef::Absolute(PathBuf::from(r"C:\bin\pixtuoid-hook.exe"))
        );
    }

    #[test]
    fn shell_shim_ref_strips_codewhale_event_tail() {
        // CodeWhale bakes ` --event <name>` onto each entry — both platforms.
        assert_eq!(
            shell_shim_ref("PIXTUOID_SOURCE=codewhale '/opt/pixtuoid-hook' --event session_start"),
            ShimRef::Absolute(PathBuf::from("/opt/pixtuoid-hook"))
        );
        assert_eq!(
            shell_shim_ref(r"C:\bin\pixtuoid-hook.exe --source codewhale --event session_start"),
            ShimRef::Absolute(PathBuf::from(r"C:\bin\pixtuoid-hook.exe"))
        );
    }

    #[test]
    fn shell_shim_ref_path_containing_event_marker_is_not_missplit() {
        // A single-quoted Unix path that literally contains ` --event ` must keep
        // it: rsplit_once strips only the genuinely-appended tail, so `head` still
        // ends with the closing `'` and posix_unquote recovers the real path. With
        // a front-anchored split_once the path would be cut mid-way → a bogus
        // partial token → a false "shim missing". Compare PathBuf STRUCTURALLY —
        // never assert on a `/`-string (the Windows path-sep class).
        assert_eq!(
            shell_shim_ref(
                "PIXTUOID_SOURCE=codewhale '/Users/x/my --event dir/pixtuoid-hook' --event tool"
            ),
            ShimRef::Absolute(PathBuf::from("/Users/x/my --event dir/pixtuoid-hook"))
        );
    }

    #[test]
    fn shell_shim_ref_empty_is_unknown() {
        assert_eq!(shell_shim_ref(""), ShimRef::Unknown);
    }

    #[test]
    fn shell_shim_ref_windows_empty_path_is_unknown() {
        // Leading space: the substring before ` --source ` trims to empty → the
        // Windows arm must return Unknown, not Absolute("").
        assert_eq!(shell_shim_ref(" --source reasonix"), ShimRef::Unknown);
    }

    #[test]
    fn shell_shim_ref_unix_empty_quoted_token_is_unknown() {
        // The last whitespace token is `''`, which trims (of single-quotes) to
        // empty → Unknown, not Absolute("").
        assert_eq!(shell_shim_ref("PIXTUOID_SOURCE=codex ''"), ShimRef::Unknown);
    }

    #[test]
    fn flat_json_merge_uninstall_returns_non_object_unchanged() {
        // The defensive `else { return doc }` arm: a non-object document root is
        // passed through untouched (the real callers only feed objects).
        let arr = json!([1, 2, 3]);
        assert_eq!(flat_json_merge_uninstall(arr.clone(), "_pixtuoid"), arr);
        let scalar = json!(42);
        assert_eq!(
            flat_json_merge_uninstall(scalar.clone(), "_pixtuoid"),
            scalar
        );
    }

    #[test]
    fn schema_parse_broken_sets_issue_and_unknown_shim() {
        let p = SchemaParse::broken("boom");
        assert_eq!(p.issues, vec!["boom".to_string()]);
        assert_eq!(p.shim, ShimRef::Unknown);
    }

    #[test]
    fn flat_json_verify_reports_broken_on_invalid_json() {
        let p = flat_json_verify("{not json", &["PreToolUse"], "_pixtuoid");
        assert_eq!(p.shim, ShimRef::Unknown);
        assert!(
            p.issues
                .iter()
                .any(|i| i.contains("no longer parses as JSON")),
            "{:?}",
            p.issues
        );
    }

    #[test]
    fn flat_json_verify_flags_event_missing_a_managed_entry() {
        // A config with a managed entry for PreToolUse only, queried for two
        // events → PostToolUse is reported missing, and the present entry's
        // command yields an Absolute shim.
        let content = json!({
            "hooks": {
                "PreToolUse": [{
                    "_pixtuoid": true,
                    "command": "PIXTUOID_SOURCE=reasonix '/opt/pixtuoid-hook'"
                }]
            }
        })
        .to_string();
        let p = flat_json_verify(&content, &["PreToolUse", "PostToolUse"], "_pixtuoid");
        assert!(
            p.issues
                .iter()
                .any(|i| i.contains("missing hook entries for") && i.contains("PostToolUse")),
            "{:?}",
            p.issues
        );
        assert_eq!(
            p.shim,
            ShimRef::Absolute(PathBuf::from("/opt/pixtuoid-hook"))
        );
    }

    #[test]
    fn display_safe_strips_control_chars_from_a_hostile_path() {
        // A shim path crafted (via a hand-edited hook command) with an ANSI/OSC
        // escape must not reach a real terminal raw.
        let hostile = std::path::Path::new("/x/\x1b]0;pwned\x07\x1b[31mhook");
        let got = display_safe(hostile);
        assert!(!got.chars().any(|c| c.is_control()), "{got:?}");
        assert!(got.contains("hook") && got.contains("/x/"), "{got:?}");
    }

    #[test]
    fn hook_path_str_returns_utf8_path() {
        let p = std::path::Path::new("/opt/bin/pixtuoid-hook");
        assert_eq!(hook_path_str(p).unwrap(), "/opt/bin/pixtuoid-hook");
    }

    #[test]
    fn hook_path_str_rejects_non_utf8() {
        // A non-UTF-8 OsStr must be an Err, not a lossy-decoded dead hook.
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

    #[test]
    fn schema_verify_soundness_ignores_notes() {
        let clean = SchemaVerifyResult::default();
        assert!(clean.is_sound());
        let soft = SchemaVerifyResult {
            issues: vec![],
            notes: vec!["pixtuoid-hook not on PATH".into()],
        };
        assert!(
            soft.is_sound(),
            "soft notes must not make an install 'broken'"
        );
        let hard = SchemaVerifyResult {
            issues: vec!["shim missing".into()],
            notes: vec![],
        };
        assert!(!hard.is_sound());
    }
}
