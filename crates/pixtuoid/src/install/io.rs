use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

/// The user's home dir — `USERPROFILE`-first on Windows (HOME is normally
/// unset there; Git Bash's exported HOME is POSIX-form and unusable), `HOME`
/// on Unix. `None` when nothing is set: call sites keep their own fallbacks.
/// An empty value counts as unset.
///
/// Thin adapter over `pixtuoid_core::platform`: the USERPROFILE-vs-HOME rule
/// lives there (one authority, shared with core's `String`-returning
/// `user_home`); this just keeps the installer's `Option`/empty-is-unset
/// contract that the call sites here depend on.
pub fn user_home() -> Option<String> {
    pixtuoid_core::platform::user_home_opt()
}

/// Resolve a `$HOME`-relative path, falling back to the CWD when no home dir
/// is resolvable. Only safe for read-only PROBES (detection): a CWD-relative
/// existence check is at worst a false positive. WRITE paths must use
/// [`home_relative_checked`] — installing into `./.reasonix/...` produces a
/// file the CLI's global-scope loader never reads.
pub fn home_relative(rel: &str) -> PathBuf {
    let home = user_home().unwrap_or_else(|| ".".into());
    PathBuf::from(home).join(rel)
}

/// Resolve a `$HOME`-relative path, hard-erroring when no home dir is
/// resolvable (instead of `home_relative`'s CWD fallback).
pub fn home_relative_checked(rel: &str) -> Result<PathBuf> {
    checked_home_join(user_home(), rel)
}

fn checked_home_join(home: Option<String>, rel: &str) -> Result<PathBuf> {
    home.map(|h| PathBuf::from(h).join(rel)).ok_or_else(|| {
        anyhow!("cannot resolve the home directory (HOME/USERPROFILE unset); pass --config <path>")
    })
}

pub fn default_hook_binary() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("PIXTUOID_HOOK") {
        return Ok(PathBuf::from(p));
    }
    if let Ok(p) = which::which("pixtuoid-hook") {
        return Ok(p);
    }
    let exe = std::env::current_exe().context(
        "could not determine the running executable's path while locating pixtuoid-hook",
    )?;
    let dir = exe.parent().ok_or_else(|| anyhow!("exe has no parent"))?;
    let candidate = dir.join(hook_sibling_name());
    if candidate.exists() {
        return Ok(candidate);
    }
    Err(anyhow!("could not locate pixtuoid-hook; pass --hook-path"))
}

/// The hook binary's filename next to the running exe — `.exe`-suffixed on
/// Windows (exec-form spawning needs the real PE name; PATHEXT is a shell
/// behavior we must not rely on).
fn hook_sibling_name() -> String {
    format!("pixtuoid-hook{}", std::env::consts::EXE_SUFFIX)
}

/// Build a sibling path by APPENDING `.suffix` to the full filename — never
/// `with_extension`, which truncates at the last dot (corrupting `config.toml`
/// into `config.json.pixtuoid.bak` / `config.lock`).
fn sibling(target: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}.{}", target.display(), suffix))
}

/// Read raw config content, following symlinks. Returns "" for a missing or
/// empty file — the target's parser supplies the empty-document default.
pub fn read_config(path: &Path) -> Result<String> {
    let target = resolve_symlink(path);
    if !target.exists() {
        return Ok(String::new());
    }
    let mut s = String::new();
    File::open(&target)?.read_to_string(&mut s)?;
    Ok(s)
}

/// Rename `from` onto `to`, with a Windows-only bounded retry.
///
/// On Windows, `fs::rename` onto a file that another process holds open raises
/// ERROR_SHARING_VIOLATION (os error 32). Claude Code keeps `settings.json`
/// open briefly, so a bare rename can lose the write. Up to 3 attempts with
/// 50 ms sleeps between them match CC's typical hold duration; on the third
/// failure the error propagates. On Unix the rename succeeds atomically even
/// while a reader holds the old fd, so a single attempt is correct there.
fn rename_with_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        const MAX_ATTEMPTS: u32 = 3;
        for attempt in 1..=MAX_ATTEMPTS {
            match std::fs::rename(from, to) {
                Ok(()) => return Ok(()),
                Err(e) if attempt < MAX_ATTEMPTS => {
                    // ERROR_SHARING_VIOLATION = os error 32.
                    // Sleep only on the retriable attempts; propagate on the last.
                    let _ = e; // silence unused-variable lint on non-windows
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(from, to)
    }
}

/// RAII guard over a config file's advisory lock. Holds the flock on the
/// sibling `<target>.lock` file for its whole lifetime, so a caller can cover
/// an entire read→merge→write round (taking it BEFORE the read closes the
/// lost-update window the write-only lock left open). The lock FILE is
/// deliberately never unlinked: unlock-then-unlink lets a waiter holding the
/// old inode and a newcomer creating a fresh one both "hold" the lock.
///
/// Residual: an external writer (Claude Code rewriting its own settings.json)
/// can't honor this lock — it only serializes pixtuoid against pixtuoid.
#[derive(Debug)]
pub struct ConfigLock {
    /// The symlink-resolved real target — writes go here, never the symlink.
    target: PathBuf,
    file: File,
}

/// Acquire the advisory lock for `path`'s config file, resolving symlinks
/// first (invariant #4) so the lock lives beside the REAL target. FAIL on
/// contention rather than block — `try_lock` returns
/// `Err(TryLockError::WouldBlock)` when another install/uninstall holds it.
/// std-native advisory lock (stable since 1.89, our MSRV).
pub fn lock_config(path: &Path) -> Result<ConfigLock> {
    let target = resolve_symlink(path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock_path = sibling(&target, "lock");
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    file.try_lock()
        .map_err(|e| anyhow!("could not lock {}: {e}", lock_path.display()))?;
    Ok(ConfigLock { target, file })
}

impl ConfigLock {
    /// The symlink-resolved real config path this guard locks.
    pub fn target(&self) -> &Path {
        &self.target
    }

    /// Atomic write to the locked target: temp file beside it, fsync, then
    /// rename onto it. Writing through the guard (instead of re-calling
    /// `write_config_atomic`) is what avoids the same-process flock
    /// self-deadlock — a second open description on the same lock file
    /// conflicts even within one process.
    ///
    /// Permissions: the temp file is created 0600 on Unix and, when the
    /// target already exists, restated to the target's exact mode BEFORE any
    /// content is written — so a user-tightened settings.json (API keys) is
    /// never widened, and a fresh file defaults tight rather than
    /// umask-default. Windows is a no-op (ACLs inherit from the directory).
    pub fn write_atomic(&self, contents: &str) -> Result<()> {
        let tmp = sibling(&self.target, "tmp");
        {
            let mut opts = OpenOptions::new();
            opts.create(true).write(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(&tmp)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                // `mode(0o600)` only applies on CREATE — a stale tmp left by a
                // crashed run keeps its old mode, so restate explicitly.
                let perms = std::fs::metadata(&self.target)
                    .map(|m| m.permissions())
                    .unwrap_or_else(|_| std::fs::Permissions::from_mode(0o600));
                f.set_permissions(perms)?;
            }
            f.write_all(contents.as_bytes())?;
            f.sync_all()?;
        }
        rename_with_retry(&tmp, &self.target)?;
        Ok(())
    }
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Atomic write that follows symlinks: write a temp file beside the resolved
/// target, fsync, then rename onto it. Advisory-locked for the duration of
/// the write only — callers doing a read→merge→write round must instead take
/// [`lock_config`] before the read and write via [`ConfigLock::write_atomic`].
/// Format-agnostic (&str).
pub fn write_config_atomic(path: &Path, contents: &str) -> Result<()> {
    lock_config(path)?.write_atomic(contents)
}

pub fn backup_once(path: &Path, suffix: &str) -> Result<Option<PathBuf>> {
    let target = resolve_symlink(path);
    if !target.exists() {
        return Ok(None);
    }
    let bak = sibling(&target, suffix);
    if bak.exists() {
        return Ok(Some(bak));
    }
    std::fs::copy(&target, &bak)?;
    Ok(Some(bak))
}

pub fn remove_backup(path: &Path, suffix: &str) -> Result<Option<PathBuf>> {
    let target = resolve_symlink(path);
    let bak = sibling(&target, suffix);
    if !bak.exists() {
        return Ok(None);
    }
    std::fs::remove_file(&bak)?;
    Ok(Some(bak))
}

/// Whether the bare `pixtuoid-hook` name resolves on PATH. settings.json stores
/// the bare name for portability, and Claude Code spawns hooks via PATH — so if
/// this is false the installed hooks silently never fire.
pub fn hook_on_path() -> bool {
    which::which("pixtuoid-hook").is_ok()
}

/// Follow symlink chain to the final target, even if that target doesn't exist
/// yet (stow creates the link before the dotfiles repo is fully set up).
/// `canonicalize` fails on a dangling symlink, so we walk `read_link` manually.
pub fn resolve_symlink(path: &Path) -> PathBuf {
    let mut cur = path.to_path_buf();
    for _ in 0..32 {
        match std::fs::symlink_metadata(&cur) {
            Ok(meta) if meta.file_type().is_symlink() => match std::fs::read_link(&cur) {
                Ok(target) => {
                    cur = if target.is_relative() {
                        cur.parent().unwrap_or(Path::new(".")).join(&target)
                    } else {
                        target
                    };
                }
                Err(_) => return cur,
            },
            _ => return cur,
        }
    }
    cur
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // rename_with_retry: the retry loop's Windows sharing-violation path is not
    // cheaply testable cross-platform (triggering os error 32 requires another
    // process holding the file). The success path is tested here on all
    // platforms; the retry guard + WHY comment carry the Windows-specific
    // reasoning. The existing write_config_atomic tests exercise rename_with_retry
    // end-to-end on every platform (the non-windows branch is a direct rename).
    #[test]
    fn rename_with_retry_moves_file() {
        let dir = TempDir::new().unwrap();
        let from = dir.path().join("src.tmp");
        let to = dir.path().join("dst.json");
        std::fs::write(&from, "hello").unwrap();
        rename_with_retry(&from, &to).unwrap();
        assert!(!from.exists());
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "hello");
    }

    #[test]
    fn resolve_symlink_regular_file_returns_as_is() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("plain.json");
        std::fs::write(&file, "{}").unwrap();
        assert_eq!(resolve_symlink(&file), file);
    }

    #[test]
    fn resolve_symlink_nonexistent_returns_as_is() {
        let path = PathBuf::from("/tmp/pixtuoid-test-nonexistent-xyz");
        assert_eq!(resolve_symlink(&path), path);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_symlink_follows_single_hop() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("real.json");
        std::fs::write(&target, "{}").unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(resolve_symlink(&link), target);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_symlink_follows_chain() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("real.json");
        std::fs::write(&target, "{}").unwrap();
        let mid = dir.path().join("mid.json");
        std::os::unix::fs::symlink(&target, &mid).unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&mid, &link).unwrap();
        assert_eq!(resolve_symlink(&link), target);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_symlink_dangling_returns_target() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("nonexistent.json");
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(resolve_symlink(&link), target);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_symlink_relative_target() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let target = sub.join("real.json");
        std::fs::write(&target, "{}").unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(Path::new("sub/real.json"), &link).unwrap();
        let resolved = resolve_symlink(&link);
        assert_eq!(
            std::fs::canonicalize(&resolved).unwrap(),
            std::fs::canonicalize(&target).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_symlink_cycle_terminates_after_budget() {
        // A 2-node cycle a→b→a: symlink_metadata (lstat) + read_link (readlink)
        // both succeed on every hop without following, so the 32-hop budget is
        // exhausted and the loop falls through to `cur` (line 131) instead of
        // looping forever. The assertion is simply that it TERMINATES (and
        // returns one of the two cycle nodes).
        let dir = TempDir::new().unwrap();
        let a = dir.path().join("a.link");
        let b = dir.path().join("b.link");
        std::os::unix::fs::symlink(&b, &a).unwrap();
        std::os::unix::fs::symlink(&a, &b).unwrap();
        let resolved = resolve_symlink(&a);
        assert!(resolved == a || resolved == b, "got {resolved:?}");
    }

    #[test]
    fn read_config_missing_returns_empty_string() {
        let dir = TempDir::new().unwrap();
        assert_eq!(read_config(&dir.path().join("nope.json")).unwrap(), "");
    }

    #[test]
    fn read_config_empty_file_returns_empty_string() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("empty.json");
        std::fs::write(&p, "").unwrap();
        assert_eq!(read_config(&p).unwrap(), "");
    }

    #[test]
    fn read_config_returns_raw_content() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("c.toml");
        std::fs::write(&p, "a = 1\n").unwrap();
        assert_eq!(read_config(&p).unwrap(), "a = 1\n");
    }

    #[cfg(unix)]
    #[test]
    fn write_config_atomic_through_symlink_preserves_link() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("real.json");
        std::fs::write(&target, "{}").unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        write_config_atomic(&link, "{\"a\":1}").unwrap();
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "{\"a\":1}");
    }

    // --- ConfigLock: the read→merge→write guard (#7/#16) ------------------------

    #[test]
    fn lock_config_excludes_a_second_locker_until_dropped() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("settings.json");
        let guard = lock_config(&p).unwrap();
        let err = lock_config(&p).expect_err("a second lock on the same config must fail");
        assert!(err.to_string().contains("could not lock"), "got: {err:#}");
        drop(guard);
        lock_config(&p).expect("the lock is released when the guard drops");
    }

    #[test]
    fn write_atomic_under_a_held_guard_does_not_self_deadlock() {
        // The trap the old delegation idea fell into: flock conflicts across
        // separate open descriptions WITHIN one process, so a guard-holder
        // calling write_config_atomic would WouldBlock against itself. Writing
        // through the guard must succeed.
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("settings.json");
        let guard = lock_config(&p).unwrap();
        guard.write_atomic("{\"a\":1}").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{\"a\":1}");
    }

    #[cfg(unix)]
    #[test]
    fn lock_config_resolves_symlinks_to_the_real_target() {
        // Invariant #4: the lock must live beside the REAL file, so two
        // writers reaching it via different link paths still contend.
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("real.json");
        std::fs::write(&target, "{}").unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let guard = lock_config(&link).unwrap();
        assert_eq!(guard.target(), target);
        assert!(dir.path().join("real.json.lock").exists());
        assert!(lock_config(&target).is_err(), "same lock via either path");
    }

    // --- checked home resolution (#20) ------------------------------------------

    #[test]
    fn checked_home_join_errors_without_home() {
        let err = checked_home_join(None, ".reasonix/settings.json").unwrap_err();
        assert!(
            err.to_string().contains("--config"),
            "must point at the workaround: {err}"
        );
    }

    #[test]
    fn checked_home_join_joins_a_resolved_home() {
        assert_eq!(
            checked_home_join(Some("/home/u".into()), ".reasonix/settings.json").unwrap(),
            PathBuf::from("/home/u/.reasonix/settings.json")
        );
    }

    // --- permissions preservation (#6) -----------------------------------------

    #[cfg(unix)]
    #[test]
    fn write_config_atomic_preserves_target_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("settings.json");
        std::fs::write(&p, "{}").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();

        write_config_atomic(&p, "{\"a\":1}").unwrap();

        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "a user-tightened settings.json must not be widened by a rewrite"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_config_atomic_creates_new_files_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("settings.json");

        write_config_atomic(&p, "{}").unwrap();

        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "settings.json can carry API keys — a fresh file defaults tight"
        );
    }

    #[test]
    fn backup_and_lock_and_tmp_names_use_string_append() {
        // multi-dot filename must keep its full name + suffix (not with_extension truncation)
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("config.local.toml");
        std::fs::write(&p, "x = 1\n").unwrap();
        let bak = backup_once(&p, "pixtuoid.bak").unwrap().unwrap();
        assert_eq!(bak.file_name().unwrap(), "config.local.toml.pixtuoid.bak");
    }

    #[test]
    fn backup_once_idempotent_and_remove() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("settings.json");
        std::fs::write(&p, "{}").unwrap();
        let b1 = backup_once(&p, "pixtuoid.bak").unwrap().unwrap();
        assert_eq!(b1.file_name().unwrap(), "settings.json.pixtuoid.bak");
        let b2 = backup_once(&p, "pixtuoid.bak").unwrap().unwrap();
        assert_eq!(b1, b2);
        assert_eq!(remove_backup(&p, "pixtuoid.bak").unwrap(), Some(b1.clone()));
        assert!(!b1.exists());
        assert_eq!(remove_backup(&p, "pixtuoid.bak").unwrap(), None);
    }

    #[test]
    fn user_home_is_none_when_no_home_vars() {
        // The USERPROFILE-vs-HOME rule (and the Option/empty=unset contract this
        // adapter keeps) now lives in ONE place: pixtuoid_core::platform. Pin the
        // installer-relevant semantics against that pure seam — testable on macOS.
        use pixtuoid_core::platform::resolve_user_home_opt;
        assert_eq!(resolve_user_home_opt(true, None, None), None);
        assert_eq!(resolve_user_home_opt(false, None, None), None);
    }

    #[test]
    fn user_home_userprofile_wins_on_windows_home_wins_on_unix() {
        use pixtuoid_core::platform::resolve_user_home_opt;
        let up = Some(r"C:\Users\me".to_string());
        let posix = Some("/c/Users/me".to_string());
        assert_eq!(resolve_user_home_opt(true, up.clone(), posix.clone()), up);
        assert_eq!(
            resolve_user_home_opt(false, up, Some("/Users/me".into())),
            Some("/Users/me".into())
        );
    }

    #[test]
    fn default_hook_binary_sibling_appends_exe_suffix() {
        // Pin the per-platform LITERAL (not a re-computation via EXE_SUFFIX,
        // which would be tautological): catches a base-name typo or an
        // accidental double-suffix.
        #[cfg(unix)]
        assert_eq!(hook_sibling_name(), "pixtuoid-hook");
        #[cfg(windows)]
        assert_eq!(hook_sibling_name(), "pixtuoid-hook.exe");
    }
}
