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
    p.display()
        .to_string()
        .chars()
        .filter(|c| !c.is_control())
        .collect()
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
/// ` --event <name>` (CodeWhale). Source-agnostic (splits on the literal
/// ` --source `/` --event ` markers). Returns `Absolute` for a real path,
/// `Unknown` if nothing path-like can be peeled out. Mirrors the read-back
/// `codex::command_basename_is_hook` already does.
pub fn shell_shim_ref(command: &str) -> ShimRef {
    // Strip a trailing ` --event <name>` (CodeWhale bakes one per entry).
    let head = match command.split_once(" --event ") {
        Some((before, _)) => before,
        None => command,
    };
    // Windows bare form: `<abs> --source <source>`.
    if let Some((path, _)) = head.split_once(" --source ") {
        let p = path.trim();
        return if p.is_empty() {
            ShimRef::Unknown
        } else {
            ShimRef::Absolute(PathBuf::from(p))
        };
    }
    // Unix env-prefix form: the path is the LAST whitespace token, single-quoted.
    match head.split_whitespace().next_back() {
        Some(tok) => {
            let p = tok.trim_matches('\'');
            if p.is_empty() {
                ShimRef::Unknown
            } else {
                ShimRef::Absolute(PathBuf::from(p))
            }
        }
        None => ShimRef::Unknown,
    }
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
    fn shell_shim_ref_empty_is_unknown() {
        assert_eq!(shell_shim_ref(""), ShimRef::Unknown);
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
