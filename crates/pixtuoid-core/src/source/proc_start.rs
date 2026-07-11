//! Kernel process-start MARKERS — the identity half of pid-recycle guards.
//!
//! [`pid_start_marker`] returns an opaque per-OS value that is stable for a
//! process's whole life and different for a recycled pid: macOS epoch seconds
//! (`proc_pidinfo(PROC_PIDTBSDINFO)` → `pbi_start_tvsec`), Linux clock ticks
//! since boot (`/proc/<pid>/stat` field 22, read RAW — equality needs no
//! boot-time/ticks-per-sec conversion, the blocker that kept #220's epoch
//! check macOS-only). The units DIFFER per OS: compare two markers from the
//! SAME machine for equality, never across hosts and never as wall-clock.
//! `None` on any failure (pid gone, EPERM, unsupported OS) — callers treat a
//! missing marker as "no identity check available", never an error.
//!
//! Two consumers: the hook plane stamps `(pid, marker)` at `_pid` peek time
//! (`PidIdentity`), and the binary's focus click re-reads the marker to
//! refuse a recycled pid (#527).

/// Opaque start marker for `pid`, or `None` when unreadable/unsupported.
pub fn pid_start_marker(pid: i32) -> Option<u64> {
    imp(pid)
}

#[cfg(target_os = "macos")]
fn imp(pid: i32) -> Option<u64> {
    // SAFETY: all-zero bytes are a valid value for this repr(C) plain-old-data
    // struct (integers + byte arrays only).
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: the buffer is exactly `size` bytes of a repr(C) struct matching
    // the macOS SDK's proc_bsdinfo layout (proc_info.h, ABI-stable since
    // 10.5), so the kernel fills only memory we own. PROC_PIDTBSDINFO returns
    // the full struct or <= 0 on failure.
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut std::ffi::c_void,
            size,
        )
    };
    if n != size {
        return None;
    }
    Some(info.pbi_start_tvsec)
}

#[cfg(target_os = "linux")]
fn imp(pid: i32) -> Option<u64> {
    // /proc/<pid>/stat field 22 is starttime; the comm field (2) can contain
    // spaces/parens, so count fields AFTER the last ')' (comm is field 2,
    // so starttime is the 20th token past it).
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = stat.rsplit_once(')')?.1;
    after.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn imp(_pid: i32) -> Option<u64> {
    None
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;

    #[test]
    fn marker_is_stable_for_a_live_process_and_none_after_it_dies() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn a child to mark");
        let pid = child.id() as i32;
        let first = pid_start_marker(pid).expect("a live child has a marker");
        let second = pid_start_marker(pid).expect("still alive");
        assert_eq!(first, second, "the marker never changes for one process");
        child.kill().expect("kill the child");
        child.wait().expect("reap so the pid leaves the table");
        // A reaped pid has no /proc entry / bsdinfo — the read fails to None.
        assert_eq!(pid_start_marker(pid), None);
    }

    #[test]
    fn own_process_has_a_marker() {
        assert!(pid_start_marker(std::process::id() as i32).is_some());
    }

    #[test]
    fn garbage_pid_is_none() {
        assert_eq!(pid_start_marker(-1), None);
        assert_eq!(pid_start_marker(i32::MAX), None);
    }
}
