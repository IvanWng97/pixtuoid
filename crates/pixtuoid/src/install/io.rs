use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

/// The user's home dir — `USERPROFILE`-first on Windows (HOME is normally
/// unset there; Git Bash's exported HOME is POSIX-form and unusable), `HOME`
/// on Unix. `None` when nothing is set: call sites keep their own fallbacks.
/// An empty value counts as unset.
pub fn user_home() -> Option<String> {
    resolve_user_home(
        cfg!(windows),
        std::env::var("USERPROFILE").ok(),
        std::env::var("HOME").ok(),
    )
}

fn resolve_user_home(
    windows: bool,
    userprofile: Option<String>,
    home: Option<String>,
) -> Option<String> {
    let nonempty = |v: Option<String>| v.filter(|s| !s.is_empty());
    if windows {
        return nonempty(userprofile).or_else(|| nonempty(home));
    }
    nonempty(home)
}

/// Resolve a `$HOME`-relative path, falling back to the CWD when no home dir
/// is resolvable.
pub fn home_relative(rel: &str) -> PathBuf {
    let home = user_home().unwrap_or_else(|| ".".into());
    PathBuf::from(home).join(rel)
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

/// POSIX single-quote a string so a shell treats it as one literal token —
/// embedded single quotes become `'\''`. Codex and Reasonix both run the hook
/// `command` under a shell, so an unquoted path containing spaces would split
/// into multiple args and the hook would never be found. Unix-only: on Windows
/// these targets use `windows_bare_hook_command` (cmd.exe, not `sh`).
#[cfg(unix)]
pub(crate) fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Characters that are special to cmd.exe's command-line parser (so they're
/// unsafe in an UNQUOTED hook path): a space TRUNCATES the command; a
/// separator/redirect/escape/expansion char injects or redirects. (`!` is
/// deliberately excluded — it's special only under delayed expansion, `cmd
/// /V:ON`, which the codex/reasonix hook runners don't enable.)
const CMD_UNSAFE: &[char] = &[' ', '"', '&', '|', '<', '>', '(', ')', '^', '%'];

#[cfg_attr(not(windows), allow(dead_code))]
fn first_cmd_unsafe_char(p: &str) -> Option<char> {
    p.chars().find(|c| CMD_UNSAFE.contains(c))
}

/// The BARE Windows hook `command` for a CLI whose hook runner shells via
/// `cmd.exe /C` — Codex AND Reasonix both do (verified: codex-rs
/// `command_runner.rs`; reasonix `internal/hook/hook.go:414` `shellInvocation`).
/// Form: `<path> --source <name>`. The source rides as the shim's `--source`
/// flag because cmd.exe has no `VAR=value cmd` env-prefix and neither CLI injects
/// a per-hook env. The path is UNQUOTED: a quoted path can't survive `cmd /C`
/// (the host's argv-quoting escapes `"`→`\"`, which cmd then mangles), so cmd
/// PARSES the path — meaning any cmd-special char in it would truncate (`space`,
/// #195) or inject (`& | < > ( ) ^ %` — `C:\Users\a&b\h.exe --source x` splits on
/// `&` and cmd runs the relative tail from the CWD). When the resolved path has
/// such a char we substitute its DOS 8.3 SHORT name (`C:\PROGRA~1\…`, which is
/// space- and metacharacter-free by construction) and only REJECT if the short
/// name is unavailable (8.3 generation disabled on the volume). One place for
/// both targets so the guard can't drift.
#[cfg(windows)]
pub(crate) fn windows_bare_hook_command(resolved_path: &str, source: &str) -> Result<String> {
    resolve_windows_command(resolved_path, source, short_path_windows)
}

/// Pure decision core, with the 8.3 resolver injected so every branch is testable
/// without the Win32 FFI (and on any OS).
#[cfg_attr(not(windows), allow(dead_code))]
fn resolve_windows_command(
    path: &str,
    source: &str,
    short_path: impl FnOnce(&str) -> Option<String>,
) -> Result<String> {
    // Defense-in-depth: `source` is interpolated into the command alongside the
    // path. It's a hardcoded "codex"/"reasonix" at every call site today, but this
    // is a general guard — screen it for the same cmd-unsafe chars rather than
    // trust the caller, so the command string can never be made injectable here.
    if let Some(bad) = first_cmd_unsafe_char(source) {
        anyhow::bail!(
            "internal: hook source name {source:?} contains a cmd-unsafe character {bad:?}"
        );
    }
    let Some(bad) = first_cmd_unsafe_char(path) else {
        return Ok(format!("{path} --source {source}"));
    };
    // Try the DOS 8.3 short form — space/metacharacter-free by construction.
    // (When 8.3 generation is disabled on the volume, GetShortPathNameW returns
    // the long path unchanged, so we re-check and fall through to the reject.)
    if let Some(s) = short_path(path) {
        if first_cmd_unsafe_char(&s).is_none() {
            return Ok(format!("{s} --source {source}"));
        }
    }
    anyhow::bail!(
        "pixtuoid-hook is at a path containing {bad:?} ({path}) that the cmd.exe /C hook \
         runner can't safely invoke, and no DOS 8.3 short name is available (8.3 \
         generation is disabled on this volume). Install pixtuoid to a path of ordinary \
         characters (e.g. %USERPROFILE%\\.cargo\\bin or the npm global prefix) and re-run \
         `install-hooks`. (Tracking: #195.)"
    );
}

/// The DOS 8.3 short path for an EXISTING path via `GetShortPathNameW` (two-call
/// length-then-fill pattern). Returns `None` on any failure — a missing path, or
/// a volume with 8.3 generation disabled (then the API yields the long path,
/// which the caller re-checks).
#[cfg(windows)]
fn short_path_windows(long: &str) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;

    let wide: Vec<u16> = std::ffi::OsStr::new(long)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is a valid NUL-terminated UTF-16 string. Passing a null
    // out-buffer with length 0 is the documented "return required size (incl. NUL)"
    // probe; it writes nothing and returns 0 on failure.
    let needed = unsafe { GetShortPathNameW(wide.as_ptr(), std::ptr::null_mut(), 0) };
    if needed == 0 {
        return None;
    }
    let mut buf = vec![0u16; needed as usize];
    // SAFETY: `buf` has `needed` u16 slots; the API writes at most `needed-1` chars
    // plus a NUL and returns the count written (excl. NUL), or 0 on failure.
    let written = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), needed) };
    if written == 0 || written >= needed {
        return None;
    }
    String::from_utf16(&buf[..written as usize]).ok()
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
    // UFCS onto fs4's trait (not std's inherent File::try_lock, stable only in
    // 1.89) so the advisory lock keeps working on the declared MSRV (1.78).
    fs4::FileExt::try_lock(&lock)
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
    rename_with_retry(&tmp, &target)?;
    fs4::FileExt::unlock(&lock).ok();
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
        // resolve_user_home is pure — the Windows arm is testable on macOS.
        assert_eq!(resolve_user_home(true, None, None), None);
        assert_eq!(resolve_user_home(false, None, None), None);
    }

    #[test]
    fn user_home_userprofile_wins_on_windows_home_wins_on_unix() {
        let up = Some(r"C:\Users\me".to_string());
        let posix = Some("/c/Users/me".to_string());
        assert_eq!(resolve_user_home(true, up.clone(), posix.clone()), up);
        assert_eq!(
            resolve_user_home(false, up, Some("/Users/me".into())),
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

    // The 8.3 decision logic, with the short-path resolver injected so every
    // branch runs on any OS (no FFI). The real GetShortPathNameW is smoke-tested
    // separately below on windows-test.
    #[test]
    fn windows_command_is_bare_for_a_clean_path() {
        let c = resolve_windows_command(r"C:\tools\pixtuoid-hook.exe", "codex", |_| {
            panic!("short_path must NOT be called for a clean path")
        });
        assert_eq!(c.unwrap(), r"C:\tools\pixtuoid-hook.exe --source codex");
    }

    #[test]
    fn windows_command_uses_8dot3_short_form_when_path_has_a_space() {
        let c =
            resolve_windows_command(r"C:\Program Files\x\pixtuoid-hook.exe", "reasonix", |_| {
                Some(r"C:\PROGRA~1\x\PIXTUO~1.EXE".to_string())
            });
        assert_eq!(c.unwrap(), r"C:\PROGRA~1\x\PIXTUO~1.EXE --source reasonix");
    }

    #[test]
    fn windows_command_rejects_when_8dot3_is_unavailable() {
        // 8.3 disabled → resolver returns the long (still-unsafe) path → reject.
        let long = resolve_windows_command(r"C:\Program Files\x\h.exe", "codex", |p| {
            Some(p.to_string())
        });
        assert!(long.is_err());
        // resolver fails outright (missing path) → reject.
        let none = resolve_windows_command(r"C:\a&b\h.exe", "codex", |_| None);
        let err = none.unwrap_err().to_string();
        assert!(
            err.contains("cmd.exe") && err.contains("ordinary characters"),
            "reject message must stay actionable: {err}"
        );
    }

    #[test]
    fn windows_command_rejects_a_cmd_unsafe_source() {
        // `source` is interpolated too — a metacharacter-bearing source is rejected
        // even with a perfectly clean path (defense-in-depth; never injectable here).
        let c = resolve_windows_command(r"C:\tools\hook.exe", "co&dex", |_| {
            panic!("must reject on source before touching the short-path resolver")
        });
        assert!(c.unwrap_err().to_string().contains("source name"));
    }

    // Smoke-test the real FFI: for an EXISTING dir it returns Some(non-empty),
    // whether or not 8.3 is enabled (disabled → the long path, still Some). Pins
    // that the two-call length-then-fill pattern doesn't panic / mis-size.
    #[cfg(windows)]
    #[test]
    fn short_path_windows_resolves_an_existing_dir() {
        let tmp = std::env::temp_dir();
        let got = short_path_windows(&tmp.to_string_lossy());
        assert!(
            got.is_some_and(|s| !s.is_empty()),
            "GetShortPathNameW must resolve an existing dir to a non-empty path"
        );
    }
}
