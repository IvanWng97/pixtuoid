//! macOS focus glue: `proc_pidinfo(PROC_PIDTBSDINFO)` for the ancestor walk
//! (the same libproc family — and the same `proc_bsdinfo` struct — the core's
//! `cc_probe::pid_start_time_secs` reads) + `NSRunningApplication` for the
//! focusable test and activation. Zero TCC permissions: `activate()` is plain
//! Cocoa, not an Apple Event.
//!
//! codecov-ignored glue (needs a real GUI session — the `floating/window.rs`
//! class); the walk logic itself is tested in `focus::tests` on mock tables.

use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication};

use super::ProcessTable;

pub(crate) struct OsProcessTable;

impl ProcessTable for OsProcessTable {
    fn ppid(&self, pid: i32) -> Option<i32> {
        // SAFETY: all-zero bytes are a valid value for this repr(C)
        // plain-old-data struct (integers + byte arrays only).
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: the buffer is exactly `size` bytes of a repr(C) struct
        // matching the macOS SDK's proc_bsdinfo layout (proc_info.h,
        // ABI-stable since 10.5); the kernel fills only memory we own.
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
            return None; // gone / EPERM — the walk ends (silent no-op)
        }
        i32::try_from(info.pbi_ppid).ok()
    }

    fn focusable(&self, pid: i32) -> bool {
        // A REGULAR activation policy = a real Dock app (the terminal);
        // shells/daemons have no NSRunningApplication or are Prohibited.
        // SAFETY: plain Cocoa class-method calls on valid arguments; objc2's
        // 0.2-generation bindings mark every msg-send unsafe.
        unsafe {
            NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
                .is_some_and(|app| app.activationPolicy() == NSApplicationActivationPolicy::Regular)
        }
    }
}

/// Bring the app owning `pid` to the foreground. Returns whether macOS
/// accepted the request (a `false` is the caller's silent-no-op path).
pub(crate) fn activate_os(pid: i32) -> bool {
    // SAFETY: plain Cocoa calls on a valid pid; see `focusable`.
    unsafe {
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            .map(|app| {
                // ActivateIgnoringOtherApps is deprecated on 14+ but still
                // honored; the plain no-options activate() is "cooperative"
                // and drops the request while the user interacts elsewhere.
                #[allow(deprecated)]
                app.activateWithOptions(
                    objc2_app_kit::NSApplicationActivationOptions::NSApplicationActivateIgnoringOtherApps,
                )
            })
            .unwrap_or(false)
    }
}
