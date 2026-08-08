//! Windows focus glue: Toolhelp32 snapshot for the ancestor walk +
//! `EnumWindows`/`GetWindowThreadProcessId` for the focusable test +
//! `SetForegroundWindow` for activation. Zero permissions.
//!
//! A console app does not own its host window — conhost/WindowsTerminal does —
//! and the click asking for the jump is an input event THAT process received,
//! not ours. So none of the conditions the system grants `SetForegroundWindow`
//! on hold for us, and it refuses: per its Remarks an application "cannot force
//! a window to the foreground while the user is working with another window.
//! Instead, Windows flashes the taskbar button". [`activate_os`] therefore
//! borrows the foreground thread's input state (`AttachThreadInput`), which
//! satisfies the "calling process received the last input event" condition for
//! as long as the attachment lasts. Two documented bypasses are deliberately
//! NOT taken — see the binary guide's foreground-lock sharp edge.

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
/// Tries unattached first — that is the whole story when we ARE the foreground
/// process (the `floating` window painter) — then borrows the foreground
/// thread's input state for one retry.
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

/// One activation attempt, judged by OBSERVATION: `SetForegroundWindow`'s BOOL
/// is not a verdict we can act on, since the denial path raises no error and
/// merely flashes the taskbar button, so the truth is which window the system
/// reports as foreground afterwards. A wrong `false` costs one extra attempt.
fn raise(hwnd: HWND) -> bool {
    // SAFETY: hwnd is a live window handle from the enumeration above;
    // GetForegroundWindow takes no arguments.
    unsafe {
        SetForegroundWindow(hwnd);
        GetForegroundWindow() == hwnd
    }
}

/// The thread whose input state is worth borrowing — the one owning the current
/// foreground window. `None` when there is no foreground window (no lock exists
/// then, so the plain attempt already had the right) or when it is our own
/// thread (nothing to borrow, and a self-attach is rejected by definition).
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

/// An input attachment that undoes itself. `AttachThreadInput` serializes both
/// threads' input processing until it is called again with `FALSE`, so the
/// detach rides `Drop` — an early return must not be able to leave the terminal
/// sharing our input queue.
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

/// Give this thread a message queue, without which `AttachThreadInput` fails
/// outright — and a console TUI is exactly the thread that may not have one,
/// since threads are created without a queue and the system only mints it at
/// the first USER call. The docs disagree on which calls count (the
/// `AttachThreadInput` page says USER *or GDI*, the message-queue page says
/// only "specific user functions"), so prime it explicitly rather than trust
/// the `EnumWindows` above to have done it.
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
