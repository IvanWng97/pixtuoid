pub(crate) mod claude;
pub(crate) mod codewhale;
pub(crate) mod codex;
pub(crate) mod cursor;
pub(crate) mod grok;
pub(crate) mod hermes;
mod hook_cmd;
pub(crate) mod kimi;
// `io` holds the config-write authority (invariant #4), which must never be
// cross-crate reachable — only its env filters (below) are re-exported.
pub(crate) mod io;
pub use io::{nonempty, nonempty_abs_env, nonempty_env, owner_only_create, tighten_to_owner_only};
pub(crate) mod merge;
pub(crate) mod omp;
pub(crate) mod openclaw;
pub(crate) mod opencode;
pub(crate) mod reasonix;
pub(crate) mod target;
pub use target::TARGETS;
pub(crate) mod verify;

use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use target::{BinaryStrategy, Target, BACKUP_SUFFIX};

/// The idempotency sentinel stamped on every hook entry pixtuoid installs — the
/// config-file targets key install/uninstall/detect on this, not the command shape.
pub(crate) const SENTINEL_KEY: &str = "_pixtuoid";

/// Whether `t`'s config currently bears pixtuoid hooks — the load-bearing gate for
/// `verify_target`, which would report an uninstalled config as "broken". Errs toward
/// INCLUDED wherever it cannot tell: an UNREADABLE config counts as installed, because
/// we cannot look and so must not claim otherwise, and one that reads but cannot be
/// PARSED falls back to the marker probe (`config_mentions_us`).
pub(crate) fn has_hooks(t: &'static Target, config: Option<PathBuf>) -> bool {
    // The gate (this) and the verify it guards MUST read the SAME config, or
    // `diagnose` through an injected root could never observe an install.
    let path = match config.map(Ok).unwrap_or_else(|| (t.default_config_path)()) {
        Ok(p) => p,
        Err(_) => return false,
    };
    match io::read_config(&path) {
        Ok(c) if c.trim().is_empty() => false,
        // A merge that ERRS means "we could not tell" — never "installed".
        Ok(c) => (t.merge_uninstall)(&c)
            .map(|o| o.changed)
            .unwrap_or_else(|_| config_mentions_us(&c)),
        Err(_) => true,
    }
}

const PLUGIN_MENTION: &str = "pixtuoid";

/// The fallback when a config cannot be PARSED for a real answer — without it, a
/// never-connected OpenClaw user with a comment in their legal-JSON5 config was reported
/// hooks-INSTALLED and then verified BROKEN. Lowercased for `kimi`, the one target with
/// no `_pixtuoid` sentinel (UPPERCASE `PIXTUOID_SOURCE=kimi`); an OpenClaw plugin id is
/// case-SENSITIVE upstream (`openclaw::is_plugin_id`) — don't harmonize the two.
fn config_mentions_us(content: &str) -> bool {
    content.to_ascii_lowercase().contains(PLUGIN_MENTION)
}

/// Verify a target's installed config is structurally SOUND (the silent-dead check) —
/// read-only, false-positive-free. Call only when hooks are claimed installed
/// (`has_hooks(t, config)`, same `config`). Returns the per-source `verify_schema`
/// verdict PLUS the shim-on-disk checks this (the only I/O) layer adds. `config`
/// overrides the default path; `None` = the target's default.
pub(crate) fn verify_target(
    t: &'static Target,
    config: Option<PathBuf>,
) -> verify::SchemaVerifyResult {
    use verify::ShimRef;
    let path = match config.map(Ok).unwrap_or_else(|| (t.default_config_path)()) {
        Ok(p) => p,
        Err(_) => {
            return verify::SchemaVerifyResult {
                issues: vec!["no config path resolves (no home dir)".into()],
                notes: vec![],
            }
        }
    };
    let content = match io::read_config(&path) {
        Ok(c) if c.trim().is_empty() => {
            return verify::SchemaVerifyResult {
                issues: vec!["config is empty — hooks are not installed".into()],
                notes: vec![],
            }
        }
        Ok(c) => c,
        Err(_) => {
            return verify::SchemaVerifyResult {
                issues: vec![format!(
                    "config unreadable: {}",
                    verify::display_safe(&path)
                )],
                notes: vec![],
            }
        }
    };
    let parse = (t.verify_schema)(&content);
    let mut issues = parse.issues;
    let mut notes = parse.notes;
    match parse.shim {
        ShimRef::Absolute(p) => check_shim_binary(&p, &mut issues),
        ShimRef::BareName => {
            // A doctor-process PATH miss is NOT proof the CLI can't resolve the bare
            // `pixtuoid-hook` → soft note only.
            if !io::hook_on_path() {
                notes.push(
                    "pixtuoid-hook not on this process's PATH (the CLI's PATH may differ)".into(),
                );
            }
        }
        ShimRef::Unknown => {
            // SOFT, not hard: a novel-but-valid command shape lands here, and the
            // genuine no-hooks case is already a HARD issue from `verify_schema`.
            notes.push("could not read the shim path from the managed hook command".into());
        }
    }
    verify_extra_artifacts(t, &mut issues, &mut notes);
    verify::SchemaVerifyResult { issues, notes }
}

/// The silent-dead class `verify_schema` is blind to — the config merge reads clean while
/// the plugin FILES the gateway loads (the OpenClaw plugin DIR) are missing. INVARIANT
/// (#387): a NEW code-shipping path in `install_target` MUST gain a check here, UNLESS the
/// artifact IS that target's own config — with no `extra_artifacts` to ride this loop it
/// belongs in that target's `verify_schema` instead (the opencode plugin and the omp
/// bridge extension are that shape).
fn verify_extra_artifacts(t: &Target, issues: &mut Vec<String>, notes: &mut Vec<String>) {
    let Some(make) = t.extra_artifacts else {
        return;
    };
    // A placeholder hook arg yields the real install locations WITHOUT resolving the
    // binary: a read-only check must not hard-error just because it isn't locatable.
    let arts = match make(std::path::Path::new("pixtuoid-hook")) {
        Ok(arts) => arts,
        Err(e) => {
            notes.push(format!("could not resolve plugin artifact paths: {e}"));
            return;
        }
    };
    let mut missing: Vec<PathBuf> = Vec::new();
    // Pinned by
    // `verify_target_hard_flags_a_missing_code_artifact_for_every_extra_artifacts_target`.
    for (p, intended) in arts {
        if !p.exists() {
            missing.push(p);
            continue;
        }
        let Ok(installed) = io::read_config(&p) else {
            notes.push(format!("could not read {}", verify::display_safe(&p)));
            continue;
        };
        check_artifact_content(&p, &installed, &intended, issues, notes);
    }
    issues.extend(missing_artifact_issue(&missing));
}

/// Compare one installed artifact against what this binary would render now, then stat the
/// shim path it bakes. A config-shaped target names the EVENTS an old install is missing;
/// a code artifact has no per-event config, so the equivalent is the whole file — nothing
/// re-installs on a pixtuoid upgrade, so without this an upgrader runs the plugin they
/// connected with forever and doctor says fine.
fn check_artifact_content(
    p: &std::path::Path,
    installed: &str,
    intended: &str,
    issues: &mut Vec<String>,
    notes: &mut Vec<String>,
) {
    // EVERY artifact, not just the one baking a shim path: nested under the marker guard
    // below, OpenClaw's manifest and its `package.json` got existence-only checks.
    if strip_baked_line(installed) != strip_baked_line(intended) {
        issues.push(format!(
            "{} differs from the plugin this pixtuoid ships — it \
             predates an upgrade, so anything added since is not \
             forwarded. Reconnect the source to refresh it.",
            verify::display_safe(p)
        ));
    }
    // Existence misses a shim that MOVED — a green doctor over a plugin whose every
    // forward fails — so stat the baked path too.
    if !intended.contains(verify::BAKED_HOOK_MARKER) {
        return;
    }
    match verify::baked_hook_path(installed) {
        Some(baked) => check_shim_binary(&baked, issues),
        None => notes.push(format!(
            "could not read the baked shim path from {}",
            verify::display_safe(p)
        )),
    }
}

/// A rendered artifact without its `const HOOK_PATH` line — the only line that legitimately
/// differs between what is installed (a real shim path) and what this binary would render
/// (a placeholder). Anchored with `starts_with`, exactly as `verify::baked_hook_path`
/// anchors: `contains` also stripped opencode's comment ABOVE the binding, so a change
/// confined to that comment was invisible to the staleness compare.
fn strip_baked_line(content: &str) -> String {
    content
        .lines()
        .filter(|l| !l.trim_start().starts_with(verify::BAKED_HOOK_MARKER))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The HARD issue(s) for missing code artifacts, collapsed when they share a
/// directory — the whole plugin dir being gone is ONE fact, and listing every
/// absolute path overran the Sources panel's detail line into a marquee crawl.
fn missing_artifact_issue(missing: &[PathBuf]) -> Vec<String> {
    let [first, rest @ ..] = missing else {
        return Vec::new();
    };
    let dir = first.parent();
    if !rest.is_empty() && dir.is_some() && rest.iter().all(|p| p.parent() == dir) {
        let names: Vec<String> = missing
            .iter()
            .filter_map(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .collect();
        return vec![format!(
            "{} plugin artifacts missing from {}: {}",
            missing.len(),
            verify::display_safe(dir.unwrap_or(first)),
            crate::strip_control_chars(&names.join(", "))
        )];
    }
    missing
        .iter()
        .map(|p| format!("plugin artifact missing: {}", verify::display_safe(p)))
        .collect()
}

/// Stat one resolved shim path — the ONE check shared by an embedded hook command
/// and a code artifact's baked `HOOK_PATH`, so the two can't report a moved binary
/// differently. `display_safe` because the path comes from a hand-editable hook
/// command and these issues reach a real terminal.
fn check_shim_binary(p: &std::path::Path, issues: &mut Vec<String>) {
    let shown = verify::display_safe(p);
    if !p.exists() {
        issues.push(format!("shim binary missing: {shown}"));
    } else if !is_executable(p) {
        issues.push(format!("shim binary not executable: {shown}"));
    }
}

#[cfg(unix)]
fn is_executable(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &std::path::Path) -> bool {
    // Windows has no executable bit; the caller already confirmed existence.
    p.exists()
}

/// A Windows drive-relative path (`C:foo.exe` — a drive prefix but no root).
/// `is_relative()` is true for it, yet `cwd.join` replaces NOTHING (std: a
/// path with a prefix replaces self in its entirety), so the absolutization
/// arm would silently no-op and embed a command that resolves against the
/// hook-spawner's per-drive cwd.
fn is_drive_relative(p: &std::path::Path) -> bool {
    !p.has_root() && matches!(p.components().next(), Some(std::path::Component::Prefix(_)))
}

/// An explicit path always wins — `--hook-path` first, then the `PIXTUOID_HOOK` env
/// override — and the returned bool reports that, so `install_target` EMBEDS it rather
/// than discarding the user's choice for a bare PATH-resolved name. A `locate` failure is
/// fatal only for targets that EMBED the path (`BinaryStrategy::EmbedAbsolute`); the
/// bare-name/PATH ones fall back to it so a fresh-machine install still succeeds.
fn resolve_hook_binary_from(
    t: &Target,
    hook_path: Option<PathBuf>,
    env_hook: Option<PathBuf>,
    locate: impl FnOnce() -> Result<PathBuf>,
) -> Result<(PathBuf, bool)> {
    // Both are EXPLICIT paths that get EMBEDDED into the config, where a relative path
    // would resolve against the CLI's cwd at hook time and hooks would never fire.
    let explicit = hook_path
        .map(|p| (p, "--hook-path"))
        .or(env_hook.map(|p| (p, io::HOOK_OVERRIDE_ENV)));
    if let Some((p, origin)) = explicit {
        if is_drive_relative(&p) {
            bail!(
                "{origin} {} is drive-relative (a drive prefix with no root, like C:foo.exe) \
                 and would resolve against a per-drive cwd at hook time; pass an absolute path",
                p.display()
            );
        }
        // Plain join, not canonicalize — Windows canonicalize yields a \\?\ verbatim
        // path that the cmd.exe bare form can't take.
        let p = if p.is_relative() {
            // A failed cwd query must NOT fall back to embedding the relative path —
            // that re-creates the never-fires bug absolutization exists to prevent.
            let cwd = std::env::current_dir().with_context(|| {
                format!("{origin} is relative and the current directory is unreadable; pass an absolute path")
            })?;
            cwd.join(&p)
        } else {
            p
        };
        if !p.exists() {
            // tracing, not println!: install runs under the TUI alt-screen, where a stdout
            // write corrupts the frame. Stripped — `connect`/`setup` route to RAW stderr.
            tracing::warn!(
                "{origin} {} does not exist yet; the hook will fail until it does",
                crate::strip_control_chars(&p.display().to_string())
            );
        }
        return Ok((p, true));
    }
    match locate() {
        Ok(p) => Ok((p, false)),
        Err(e) if t.binary_strategy == BinaryStrategy::EmbedAbsolute => Err(e),
        Err(_) => Ok((PathBuf::from("pixtuoid-hook"), false)),
    }
}

#[derive(Debug)]
pub enum InstallOutcome {
    Installed,
    AlreadyUpToDate,
}

#[derive(Debug)]
pub struct InstallReport {
    pub outcome: InstallOutcome,
    pub config_path: PathBuf,
    /// The backup taken this round (`None` on a no-op, or when one already exists).
    pub backup: Option<PathBuf>,
    /// True when the bare `pixtuoid-hook` isn't on PATH (Claude/Unix, no explicit
    /// hook).
    pub path_warning: bool,
    /// The target's `post_install_hint` — a step the user must still take for the
    /// install to take effect (OpenClaw's running gateway must restart).
    pub post_install_hint: Option<&'static str>,
}

/// Install pixtuoid hooks into `t`'s config, returning a structured report. The ConfigLock
/// round (read→merge→backup→write) is the load-bearing write authority (invariant #4) and
/// stays intact here; it serializes pixtuoid only against pixtuoid, since the agent CLI
/// itself cannot honor this lock. Reads and backups go through the guard's PINNED
/// resolution — re-resolving `path` splits the round across two files on a symlink retarget.
pub(crate) fn install_target(
    t: &Target,
    config: Option<PathBuf>,
    hook_path: Option<PathBuf>,
) -> Result<InstallReport> {
    let path = config
        .map(Ok)
        .unwrap_or_else(|| (t.default_config_path)())?;
    let env_hook = io::nonempty_env(io::HOOK_OVERRIDE_ENV);
    let (binary, explicit_hook) =
        resolve_hook_binary_from(t, hook_path, env_hook, io::default_hook_binary)?;
    let hook_cmd = (t.hook_command)(&binary, explicit_hook)?;
    // Lost-update TOCTOU: two concurrent pixtuoid runs would otherwise interleave
    // read(A)→write(B)→write(A), and A's rename clobbers B's change.
    let lock = io::lock_config(&path)?;
    let content = lock.read()?;
    // Merge FIRST so a present-but-malformed config bails BEFORE we touch the filesystem —
    // else the extra artifacts land on disk as orphans registered nowhere (partial install).
    let outcome = (t.merge_install)(&content, &hook_cmd)
        .with_context(|| format!("processing {}", path.display()))?;
    write_extra_artifacts(t, &binary)?;
    // Independent of whether the content changed — a no-op re-install on a box without
    // pixtuoid-hook on PATH would otherwise warn nothing. Skipped for an embedded path.
    let path_warning = t.binary_strategy == BinaryStrategy::BareNameOnPath
        && !explicit_hook
        && !io::hook_on_path();
    if !outcome.changed {
        return Ok(InstallReport {
            outcome: InstallOutcome::AlreadyUpToDate,
            config_path: path,
            backup: None,
            path_warning,
            post_install_hint: t.post_install_hint,
        });
    }
    let backup = lock.backup_once(BACKUP_SUFFIX)?;
    lock.write_atomic(&outcome.content)?;
    Ok(InstallReport {
        outcome: InstallOutcome::Installed,
        config_path: path,
        backup,
        path_warning,
        post_install_hint: t.post_install_hint,
    })
}

/// Render the wholly-owned artifacts a target ships beside its config. Called before the
/// config WRITE so a re-install refreshes them even when the merge is a no-op, which heals
/// a deleted plugin file. Each artifact is its own lock target, disjoint from the config
/// lock the caller holds and taken in a consistent config→artifact order, so no
/// self-deadlock.
fn write_extra_artifacts(t: &Target, binary: &std::path::Path) -> Result<()> {
    let Some(make) = t.extra_artifacts else {
        return Ok(());
    };
    for (p, c) in make(binary)? {
        // A real write here would be invisible: CI has no such dir, and the shim exits
        // 0 by invariant #5, so nothing downstream would report it.
        #[cfg(test)]
        assert!(
            p.starts_with(std::env::temp_dir()),
            "a test is about to write {} outside the temp dir — redirect the target's \
             own state resolver (OpenClaw: OPENCLAW_STATE_DIR) at a TempDir first",
            p.display()
        );
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("creating plugin dir {}", dir.display()))?;
        }
        // Atomic (temp-in-dir → fsync → rename), NOT a plain `fs::write`: a torn write
        // leaves a half-rendered plugin the gateway then fails to load.
        io::write_config_atomic(&p, &c).with_context(|| format!("writing {}", p.display()))?;
    }
    Ok(())
}

#[derive(Debug)]
pub enum UninstallOutcome {
    Removed,
    NothingToRemove,
}

#[derive(Debug)]
pub struct UninstallReport {
    pub outcome: UninstallOutcome,
    pub config_path: PathBuf,
    /// The backup deleted on a successful removal (no longer needed once the hooks
    /// are gone).
    pub removed_backup: Option<PathBuf>,
}

/// Remove pixtuoid hooks from `t`'s config, returning a structured report. Same
/// lock scope as `install_target`, plus the load-bearing "never rewrite or delete
/// the backup on a semantic no-op" rule.
pub(crate) fn uninstall_target(t: &Target, config: Option<PathBuf>) -> Result<UninstallReport> {
    let path = config
        .map(Ok)
        .unwrap_or_else(|| (t.default_config_path)())?;
    // Decided BEFORE locking: `lock_config` creates the parent dir + a .lock sidecar, and
    // materializing ~/.reasonix here would flip that target's presence probe on a no-op.
    if !target::config_present(&path) {
        return Ok(UninstallReport {
            outcome: UninstallOutcome::NothingToRemove,
            config_path: path,
            removed_backup: None,
        });
    }
    let lock = io::lock_config(&path)?;
    let content = lock.read()?;
    let outcome =
        (t.merge_uninstall)(&content).with_context(|| format!("processing {}", path.display()))?;
    if !outcome.changed {
        // SEMANTIC no-op — never rewrite the file or delete the backup here: the backup is
        // the user's only recovery, and a byte compare would falsely fire on hand formatting.
        return Ok(UninstallReport {
            outcome: UninstallOutcome::NothingToRemove,
            config_path: path,
            removed_backup: None,
        });
    }
    lock.write_atomic(&outcome.content)?;
    let removed_backup = lock.remove_backup(BACKUP_SUFFIX)?;
    Ok(UninstallReport {
        outcome: UninstallOutcome::Removed,
        config_path: path,
        removed_backup,
    })
}

/// Deleting a registered event ships GREEN — cargo-mutants does not mutate slice
/// initializers and nothing else asserts the SET, which is how both of #929's headline
/// registration fixes could be silently removed. Update a pin deliberately when its
/// roster changes.
#[cfg(test)]
pub(crate) fn assert_event_roster<T: Ord + std::fmt::Debug + Copy>(
    name: &str,
    actual: &[T],
    expected: &[T],
) {
    use std::collections::BTreeSet;
    assert_eq!(
        actual.iter().copied().collect::<BTreeSet<_>>(),
        expected.iter().copied().collect::<BTreeSet<_>>(),
        "{name} membership changed — a registered event that vanishes is a \
         shipping bug no other test can see."
    );
}

#[cfg(test)]
mod tests;
