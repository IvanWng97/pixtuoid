// The per-CLI installers + merge/verify/target have no cross-crate/cross-target
// consumers (all callers are in-lib via `crate::install::…`) EXCEPT the three
// `io` env filters and `target::TARGETS` (the bin's registry walk), so they are
// `pub(crate) mod` with just those items re-exported — making `unreachable_pub`
// the compiler tooth for the rest of their item surface (extends #573's io-only
// tooth). `pub mod install` (lib.rs) stays pub to carry the re-exports.
pub(crate) mod claude;
pub(crate) mod codewhale;
pub(crate) mod codex;
pub(crate) mod cursor;
pub(crate) mod grok;
pub(crate) mod hermes;
mod hook_cmd;
pub(crate) mod kimi;
// io stays pub(crate) for a STRONGER reason than its siblings: it holds the
// config-write authority (invariant #4), which must never be cross-crate
// reachable — only its three env filters (below) are re-exported.
pub(crate) mod io;
pub use io::{nonempty, nonempty_abs_env, nonempty_env};
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
/// six JSON/TOML/YAML-config targets (Claude/Codex/CodeWhale/Cursor/Reasonix/Hermes)
/// install/uninstall/detect key on this, not the command shape. (opencode, openclaw and grok write their own wholly-owned files with their own sentinel.)
pub(crate) const SENTINEL_KEY: &str = "_pixtuoid";

/// Whether `t`'s config currently bears pixtuoid hooks — the load-bearing gate
/// for `verify_target` (an uninstalled config would verify "broken"; see
/// `doctor::diagnose`). A dry-run uninstall that would change the parsed doc
/// means managed hooks are present. An absent/empty config is excluded; a
/// config present but unreadable or unparseable is INCLUDED (true) so a
/// hooks-bearing-but-malformed config still counts as installed. (Until 0.12.0
/// this was also `config::resolve_connected`'s migrate-default signal for an
/// absent `[sources]` flag — that inference was dropped, so this went
/// `pub` → `pub(crate)`. Callers: `doctor::diagnose`'s verify gate,
/// `doctor::run`'s per-source `hooks_installed` report row, and
/// `sources::skip_freeze` (the onboarding-skip freeze probes it so a pre-0.12
/// upgrader's hooks survive a skip).)
pub(crate) fn has_hooks(t: &'static Target, config: Option<PathBuf>) -> bool {
    // Mirror `verify_target`'s config resolution: an injected root (fixture
    // tests, a non-default config) else the target's real default path. The
    // gate (this) and the verify it guards MUST read the SAME config, or
    // `diagnose` through an injected root could never observe an install.
    let path = match config.map(Ok).unwrap_or_else(|| (t.default_config_path)()) {
        Ok(p) => p,
        // No resolvable default path (no home dir) → no config to bear hooks.
        Err(_) => return false,
    };
    match io::read_config(&path) {
        Ok(c) if c.trim().is_empty() => false,
        Ok(c) => (t.merge_uninstall)(&c).map(|o| o.changed).unwrap_or(true),
        Err(_) => true,
    }
}

/// Verify a target's installed config is structurally SOUND (the silent-dead
/// check, #309) — read-only, false-positive-free. Call only when hooks are
/// claimed installed (`has_hooks(t, config)`, same `config`). Returns the per-source `verify_schema`
/// verdict (sentinel + event-set + target extras) PLUS the shim-on-disk check
/// this (the only I/O) layer adds: an embedded absolute path is stat'd for
/// exists+executable (HARD); a Claude/Unix bare name is a soft PATH note (a
/// doctor-process PATH miss is not proof the CLI can't resolve it). `config`
/// overrides the default path (tests + a `--config` round); `None` = the
/// target's default — mirrors `install_target`. Layer-internal (`pub(crate)`,
/// like its sibling `has_hooks`): the sole caller is `doctor::diagnose`'s verify
/// gate — `pub mod install` makes the path pub-reachable, so `unreachable_pub`
/// can't mechanically catch the over-broad `pub`.
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
    let mut notes = Vec::new();
    match parse.shim {
        ShimRef::Absolute(p) => {
            // `display_safe`: the path came from the user's hand-editable hook
            // command, and these issues reach a real terminal (doctor stdout /
            // boot eprintln) — strip control chars at the SOURCE so no surface
            // can leak an ANSI/OSC escape (R0615-06 discipline; online review).
            let shown = verify::display_safe(&p);
            if !p.exists() {
                issues.push(format!("shim binary missing: {shown}"));
            } else if !is_executable(&p) {
                issues.push(format!("shim binary not executable: {shown}"));
            }
        }
        ShimRef::BareName => {
            // Claude/Unix bare `pixtuoid-hook` relies on PATH; a doctor-process
            // PATH miss is NOT proof the CLI can't resolve it → soft note only.
            if !io::hook_on_path() {
                notes.push(
                    "pixtuoid-hook not on this process's PATH (the CLI's PATH may differ)".into(),
                );
            }
        }
        ShimRef::Unknown => {
            // SOFT, not hard: we couldn't extract a path from the command, so we
            // can't CONFIRM the shim exists — but we also can't prove it's broken
            // (a future source with a novel-but-valid command shape lands here).
            // False-positive-free wins: a note, never a "broken" verdict. The
            // genuine no-hooks case is already a HARD issue from verify_schema's
            // sentinel/event-set check, so this never masks a real break.
            notes.push("could not read the shim path from the managed hook command".into());
        }
    }
    // Wholly-owned extra artifacts (the OpenClaw plugin DIR): the config merge can
    // verify clean while the plugin FILES the gateway actually loads are
    // missing/clobbered — the exact silent-dead class doctor exists to catch, and
    // the config-level `verify_schema` is blind to it (#332). Stat each artifact
    // path: a missing file is a HARD break (like a missing shim). The artifact
    // PATHS are independent of the baked shim path (only the entry-module CONTENT
    // bakes it), so a placeholder hook arg yields the real install locations
    // WITHOUT resolving the binary — a read-only doctor check must not hard-error
    // just because pixtuoid-hook isn't locatable. Calling the SAME fn install uses
    // means the verified path set can never drift from the writer's.
    //
    // INVARIANT (#387): `install_target`'s code-write surface is exactly
    // {`config_path` merge, `extra_artifacts` dir}, and BOTH are verify-covered —
    // the config by `verify_schema` (opencode's plugin IS its config), the dir by
    // this stat. A NEW code-shipping path added to `install_target` MUST gain a
    // matching check here, or it ships the silent-dead class for a 3rd code-artifact
    // target. Pinned by `verify_target_hard_flags_a_missing_code_artifact_for_every_extra_artifacts_target`.
    if let Some(make) = t.extra_artifacts {
        match make(std::path::Path::new("pixtuoid-hook")) {
            Ok(arts) => {
                for (p, _) in arts {
                    if !p.exists() {
                        issues.push(format!(
                            "plugin artifact missing: {}",
                            verify::display_safe(&p)
                        ));
                    }
                }
            }
            // Couldn't even compute the paths (e.g. no home dir) — can't confirm,
            // so a soft note, never a spurious "broken" (the config path would have
            // failed to resolve first anyway).
            Err(e) => notes.push(format!("could not resolve plugin artifact paths: {e}")),
        }
    }
    verify::SchemaVerifyResult { issues, notes }
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
/// hook-spawner's per-drive cwd. Always false on Unix (`Component::Prefix`
/// is Windows-only).
fn is_drive_relative(p: &std::path::Path) -> bool {
    !p.has_root() && matches!(p.components().next(), Some(std::path::Component::Prefix(_)))
}

/// Resolve the hook binary for a target. An explicit path always wins —
/// `--hook-path` first, then the `PIXTUOID_HOOK` env override (empty =
/// unset, see `io::nonempty_env`); both flow through the same
/// absolutize-and-warn arm, and the returned bool reports that an explicit
/// override was used so `install_target` EMBEDS it (the user pointed at a
/// specific binary — writing the bare PATH-resolved name would discard their
/// choice) and skips the PATH warning. Otherwise `locate` tries to find
/// `pixtuoid-hook`; if that fails we only hard-error for targets that EMBED
/// the path (`BinaryStrategy::EmbedAbsolute`, e.g. Codex). Targets that write the
/// bare name and rely on PATH (Claude) fall back to the bare name so a
/// fresh-machine install still succeeds — the `path_warning` flag in the
/// Sources panel covers the not-yet-on-PATH case. The env override is injected by the
/// caller so the whole decision is testable without mutating process env.
fn resolve_hook_binary_from(
    t: &Target,
    hook_path: Option<PathBuf>,
    env_hook: Option<PathBuf>,
    locate: impl FnOnce() -> Result<PathBuf>,
) -> Result<(PathBuf, bool)> {
    // The CLI flag outranks the ambient env override. Both are EXPLICIT paths
    // that get EMBEDDED into the config, where a relative path would resolve
    // against the CLI's cwd at hook time — hooks would silently never fire
    // from other dirs — so both take the same absolutize-and-warn arm (the
    // env seam used to pass through `locate()` verbatim and bypass it).
    let explicit = hook_path
        .map(|p| (p, "--hook-path"))
        .or(env_hook.map(|p| (p, "PIXTUOID_HOOK")));
    if let Some((p, origin)) = explicit {
        // Drive-relative input would make the cwd-join below a silent no-op
        // (see `is_drive_relative`) — the exact never-fires embed this arm
        // exists to prevent, so hard-error like the unreadable-cwd case.
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
            // A failed cwd query must NOT fall back to silently embedding the
            // relative path — that re-creates exactly the never-fires bug the
            // absolutization exists to prevent.
            let cwd = std::env::current_dir().with_context(|| {
                format!("{origin} is relative and the current directory is unreadable; pass an absolute path")
            })?;
            cwd.join(&p)
        } else {
            p
        };
        if !p.exists() {
            // tracing, not println!: install runs under the TUI alt-screen
            // (the Sources panel), where a stdout write corrupts the frame.
            tracing::warn!(
                "{origin} {} does not exist yet; the hook will fail until it does",
                p.display()
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

/// Whether an install changed the config or was already current. Carried by
/// `InstallReport` so both presenters (CLI stdout, TUI panel) render the same
/// outcome from one core.
#[derive(Debug)]
pub enum InstallOutcome {
    Installed,
    AlreadyUpToDate,
}

/// Structured result of `install_target` — the data the in-TUI Sources panel
/// renders. NO I/O: the core does the ConfigLock round and returns this; the
/// panel decides how to surface it.
#[derive(Debug)]
pub struct InstallReport {
    pub outcome: InstallOutcome,
    pub config_path: PathBuf,
    /// The backup taken this round (`None` on a no-op, or when one already exists).
    pub backup: Option<PathBuf>,
    /// True when the bare `pixtuoid-hook` isn't on PATH (Claude/Unix, no explicit
    /// hook). An install-time environment check, surfaced by the presenter.
    pub path_warning: bool,
}

/// Install pixtuoid hooks into `t`'s config, returning a structured report.
/// The ConfigLock round (read→merge→backup→write) is the load-bearing write
/// authority (invariant #4); it stays intact here. **`pub(crate)`: the ONLY
/// caller is `crate::sources::connect_target`** — the install trigger is not a
/// public API; everything binds a source through `crate::sources`.
pub(crate) fn install_target(
    t: &Target,
    config: Option<PathBuf>,
    hook_path: Option<PathBuf>,
) -> Result<InstallReport> {
    let path = config
        .map(Ok)
        .unwrap_or_else(|| (t.default_config_path)())?;
    let env_hook = io::nonempty_env("PIXTUOID_HOOK").map(PathBuf::from);
    let (binary, explicit_hook) =
        resolve_hook_binary_from(t, hook_path, env_hook, io::default_hook_binary)?;
    let hook_cmd = (t.hook_command)(&binary, explicit_hook)?;
    // The lock covers the WHOLE read→merge→backup→write round (lost-update
    // TOCTOU: two concurrent pixtuoid runs would otherwise interleave
    // read(A)→write(B)→write(A) and A's rename clobbers B's change). Residual:
    // the CLI itself (e.g. CC rewriting settings.json) can't honor this lock —
    // it only serializes pixtuoid against pixtuoid.
    let lock = io::lock_config(&path)?;
    // Read + backup through the guard's pinned resolution (ConfigLock::read /
    // ::backup_once), NOT by re-resolving `path`: a symlink retarget between
    // lock and read would otherwise split the round across two files (see
    // ConfigLock::read).
    let content = lock.read()?;
    // Merge FIRST so a present-but-malformed config bails (merge_install's
    // parse_*_or_empty "refusing to overwrite") BEFORE we touch the filesystem —
    // else the wholly-owned extra artifacts below were left on disk as orphan
    // plugin files registered nowhere (a partial install).
    let outcome = (t.merge_install)(&content, &hook_cmd)
        .with_context(|| format!("processing {}", path.display()))?;
    // Wholly-owned extra artifacts (the OpenClaw plugin dir) — written before the
    // config WRITE so a re-install refreshes them even when the merge is a no-op
    // (heals a deleted plugin file), but only AFTER the merge confirmed the config
    // parses. The shim's resolved path is baked into the entry module.
    if let Some(make) = t.extra_artifacts {
        for (p, c) in make(&binary)? {
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("creating plugin dir {}", dir.display()))?;
            }
            // Atomic + symlink-safe (temp-in-dir → fsync → rename), NOT a plain
            // `fs::write`: the rename REPLACES `p` rather than following a symlink
            // planted at it, and a torn write can't leave a half-rendered plugin
            // the gateway then fails to load. Reuses the ConfigLock write authority
            // (each artifact is its own lock target — disjoint from the config lock
            // held here, consistent lock order config→artifact, so no self-deadlock).
            io::write_config_atomic(&p, &c).with_context(|| format!("writing {}", p.display()))?;
        }
    }
    // The PATH check is an install-time environment check, independent of whether
    // the file content changed — always surface it (a no-op re-install on a box
    // where pixtuoid-hook isn't on PATH would otherwise warn nothing). Skipped
    // when an explicit --hook-path was written: the absolute path is embedded,
    // so PATH resolution never happens.
    let path_warning = t.binary_strategy == BinaryStrategy::BareNameOnPath
        && !explicit_hook
        && !io::hook_on_path();
    if !outcome.changed {
        return Ok(InstallReport {
            outcome: InstallOutcome::AlreadyUpToDate,
            config_path: path,
            backup: None,
            path_warning,
        });
    }
    let backup = lock.backup_once(BACKUP_SUFFIX)?;
    lock.write_atomic(&outcome.content)?;
    Ok(InstallReport {
        outcome: InstallOutcome::Installed,
        config_path: path,
        backup,
        path_warning,
    })
}

/// Whether an uninstall removed managed entries or found nothing to remove.
#[derive(Debug)]
pub enum UninstallOutcome {
    Removed,
    NothingToRemove,
}

/// Structured result of `uninstall_target`.
#[derive(Debug)]
pub struct UninstallReport {
    pub outcome: UninstallOutcome,
    pub config_path: PathBuf,
    /// The backup deleted on a successful removal (the install backup is no
    /// longer needed once the hooks are gone).
    pub removed_backup: Option<PathBuf>,
}

/// Remove pixtuoid hooks from `t`'s config, returning a structured report. The
/// pure core behind the TUI Sources panel's disconnect action. Same lock
/// scope + the load-bearing "never rewrite/delete-backup on a semantic no-op"
/// rule as before.
/// Remove pixtuoid hooks from `t`'s config. **`pub(crate)`: the ONLY caller is
/// `crate::sources::disconnect_target`** — go through `crate::sources`.
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
    // Same lock scope as install_target: the whole read→merge→write round, all
    // addressed through the guard's pinned resolution.
    let lock = io::lock_config(&path)?;
    let content = lock.read()?;
    let outcome =
        (t.merge_uninstall)(&content).with_context(|| format!("processing {}", path.display()))?;
    if !outcome.changed {
        // SEMANTIC no-op (covers an empty config and no managed entries).
        // Never rewrite the file or delete the backup here: the backup is the
        // user's only recovery path. A byte comparison here would falsely
        // fire on any hand-formatted config and destroy the backup.
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
