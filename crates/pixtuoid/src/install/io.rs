use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

/// The ONE empty-as-unset filter for env values, trim-based: empty means
/// unset (the #172 RUST_LOG policy; the XDG basedir spec says the same), and
/// a whitespace-only value can never be the absolute path the env contracts
/// here require, so it counts as unset too. Used for `XDG_CONFIG_HOME`
/// (config.rs), `XDG_STATE_HOME` (main.rs log paths), and `PIXTUOID_HOOK` —
/// keep new env reads on this helper so the workspace has one semantics.
pub fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

/// [`nonempty`] over a live env read.
pub fn nonempty_env(name: &str) -> Option<String> {
    nonempty(std::env::var(name).ok())
}

/// Normalize a config-location env override (#342): TRIM it, and — when `home` is
/// `Some` — expand a leading `~`, `~/`, or `~\` against `home`, mirroring the CLIs
/// that home-expand their overrides (OpenClaw's `resolveRawHomeDir`/`resolveUserPath`
/// do exactly this `^~(?=$|[/\\])` replace). Pass `home: None` for CLIs that only
/// TRIM and never `~`-expand (CodeWhale, Reasonix) — expanding there would DIVERGE
/// from a CLI that takes the value verbatim. A value without a leading `~`-segment
/// (or `home: None`) is the trimmed verbatim path, so an absolute override is
/// untouched.
///
/// Returns a [`PathBuf`], NOT a `String`: paths stay in path-land end-to-end so
/// comparisons are STRUCTURAL (component-wise), never byte-wise on a `/`-vs-`\`
/// string — the recurring `windows-test` failure mode. The join also preserves the
/// home's `OsString` (no lossy round-trip).
pub(crate) fn expand_tilde(value: &str, home: Option<&Path>) -> PathBuf {
    let v = value.trim();
    match home {
        Some(home) if v == "~" => home.to_path_buf(),
        Some(home) => match v.strip_prefix("~/").or_else(|| v.strip_prefix("~\\")) {
            Some(rest) => home.join(rest),
            None => PathBuf::from(v),
        },
        None => PathBuf::from(v),
    }
}

/// Resolve a `$HOME`-relative path, falling back to the CWD when no home dir
/// is resolvable. Only safe for read-only PROBES (detection): a CWD-relative
/// existence check is at worst a false positive. WRITE paths must use
/// [`home_relative_checked`] — installing into `./.reasonix/...` produces a
/// file the CLI's global-scope loader never reads.
pub(crate) fn home_relative(rel: &str) -> PathBuf {
    let home = pixtuoid_core::platform::user_home_opt().unwrap_or_else(|| ".".into());
    PathBuf::from(home).join(rel)
}

/// Resolve a `$HOME`-relative path, hard-erroring when no home dir is
/// resolvable (instead of `home_relative`'s CWD fallback).
pub(crate) fn home_relative_checked(rel: &str) -> Result<PathBuf> {
    checked_home_join(pixtuoid_core::platform::user_home_opt(), rel)
}

fn checked_home_join(home: Option<String>, rel: &str) -> Result<PathBuf> {
    home.map(|h| PathBuf::from(h).join(rel)).ok_or_else(|| {
        anyhow!("cannot resolve the home directory (HOME/USERPROFILE unset); pass --config <path>")
    })
}

/// AUTO-locate `pixtuoid-hook`: PATH, then a sibling of the running exe —
/// both arms return absolute, verified-existing paths. The `PIXTUOID_HOOK`
/// env override is deliberately NOT read here: it is an explicit path like
/// `--hook-path` and is handled by `resolve_hook_binary`'s absolutize-and-warn
/// arm (returned verbatim from here, a relative value would get embedded into
/// Codex/Reasonix configs and silently never fire from other cwds).
pub(crate) fn default_hook_binary() -> Result<PathBuf> {
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
/// For a locked read→merge→write round use [`ConfigLock::read`] instead, so
/// the read shares the guard's pinned resolution.
pub(crate) fn read_config(path: &Path) -> Result<String> {
    read_resolved(&resolve_symlink(path))
}

/// `target` must already be symlink-resolved (or be a plain path).
fn read_resolved(target: &Path) -> Result<String> {
    if !target.exists() {
        return Ok(String::new());
    }
    let mut s = String::new();
    File::open(target)?.read_to_string(&mut s)?;
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
        const RENAME_RETRY_SLEEP_MS: u64 = 50;
        // ERROR_SHARING_VIOLATION = os error 32. The retriable attempts sleep
        // and loop; the FINAL attempt sits outside the loop so its result is
        // returned directly — no unreachable fall-through arm to maintain.
        for _ in 1..MAX_ATTEMPTS {
            match std::fs::rename(from, to) {
                Ok(()) => return Ok(()),
                Err(_) => {
                    std::thread::sleep(std::time::Duration::from_millis(RENAME_RETRY_SLEEP_MS))
                }
            }
        }
        std::fs::rename(from, to)
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(from, to)
    }
}

/// Create `path` for writing with the config-dir hardening every atomic write
/// under `install/` shares. On Unix, `O_NOFOLLOW | O_EXCL`: a symlink an attacker
/// with config-dir write pre-plants at the tmp name is NOT followed (that would
/// clobber the link's target with our bytes), and the create owns a fresh inode
/// rather than an adopted one. A pre-existing tmp — our own crash residue, or a
/// hostile symlink O_NOFOLLOW rejected — is reclaimed ONCE (`remove_file` unlinks
/// the entry itself, never following a symlink; the retry stays hardened). `mode`
/// is the Unix create mode; the caller restates perms afterward as needed. Windows
/// keeps plain create+truncate (its rename-retry path owns the sharing semantics).
/// The final rename onto the RESOLVED target still follows that target's own
/// symlink (invariant #4) — only the distinct, attacker-controllable tmp is guarded.
fn create_hardened_tmp(path: &Path, mode: u32) -> Result<File> {
    let mut opts = OpenOptions::new();
    opts.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(mode);
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_EXCL);
    }
    #[cfg(not(unix))]
    let _ = mode;
    match opts.open(path) {
        Ok(f) => Ok(f),
        #[cfg(unix)]
        Err(_) if path.symlink_metadata().is_ok() => {
            std::fs::remove_file(path)?;
            Ok(opts.open(path)?)
        }
        Err(e) => Err(e.into()),
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
pub(crate) struct ConfigLock {
    /// The symlink-resolved real target — writes go here, never the symlink.
    target: PathBuf,
    file: File,
}

/// Acquire the advisory lock for `path`'s config file, resolving symlinks
/// first (invariant #4) so the lock lives beside the REAL target. FAIL on
/// contention rather than block — `try_lock` returns
/// `Err(TryLockError::WouldBlock)` when another install/uninstall holds it.
/// std-native advisory lock (stable since 1.89, our MSRV).
pub(crate) fn lock_config(path: &Path) -> Result<ConfigLock> {
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
    pub(crate) fn target(&self) -> &Path {
        &self.target
    }

    /// Read the locked config (missing → "") through the guard's PINNED
    /// resolution — never re-resolving the symlink. A locked round's read,
    /// backup, and write must all address the ONE file the flock protects: a
    /// concurrent symlink retarget (e.g. `stow --restow` mid-install) would
    /// otherwise split them across two files — merge input from the NEW
    /// target, write onto the OLD — under a lock that excludes nobody at the
    /// new path.
    pub(crate) fn read(&self) -> Result<String> {
        read_resolved(&self.target)
    }

    /// [`backup_once_resolved`] against the pinned resolution (see [`Self::read`]).
    pub(crate) fn backup_once(&self, suffix: &str) -> Result<Option<PathBuf>> {
        backup_once_resolved(&self.target, suffix)
    }

    /// [`remove_backup_resolved`] against the pinned resolution (see [`Self::read`]).
    pub(crate) fn remove_backup(&self, suffix: &str) -> Result<Option<PathBuf>> {
        remove_backup_resolved(&self.target, suffix)
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
    ///
    /// Security: the tmp create is hardened (see [`create_hardened_tmp`] — a
    /// symlink pre-planted at `<target>.tmp` is not followed); the final rename
    /// simply lands on the already-resolved real target (invariant #4).
    pub(crate) fn write_atomic(&self, contents: &str) -> Result<()> {
        let tmp = sibling(&self.target, "tmp");
        {
            let mut f = create_hardened_tmp(&tmp, 0o600)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                // Match the TARGET's mode (the 0600 create is just the floor): a
                // user-tightened settings.json keeps its exact perms across the rename.
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
pub(crate) fn write_config_atomic(path: &Path, contents: &str) -> Result<()> {
    lock_config(path)?.write_atomic(contents)
}

// Path-based wrapper (resolve + backup) — production backs up through the
// held `ConfigLock` (already-resolved target); only tests want the combo.
#[cfg(test)]
fn backup_once(path: &Path, suffix: &str) -> Result<Option<PathBuf>> {
    backup_once_resolved(&resolve_symlink(path), suffix)
}

/// Take a ONE-TIME backup of the resolved target to `<target><suffix>` (skipped
/// if it already exists — the take-once latch trusts those bytes forever). Temp +
/// fsync + rename, NOT `fs::copy`: the .bak is the user's only recovery path, so a
/// crash mid-copy must leave it complete or absent, never a latched truncated
/// fragment. The tmp is hardened like `write_atomic`'s ([`create_hardened_tmp`] —
/// a symlink at `<bak>.tmp` is not followed, closing the same class as `.tmp`).
fn backup_once_resolved(target: &Path, suffix: &str) -> Result<Option<PathBuf>> {
    if !target.exists() {
        return Ok(None);
    }
    let bak = sibling(target, suffix);
    if bak.exists() {
        return Ok(Some(bak));
    }
    let tmp = sibling(&bak, "tmp");
    let mut dst = create_hardened_tmp(&tmp, 0o600)?;
    // Preserve the source's mode (fs::copy's old behavior: 0600 → 0600).
    if let Ok(m) = std::fs::metadata(target) {
        let _ = dst.set_permissions(m.permissions());
    }
    std::io::copy(&mut File::open(target)?, &mut dst)?;
    // Best-effort fsync of the owned write handle (no read-only re-open, which
    // Windows' FlushFileBuffers rejects); the rename is the atomicity.
    let _ = dst.sync_all();
    rename_with_retry(&tmp, &bak)?;
    Ok(Some(bak))
}

#[cfg(test)]
fn remove_backup(path: &Path, suffix: &str) -> Result<Option<PathBuf>> {
    remove_backup_resolved(&resolve_symlink(path), suffix)
}

fn remove_backup_resolved(target: &Path, suffix: &str) -> Result<Option<PathBuf>> {
    let bak = sibling(target, suffix);
    if !bak.exists() {
        return Ok(None);
    }
    std::fs::remove_file(&bak)?;
    Ok(Some(bak))
}

/// Whether the bare `pixtuoid-hook` name resolves on PATH. settings.json stores
/// the bare name for portability, and Claude Code spawns hooks via PATH — so if
/// this is false the installed hooks silently never fire.
pub(crate) fn hook_on_path() -> bool {
    which::which("pixtuoid-hook").is_ok()
}

/// Follow symlink chain to the final target, even if that target doesn't exist
/// yet (stow creates the link before the dotfiles repo is fully set up).
/// `canonicalize` fails on a dangling symlink, so we walk `read_link` manually.
pub(crate) fn resolve_symlink(path: &Path) -> PathBuf {
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

    #[test]
    fn expand_tilde_home_some_expands_leading_tilde_only() {
        let home = Path::new("/home/u");
        // PathBuf == PathBuf is STRUCTURAL: both sides built via `join` use the same
        // platform separator, so this is correct on Windows AND Unix — no hardcoded
        // `/` to drift from `\` (the windows-test trap).
        // bare `~` → the home itself.
        assert_eq!(expand_tilde("~", Some(home)), home.to_path_buf());
        // `~/x` and `~\x` (Windows form) → home-joined.
        assert_eq!(expand_tilde("~/claw", Some(home)), home.join("claw"));
        assert_eq!(expand_tilde(r"~\claw", Some(home)), home.join("claw"));
        // trims first, THEN expands.
        assert_eq!(expand_tilde("  ~/claw  ", Some(home)), home.join("claw"));
        // a leading `~` WITHOUT a separator (`~foo`) is NOT a home-prefix → verbatim
        // (matches OpenClaw's `^~(?=$|[/\\])` anchor).
        assert_eq!(expand_tilde("~foo", Some(home)), PathBuf::from("~foo"));
        // `~user/...` is ANOTHER user's home — we never resolve it → verbatim.
        assert_eq!(
            expand_tilde("~user/p", Some(home)),
            PathBuf::from("~user/p")
        );
        // a NON-leading `~` is never replaced.
        assert_eq!(
            expand_tilde("rel/~/x", Some(home)),
            PathBuf::from("rel/~/x")
        );
        // an absolute path is untouched (no leading `~`).
        assert_eq!(expand_tilde("/abs/x", Some(home)), PathBuf::from("/abs/x"));
    }

    #[test]
    fn expand_tilde_home_none_trims_only_never_expands() {
        // CodeWhale/Reasonix: trim, but a leading `~` stays VERBATIM (they don't
        // home-expand, so expanding would diverge from a verbatim-taking CLI).
        assert_eq!(expand_tilde("  /abs/x  ", None), PathBuf::from("/abs/x"));
        assert_eq!(expand_tilde("~/claw", None), PathBuf::from("~/claw"));
        assert_eq!(expand_tilde("~", None), PathBuf::from("~"));
    }

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

    #[cfg(unix)]
    #[test]
    fn write_atomic_refuses_a_symlink_planted_at_the_tmp() {
        // A symlink pre-planted at `<target>.tmp` must NOT be followed: the victim
        // keeps its content and the real target still receives the write.
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("settings.json");
        std::fs::write(&target, "{}").unwrap();
        let victim = dir.path().join("victim");
        std::fs::write(&victim, "PRECIOUS").unwrap();
        std::os::unix::fs::symlink(&victim, sibling(&target, "tmp")).unwrap();

        lock_config(&target)
            .unwrap()
            .write_atomic("{\"hooks\":{}}")
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(&victim).unwrap(),
            "PRECIOUS",
            "the planted symlink must not be followed — victim untouched"
        );
        assert!(
            std::fs::read_to_string(&target).unwrap().contains("hooks"),
            "the real target still receives the write (the .tmp symlink was reclaimed)"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_refuses_a_hardlink_planted_at_the_tmp() {
        // O_NOFOLLOW ignores HARDLINKS; O_EXCL is what stops an attacker's hardlink
        // at `<target>.tmp` from being opened+truncated through the shared inode.
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("settings.json");
        std::fs::write(&target, "{}").unwrap();
        let victim = dir.path().join("victim");
        std::fs::write(&victim, "PRECIOUS").unwrap();
        std::fs::hard_link(&victim, sibling(&target, "tmp")).unwrap();

        lock_config(&target)
            .unwrap()
            .write_atomic("{\"hooks\":{}}")
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(&victim).unwrap(),
            "PRECIOUS",
            "O_EXCL must reclaim the hardlink, not write through it — victim untouched"
        );
    }

    #[cfg(unix)]
    #[test]
    fn backup_refuses_a_symlink_planted_at_the_bak_tmp() {
        // The backup write is part of the config-write chokepoint: a symlink at
        // `<target>.bak.tmp` must not be followed either (the `.tmp` sibling class).
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("settings.json");
        std::fs::write(&target, "REAL").unwrap();
        let victim = dir.path().join("victim");
        std::fs::write(&victim, "PRECIOUS").unwrap();
        let bak = sibling(&target, "bak");
        std::os::unix::fs::symlink(&victim, sibling(&bak, "tmp")).unwrap();

        backup_once(&target, "bak").unwrap();

        assert_eq!(
            std::fs::read_to_string(&victim).unwrap(),
            "PRECIOUS",
            "the planted .bak.tmp symlink must not be followed — victim untouched"
        );
        assert_eq!(
            std::fs::read_to_string(&bak).unwrap(),
            "REAL",
            "the backup still captures the target's content"
        );
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

    #[test]
    fn lock_config_creates_missing_parent_dir() {
        // The .lock file lives beside the target, so a config under a not-yet-
        // existing parent (a fresh `~/.config/pixtuoid/` on first install) must
        // get its parent created before the lock open — else OpenOptions::open
        // errors NotFound. Every other lock_config test pre-creates the parent
        // (TempDir root), so this is the only one that fails if the
        // create_dir_all (or its `if let Some(parent)` guard) is dropped.
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("sub/nested");
        assert!(!parent.exists(), "precondition: parent must not exist yet");
        let p = parent.join("settings.json");
        let _guard = lock_config(&p).expect("lock_config must create the missing parent");
        assert!(parent.is_dir(), "the missing parent chain was created");
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

    #[cfg(unix)]
    #[test]
    fn config_lock_read_and_backup_pin_the_lock_time_resolution() {
        // A symlink retarget AFTER lock acquisition (a dotfiles tool swapping
        // ~/.claude/settings.json mid-install) must not split the round:
        // read, backup, and remove_backup all go through the guard's pinned
        // target — the file the flock and write_atomic address — never a
        // re-resolve of the link.
        let dir = TempDir::new().unwrap();
        let old = dir.path().join("old.json");
        std::fs::write(&old, "old-content").unwrap();
        let new = dir.path().join("new.json");
        std::fs::write(&new, "new-content").unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&old, &link).unwrap();

        let guard = lock_config(&link).unwrap();
        // The concurrent retarget lands inside the locked round.
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&new, &link).unwrap();

        assert_eq!(
            guard.read().unwrap(),
            "old-content",
            "the read is pinned to the lock-time target"
        );
        let bak = guard.backup_once("pixtuoid.bak").unwrap().unwrap();
        assert_eq!(
            bak,
            dir.path().join("old.json.pixtuoid.bak"),
            "the backup lands beside the lock-time target"
        );
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "old-content");
        assert_eq!(guard.remove_backup("pixtuoid.bak").unwrap(), Some(bak));
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
    fn backup_once_writes_via_temp_rename_and_survives_a_stale_tmp() {
        // The backup is "the user's only recovery path" (mod.rs uninstall arm)
        // and its take-once latch makes whatever lands at the .bak name
        // permanent — so the copy must be temp+rename atomic (a crash mid-copy
        // must never latch a truncated backup as the trusted snapshot).
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("settings.json");
        std::fs::write(&p, "{\"user\": \"content\"}").unwrap();
        // A stale tmp sidecar from a crashed earlier run must not poison the copy.
        let stale_tmp = dir.path().join("settings.json.pixtuoid.bak.tmp");
        std::fs::write(&stale_tmp, "torn garbage").unwrap();

        let bak = backup_once(&p, "pixtuoid.bak").unwrap().unwrap();
        assert_eq!(
            std::fs::read_to_string(&bak).unwrap(),
            "{\"user\": \"content\"}",
            "the latched backup is the complete snapshot"
        );
        assert!(
            !stale_tmp.exists(),
            "the tmp sidecar is consumed by the atomic rename"
        );
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
