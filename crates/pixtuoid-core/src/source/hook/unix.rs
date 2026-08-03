use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::net::UnixListener;
use tokio::sync::Semaphore;
use tracing::warn;

use crate::source::jsonl::FailureLatch;
use crate::source::TaggedSender;

use super::{handle_conn, CONN_TIMEOUT, MAX_CONCURRENT_CONNS};

/// First retry delay after an accept() error. mio/tokio only clear readiness
/// on EWOULDBLOCK, so a persistent accept errno (EMFILE/ENFILE while a shim
/// connection sits in the backlog) returns Ready(Err) on every await — an
/// unthrottled retry is a 100% CPU spin. 100ms is invisible next to the
/// shim's 200ms send bound.
const ACCEPT_BACKOFF_FIRST: Duration = Duration::from_millis(100);
/// Backoff ceiling: fd pressure can persist for minutes, but the daemon must
/// pick pending shim connections up promptly once fds free — 5s caps the
/// retry latency while keeping the error-loop duty cycle negligible.
const ACCEPT_BACKOFF_MAX: Duration = Duration::from_secs(5);

/// Bounded exponential backoff for the accept loop's Err arm: doubles from
/// [`ACCEPT_BACKOFF_FIRST`] to [`ACCEPT_BACKOFF_MAX`], reset by any
/// successful accept.
struct AcceptBackoff {
    next: Duration,
}

impl Default for AcceptBackoff {
    fn default() -> Self {
        Self {
            next: ACCEPT_BACKOFF_FIRST,
        }
    }
}

impl AcceptBackoff {
    /// The delay to sleep before retrying; advances the ladder.
    fn on_error(&mut self) -> Duration {
        let delay = self.next;
        self.next = (self.next * 2).min(ACCEPT_BACKOFF_MAX);
        delay
    }

    fn on_success(&mut self) {
        self.next = ACCEPT_BACKOFF_FIRST;
    }
}

/// Ensure the socket's parent is a private, us-owned `0700` directory BEFORE the
/// bind opens the lock / socket inside it — but ONLY for the `/tmp/pixtuoid-{uid}/`
/// no-XDG fallback we manage (#485). `XDG_RUNTIME_DIR` is systemd-managed 0700 and
/// an explicit `PIXTUOID_SOCKET` parent is the user's choice — neither is ours to
/// create or police, so both are left untouched (returns `Ok` without touching the
/// filesystem).
///
/// TOCTOU-safe: `mkdir(0700)` is an atomic create-if-absent. On `EEXIST` we `lstat`
/// (never follow) and require a real directory, owned by us, with no group/other
/// bits — a co-located user who pre-squatted `/tmp/pixtuoid-{uid}` (as a dir, file,
/// or symlink) fails the bind LOUDLY here instead of silently locking the hook plane
/// out. `/tmp`'s sticky bit stops another uid from renaming/replacing our directory
/// once it exists, so the validated `lstat` result can't be swapped before the lock
/// open. The create-side half of #485 — the shim closes the read side with a
/// connected-peer-uid check (`transport::peer_is_us`).
///
/// This fn is the one-line BINDER that substitutes the real `/tmp/pixtuoid-{uid}`
/// and our real uid into [`ensure_owned_socket_dir_in`]; all the decision logic
/// lives there so it is testable against a `TempDir`. Nothing can kill a mutation
/// of this line — driving it means creating/validating the REAL per-user socket
/// dir, which would race a live daemon — so it is excluded by function in
/// `.cargo/mutants.toml`. Keep it a pure substitution: any logic added HERE
/// becomes unmeasurable, which is exactly how the whole guard once sat unpinned
/// (a `-> Ok(())` mutation of the pre-split fn survived the entire suite).
#[cfg(unix)]
fn ensure_owned_socket_dir(path: &Path) -> Result<()> {
    ensure_owned_socket_dir_in(
        path,
        &owned_socket_dir(),
        rustix::process::getuid().as_raw(),
    )
}

/// The testable COMPOSITION behind [`ensure_owned_socket_dir`]: harden `owned`
/// only when `path` actually lives inside it, else no-op. Taking `owned` + `uid`
/// as parameters is what makes the FIRES direction reachable from a test — with
/// the real dir hardcoded, deleting the whole guard body was invisible to the
/// suite even though both halves it composes were themselves well covered. The
/// gap was never in a line you could read; it was in the wiring between them.
#[cfg(unix)]
fn ensure_owned_socket_dir_in(path: &Path, owned: &Path, uid: u32) -> Result<()> {
    if !is_owned_fallback_in(path, owned) {
        return Ok(());
    }
    ensure_private_dir(owned, uid)
}

/// The socket file name inside [`owned_socket_dir`]. Shared with
/// `ClaudeCodeSource::default_socket_path`'s branch 3 so the endpoint and the
/// guard below are built from the same two pieces.
#[cfg(unix)]
pub(crate) const SOCKET_FILE_NAME: &str = "pixtuoid.sock";

/// THE definition of the no-XDG `/tmp` fallback directory (#485). Both the
/// socket path (`ClaudeCodeSource::default_socket_path`'s branch 3) and the
/// ownership guard below derive from this one fn, so "is this the dir we own?"
/// is a comparison against the same value the path was built from — a second
/// hand-copied literal would let a change to the fallback form silently disarm
/// the guard while every test stayed green. (The SHIM keeps its own copy in
/// `pixtuoid-hook/src/paths.rs` — no dep edge is allowed between the two
/// crates — pinned to this one by `tests/socket_path_parity.rs`.)
#[cfg(unix)]
pub(crate) fn owned_socket_dir() -> std::path::PathBuf {
    let uid = rustix::process::getuid().as_raw();
    std::path::PathBuf::from(format!("/tmp/pixtuoid-{uid}"))
}

/// Whether `path` is the socket endpoint directly inside `owned` — the pure
/// decision half of [`ensure_owned_socket_dir_in`]. `owned` is a parameter so
/// the composition above can be driven against a `TempDir`; pass
/// [`owned_socket_dir`] to ask the production question.
#[cfg(unix)]
fn is_owned_fallback_in(path: &Path, owned: &Path) -> bool {
    path.parent() == Some(owned)
}

/// Create `dir` as a `0700` directory owned by `uid`, or — if it already exists —
/// validate it IS one, else error. The testable core of [`ensure_owned_socket_dir`]
/// (which supplies the real `/tmp/pixtuoid-{uid}` we must not touch under test).
#[cfg(unix)]
fn ensure_private_dir(dir: &Path, uid: u32) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // lstat (never follow): a planted symlink is caught as non-dir.
            let md = std::fs::symlink_metadata(dir)
                .with_context(|| format!("stat-ing hook socket dir {}", dir.display()))?;
            if !md.file_type().is_dir() {
                anyhow::bail!(
                    "hook socket dir {} exists but is not a directory (hostile squat) — \
                     refusing to bind",
                    dir.display()
                );
            }
            if md.uid() != uid {
                anyhow::bail!(
                    "hook socket dir {} is owned by uid {} not {} (hostile squat) — \
                     refusing to bind",
                    dir.display(),
                    md.uid(),
                    uid
                );
            }
            if md.mode() & 0o077 != 0 {
                anyhow::bail!(
                    "hook socket dir {} is group/other-accessible (mode {:o}) — \
                     refusing to bind",
                    dir.display(),
                    md.mode() & 0o7777
                );
            }
            Ok(())
        }
        Err(e) => Err(e).with_context(|| format!("creating hook socket dir {}", dir.display())),
    }
}

/// A sibling of the FINAL socket path, named `<sock>.<suffix>`.
///
/// Both siblings `bind` derives — the arbitration lock and the pre-rename temp
/// endpoint — MUST be built from the same base, because both bind branches have
/// to arbitrate on one lock file (see the lock's own note below). That was two
/// hand-copied copies of the same `file_name()` expression: precisely the drift
/// shape this file already warns about for the `#485` guard, where "a second
/// hand-copied literal" is named as the thing that disarms an invariant while
/// every test stays green. One fn, so the two cannot drift apart.
#[cfg(unix)]
fn socket_sibling(path: &Path, suffix: &str) -> std::path::PathBuf {
    path.with_file_name(format!(
        "{}.{suffix}",
        path.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default()
    ))
}

pub(super) struct Listener {
    listener: UnixListener,
    // Held (never unlocked) for the daemon's lifetime: the kernel releases
    // the advisory lock when the process dies, however abruptly, so the lock
    // — not the socket file, which nothing unlinks on exit/crash — is what
    // the next bind's liveness arbitration reads.
    _lock: std::fs::File,
}

impl Listener {
    pub(super) async fn bind(path: &Path) -> Result<Self> {
        // Harden the per-user `/tmp/pixtuoid-{uid}/` fallback dir (0700, us-owned)
        // BEFORE opening the lock / binding inside it (#485). A no-op for the
        // XDG / explicit-PIXTUOID_SOCKET parents.
        ensure_owned_socket_dir(path)?;
        // Liveness arbitration is an EXCLUSIVE advisory lock on a sibling
        // `<sock>.lock`, NOT connect() errnos: a backlog-saturated LIVE
        // daemon yields ECONNREFUSED on macOS (the kernel behavior the shim's
        // stalled-listener test documents) and EAGAIN on Linux, so an
        // errno-guessing probe can unlink a live daemon's socket — leaving it
        // accepting on an anonymous inode forever while every hook-borne
        // signal silently routes here. The lock file is NEVER unlinked
        // (unlock-then-unlink lets a waiter on the old inode and a newcomer
        // on a fresh one both "hold" it — same rule as install/io.rs's
        // ConfigLock) and is derived from the FINAL socket path so both bind
        // branches (temp-rename and the sun_path>100 fallback below — a
        // regular file has no sun_path cap) arbitrate on the same file.
        let lock_path = socket_sibling(path, "lock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            // O_NOFOLLOW: a symlink planted at `<sock>.lock` (the parent dir
            // may be a shared /tmp) must fail the open, not make the daemon
            // flock — and hold for its lifetime — an arbitrary file.
            .custom_flags(libc::O_NOFOLLOW)
            .open(&lock_path)
            .with_context(|| format!("opening hook socket lock at {}", lock_path.display()))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                // A live owner holds the lock. Typed so the CC source can
                // degrade to transcript-only instead of dying wholesale. This
                // also closes the old simultaneous-start rename TOCTOU: of
                // two racing first starts exactly one acquires the lock; the
                // loser degrades instead of leaving an anonymous listener.
                return Err(anyhow::Error::new(super::SocketBusy {
                    path: path.to_path_buf(),
                }));
            }
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(e)
                    .with_context(|| format!("locking hook socket at {}", lock_path.display()));
            }
        }
        if path.exists() {
            // Lock acquired ⇒ any previous lock-holding owner is dead ⇒ the
            // socket file is residue. Belt-and-braces probe before
            // reclaiming: a connect that SUCCEEDS — or backlogs (WouldBlock:
            // a full accept queue only happens on a live listener) — proves a
            // LIVE owner that predates the lock protocol (an older pixtuoid
            // mid-upgrade, or an arbitrary squatter); defer to it rather than
            // steal. Any OTHER connect error is NOT evidence of life — the
            // lock already arbitrated — so reclaim. Honest residual: a
            // lock-LESS live owner under a saturated backlog yields
            // ECONNREFUSED on macOS (not WouldBlock), so this probe still
            // steals from it — accepted, because the window only exists while
            // pre-lock daemons run (mixed-version upgrade) and ages out once
            // every daemon holds the lock.
            let alive = match tokio::net::UnixStream::connect(path).await {
                // Close immediately — the probe counts against the live
                // daemon's MAX_CONCURRENT_CONNS (its CONN_TIMEOUT bounds it
                // regardless).
                Ok(_stream) => true,
                Err(e) => e.kind() == std::io::ErrorKind::WouldBlock,
            };
            if alive {
                return Err(anyhow::Error::new(super::SocketBusy {
                    path: path.to_path_buf(),
                }));
            }
            let _ = tokio::fs::remove_file(path).await;
        }
        // Bind at a temp name, chmod to owner-only, then atomically rename
        // onto the final path (a rename doesn't disturb the listening inode).
        // The shim only ever connects to the FINAL path, so the socket is
        // never reachable there with looser-than-0600 modes — without
        // touching the process-global umask, which raced every other tokio
        // worker's concurrent file creation (e.g. a JsonlWatcher's
        // create_dir_all) for the duration of the bind.
        let tmp = socket_sibling(path, &format!("{}.tmp", std::process::id()));
        // sun_path caps at 104 bytes (macOS; 108 Linux). A custom
        // PIXTUOID_SOCKET whose FINAL path fits but whose `.<pid>.tmp` twin
        // doesn't must not fail the bind — fall back to a direct bind +
        // chmod at the final name, re-accepting the micro-TOCTOU (pre-chmod
        // window) the temp-rename dance exists to avoid.
        if tmp.as_os_str().len() > 100 {
            let listener = UnixListener::bind(path)
                .with_context(|| format!("binding hook socket at {}", path.display()))?;
            tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .await
                .with_context(|| format!("restricting hook socket mode at {}", path.display()))?;
            return Ok(Self {
                listener,
                _lock: lock,
            });
        }
        // A leftover temp can only be ours-by-name from a crashed prior run
        // that had this very pid — never a live socket.
        let _ = tokio::fs::remove_file(&tmp).await;
        let listener = UnixListener::bind(&tmp)
            .with_context(|| format!("binding hook socket at {}", tmp.display()))?;
        if let Err(e) =
            tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).await
        {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e)
                .with_context(|| format!("restricting hook socket mode at {}", tmp.display()));
        }
        if let Err(e) = tokio::fs::rename(&tmp, path).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e).with_context(|| {
                format!(
                    "moving hook socket into place at {} (from {})",
                    path.display(),
                    tmp.display()
                )
            });
        }
        Ok(Self {
            listener,
            _lock: lock,
        })
    }

    pub(super) async fn run(
        self,
        tx: TaggedSender,
        pid_watch: Option<super::HookPidWatch>,
        presence_tx: Option<super::PresenceSender>,
    ) -> Result<()> {
        let sem = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNS));
        let mut backoff = AcceptBackoff::default();
        let mut accept_health = FailureLatch::default();
        loop {
            let permit = match Arc::clone(&sem).acquire_owned().await {
                Ok(p) => p,
                Err(_) => {
                    anyhow::bail!("hook socket semaphore closed unexpectedly");
                }
            };
            match self.listener.accept().await {
                Ok((stream, _addr)) => {
                    if accept_health.on_success() {
                        tracing::info!("hook socket accepting connections again");
                    }
                    backoff.on_success();
                    let tx = tx.clone();
                    let pid_watch = pid_watch.clone();
                    let presence_tx = presence_tx.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        let _ = tokio::time::timeout(
                            CONN_TIMEOUT,
                            handle_conn(stream, tx, pid_watch, presence_tx),
                        )
                        .await;
                    });
                }
                Err(e) => {
                    // A Unix accept error leaves the listener fd valid, so
                    // retrying is right (contrast the Windows twin, which
                    // must recreate-or-bail) — just not at CPU speed, and not
                    // one warn per iteration: a persistent errno (the EMFILE
                    // class) would otherwise peg a core and rotate real
                    // diagnostics out of the warn-floor log.
                    if accept_health.on_failure() {
                        warn!("hook socket accept error (retrying with backoff): {e}");
                    }
                    tokio::time::sleep(backoff.on_error()).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_backoff_doubles_to_the_cap_and_resets_on_success() {
        let mut b = AcceptBackoff::default();
        assert_eq!(b.on_error(), ACCEPT_BACKOFF_FIRST);
        assert_eq!(b.on_error(), ACCEPT_BACKOFF_FIRST * 2);
        let mut last = Duration::ZERO;
        for _ in 0..16 {
            last = b.on_error();
        }
        assert_eq!(
            last, ACCEPT_BACKOFF_MAX,
            "the ladder must cap, not overflow"
        );
        b.on_success();
        assert_eq!(
            b.on_error(),
            ACCEPT_BACKOFF_FIRST,
            "a successful accept must reset the ladder"
        );
    }

    // --- #485: private-dir hardening ---
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    fn my_uid() -> u32 {
        rustix::process::getuid().as_raw()
    }

    #[test]
    fn ensure_private_dir_creates_a_fresh_0700_dir_owned_by_us() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let dir = tmp.path().join("pixtuoid-sockdir");
        ensure_private_dir(&dir, my_uid()).expect("fresh create must succeed");
        let md = std::fs::symlink_metadata(&dir).expect("stat");
        assert!(md.is_dir());
        assert_eq!(md.mode() & 0o777, 0o700, "must be created private");
        assert_eq!(md.uid(), my_uid());
    }

    #[test]
    fn ensure_private_dir_accepts_an_existing_owned_0700_dir_idempotently() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let dir = tmp.path().join("d");
        ensure_private_dir(&dir, my_uid()).expect("first create");
        // Second call hits the EEXIST validate path — must still pass.
        ensure_private_dir(&dir, my_uid()).expect("re-validate an owned 0700 dir");
    }

    #[test]
    fn ensure_private_dir_rejects_a_symlink() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let target = tmp.path().join("real");
        std::fs::create_dir(&target).expect("mkdir target");
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        assert!(
            ensure_private_dir(&link, my_uid()).is_err(),
            "a symlink at the socket dir path is hostile (lstat catches it)"
        );
    }

    #[test]
    fn ensure_private_dir_rejects_a_regular_file() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let file = tmp.path().join("f");
        std::fs::write(&file, b"squat").expect("write file");
        assert!(
            ensure_private_dir(&file, my_uid()).is_err(),
            "a regular file squatting the dir path is hostile"
        );
    }

    #[test]
    fn ensure_private_dir_rejects_a_group_or_other_accessible_dir() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let dir = tmp.path().join("loose");
        std::fs::create_dir(&dir).expect("mkdir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert!(
            ensure_private_dir(&dir, my_uid()).is_err(),
            "a world/group-accessible dir must be rejected (mode & 0o077 != 0)"
        );
    }

    #[test]
    fn ensure_private_dir_rejects_a_dir_owned_by_another_uid() {
        // We own the dir; assert the fn refuses it when it expects a DIFFERENT
        // uid — the owned-by-another-uid branch, testable without privilege.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let dir = tmp.path().join("d");
        ensure_private_dir(&dir, my_uid()).expect("create as us");
        assert!(
            ensure_private_dir(&dir, my_uid().wrapping_add(1)).is_err(),
            "a dir owned by a uid other than the expected one is hostile"
        );
    }

    /// The EEXIST match guard must not swallow OTHER create errors into the
    /// validate branch. `DirBuilder` is non-recursive here, so a missing PARENT
    /// fails ENOENT — with the guard widened to `true` this lands in the validate
    /// arm, whose `symlink_metadata` then fails NotFound and misattributes a
    /// create failure to a stat failure.
    #[test]
    fn ensure_private_dir_reports_a_non_eexist_create_failure_as_a_create_failure() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let dir = tmp.path().join("missing-parent").join("d");
        let err = ensure_private_dir(&dir, my_uid()).expect_err("ENOENT must fail");
        assert!(
            format!("{err:#}").contains("creating hook socket dir"),
            "a non-EEXIST create error must be reported as a CREATE failure: {err:#}"
        );
    }

    /// The POSITIVE direction (#485): the guard is a path comparison, and a
    /// second hand-copied literal disarms it with the whole suite green — so ask
    /// about the REAL production endpoint, built from the same fn.
    #[test]
    fn is_owned_fallback_fires_for_the_dir_the_socket_path_is_built_from() {
        let owned = owned_socket_dir();
        assert!(is_owned_fallback_in(&owned.join(SOCKET_FILE_NAME), &owned));
        // Negative controls: a sibling dir and the flat pre-#485 form (the
        // exact shape whose reintroduction used to silently disarm it).
        assert!(!is_owned_fallback_in(
            Path::new("/tmp/pixtuoid.sock"),
            &owned
        ));
        assert!(!is_owned_fallback_in(&owned.with_extension("sock"), &owned));
    }

    /// An XDG-style / explicit-override socket path (parent is NOT the dir we
    /// manage) must not create or touch anything. Also the `delete !` pin from the
    /// OTHER side: an inverted guard hardens exactly the paths it should skip,
    /// which shows up here as the owned dir being created anyway.
    #[test]
    fn ensure_owned_socket_dir_in_is_a_noop_for_non_fallback_parents() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let owned = tmp.path().join("owned");
        let sock = tmp.path().join("elsewhere").join("pixtuoid.sock");
        ensure_owned_socket_dir_in(&sock, &owned, my_uid())
            .expect("non-fallback parent is a no-op");
        assert!(
            !sock.parent().expect("parent").exists(),
            "a non-fallback parent must not be created"
        );
        assert!(
            !owned.exists(),
            "a no-op must not create the managed dir either"
        );
    }

    // THE fires-direction pin. Until the owned dir became a parameter this was
    // unwritable — driving it meant creating the REAL `/tmp/pixtuoid-{uid}`,
    // racing any live daemon — so nothing drove it, and a 214-mutant run found
    // BOTH `ensure_owned_socket_dir -> Ok(())` and `delete !` surviving the
    // entire suite. `ensure_private_dir` had six tests and `is_owned_fallback_in`
    // three; the untested part was the line WIRING them together.
    #[test]
    fn ensure_owned_socket_dir_in_hardens_the_dir_the_socket_lives_in() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let owned = tmp.path().join("pixtuoid-42");
        let sock = owned.join(SOCKET_FILE_NAME);
        ensure_owned_socket_dir_in(&sock, &owned, my_uid()).expect("must harden");
        let md = std::fs::symlink_metadata(&owned).expect("the managed dir must now exist");
        assert!(md.is_dir());
        assert_eq!(md.mode() & 0o777, 0o700, "created private");
    }

    #[test]
    fn ensure_owned_socket_dir_in_refuses_a_squatted_dir_the_socket_lives_in() {
        // The whole POINT of the guard: a hostile pre-squat fails the bind
        // LOUDLY rather than silently locking the hook plane out.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let owned = tmp.path().join("squatted");
        std::fs::create_dir(&owned).expect("mkdir");
        std::fs::set_permissions(&owned, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert!(
            ensure_owned_socket_dir_in(&owned.join(SOCKET_FILE_NAME), &owned, my_uid()).is_err(),
            "a group/other-accessible dir at the socket's parent must refuse the bind"
        );
    }
}
