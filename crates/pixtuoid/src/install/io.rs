use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use fs2::FileExt;

/// Resolve a `$HOME`-relative path, falling back to the CWD when `HOME` is unset.
pub fn home_relative(rel: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(rel)
}

pub fn default_hook_binary() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("PIXTUOID_HOOK") {
        return Ok(PathBuf::from(p));
    }
    if let Ok(p) = which::which("pixtuoid-hook") {
        return Ok(p);
    }
    let exe = std::env::current_exe().context("current_exe")?;
    let dir = exe.parent().ok_or_else(|| anyhow!("exe has no parent"))?;
    let candidate = dir.join("pixtuoid-hook");
    if candidate.exists() {
        return Ok(candidate);
    }
    Err(anyhow!("could not locate pixtuoid-hook; pass --hook-path"))
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

/// Atomic write that follows symlinks: write a temp file beside the resolved
/// target, fsync, then rename onto it. Advisory-locked. Format-agnostic (&str).
pub fn write_config_atomic(path: &Path, contents: &str) -> Result<()> {
    let target = resolve_symlink(path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock_path = sibling(&target, "lock");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    lock.try_lock_exclusive()
        .map_err(|e| anyhow!("could not lock {}: {e}", lock_path.display()))?;

    let tmp = sibling(&target, "tmp");
    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &target)?;
    fs2::FileExt::unlock(&lock).ok();
    Ok(())
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

    #[test]
    fn resolve_symlink_follows_single_hop() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("real.json");
        std::fs::write(&target, "{}").unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(resolve_symlink(&link), target);
    }

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

    #[test]
    fn resolve_symlink_dangling_returns_target() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("nonexistent.json");
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(resolve_symlink(&link), target);
    }

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
}
