//! Windows focus glue: Toolhelp32 snapshot for the ancestor walk +
//! `EnumWindows`/`GetWindowThreadProcessId` for the focusable test +
//! `SetForegroundWindow` for activation. Zero permissions.
//!
//! Honesty note (from the plan review): a console app does not own its host
//! window — conhost/WindowsTerminal does — so `SetForegroundWindow` from this
//! process may be DENIED by the foreground lock; that's the caller's silent
//! no-op path, per the ONE failure rule (AttachThreadInput workaround =
//! backlog). codecov-ignored glue; behavior rides windows-test/dogfood.

use windows_sys::Win32::Foundation::{CloseHandle, HWND, INVALID_HANDLE_VALUE, LPARAM};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, IsWindowVisible, SetForegroundWindow,
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

/// Bring `pid`'s top-level window to the foreground. A foreground-lock denial
/// returns false — the caller's silent no-op.
pub(crate) fn activate_os(pid: i32) -> bool {
    let Some(hwnd) = top_level_window_of(pid) else {
        return false;
    };
    // SAFETY: hwnd is a live window handle from the enumeration above.
    unsafe { SetForegroundWindow(hwnd) != 0 }
}
