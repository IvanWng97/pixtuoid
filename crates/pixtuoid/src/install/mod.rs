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

/// Whether `t`'s config currently bears pixtuoid hooks — the load-bearing gate
/// for `verify_target` (an uninstalled config would verify "broken"). An
/// absent/empty config is excluded. A config present but UNREADABLE is INCLUDED
/// (true) — we cannot look, so we must not claim it is uninstalled. A config that
/// reads but cannot be PARSED falls back to the marker probe
/// (`config_mentions_us`).
pub(crate) fn has_hooks(t: &'static Target, config: Option<PathBuf>) -> bool {
    // The gate (this) and the verify it guards MUST read the SAME config, or
    // `diagnose` through an injected root could never observe an install.
    let path = match config.map(Ok).unwrap_or_else(|| (t.default_config_path)()) {
        Ok(p) => p,
        Err(_) => return false,
    };
    match io::read_config(&path) {
        Ok(c) if c.trim().is_empty() => false,
        // A merge that ERRS means "we could not tell", so ask the cheaper question:
        // does this document mention US at all? Asserting "installed" about a config
        // we could not parse produced advice that cannot succeed — OpenClaw's config
        // is legal JSON5, so a never-connected user with a comment in theirs was
        // reported hooks-INSTALLED and then verified BROKEN.
        Ok(c) => (t.merge_uninstall)(&c)
            .map(|o| o.changed)
            .unwrap_or_else(|_| config_mentions_us(&c)),
        Err(_) => true,
    }
}

const PLUGIN_MENTION: &str = "pixtuoid";

/// Does `content` bear OUR marker? — the fallback when a config cannot be parsed
/// for a real answer. Folded to lowercase because `kimi`, the one target with no
/// `_pixtuoid` sentinel, marks itself with the UPPERCASE `PIXTUOID_SOURCE=kimi`.
/// This is NOT the same question as matching an OpenClaw plugin id, which is
/// case-SENSITIVE upstream (see `openclaw::is_plugin_id`); don't harmonize the two.
fn config_mentions_us(content: &str) -> bool {
    content.to_ascii_lowercase().contains(PLUGIN_MENTION)
}

/// Verify a target's installed config is structurally SOUND (the silent-dead
/// check) — read-only, false-positive-free. Call only when hooks are claimed
/// installed (`has_hooks(t, config)`, same `config`). Returns the per-source
/// `verify_schema` verdict PLUS the shim-on-disk check this (the only I/O) layer
/// adds: an embedded absolute path is stat'd for exists+executable (HARD); a
/// Claude/Unix bare name is a soft PATH note. `config` overrides the default path;
/// `None` = the target's default.
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
            // SOFT, not hard: we can't CONFIRM the shim exists, but we can't prove
            // it's broken either (a future source with a novel-but-valid command
            // shape lands here). The genuine no-hooks case is already a HARD issue
            // from `verify_schema`, so this never masks a real break.
            notes.push("could not read the shim path from the managed hook command".into());
        }
    }
    // Wholly-owned extra artifacts (the OpenClaw plugin DIR): the config merge can
    // verify clean while the plugin FILES the gateway actually loads are missing —
    // the silent-dead class `verify_schema` is blind to. A placeholder hook arg
    // yields the real install locations WITHOUT resolving the binary, because a
    // read-only doctor check must not hard-error just because pixtuoid-hook isn't
    // locatable.
    //
    // INVARIANT (#387): a NEW code-shipping path added to `install_target` MUST gain
    // a matching check here, or it ships the silent-dead class for a 3rd
    // code-artifact target. Where the artifact IS the target's own config file it
    // has no `extra_artifacts` to ride this loop, so the check belongs in that
    // target's `verify_schema` instead — opencode's plugin is the one such shape.
    if let Some(make) = t.extra_artifacts {
        match make(std::path::Path::new("pixtuoid-hook")) {
            Ok(arts) => {
                let mut missing: Vec<PathBuf> = Vec::new();
                for (p, intended) in arts {
                    if !p.exists() {
                        missing.push(p);
                        continue;
                    }
                    // Existence misses a shim that MOVED — a green doctor over a
                    // plugin whose every forward fails — so stat the baked path too.
                    if !intended.contains(verify::BAKED_HOOK_MARKER) {
                        continue;
                    }
                    let installed = io::read_config(&p).unwrap_or_default();
                    match verify::baked_hook_path(&installed) {
                        Some(baked) => {
                            check_shim_binary(&baked, &mut issues);
                            // A config-shaped target names the EVENTS an old
                            // install is missing; a code artifact has no
                            // per-event config, so the equivalent is the file.
                            // Nothing re-installs on a pixtuoid upgrade, so
                            // without this an upgrader runs the plugin they
                            // connected with forever and doctor says fine.
                            // Compared modulo the baked path, which is the one
                            // line that legitimately differs from `intended`.
                            if strip_baked_line(&installed) != strip_baked_line(&intended) {
                                issues.push(format!(
                                    "{} differs from the plugin this pixtuoid ships — it \
                                     predates an upgrade, so anything added since is not \
                                     forwarded. Reconnect the source to refresh it.",
                                    verify::display_safe(&p)
                                ));
                            }
                        }
                        None => notes.push(format!(
                            "could not read the baked shim path from {}",
                            verify::display_safe(&p)
                        )),
                    }
                }
                issues.extend(missing_artifact_issue(&missing));
            }
            Err(e) => notes.push(format!("could not resolve plugin artifact paths: {e}")),
        }
    }
    verify::SchemaVerifyResult { issues, notes }
}

/// A rendered artifact without its `const HOOK_PATH` line — the only line that
/// legitimately differs between what is installed (a real shim path) and what
/// this binary would render (a placeholder).
fn strip_baked_line(content: &str) -> String {
    content
        .lines()
        .filter(|l| !l.contains(verify::BAKED_HOOK_MARKER))
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

/// Resolve the hook binary for a target. An explicit path always wins —
/// `--hook-path` first, then the `PIXTUOID_HOOK` env override — and the returned
/// bool reports that, so `install_target` EMBEDS it rather than discarding the
/// user's choice for a bare PATH-resolved name. Otherwise `locate` tries to find
/// `pixtuoid-hook`; a failure is fatal only for targets that EMBED the path
/// (`BinaryStrategy::EmbedAbsolute`), while bare-name/PATH targets fall back to the
/// bare name so a fresh-machine install still succeeds.
fn resolve_hook_binary_from(
    t: &Target,
    hook_path: Option<PathBuf>,
    env_hook: Option<PathBuf>,
    locate: impl FnOnce() -> Result<PathBuf>,
) -> Result<(PathBuf, bool)> {
    // Both are EXPLICIT paths that get EMBEDDED into the config, where a relative
    // path would resolve against the CLI's cwd at hook time — hooks would silently
    // never fire from other dirs — so both take the same absolutize-and-warn arm.
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
        // Absolutize against our cwd (plain join, not canonicalize — Windows
        // canonicalize yields a \\?\ verbatim path that the cmd.exe bare form
        // can't take).
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
            // tracing, not println!: install runs under the TUI alt-screen, where a
            // stdout write corrupts the frame. Stripped because `connect`/`setup`
            // route tracing to RAW stderr and `p` is a user-supplied value.
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

/// Install pixtuoid hooks into `t`'s config, returning a structured report. The
/// ConfigLock round (read→merge→backup→write) is the load-bearing write authority
/// (invariant #4); it stays intact here.
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
    // The lock covers the WHOLE read→merge→backup→write round (lost-update TOCTOU:
    // two concurrent pixtuoid runs would otherwise interleave read(A)→write(B)→
    // write(A) and A's rename clobbers B's change). It only serializes pixtuoid
    // against pixtuoid — the CLI itself can't honor this lock.
    let lock = io::lock_config(&path)?;
    // Read + backup through the guard's pinned resolution, NOT by re-resolving
    // `path`: a symlink retarget between lock and read would otherwise split the
    // round across two files.
    let content = lock.read()?;
    // Merge FIRST so a present-but-malformed config bails BEFORE we touch the
    // filesystem — else the wholly-owned extra artifacts below were left on disk as
    // orphan plugin files registered nowhere (a partial install).
    let outcome = (t.merge_install)(&content, &hook_cmd)
        .with_context(|| format!("processing {}", path.display()))?;
    // Written before the config WRITE so a re-install refreshes them even when the
    // merge is a no-op (heals a deleted plugin file).
    if let Some(make) = t.extra_artifacts {
        for (p, c) in make(&binary)? {
            // A real write here would be invisible: CI has no such dir, and the
            // shim exits 0 by invariant #5 so nothing downstream reports it.
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
            // Atomic (temp-in-dir → fsync → rename), NOT a plain `fs::write`: a
            // torn write would leave a half-rendered plugin the gateway then fails
            // to load. Each artifact is its own lock target — disjoint from the
            // config lock held here, consistent lock order config→artifact, so no
            // self-deadlock.
            io::write_config_atomic(&p, &c).with_context(|| format!("writing {}", p.display()))?;
        }
    }
    // Independent of whether the file content changed — a no-op re-install on a box
    // where pixtuoid-hook isn't on PATH would otherwise warn nothing. Skipped when an
    // explicit path was embedded, since PATH resolution then never happens.
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
    // Absent config → nothing to remove, decided BEFORE locking: lock_config
    // creates the parent dir + a .lock sidecar, and materializing ~/.reasonix
    // here would flip that target's presence probe on a pure no-op.
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
        // SEMANTIC no-op. Never rewrite the file or delete the backup here: the
        // backup is the user's only recovery path, and a byte comparison would
        // falsely fire on any hand-formatted config and destroy it.
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

#[cfg(test)]
mod tests;
