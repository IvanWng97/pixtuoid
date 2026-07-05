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
//! (invariant #3) — this module holds only the READ-side machinery: the shared
//! result types, the shell shim-path extractor (4 targets share
//! `shell_hook_command`), and the filesystem layer (`verify_target` in `mod.rs`
//! stats the shim). The install-WRITE shared helpers (config parse, the
//! sentinel-keyed hook merge) live in the sibling [`crate::install::merge`] —
//! this module no longer hosts any mutation.

use std::path::PathBuf;

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
    // AND only honor the strip when the residual head still parses as a writer
    // shape (ends with `'` = the Unix quoted form; contains " --source " = the
    // Windows bare form): a NO-tail command (Codex/Reasonix/Cursor never append
    // one) whose quoted path contains " --event " would otherwise be cut at the
    // in-quotes occurrence and mis-parse to a bogus partial path — the tail
    // strip is only ever needed for CodeWhale entries, whose head always
    // matches one of the two shapes.
    let head = match command.rsplit_once(crate::install::hook_cmd::EVENT_FLAG) {
        Some((before, _))
            if before.ends_with('\'') || before.contains(crate::install::hook_cmd::SOURCE_FLAG) =>
        {
            before
        }
        _ => command,
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
    if let Some((path, _)) = head.split_once(crate::install::hook_cmd::SOURCE_FLAG) {
        let p = path.trim();
        return if p.is_empty() {
            ShimRef::Unknown
        } else {
            ShimRef::Absolute(PathBuf::from(p))
        };
    }
    // Unquoted fallback (hand-edited configs; no released version ever wrote this form): the last whitespace token.
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

/// If `s` is a whole POSIX single-quoted span (`'…'`, length ≥ 2), reverse the
/// quoting via [`posix_unquote`]; otherwise return it verbatim. THE one
/// "whole-span quoted → unquote else literal" decision — the exec-form decoders
/// (`claude`, `hermes`) route through it so the `len() >= 2` guard (a lone `'` is
/// NOT a quoted span, and slicing it would strip to nothing) can't drift between
/// them. A half-quoted `'/opt/x` (no close) is a literal, not unquoted.
pub(crate) fn posix_unquote_if_quoted(s: &str) -> String {
    if s.len() >= 2 && s.starts_with('\'') && s.ends_with('\'') {
        posix_unquote(s)
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn posix_unquote_if_quoted_guards_len_and_pairing() {
        // A whole single-quoted span is unquoted, incl. an embedded `'\''`.
        assert_eq!(posix_unquote_if_quoted("'/opt/hook'"), "/opt/hook");
        assert_eq!(posix_unquote_if_quoted(r"'/U/O'\''B/hook'"), "/U/O'B/hook");
        // Half-quoted (open, no close) → literal, not unquoted.
        assert_eq!(posix_unquote_if_quoted("'/opt/hook"), "'/opt/hook");
        // A LONE `'` (len 1) is NOT a quoted span — the `len() >= 2` guard keeps
        // it literal instead of slicing to "" (the claude/hermes guard drift this
        // consolidation fixes: claude previously lacked the length check).
        assert_eq!(posix_unquote_if_quoted("'"), "'");
        // Empty + plain unquoted pass through verbatim.
        assert_eq!(posix_unquote_if_quoted(""), "");
        assert_eq!(posix_unquote_if_quoted("/plain/path"), "/plain/path");
    }

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
    fn shell_shim_ref_tailless_path_containing_event_marker_is_not_missplit() {
        // The tail-less twin of the test above: a Codex/Reasonix/Cursor command
        // (which NEVER appends ` --event `) whose quoted path literally contains
        // " --event " must not have the in-quotes occurrence stripped — the
        // residual head (`PIXTUOID_SOURCE=codex '/opt/my`) is no writer shape,
        // so the strip is only honored when the head still parses as one
        // (ends with `'` on Unix / contains " --source " on Windows).
        assert_eq!(
            shell_shim_ref("PIXTUOID_SOURCE=codex '/opt/my --event dir/pixtuoid-hook'"),
            ShimRef::Absolute(PathBuf::from("/opt/my --event dir/pixtuoid-hook"))
        );
        // …and the Windows bare form of the same case.
        assert_eq!(
            shell_shim_ref(r"C:\my --event dir\pixtuoid-hook.exe --source cursor"),
            ShimRef::Absolute(PathBuf::from(r"C:\my --event dir\pixtuoid-hook.exe"))
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
