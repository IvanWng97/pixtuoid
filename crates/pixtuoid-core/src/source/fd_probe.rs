//! Process → open-file-descriptor enumeration, the OS half of the Codex
//! liveness probe (`codex::live_codex_rollout_ids`). A live `codex` process
//! holds its rollout `.jsonl` open in append mode for the whole session
//! (upstream `RolloutRecorder` owns the handle), so "which rollout files does
//! a running `codex` hold open" is a first-party liveness signal — no
//! registry, no PID file, no log-content reads.
//!
//! Platform split: macOS uses the libproc syscall wrappers (`proc_listallpids`
//! / `proc_name` / `proc_pidinfo(PROC_PIDLISTFDS)` / `proc_pidfdinfo`); Linux
//! reads `/proc/<pid>/{comm,fd}`. Everything else returns empty — the probe is
//! ADDITIVE-ONLY, so empty = today's pure-mtime first-sight gate.
//!
//! Contract: never panic, never block. Every failure path (process exited
//! mid-probe, EPERM, short read) skips the entry or returns empty; the probe
//! runs inside the watcher task on every scan pass.

use std::path::PathBuf;

/// PIDs of running processes whose (kernel-truncated) name equals `name`
/// exactly. Both kernels truncate the comparand — macOS `proc_name` and Linux
/// `comm` cap at well under 32 bytes — so only short names like `codex` can
/// ever match; that's the intended use.
///
/// `None` = the proc-table enumeration ITSELF failed (macOS
/// `proc_listallpids` sizing/fill <= 0; Linux `/proc` unreadable) — the
/// caller must treat the whole probe pass as failed (#223: an empty result
/// from a broken enumeration must not read as "every process exited").
/// `Some(vec![])` = the enumeration ran fine and nothing matched
/// (meaningful). Per-PID failures inside [`open_vnode_paths`] stay
/// non-failures — a pid vanishing mid-probe is normal.
pub(crate) fn pids_by_name(name: &str) -> Option<Vec<i32>> {
    imp::pids_by_name(name)
}

/// Filesystem paths of the regular-file (vnode) descriptors `pid` holds open.
/// A pid that exited, is unreadable (EPERM), or closes an fd between the list
/// and the per-fd query simply contributes nothing.
pub(crate) fn open_vnode_paths(pid: i32) -> Vec<PathBuf> {
    imp::open_vnode_paths(pid)
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::c_void;
    use std::path::PathBuf;

    // libc 0.2.x ships proc_fdinfo / vnode_info_path / PROC_PIDLISTFDS /
    // PROX_FDTYPE_VNODE and the four libproc fns (verified against the
    // vendored libc 0.2.186), but lacks ONLY the per-fd vnode-path flavor —
    // the constant and the two structs below, from <sys/proc_info.h>
    // (ABI-stable since 10.5).
    const PROC_PIDFDVNODEPATHINFO: libc::c_int = 2;

    #[repr(C)]
    #[allow(non_camel_case_types)]
    struct proc_fileinfo {
        fi_openflags: u32,
        fi_status: u32,
        fi_offset: i64,
        fi_type: i32,
        fi_guardflags: u32,
    }

    #[repr(C)]
    #[allow(non_camel_case_types)]
    struct vnode_fdinfowithpath {
        pfi: proc_fileinfo,
        pvip: libc::vnode_info_path,
    }

    pub(super) fn pids_by_name(name: &str) -> Option<Vec<i32>> {
        // Two-call sizing: a null buffer returns the current process count.
        // SAFETY: the null-buffer/0-size form is the documented sizing call —
        // nothing is written.
        let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        if count <= 0 {
            tracing::debug!("proc_listallpids sizing failed ({count}); probe pass failed");
            return None;
        }
        // Slack for processes spawned between the sizing call and the fill.
        const PID_LIST_SLACK: usize = 32;
        let cap = count as usize + PID_LIST_SLACK;
        let mut pids = vec![0 as libc::pid_t; cap];
        // SAFETY: `pids` owns exactly `cap` pid_t elements and `buffersize` is
        // exactly the matching byte length, so the kernel cannot write past
        // the allocation.
        let filled = unsafe {
            libc::proc_listallpids(
                pids.as_mut_ptr() as *mut c_void,
                (cap * std::mem::size_of::<libc::pid_t>()) as libc::c_int,
            )
        };
        if filled <= 0 {
            tracing::debug!("proc_listallpids fill failed ({filled}); probe pass failed");
            return None;
        }
        pids.truncate(usize::min(filled as usize, cap));
        pids.retain(|&pid| pid > 0 && process_name(pid).as_deref() == Some(name));
        Some(pids)
    }

    fn process_name(pid: libc::pid_t) -> Option<String> {
        let mut buf = [0u8; 64];
        // SAFETY: buffer length passed matches the allocation; proc_name
        // writes at most `buffersize` bytes and returns the length written
        // (<= 0 on failure — pid gone or unreadable, skipped by the caller).
        let n = unsafe { libc::proc_name(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
        if n <= 0 || n as usize > buf.len() {
            return None;
        }
        Some(String::from_utf8_lossy(&buf[..n as usize]).into_owned())
    }

    pub(super) fn open_vnode_paths(pid: i32) -> Vec<PathBuf> {
        let fdinfo_size = std::mem::size_of::<libc::proc_fdinfo>();
        // SAFETY: null-buffer sizing call — returns the byte size of the fd
        // list, writes nothing.
        let bytes =
            unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
        if bytes <= 0 {
            // Dead pid / EPERM — expected during any probe pass; skip quietly.
            return Vec::new();
        }
        // Slack for fds opened between the sizing call and the fill.
        let cap = bytes as usize / fdinfo_size + 8;
        let mut fds = vec![
            libc::proc_fdinfo {
                proc_fd: 0,
                proc_fdtype: 0,
            };
            cap
        ];
        // SAFETY: `fds` owns exactly `cap` proc_fdinfo entries and
        // `buffersize` is the matching byte length; proc_fdinfo is repr(C)
        // plain-old-data matching the SDK layout.
        let filled_bytes = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDLISTFDS,
                0,
                fds.as_mut_ptr() as *mut c_void,
                (cap * fdinfo_size) as libc::c_int,
            )
        };
        if filled_bytes <= 0 {
            return Vec::new();
        }
        fds.truncate(usize::min(filled_bytes as usize / fdinfo_size, cap));

        let mut out = Vec::new();
        for fd in &fds {
            if fd.proc_fdtype != libc::PROX_FDTYPE_VNODE as u32 {
                continue;
            }
            // SAFETY: all-zero bytes are a valid value for this repr(C)
            // plain-old-data struct (integers + byte arrays only).
            let mut info: vnode_fdinfowithpath = unsafe { std::mem::zeroed() };
            let size = std::mem::size_of::<vnode_fdinfowithpath>() as libc::c_int;
            // SAFETY: the buffer is exactly `size` bytes of a repr(C) struct
            // matching the macOS SDK's vnode_fdinfowithpath layout (proc_info.h,
            // ABI-stable since 10.5), so the kernel fills only memory we own.
            let n = unsafe {
                libc::proc_pidfdinfo(
                    pid,
                    fd.proc_fd,
                    PROC_PIDFDVNODEPATHINFO,
                    &mut info as *mut _ as *mut c_void,
                    size,
                )
            };
            if n != size {
                // fd closed between the list and this query (TOCTOU), or the
                // vnode has no resolvable path — skip this fd.
                continue;
            }
            // vip_path is [[c_char; 32]; 32] (libc's old-rustc stand-in for a
            // flat 1024 array); flatten and take the NUL-terminated prefix.
            let path_bytes: Vec<u8> = info
                .pvip
                .vip_path
                .iter()
                .flatten()
                .map(|&c| c as u8)
                .take_while(|&b| b != 0)
                .collect();
            if path_bytes.is_empty() {
                continue;
            }
            out.push(PathBuf::from(
                String::from_utf8_lossy(&path_bytes).into_owned(),
            ));
        }
        out
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::path::PathBuf;

    pub(super) fn pids_by_name(name: &str) -> Option<Vec<i32>> {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            tracing::debug!("/proc unreadable; probe pass failed");
            return None;
        };
        let mut pids = Vec::new();
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<i32>().ok())
            else {
                continue; // non-numeric /proc entries (self, sys, ...)
            };
            // A process exiting mid-scan makes the read fail — skip it.
            let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) else {
                continue;
            };
            // `pid > 0` for parity with the macOS arm: /proc never lists 0 or
            // negative pids, but a 0 leaking downstream would collide with the
            // kqueue wake slot's ident-0 on the macOS twin — keep both arms
            // structurally incapable of emitting it.
            if pid > 0 && comm.trim_end_matches('\n') == name {
                pids.push(pid);
            }
        }
        Some(pids)
    }

    pub(super) fn open_vnode_paths(pid: i32) -> Vec<PathBuf> {
        // Dead pid / EPERM → read_dir fails → empty, by design. A deleted
        // file's link ends with " (deleted)", which simply never matches a
        // rollout path — no stripping needed.
        let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|e| std::fs::read_link(e.path()).ok())
            .collect()
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod imp {
    use std::path::PathBuf;

    // No validated fd-enumeration path on this platform (Windows needs
    // NtQuerySystemInformation handle walks we haven't vetted). Some(empty) =
    // "ran fine, nothing matched": nothing is ever vouched here, so the
    // pure-mtime gate applies AND the negative vouch can never fire (an empty
    // prev_vouched set has nothing to lose) — Some keeps the un-gated
    // empty-result tests platform-uniform.
    pub(super) fn pids_by_name(_name: &str) -> Option<Vec<i32>> {
        Some(Vec::new())
    }

    pub(super) fn open_vnode_paths(_pid: i32) -> Vec<PathBuf> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercises the REAL enumeration (libproc FFI on macOS, /proc on Linux):
    /// this very test process holds a tempfile open, so its canonical path
    /// must appear among our own open vnodes.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn open_vnode_paths_sees_a_file_this_process_holds_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("held-open.txt");
        std::fs::write(&path, b"x").unwrap();
        let handle = std::fs::File::open(&path).unwrap();
        // Kernel-reported fd paths are fully resolved; compare canonical.
        let canonical = path.canonicalize().unwrap();
        let pid = std::process::id() as i32;
        let open = open_vnode_paths(pid);
        drop(handle);
        assert!(
            open.contains(&canonical),
            "expected {canonical:?} among this process's open vnodes, got {open:?}"
        );
    }

    #[test]
    fn open_vnode_paths_for_dead_pid_is_empty_not_panic() {
        // Far above any real pid range on macOS/Linux.
        assert!(open_vnode_paths(999_999_999).is_empty());
    }

    #[test]
    fn pids_by_name_for_nonexistent_process_is_some_empty() {
        // Longer than any kernel-truncated process name, so it can never
        // match. Some(empty) — the enumeration RAN and found nothing; None is
        // reserved for the proc table itself failing (#223: the distinction
        // is what keeps a broken enumeration from reading as "everything
        // exited").
        assert_eq!(
            pids_by_name("definitely-not-a-process-7q3z9"),
            Some(Vec::new())
        );
    }

    #[test]
    fn pids_by_name_returns_without_panic() {
        // Shape-only: whether a codex process is running is environmental.
        let _ = pids_by_name("codex");
    }

    /// Positive-path proof for the pid enumeration (the held-open test above
    /// only proves the fd half): a spawned `sleep` child must be found by its
    /// kernel-reported name. Guards the `proc_listallpids` two-call sizing
    /// semantics on macOS and the `/proc` comm scan on Linux.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn pids_by_name_finds_a_spawned_child() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        // A just-spawned child isn't always named in the proc-table at the first
        // probe (/proc/<pid>/comm materializes a beat after spawn) — poll ≤500ms.
        let found = (0..50).any(|_| {
            if pids_by_name("sleep")
                .expect("a healthy system's proc-table enumeration must succeed")
                .contains(&pid)
            {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            false
        });
        let _ = child.kill();
        let _ = child.wait();
        assert!(found, "expected spawned sleep (pid {pid}) in pids_by_name");
    }
}
