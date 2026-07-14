pub mod claude;
pub mod codewhale;
pub mod codex;
pub mod cursor;
pub mod hermes;
mod hook_cmd;
// io is crate-private: its config-write authority (invariant #4) must never be
// reachable cross-crate. Only the two env filters are re-exported below, so a
// FUTURE stray `pub` in io trips unreachable_pub (-D warnings) — a compiler
// tooth, not a review-practice backstop.
pub(crate) mod io;
pub use io::{nonempty, nonempty_env};
pub mod merge;
pub mod openclaw;
pub mod opencode;
pub mod reasonix;
pub mod target;
pub mod verify;

use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use target::{BinaryStrategy, Target, BACKUP_SUFFIX};

/// The idempotency sentinel stamped on every hook entry pixtuoid installs — the
/// five JSON/TOML-config targets (Claude/Codex/CodeWhale/Cursor/Reasonix)
/// install/uninstall/detect key on this, not the command shape. (opencode and
/// openclaw are code artifacts and use their own plugin-file sentinel.)
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
pub(crate) fn has_hooks(t: &'static Target) -> bool {
    // No resolvable default path (no home dir) → no config to bear hooks.
    let Ok(path) = (t.default_config_path)() else {
        return false;
    };
    match io::read_config(&path) {
        Ok(c) if c.trim().is_empty() => false,
        Ok(c) => (t.merge_uninstall)(&c).map(|o| o.changed).unwrap_or(true),
        Err(_) => true,
    }
}

/// Verify a target's installed config is structurally SOUND (the silent-dead
/// check, #309) — read-only, false-positive-free. Call only when hooks are
/// claimed installed (`has_hooks(t)`). Returns the per-source `verify_schema`
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
mod tests {
    use super::*;
    use crate::install::target::{MergeOutcome, Target, CLAUDE, CODEX, OPENCLAW};

    /// RAII override of a process-global env var: sets `key` for the test's
    /// scope and restores the PRIOR value (or unsets) on drop — panic-safe, so
    /// a failing assert can't leak the override past the test. Callers must
    /// hold `TEST_ENV_LOCK` first, declared BEFORE this guard (locals drop in
    /// reverse order, so the env restore happens while the lock is still held).
    struct EnvVarOverride {
        key: &'static str,
        prior: Option<std::ffi::OsString>,
    }

    impl EnvVarOverride {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let prior = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, prior }
        }
    }

    impl Drop for EnvVarOverride {
        fn drop(&mut self) {
            match self.prior.take() {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    // A second fake target for "both present" rows (avoids depending on Phase 2's CODEX).
    static FAKE: Target = Target {
        name: "fake",
        core_source: "fake",
        display_name: "Fake",
        default_config_path: || Ok(std::path::PathBuf::from("/nonexistent/fake")),
        hook_command: |_, _| Ok("x".into()),
        merge_install: |c, _| {
            Ok(MergeOutcome {
                content: c.to_string(),
                changed: false,
            })
        },
        merge_uninstall: |c| {
            Ok(MergeOutcome {
                content: c.to_string(),
                changed: false,
            })
        },
        verify_schema: |_| crate::install::verify::SchemaParse::broken("test fake"),
        binary_strategy: BinaryStrategy::EmbedAbsolute,
        presence_probe: None,
        extra_artifacts: None,
    };

    // A per-process config path under the system temp dir, used by FAKE2/FAKE_DIR
    // so their fn-pointer `default_config_path` can point at a test-controlled
    // file (the `fn() -> PathBuf` signature can't capture a TempDir). The PID
    // suffix keeps two concurrent `cargo test` invocations of this binary from
    // racing on the same fixed path.
    fn fake2_config_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("pixtuoid-test-fake2-{}.toml", std::process::id()))
    }

    fn fake_dir_config_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("pixtuoid-test-fake-dir-{}", std::process::id()))
    }

    // FAKE2: default_config_path points at a test-writable file, and its
    // merge_uninstall reports `changed` iff the content is non-empty — so
    // has_hooks can be driven through both the changed (true) and unchanged
    // (false) arms by controlling the on-disk content.
    static FAKE2: Target = Target {
        name: "fake2",
        core_source: "fake2",
        display_name: "Fake2",
        default_config_path: || Ok(fake2_config_path()),
        hook_command: |_, _| Ok("x".into()),
        merge_install: |c, _| {
            Ok(MergeOutcome {
                content: c.to_string(),
                changed: false,
            })
        },
        merge_uninstall: |c| {
            Ok(MergeOutcome {
                content: c.to_string(),
                changed: !c.trim().is_empty(),
            })
        },
        verify_schema: |_| crate::install::verify::SchemaParse::broken("test fake"),
        binary_strategy: BinaryStrategy::EmbedAbsolute,
        presence_probe: None,
        extra_artifacts: None,
    };

    // FAKE_DIR: default_config_path points at a path the test creates as a
    // DIRECTORY, so read_config's File::open(dir).read_to_string errors → the
    // has_hooks Err(_) => true arm.
    static FAKE_DIR: Target = Target {
        name: "fakedir",
        core_source: "fakedir",
        display_name: "FakeDir",
        default_config_path: || Ok(fake_dir_config_path()),
        hook_command: |_, _| Ok("x".into()),
        merge_install: |c, _| {
            Ok(MergeOutcome {
                content: c.to_string(),
                changed: false,
            })
        },
        merge_uninstall: |c| {
            Ok(MergeOutcome {
                content: c.to_string(),
                changed: false,
            })
        },
        verify_schema: |_| crate::install::verify::SchemaParse::broken("test fake"),
        binary_strategy: BinaryStrategy::EmbedAbsolute,
        presence_probe: None,
        extra_artifacts: None,
    };

    // FAKE_NO_HOME: default_config_path returns Err (no home dir resolves) and
    // there's no config override → exercises has_hooks's `let Ok(path) = … else`
    // early-return and verify_target's matching `Err(_)` arm.
    static FAKE_NO_HOME: Target = Target {
        name: "fakenohome",
        core_source: "fakenohome",
        display_name: "FakeNoHome",
        default_config_path: || Err(anyhow::anyhow!("no home dir")),
        hook_command: |_, _| Ok("x".into()),
        merge_install: |c, _| {
            Ok(MergeOutcome {
                content: c.to_string(),
                changed: false,
            })
        },
        merge_uninstall: |c| {
            Ok(MergeOutcome {
                content: c.to_string(),
                changed: false,
            })
        },
        verify_schema: |_| crate::install::verify::SchemaParse::broken("test fake"),
        binary_strategy: BinaryStrategy::EmbedAbsolute,
        presence_probe: None,
        extra_artifacts: None,
    };

    /// A platform-absolute fixture path: `/x/hook` is DRIVE-RELATIVE on
    /// Windows, so the absolutization would rewrite it there.
    fn abs_fixture(unix: &str, windows: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(windows)
        } else {
            PathBuf::from(unix)
        }
    }

    #[test]
    fn resolve_hook_binary_explicit_path_wins() {
        // --hook-path always short-circuits resolution (locate is never called).
        let p = abs_fixture("/x/hook", r"C:\x\hook");
        let got = resolve_hook_binary_from(&CLAUDE, Some(p.clone()), None, || {
            panic!("locate must not be called when --hook-path is given")
        });
        assert_eq!(got.unwrap(), (p, true));
    }

    #[test]
    fn resolve_hook_binary_absolutizes_a_relative_explicit_path() {
        // An embedded relative path would resolve against the CLI's cwd at
        // hook time and silently never fire from other dirs.
        let (got, explicit) = resolve_hook_binary_from(
            &CLAUDE,
            Some(PathBuf::from("target/debug/pixtuoid-hook")),
            None,
            || unreachable!("explicit path must win"),
        )
        .unwrap();
        assert!(explicit);
        assert!(got.is_absolute(), "expected absolutized path, got {got:?}");
        assert!(got.ends_with("target/debug/pixtuoid-hook"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_hook_binary_claude_falls_back_to_bare_name_when_unresolvable() {
        // Regression: a fresh-machine connect hard-failed when pixtuoid-hook
        // wasn't yet on PATH. Claude writes the bare name and relies on PATH, so an
        // unresolvable binary must fall back to the bare name (the PATH warning covers
        // the not-found case), NOT abort the install.
        // Routed through the injected seam (env_hook: None) so an ambient
        // PIXTUOID_HOOK on the dev machine cannot short-circuit the
        // locate-failure scenario this test stages.
        let got = resolve_hook_binary_from(&CLAUDE, None, None, || {
            Err(anyhow::anyhow!("could not locate"))
        });
        assert_eq!(got.unwrap(), (PathBuf::from("pixtuoid-hook"), false));
    }

    // The Windows twin of the claude fallback test above: exec form embeds the
    // absolute path, so an unresolvable binary is fatal there too — the bare-
    // name fallback is the unix-only contract.
    #[cfg(windows)]
    #[test]
    fn resolve_hook_binary_claude_errors_when_unresolvable_on_windows() {
        let got = resolve_hook_binary_from(&CLAUDE, None, None, || {
            Err(anyhow::anyhow!("could not locate"))
        });
        assert!(got.is_err(), "exec form requires a real resolved .exe");
    }

    #[test]
    fn resolve_hook_binary_codex_errors_when_unresolvable() {
        // Codex embeds the absolute path in the command, so an unresolvable binary
        // is genuinely fatal for that target.
        let got = resolve_hook_binary_from(&CODEX, None, None, || {
            Err(anyhow::anyhow!("could not locate"))
        });
        assert!(got.is_err());
    }

    #[test]
    fn resolve_hook_binary_env_override_routes_through_the_explicit_arm() {
        // PIXTUOID_HOOK is the env twin of --hook-path: a relative value must
        // be absolutized (embedded verbatim it resolves against the CLI's cwd
        // at hook time → silent dead hook for the embed targets), never passed
        // through locate() untouched.
        let (got, explicit) = resolve_hook_binary_from(
            &CODEX,
            None,
            Some(PathBuf::from("target/debug/pixtuoid-hook")),
            || unreachable!("the env override must win over locate"),
        )
        .unwrap();
        assert!(
            got.is_absolute(),
            "expected absolutized env path, got {got:?}"
        );
        assert!(got.ends_with("target/debug/pixtuoid-hook"));
        // The env override is EXPLICIT: install_target embeds it (Claude/Unix
        // included) instead of writing the bare PATH-resolved name, and the
        // PATH warning is skipped — same contract as --hook-path.
        assert!(explicit);
    }

    #[test]
    fn resolve_hook_binary_cli_flag_outranks_env_override() {
        let cli = abs_fixture("/cli/hook", r"C:\cli\hook");
        let env = abs_fixture("/env/hook", r"C:\env\hook");
        let got = resolve_hook_binary_from(&CLAUDE, Some(cli.clone()), Some(env), || {
            unreachable!("an explicit path must win over locate")
        });
        assert_eq!(got.unwrap(), (cli, true));
    }

    #[test]
    fn resolve_hook_binary_no_overrides_uses_locate() {
        let located = abs_fixture("/located/hook", r"C:\located\hook");
        let expect = located.clone();
        let got = resolve_hook_binary_from(&CLAUDE, None, None, || Ok(located));
        assert_eq!(got.unwrap(), (expect, false));
    }

    #[test]
    fn empty_env_override_counts_as_unset_at_the_live_read() {
        // io::nonempty_env is the live seam install_target reads PIXTUOID_HOOK
        // through: empty/whitespace must be unset (the #172 policy), so ""
        // never becomes an embedded "" command.
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var_os("PIXTUOID_HOOK");
        std::env::set_var("PIXTUOID_HOOK", "");
        let empty = io::nonempty_env("PIXTUOID_HOOK");
        std::env::set_var("PIXTUOID_HOOK", "   ");
        let blank = io::nonempty_env("PIXTUOID_HOOK");
        std::env::set_var("PIXTUOID_HOOK", "/real/hook");
        let real = io::nonempty_env("PIXTUOID_HOOK");
        match saved {
            Some(v) => std::env::set_var("PIXTUOID_HOOK", v),
            None => std::env::remove_var("PIXTUOID_HOOK"),
        }
        assert_eq!(empty, None);
        assert_eq!(blank, None);
        assert_eq!(real, Some("/real/hook".into()));
    }

    #[test]
    fn is_drive_relative_only_matches_prefix_without_root() {
        use std::path::Path;
        #[cfg(windows)]
        {
            assert!(is_drive_relative(Path::new(r"C:rel\hook.exe")));
            assert!(!is_drive_relative(Path::new(r"C:\abs\hook.exe")));
            assert!(!is_drive_relative(Path::new(r"rel\hook.exe")));
            // Rooted-no-prefix (`\x\hook`) IS handled by join (keeps cwd's
            // drive) — it must not trip the hard error.
            assert!(!is_drive_relative(Path::new(r"\rooted\hook.exe")));
        }
        // Unix has no path prefixes — `C:foo` is an ordinary relative path there.
        #[cfg(unix)]
        assert!(!is_drive_relative(Path::new("C:foo.exe")));
    }

    // Drive-relative `C:foo.exe` (prefix, no root): is_relative() is true but
    // `cwd.join` no-ops on it, so the "absolutized" embed would still resolve
    // against a per-drive cwd at hook time — hard-error instead.
    #[cfg(windows)]
    #[test]
    fn resolve_hook_binary_rejects_a_drive_relative_explicit_path() {
        let err = resolve_hook_binary_from(
            &CLAUDE,
            Some(PathBuf::from(r"C:rel\hook.exe")),
            None,
            || unreachable!("the explicit path must win"),
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("drive-relative") && msg.contains("absolute path"),
            "got: {msg}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn resolve_hook_binary_rejects_a_drive_relative_env_override() {
        let err =
            resolve_hook_binary_from(&CODEX, None, Some(PathBuf::from(r"C:rel\hook.exe")), || {
                unreachable!("the env override must win")
            })
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("PIXTUOID_HOOK") && msg.contains("drive-relative"),
            "the error must name the seam that supplied the bad path: {msg}"
        );
    }

    // --- has_hooks arms --------------------------------------------------------

    #[test]
    fn has_hooks_empty_config_is_false() {
        // FAKE's default_config_path is /nonexistent/fake → read_config returns
        // Ok("") (the missing-file early return), hitting the empty arm → false.
        assert!(!has_hooks(&FAKE));
    }

    #[test]
    fn has_hooks_unreadable_config_is_true() {
        // FAKE_DIR points at a path we create as a DIRECTORY: it exists, so
        // read_config tries File::open + read_to_string which errors → Err arm.
        let dir = fake_dir_config_path();
        let _ = std::fs::remove_file(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(has_hooks(&FAKE_DIR));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn has_hooks_changed_vs_unchanged_arms() {
        let path = fake2_config_path();
        // Non-empty content → FAKE2.merge_uninstall reports changed=true → true.
        std::fs::write(&path, "model = \"x\"\n").unwrap();
        assert!(has_hooks(&FAKE2));
        // Whitespace-only content → read_config returns it, but it trims to empty
        // → the `c.trim().is_empty()` empty arm → false (changed arm not reached).
        std::fs::write(&path, "   \n").unwrap();
        assert!(!has_hooks(&FAKE2));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn verify_target_and_has_hooks_handle_unresolvable_config_path() {
        // A target whose default_config_path Errs (no home dir) and no `config`
        // override: has_hooks's `let Ok(path) = … else { return false }` fires →
        // no config to bear hooks; verify_target's matching `Err(_)` arm fires →
        // a single, specific "no config path resolves" issue (so NOT sound). No FS
        // or env needed — the Err comes straight from the fn pointer.
        assert!(
            !has_hooks(&FAKE_NO_HOME),
            "no resolvable config path → no hooks"
        );
        let v = verify_target(&FAKE_NO_HOME, None);
        assert!(!v.is_sound());
        assert_eq!(
            v.issues,
            vec!["no config path resolves (no home dir)".to_string()]
        );
        assert!(v.notes.is_empty(), "the early return emits no notes");
    }

    // --- install_target: CLAUDE sentinel write + backup ----------------------

    #[test]
    fn install_target_claude_writes_sentinel_and_backs_up() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("settings.json");
        std::fs::write(&cfg, "{}\n").unwrap(); // existing content → triggers a backup

        // Explicit hook_path short-circuits resolution (no host PATH dependency).
        install_target(
            &CLAUDE,
            Some(cfg.clone()),
            Some(PathBuf::from("/fake/pixtuoid-hook")),
        )
        .unwrap();

        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert!(v["hooks"]["PreToolUse"][0]["_pixtuoid"].as_bool().unwrap());
        assert!(
            tmp.path().join("settings.json.pixtuoid.bak").exists(),
            "a backup of the prior content was written"
        );

        // Second install is a semantic no-op → already-up-to-date branch.
        install_target(
            &CLAUDE,
            Some(cfg.clone()),
            Some(PathBuf::from("/fake/pixtuoid-hook")),
        )
        .unwrap();
    }

    // --- the read→merge→write lock (#7) ----------------------------------------

    #[test]
    fn install_target_fails_fast_while_the_config_lock_is_held() {
        // Pins lock-before-read: even the up-to-date NO-OP path (which never
        // reaches the write) must refuse to run while another pixtuoid holds
        // the lock — we can't even safely read/decide mid-flight of a writer.
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("settings.json");
        install_target(
            &CLAUDE,
            Some(cfg.clone()),
            Some(PathBuf::from("/fake/pixtuoid-hook")),
        )
        .unwrap();

        let _guard = io::lock_config(&cfg).unwrap();
        let err = install_target(
            &CLAUDE,
            Some(cfg.clone()),
            Some(PathBuf::from("/fake/pixtuoid-hook")),
        )
        .unwrap_err();
        assert!(err.to_string().contains("could not lock"), "got: {err:#}");
    }

    #[test]
    fn uninstall_target_fails_fast_while_the_config_lock_is_held() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("settings.json");
        install_target(
            &CLAUDE,
            Some(cfg.clone()),
            Some(PathBuf::from("/fake/pixtuoid-hook")),
        )
        .unwrap();

        let _guard = io::lock_config(&cfg).unwrap();
        let err = uninstall_target(&CLAUDE, Some(cfg.clone())).unwrap_err();
        assert!(err.to_string().contains("could not lock"), "got: {err:#}");
    }

    #[test]
    fn uninstall_target_unchanged_preserves_backup() {
        // FAKE.merge_uninstall reports changed=false → the semantic no-op branch
        // must NOT delete the backup (the user's only recovery path).
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("config.toml");
        std::fs::write(&cfg, "anything\n").unwrap();
        let bak = tmp.path().join("config.toml.pixtuoid.bak");
        std::fs::write(&bak, "backup").unwrap();

        uninstall_target(&FAKE, Some(cfg.clone())).unwrap();

        assert!(bak.exists(), "a no-op uninstall must NOT delete the backup");
    }

    // --- structured report core (install_target / uninstall_target) -----------

    #[test]
    fn install_target_reports_installed_then_up_to_date() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("settings.json");
        std::fs::write(&cfg, "{}\n").unwrap(); // existing content → backup on first write

        let r = install_target(
            &CLAUDE,
            Some(cfg.clone()),
            Some(PathBuf::from("/fake/pixtuoid-hook")),
        )
        .unwrap();
        assert!(matches!(r.outcome, InstallOutcome::Installed));
        assert!(
            r.backup.is_some(),
            "first install of an existing file takes a backup"
        );
        assert_eq!(r.config_path, cfg);

        // Second install is a SEMANTIC no-op → AlreadyUpToDate, no backup churn.
        let r2 = install_target(
            &CLAUDE,
            Some(cfg.clone()),
            Some(PathBuf::from("/fake/pixtuoid-hook")),
        )
        .unwrap();
        assert!(matches!(r2.outcome, InstallOutcome::AlreadyUpToDate));
        assert!(r2.backup.is_none(), "a no-op install reports no backup");
    }

    #[test]
    fn install_target_explicit_hook_suppresses_path_warning() {
        // An explicit --hook-path embeds the absolute path, so PATH resolution
        // never happens → path_warning is deterministically false (no host PATH
        // dependency, unlike the no-explicit-hook case).
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("settings.json");
        let r = install_target(
            &CLAUDE,
            Some(cfg),
            Some(PathBuf::from("/fake/pixtuoid-hook")),
        )
        .unwrap();
        assert!(!r.path_warning);
    }

    #[test]
    fn uninstall_target_reports_removed_then_nothing() {
        // Removed: FAKE2 (changed=true on non-empty content) over a config with a backup.
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("config.toml");
        std::fs::write(&cfg, "model = \"x\"\n").unwrap();
        let bak = tmp.path().join("config.toml.pixtuoid.bak");
        std::fs::write(&bak, "backup").unwrap();

        let r = uninstall_target(&FAKE2, Some(cfg.clone())).unwrap();
        assert!(matches!(r.outcome, UninstallOutcome::Removed));
        assert_eq!(r.removed_backup.as_deref(), Some(bak.as_path()));
        assert!(!bak.exists());

        // NothingToRemove: an absent config, decided BEFORE locking (no side effects).
        let missing = tmp.path().join("missing").join("settings.json");
        let r2 = uninstall_target(&CLAUDE, Some(missing.clone())).unwrap();
        assert!(matches!(r2.outcome, UninstallOutcome::NothingToRemove));
        assert!(r2.removed_backup.is_none());
        assert!(
            !missing.parent().unwrap().exists(),
            "a no-op uninstall leaves no dirs"
        );
    }

    // Per-target end-to-end round-trip through the REAL install_target/
    // uninstall_target (each target's merge + the shared ConfigLock write),
    // replacing the per-target coverage the deleted CLI integration suite
    // (tests/install.rs) gave — now driven straight against the cores the
    // Sources panel calls, no CLI needed. Covers all 5 targets' formats:
    // Claude JSON, Codex/CodeWhale TOML, Reasonix flat-JSON, opencode TS plugin.
    #[test]
    fn install_target_round_trips_every_registered_target() {
        // Isolate OpenClaw's extra_artifacts (the plugin DIR resolves from
        // openclaw_state_dir(), NOT the config override) under a temp home, so the
        // round-trip never writes to the real ~/.openclaw. TEST_ENV_LOCK serializes
        // the process-global OPENCLAW_STATE_DIR set so a sibling env-mutating test
        // can't clobber it under plain `cargo test` (nextest isolates per-process).
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let oc_home = tempfile::TempDir::new().unwrap();
        let _state = EnvVarOverride::set("OPENCLAW_STATE_DIR", oc_home.path());
        for t in target::TARGETS {
            let tmp = tempfile::TempDir::new().unwrap();
            let cfg = tmp.path().join("cfg");
            let hook = || Some(PathBuf::from("/fake/pixtuoid-hook"));

            let r = install_target(t, Some(cfg.clone()), hook()).unwrap();
            assert!(
                matches!(r.outcome, InstallOutcome::Installed),
                "{}: first install must write hooks",
                t.name
            );
            assert!(cfg.exists(), "{}: install wrote a config", t.name);

            // Idempotent: a second install over our own output is a semantic no-op.
            let r2 = install_target(t, Some(cfg.clone()), hook()).unwrap();
            assert!(
                matches!(r2.outcome, InstallOutcome::AlreadyUpToDate),
                "{}: re-install must be a no-op (sentinel idempotency)",
                t.name
            );

            // Uninstall removes the managed entries...
            let u = uninstall_target(t, Some(cfg.clone())).unwrap();
            assert!(
                matches!(u.outcome, UninstallOutcome::Removed),
                "{}: uninstall must remove the managed entries",
                t.name
            );
            // ...and is itself idempotent.
            let u2 = uninstall_target(t, Some(cfg.clone())).unwrap();
            assert!(
                matches!(u2.outcome, UninstallOutcome::NothingToRemove),
                "{}: re-uninstall must find nothing to remove",
                t.name
            );
        }
    }

    // --- is_present / config_present detect⇄install symmetry (Test 3) ---------

    // The detect⇄install relationship, pinned per detection mechanism. The
    // spec's literal false→true→false does NOT hold in production: uninstall
    // PRESERVES the user's file/dir (the un-merge rule, see the preserve test
    // below), so detection stays TRUE after uninstall — asserting otherwise
    // would contradict the code. So this pins the REAL, deterministic arms:
    //   * config_present targets (CLAUDE/CODEX — no presence_probe): the config
    //     FILE is absent before any write and present after `install_target`
    //     writes it (the exact file `config_present` checks). After uninstall it
    //     is preserved → stays present.
    //   * the OpenClaw presence_probe: `is_present` is FALSE before install (an
    //     isolated, empty OPENCLAW_STATE_DIR) and TRUE after — install creates
    //     the state dir the probe detects (the install-creates-the-detectable
    //     symmetry; an install that wrote hooks nowhere the probe looks would be
    //     installed-but-invisible).
    #[test]
    fn config_present_target_file_is_absent_before_then_present_after_install() {
        use crate::install::target::config_present;
        // CLAUDE + CODEX are the only `presence_probe: None` (config_present)
        // targets — drive both through the real install_target. A controlled
        // config override lets us assert against the very file config_present
        // reads, without touching the real ~/.claude / ~/.codex.
        for t in [&CLAUDE, &CODEX] {
            let tmp = tempfile::TempDir::new().unwrap();
            let cfg = tmp.path().join("cfg");
            assert!(
                !config_present(&cfg),
                "{}: config_present must be FALSE before any write",
                t.name
            );
            install_target(
                t,
                Some(cfg.clone()),
                Some(PathBuf::from("/fake/pixtuoid-hook")),
            )
            .unwrap();
            assert!(
                config_present(&cfg),
                "{}: config_present must be TRUE after install writes the config",
                t.name
            );
            // Uninstall un-merges hooks but PRESERVES the file → still present.
            uninstall_target(t, Some(cfg.clone())).unwrap();
            assert!(
                config_present(&cfg),
                "{}: uninstall preserves the user's config file → still present",
                t.name
            );
        }
    }

    #[test]
    fn openclaw_is_present_is_false_before_then_true_after_install() {
        use crate::install::target::is_present;
        // TEST_ENV_LOCK serializes the process-global OPENCLAW_STATE_DIR set
        // (the sibling pattern). Point it at a NON-EXISTENT dir so the probe
        // starts FALSE — install then creates it.
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let oc_home = tempfile::TempDir::new().unwrap();
        let state = oc_home.path().join("ocstate"); // not yet created
        let _state = EnvVarOverride::set("OPENCLAW_STATE_DIR", &state);

        assert!(
            !is_present(&OPENCLAW),
            "OpenClaw must be undetected before install (empty isolated state dir)"
        );

        let exe = std::env::current_exe().unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("openclaw.json");
        install_target(&OPENCLAW, Some(cfg), Some(exe)).unwrap();

        assert!(
            is_present(&OPENCLAW),
            "install must create the state dir the presence probe detects \
             (detect⇄install symmetry — else installed-but-invisible)"
        );
    }

    // --- preserve rule: uninstall un-merges, never DELETES the file (Test 4) --

    #[test]
    fn uninstall_preserves_the_config_file_even_when_it_merges_to_empty() {
        // Codex installed ALONE: the config holds only our managed hook entry,
        // so uninstall un-merges to an effectively EMPTY TOML doc — the exact
        // case where a naive "delete if empty" would lose the user's file. The
        // documented rule is "un-merge, never delete": the file must still EXIST
        // after uninstall (so the user keeps their otherwise-empty-but-real
        // config and any future hand edits/comments survive).
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("config.toml");

        let r = install_target(
            &CODEX,
            Some(cfg.clone()),
            Some(PathBuf::from("/fake/pixtuoid-hook")),
        )
        .unwrap();
        assert!(matches!(r.outcome, InstallOutcome::Installed));
        assert!(cfg.exists());

        let u = uninstall_target(&CODEX, Some(cfg.clone())).unwrap();
        assert!(
            matches!(u.outcome, UninstallOutcome::Removed),
            "uninstall must have removed the managed entry (the merge produced a change)"
        );
        assert!(
            cfg.exists(),
            "uninstall must PRESERVE the config file (un-merge, never delete)"
        );
        // And the file no longer bears our sentinel — un-merged, not just left as-is.
        let content = io::read_config(&cfg).unwrap();
        assert!(
            !content.contains(SENTINEL_KEY),
            "the managed hook entry must be un-merged out: {content:?}"
        );
    }

    // --- data-safety: a malformed pre-existing config Errs, no data loss (Test 5)

    #[test]
    fn install_on_a_malformed_config_errors_without_rewriting_or_backing_up() {
        // The config-preserve / no-data-loss invariant: install reads the
        // existing config, the merge step PARSES it, and a parse failure must
        // bubble OUT of install_target BEFORE any backup_once / write_atomic —
        // so the user's (malformed but possibly recoverable) bytes are left
        // EXACTLY as they were, and no .pixtuoid.bak is minted.
        for (t, malformed) in [
            (&CODEX, "this is = = not valid toml [[["),
            (&CLAUDE, "{ not valid json,,, "),
        ] {
            let tmp = tempfile::TempDir::new().unwrap();
            let cfg = tmp.path().join("cfg");
            std::fs::write(&cfg, malformed).unwrap();
            let before = std::fs::read_to_string(&cfg).unwrap();

            let err = install_target(
                t,
                Some(cfg.clone()),
                Some(PathBuf::from("/fake/pixtuoid-hook")),
            )
            .unwrap_err();
            // The merge guard's message — "refusing to overwrite" — proves the
            // error came from the parse step, not a downstream write failure.
            let msg = format!("{err:#}");
            assert!(
                msg.contains("refusing to overwrite"),
                "{}: the error must come from the parse guard, got: {msg}",
                t.name
            );

            // No data loss: the file is byte-for-byte unchanged...
            assert_eq!(
                std::fs::read_to_string(&cfg).unwrap(),
                before,
                "{}: a malformed config must NOT be rewritten/truncated",
                t.name
            );
            // ...and no backup was written (the .lock sidecar may exist — the
            // lock is taken before the read by design — but the BACKUP is not).
            let bak = tmp.path().join(format!("cfg.{BACKUP_SUFFIX}"));
            assert!(
                !bak.exists(),
                "{}: a failed install must NOT mint a {BACKUP_SUFFIX} backup",
                t.name
            );
        }
    }

    #[test]
    fn install_on_a_malformed_config_leaves_no_orphan_extra_artifacts() {
        // OpenClaw is the ONLY target with extra_artifacts (a wholly-owned plugin
        // dir). A present-but-malformed config must bail BEFORE those files are
        // written — else a partial install strands orphan plugin files registered
        // nowhere. Env-isolated like the round-trip test (the plugin dir resolves
        // from OPENCLAW_STATE_DIR, NOT the config override).
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let oc_home = tempfile::TempDir::new().unwrap();
        let _state = EnvVarOverride::set("OPENCLAW_STATE_DIR", oc_home.path());

        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("openclaw.json");
        std::fs::write(&cfg, "{ not valid json,,, ").unwrap();
        let before = std::fs::read_to_string(&cfg).unwrap();

        let err = install_target(
            &OPENCLAW,
            Some(cfg.clone()),
            Some(PathBuf::from("/fake/pixtuoid-hook")),
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("refusing to overwrite"),
            "the bail must come from the parse guard, got: {err:#}"
        );
        // The malformed config is byte-for-byte preserved...
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), before);
        // ...and NO orphan plugin artifacts were written under the state dir.
        assert!(
            !oc_home.path().join("plugins").exists(),
            "a malformed-config bail must not leave orphan plugin artifacts on disk"
        );
    }

    // --- verify_target (#309 install-schema soundness) ------------------------

    /// A FRESH install of EVERY target, with a real executable as the shim, must
    /// verify SOUND (sentinel + full event-set + shim exists/executable; CodeWhale
    /// enabled). Covers all 6 formats e2e — the current test binary is the shim.
    #[test]
    fn verify_target_is_sound_after_a_real_install_for_every_target() {
        // Isolate OpenClaw's extra_artifacts under a temp home (see the round-trip
        // test) so this never writes to the real ~/.openclaw.
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let oc_home = tempfile::TempDir::new().unwrap();
        let _state = EnvVarOverride::set("OPENCLAW_STATE_DIR", oc_home.path());
        let exe = std::env::current_exe().unwrap(); // a real, executable file
        for &t in target::TARGETS {
            let tmp = tempfile::TempDir::new().unwrap();
            let cfg = tmp.path().join("cfg");
            install_target(t, Some(cfg.clone()), Some(exe.clone())).unwrap();
            let v = verify_target(t, Some(cfg));
            assert!(
                v.is_sound(),
                "{}: a fresh install must verify sound, got issues {:?}",
                t.name,
                v.issues
            );
        }
    }

    #[test]
    fn verify_target_flags_a_missing_shim_binary() {
        // Embed an absolute path that does NOT exist → the shim-on-disk check fails.
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("settings.json");
        let ghost = tmp.path().join("ghost-pixtuoid-hook");
        install_target(&CLAUDE, Some(cfg.clone()), Some(ghost)).unwrap();
        let v = verify_target(&CLAUDE, Some(cfg));
        assert!(!v.is_sound());
        assert!(
            v.issues.iter().any(|i| i.contains("shim binary missing")),
            "{:?}",
            v.issues
        );
    }

    // The empty-config arm of verify_target (a config FILE that exists but is
    // whitespace-only): distinct from has_hooks's empty→false — verify returns a
    // specific "config is empty" issue and short-circuits BEFORE verify_schema.
    #[test]
    fn verify_target_flags_an_empty_config_as_not_installed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("settings.json");
        std::fs::write(&cfg, "   \n").unwrap();
        let v = verify_target(&CLAUDE, Some(cfg));
        assert!(!v.is_sound());
        assert!(
            v.issues.iter().any(|i| i.contains("config is empty")),
            "{:?}",
            v.issues
        );
    }

    // The unreadable-config arm: the path EXISTS (so read_config's missing-file
    // early-Ok doesn't apply) but is a DIRECTORY → File::open + read_to_string
    // errors → the `Err(_)` arm pushes "config unreadable: <path>".
    #[test]
    fn verify_target_flags_an_unreadable_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("cfgdir");
        std::fs::create_dir_all(&dir).unwrap();
        let v = verify_target(&CLAUDE, Some(dir));
        assert!(!v.is_sound());
        assert!(
            v.issues.iter().any(|i| i.contains("config unreadable")),
            "{:?}",
            v.issues
        );
    }

    // The shim-EXISTS-but-not-executable arm (line 94 in mod.rs), distinct from
    // the missing-shim arm above. CODEX embeds an ABSOLUTE shim path (its hook
    // command is `PIXTUOID_SOURCE=codex '<abs>'` → ShimRef::Absolute), so a shim
    // file present with no exec bits trips `else if !is_executable(&p)`.
    #[cfg(unix)]
    #[test]
    fn verify_target_flags_a_non_executable_shim() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("config.toml");
        let shim = tmp.path().join("hook");
        std::fs::write(&shim, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o644)).unwrap();

        install_target(&CODEX, Some(cfg.clone()), Some(shim)).unwrap();
        let v = verify_target(&CODEX, Some(cfg));
        assert!(!v.is_sound());
        assert!(
            v.issues
                .iter()
                .any(|i| i.contains("shim binary not executable")),
            "{:?}",
            v.issues
        );
    }

    // INVARIANT (#387 — the code-artifact verify-coverage guard): `install_target`
    // ships a target's CODE via exactly two write paths — the merged `config_path`
    // (covered by `verify_schema`: opencode's plugin IS its config) and the
    // wholly-owned `extra_artifacts` dir (covered by verify_target's existence
    // stat, #332). A config can verify clean while the plugin FILES the runtime
    // actually loads are missing/clobbered — the silent-dead class doctor exists to
    // catch. This loops EVERY `extra_artifacts` target (today only OpenClaw) so a
    // future 3rd code-artifact target is AUTO-guarded: install → confirm sound →
    // delete the artifacts the target itself declares → assert verify goes
    // HARD-broken. A new code-shipping path added to `install_target` with no
    // matching check in `verify_target` fails this guard.
    #[test]
    fn verify_target_hard_flags_a_missing_code_artifact_for_every_extra_artifacts_target() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Isolate OpenClaw's artifacts under a temp home (never touch ~/.openclaw).
        let oc_home = tempfile::TempDir::new().unwrap();
        let _state = EnvVarOverride::set("OPENCLAW_STATE_DIR", oc_home.path());
        let exe = std::env::current_exe().unwrap();
        let mut covered = 0;
        for &t in target::TARGETS {
            let Some(make) = t.extra_artifacts else {
                continue;
            };
            let tmp = tempfile::TempDir::new().unwrap();
            let cfg = tmp.path().join("config");
            install_target(t, Some(cfg.clone()), Some(exe.clone())).unwrap();
            // A complete install (config + artifacts present) verifies sound.
            assert!(
                verify_target(t, Some(cfg.clone())).is_sound(),
                "{}: a fresh install must verify sound",
                t.name
            );
            // The wholly-owned artifacts vanish (manual delete / stale uninstall /
            // clobber) while the config still registers them — config sound,
            // artifacts gone. Locate them via the target's OWN declaration so this
            // stays generic over any future extra_artifacts target.
            for (p, _) in make(&exe).unwrap() {
                let _ = std::fs::remove_file(&p).or_else(|_| std::fs::remove_dir_all(&p));
            }
            let v = verify_target(t, Some(cfg));
            assert!(
                !v.is_sound()
                    && v.issues
                        .iter()
                        .any(|i| i.contains("plugin artifact missing")),
                "{}: a missing code artifact must be a HARD verify issue (the silent-dead \
                 invariant) — got {:?}",
                t.name,
                v.issues
            );
            covered += 1;
        }
        assert!(
            covered >= 1,
            "expected at least one extra_artifacts target (OpenClaw) — did the registry change?"
        );
    }

    #[test]
    fn verify_target_flags_a_missing_event() {
        // An older-pixtuoid install / an upstream schema change that orphaned an
        // event: hand-remove one registered event → "missing hook entries".
        let exe = std::env::current_exe().unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("settings.json");
        install_target(&CLAUDE, Some(cfg.clone()), Some(exe)).unwrap();
        let mut v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        v["hooks"].as_object_mut().unwrap().remove("SessionEnd");
        std::fs::write(&cfg, serde_json::to_string_pretty(&v).unwrap()).unwrap();
        let res = verify_target(&CLAUDE, Some(cfg));
        assert!(!res.is_sound());
        assert!(
            res.issues
                .iter()
                .any(|i| i.contains("missing hook entries") && i.contains("SessionEnd")),
            "{:?}",
            res.issues
        );
    }

    // The user's scenario: after a DISCONNECT (uninstall), the doctor/health
    // logic must NOT spuriously flag "broken". The protection is the `has_hooks`
    // gate every caller (diagnose / boot preflight) applies — pin it, AND prove
    // it's load-bearing (verify_target ALONE on an uninstalled config is broken).
    #[test]
    fn a_disconnected_source_is_gated_out_of_the_broken_check() {
        let exe = std::env::current_exe().unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("settings.json");
        install_target(&CLAUDE, Some(cfg.clone()), Some(exe)).unwrap();
        uninstall_target(&CLAUDE, Some(cfg.clone())).unwrap();
        let content = io::read_config(&cfg).unwrap();
        // The has_hooks gate: an uninstalled config reports no managed entries, so
        // diagnose/boot skip verify_target entirely → install = None → not broken.
        assert!(
            !(CLAUDE.merge_uninstall)(&content).unwrap().changed,
            "uninstalled config must report no managed hooks (the has_hooks gate)"
        );
        // Load-bearing: verify_target UNGATED on that same config IS 'broken'
        // (sentinel gone), so callers MUST gate on has_hooks — which they do.
        assert!(
            !verify_target(&CLAUDE, Some(cfg)).is_sound(),
            "ungated verify of an uninstalled config is broken — the gate is what protects it"
        );
    }

    #[test]
    fn verify_target_flags_codewhale_disabled() {
        // CodeWhale gates ALL hooks on [hooks].enabled — false-with-entries is a
        // true silent-dead the sentinel/event-set checks would miss.
        let exe = std::env::current_exe().unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("config.toml");
        install_target(&target::CODEWHALE, Some(cfg.clone()), Some(exe)).unwrap();
        let content = std::fs::read_to_string(&cfg)
            .unwrap()
            .replace("enabled = true", "enabled = false");
        std::fs::write(&cfg, content).unwrap();
        let v = verify_target(&target::CODEWHALE, Some(cfg));
        assert!(!v.is_sound());
        assert!(
            v.issues.iter().any(|i| i.contains("enabled = false")),
            "{:?}",
            v.issues
        );
    }
}
