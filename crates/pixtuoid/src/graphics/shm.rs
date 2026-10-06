//! Pixels handed to a terminal on this host through POSIX shared memory, the
//! kitty graphics protocol's `t=s` medium
//! (<https://sw.kovidgoyal.net/kitty/graphics-protocol/>, "The transmission
//! medium"): the terminal "must read the data from the memory object and then
//! unlink and close it on POSIX". An object the terminal never reads — its
//! escape dropped, or this process gone first — outlives the process, so the
//! [`Ledger`] holds every name until it is unlinked, by the terminal or by us.

use std::collections::VecDeque;
use std::io;
use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How long an object waits for the terminal before we unlink it: a terminal
/// reads a frame's escapes long before the next frames are due, so one still
/// unread this late was dropped, and unlinking it costs at most a stale tile.
const READ_WITHIN: Duration = Duration::from_secs(2);

/// The most bytes unread objects hold at once, oldest unlinked first: a
/// terminal that never reads (a dropped passthrough) must not pile up
/// [`READ_WITHIN`] of whole frames.
const MAX_UNREAD_BYTES: usize = 256 << 20;

/// The names this process has published and not yet unlinked itself, oldest
/// first.
#[derive(Debug, Default)]
struct Ledger {
    held: VecDeque<Held>,
    bytes: usize,
}

#[derive(Debug)]
struct Held {
    name: String,
    at: Instant,
    len: usize,
}

impl Ledger {
    /// Record `name`, then unlink what is past [`READ_WITHIN`] at `now` or
    /// over [`MAX_UNREAD_BYTES`].
    fn hold(&mut self, held: Held, now: Instant) {
        self.bytes += held.len;
        self.held.push_back(held);
        while let Some(oldest) = self.held.front() {
            let late = now.saturating_duration_since(oldest.at) > READ_WITHIN;
            if !late && self.bytes <= MAX_UNREAD_BYTES {
                break;
            }
            self.drop_oldest();
        }
    }

    fn drop_oldest(&mut self) {
        if let Some(held) = self.held.pop_front() {
            self.bytes -= held.len;
            unlink(&held.name);
        }
    }
}

static LEDGER: Mutex<Ledger> = Mutex::new(Ledger {
    held: VecDeque::new(),
    bytes: 0,
});

fn ledger() -> std::sync::MutexGuard<'static, Ledger> {
    LEDGER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// This process's names: `/pxt`, its kitty id block (random per process, so
/// a dead process's leftovers never collide) and a sequence number, within
/// macOS's 31-byte limit on a name (`PSHMNAMLEN`).
fn next_name() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    format!(
        "/pxt{:x}-{:x}",
        super::kitty::process_base(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// A new object holding `bytes`, and its name; held in the ledger until the
/// terminal or [`READ_WITHIN`] unlinks it.
pub(crate) fn publish(bytes: &[u8], now: Instant) -> io::Result<String> {
    let name = next_name();
    let cname = std::ffi::CString::new(name.clone()).map_err(io::Error::other)?;
    // SAFETY: `cname` is a valid NUL-terminated name; the call creates a new
    // object (O_EXCL) or fails.
    let fd = unsafe {
        libc::shm_open(
            cname.as_ptr(),
            libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
            0o600 as libc::c_uint,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` was just opened above and nothing else owns it.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    if let Err(e) = fill(&fd, bytes) {
        unlink(&name);
        return Err(e);
    }
    ledger().hold(
        Held {
            name: name.clone(),
            at: now,
            len: bytes.len(),
        },
        now,
    );
    Ok(name)
}

/// Size the object to `bytes` and copy them in. An object takes no
/// `write(2)` on macOS, so through a mapping.
fn fill(fd: &OwnedFd, bytes: &[u8]) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let len = libc::off_t::try_from(bytes.len()).map_err(io::Error::other)?;
    // SAFETY: `fd` is an open shm object this process created.
    if unsafe { libc::ftruncate(fd.as_raw_fd(), len) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if bytes.is_empty() {
        return Ok(());
    }
    // SAFETY: maps `bytes.len()` bytes of the object just sized to that length.
    let map = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            bytes.len(),
            libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        )
    };
    if map == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `map` is a fresh writable mapping of `bytes.len()` bytes that
    // cannot overlap `bytes`; it is unmapped once, here.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), map.cast::<u8>(), bytes.len());
        libc::munmap(map, bytes.len());
    }
    Ok(())
}

/// Unlink `name`; one the terminal already unlinked is no error.
fn unlink(name: &str) {
    if let Ok(cname) = std::ffi::CString::new(name) {
        // SAFETY: `cname` is a valid NUL-terminated name.
        unsafe { libc::shm_unlink(cname.as_ptr()) };
    }
}

/// Unlink every object this process still holds: at teardown, and in the
/// unwind, so none outlives the process.
pub(crate) fn unlink_all() {
    let mut ledger = ledger();
    while !ledger.held.is_empty() {
        ledger.drop_oldest();
    }
}

/// What a terminal does with `name`: its first `len` bytes, then the object
/// unlinked; `None` when there is no such object.
#[cfg(test)]
pub(crate) fn read_and_unlink(name: &str, len: usize) -> Option<Vec<u8>> {
    let mut bytes = tests::read(name)?;
    unlink(name);
    bytes.truncate(len);
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The object's bytes as a reader on this host sees them, `None` once
    /// it is unlinked.
    pub(super) fn read(name: &str) -> Option<Vec<u8>> {
        let cname = std::ffi::CString::new(name).ok()?;
        // SAFETY: read-only open of a valid name.
        let fd = unsafe { libc::shm_open(cname.as_ptr(), libc::O_RDONLY, 0) };
        if fd < 0 {
            return None;
        }
        // SAFETY: `fd` was just opened above.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let file = std::fs::File::from(fd);
        let len = usize::try_from(file.metadata().ok()?.len()).ok()?;
        if len == 0 {
            return Some(Vec::new());
        }
        use std::os::fd::AsRawFd;
        // SAFETY: maps the object's whole length read-only.
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        assert_ne!(map, libc::MAP_FAILED);
        // SAFETY: `map` holds `len` readable bytes until the unmap below.
        let out = unsafe { std::slice::from_raw_parts(map.cast::<u8>(), len) }.to_vec();
        // SAFETY: unmaps the mapping made above, once.
        unsafe { libc::munmap(map, len) };
        Some(out)
    }

    #[test]
    fn a_published_object_holds_its_bytes_under_a_fresh_name() {
        let now = Instant::now();
        let a = publish(b"rgbrgb", now).expect("published");
        let b = publish(b"x", now).expect("published");
        assert_ne!(a, b);
        assert!(
            a.starts_with('/') && !a[1..].contains('/') && a.len() <= 31,
            "{a}"
        );
        // The stat size is a page multiple on some systems; the bytes lead it.
        assert!(read(&a).expect("readable").starts_with(b"rgbrgb"));
        unlink(&a);
        unlink(&b);
        assert_eq!(read(&a), None);
    }

    /// The ledger unlinks what the terminal leaves past its deadline, or past
    /// the byte cap, oldest first, and an unlink of an object the terminal
    /// already unlinked is no error.
    #[test]
    fn the_ledger_unlinks_what_is_left_unread() {
        let mut ledger = Ledger::default();
        let t0 = Instant::now();
        let first = publish(b"a", t0).expect("published");
        let second = publish(b"b", t0).expect("published");
        unlink(&second);
        // Held by a ledger of the test's own, so the global one stays clean.
        for name in [&first, &second] {
            ledger.hold(
                Held {
                    name: name.clone(),
                    at: t0,
                    len: 1,
                },
                t0,
            );
        }
        assert!(read(&first).is_some(), "unlinked before its deadline");
        let third = next_name();
        ledger.hold(
            Held {
                name: third,
                at: t0 + READ_WITHIN * 2,
                len: 1,
            },
            t0 + READ_WITHIN * 2,
        );
        assert_eq!(read(&first), None, "left past its deadline");
        assert_eq!(ledger.held.len(), 1);
        let huge = Held {
            name: next_name(),
            at: t0 + READ_WITHIN * 2,
            len: MAX_UNREAD_BYTES + 1,
        };
        ledger.hold(huge, t0 + READ_WITHIN * 2);
        assert!(ledger.held.is_empty() && ledger.bytes == 0, "over the cap");
    }

    #[test]
    fn unlink_all_leaves_nothing_behind() {
        let name = publish(b"left", Instant::now()).expect("published");
        unlink_all();
        assert_eq!(read(&name), None);
    }
}
