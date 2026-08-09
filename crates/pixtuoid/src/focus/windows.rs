//! Windows focus glue: Toolhelp32 snapshot for the ancestor walk +
//! `EnumWindows`/`GetWindowThreadProcessId` for the focusable test +
//! `SetForegroundWindow` for activation. Zero permissions.
//!
//! A console app owns no window and never receives the click, so
//! `SetForegroundWindow` normally refuses it and flashes the taskbar instead.
//! [`activate_os`] borrows the foreground thread's input state to get the right.
//! Why not the better-known bypasses, and what the attach costs: the binary
//! guide's foreground-lock sharp edge.

use windows_sys::Win32::Foundation::{
    CloseHandle, FALSE, HWND, INVALID_HANDLE_VALUE, LPARAM, TRUE,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    PeekMessageW, SetForegroundWindow, ShowWindow, MSG, PM_NOREMOVE, SW_RESTORE, WM_USER,
};

use super::ProcessTable;

pub(crate) struct OsProcessTable;

impl ProcessTable for OsProcessTable {
    /// Re-snapshots per hop (`ancestor_walk` asks one pid at a time). The shim's
    /// `cli_pid::process_snapshot` is the deliberate second copy — fix in BOTH.
    fn ppid(&self, pid: i32) -> Option<i32> {
        // SAFETY: Toolhelp32 snapshot enumeration per its documented protocol;
        // the entry struct is plain-old-data and owned by us.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut found = None;
            if Process32FirstW(snap, &mut entry) != 0 {
                loop {
                    if entry.th32ProcessID == pid as u32 {
                        found = i32::try_from(entry.th32ParentProcessID).ok();
                        break;
                    }
                    if Process32NextW(snap, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
            found
        }
    }

    fn focusable(&self, pid: i32) -> bool {
        top_level_window_of(pid).is_some()
    }
}

/// The first visible top-level window owned by `pid`, via `EnumWindows`.
fn top_level_window_of(pid: i32) -> Option<HWND> {
    struct Search {
        pid: u32,
        hwnd: Option<HWND>,
    }
    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> i32 {
        // SAFETY: lparam is the &mut Search we passed below, alive for the call.
        let search = unsafe { &mut *(lparam as *mut Search) };
        let mut owner = 0u32;
        // SAFETY: hwnd comes from EnumWindows; owner is our own out-param.
        unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
        if owner == search.pid && unsafe { IsWindowVisible(hwnd) } != 0 {
            search.hwnd = Some(hwnd);
            return 0; // stop enumeration
        }
        1
    }
    let mut search = Search {
        pid: pid as u32,
        hwnd: None,
    };
    // SAFETY: the callback contract above; Search outlives the call.
    unsafe { EnumWindows(Some(cb), &mut search as *mut _ as LPARAM) };
    search.hwnd
}

/// Bring `pid`'s top-level window to the foreground; `false` on a denial.
/// Unattached first (all the `floating` painter needs — it IS the foreground
/// process), then one retry under a borrowed input state.
pub(crate) fn activate_os(pid: i32) -> bool {
    let Some(hwnd) = top_level_window_of(pid) else {
        return false;
    };
    // A minimized window can become the foreground window and stay an icon.
    // SAFETY: hwnd is a live handle from the enumeration above.
    if unsafe { IsIconic(hwnd) } != 0 {
        unsafe { ShowWindow(hwnd, SW_RESTORE) };
    }
    if raise(hwnd) {
        return true;
    }
    let Some(foreground) = foreground_input_thread() else {
        return false;
    };
    let Some(_attached) = AttachedInput::to(foreground) else {
        return false;
    };
    raise(hwnd)
}

/// One attempt, judged by OBSERVATION: MSDN says nothing about the flash-only
/// outcome and the call sets no error, so only the foreground window afterwards
/// separates a raise from a flash. A wrong `false` costs one extra attempt.
fn raise(hwnd: HWND) -> bool {
    // SAFETY: hwnd is a live window handle from the enumeration above;
    // GetForegroundWindow takes no arguments.
    unsafe {
        SetForegroundWindow(hwnd);
        GetForegroundWindow() == hwnd
    }
}

/// The thread owning the foreground window. `None` when there is none (nothing
/// to borrow, no lock to beat) or when it is ours (a self-attach is rejected).
fn foreground_input_thread() -> Option<u32> {
    // SAFETY: both take no arguments; `owner` is our own out-param and `fg` is
    // whatever handle the system reports, which the null check screens.
    let fg = unsafe { GetForegroundWindow() };
    if fg.is_null() {
        return None;
    }
    let mut owner = 0u32;
    // SAFETY: `fg` is a live top-level window; `owner` is ours to write.
    let thread = unsafe { GetWindowThreadProcessId(fg, &mut owner) };
    // SAFETY: takes no arguments and cannot fail.
    let me = unsafe { GetCurrentThreadId() };
    (thread != 0 && thread != me).then_some(thread)
}

/// Self-undoing input attachment: it serializes both threads' input until
/// detached, so `Drop` owns that rather than an early return.
struct AttachedInput(u32);

impl AttachedInput {
    fn to(target: u32) -> Option<Self> {
        ensure_message_queue();
        // SAFETY: both ids name live threads on this desktop; the pair is
        // detached by the Drop below.
        let attached = unsafe { AttachThreadInput(GetCurrentThreadId(), target, TRUE) };
        (attached != 0).then_some(Self(target))
    }
}

impl Drop for AttachedInput {
    fn drop(&mut self) {
        // SAFETY: undoing exactly the attachment `to` made, from the same
        // thread — a `Self` never crosses one, being a local of `activate_os`.
        unsafe { AttachThreadInput(GetCurrentThreadId(), self.0, FALSE) };
    }
}

/// `AttachThreadInput` fails outright without a message queue, and a console
/// thread may never have minted one. MSDN disagrees with itself on which calls
/// mint it, so prime it rather than trust the `EnumWindows` above.
fn ensure_message_queue() {
    // SAFETY: all-zero bytes are a valid MSG; PeekMessage writes only that
    // buffer, and a null hwnd with PM_NOREMOVE is the documented no-op probe.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        PeekMessageW(
            &mut msg,
            std::ptr::null_mut(),
            WM_USER,
            WM_USER,
            PM_NOREMOVE,
        );
    }
}
