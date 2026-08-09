//! The spawning agent CLI's pid, for the hook envelope's `_pid`.
//!
//! Unix: `getppid` — the runner execs the shim, directly or through `sh -c`, so
//! the parent IS the CLI. Windows: several runners interpose a `cmd.exe /C`
//! that dies with the shim, so a raw ppid names a corpse (#528); walk past it
//! instead. Residuals are in `pixtuoid-core/CLAUDE.md`'s focus-jump section.
//!
//! The walk compiles everywhere so it unit-tests off Windows; only the snapshot
//! FFI is `cfg(windows)`, hence the dead-code allowance.
#![cfg_attr(not(windows), allow(dead_code))]

/// One process-table row — the walk's whole input.
struct ProcRow {
    pid: u32,
    parent: u32,
    /// The image NAME with no directory (Toolhelp32's `szExeFile`).
    exe: String,
}

/// By NAME because the snapshot carries nothing structural: matching the
/// ancestor's command line needs `NtQueryInformationProcess`, and "created
/// within N ms of us" skips the CLI itself at session_start.
const INTERPOSER_SHELLS: &[&str] = &["cmd.exe", "powershell.exe", "pwsh.exe"];

/// Terminator for a cyclic/corrupt snapshot, not a tuning knob (a real chain
/// interposes one shell, two with a `.cmd` wrapper).
const MAX_HOPS: usize = 8;

fn is_interposer(exe: &str) -> bool {
    INTERPOSER_SHELLS
        .iter()
        .any(|shell| exe.eq_ignore_ascii_case(shell))
}

/// First ancestor of `start` that isn't an interposed shell. `None` when the
/// chain leaves the snapshot or outruns [`MAX_HOPS`]. An exited parent is
/// absent from it; a RECYCLED one is present and indistinguishable.
fn first_cli_ancestor(start: u32, rows: &[ProcRow]) -> Option<u32> {
    let row_of = |pid: u32| rows.iter().find(|r| r.pid == pid);
    let mut pid = start;
    for _ in 0..MAX_HOPS {
        let parent = row_of(pid)?.parent;
        let parent_row = row_of(parent)?;
        if !is_interposer(&parent_row.exe) {
            return Some(parent);
        }
        pid = parent;
    }
    None
}

/// The CLI's pid, or `None` where this OS gives no trustworthy answer.
#[cfg(unix)]
pub(crate) fn cli_pid() -> Option<u32> {
    // Safety: getppid takes no args and is infallible.
    u32::try_from(unsafe { libc::getppid() }).ok()
}

/// The walk's own slice of the send bound — an AV/EDR filter can make a
/// Toolhelp32 snapshot slow, and losing `_pid` must not cost the ENVELOPE.
/// Derived so the two cannot drift.
#[cfg(windows)]
const WALK_BUDGET: std::time::Duration = crate::transport::WRITE_TIMEOUT.checked_div(4).unwrap();

#[cfg(windows)]
pub(crate) fn cli_pid() -> Option<u32> {
    let (tx, rx) = std::sync::mpsc::channel();
    // Off-thread so a stalled snapshot is abandoned, not waited on.
    std::thread::Builder::new()
        .spawn(move || {
            let _ = tx.send(walk_now());
        })
        .ok()?;
    rx.recv_timeout(WALK_BUDGET).ok().flatten()
}

#[cfg(windows)]
fn walk_now() -> Option<u32> {
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;

    // Safety: GetCurrentProcessId takes no args and cannot fail.
    let me = unsafe { GetCurrentProcessId() };
    first_cli_ancestor(me, &process_snapshot())
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn cli_pid() -> Option<u32> {
    None
}

/// Every live process, from ONE snapshot so the walk sees a consistent tree.
/// Empty or truncated on failure; the walk reads both as no answer.
///
/// A second Toolhelp32 reader on purpose — `focus::windows::OsProcessTable` is
/// the other, and the shim can't depend on that crate. Fix bugs in BOTH.
#[cfg(windows)]
fn process_snapshot() -> Vec<ProcRow> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut rows = Vec::new();
    // Safety: the entry is plain-old-data we own, sized as the API requires,
    // and the snapshot handle is closed on the one exit path below.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return rows;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut entry) != 0 {
            loop {
                rows.push(ProcRow {
                    pid: entry.th32ProcessID,
                    parent: entry.th32ParentProcessID,
                    exe: exe_name(&entry.szExeFile),
                });
                if Process32NextW(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    rows
}

/// `szExeFile` is a NUL-padded UTF-16 buffer; decode up to the first NUL.
#[cfg(windows)]
fn exe_name(raw: &[u16]) -> String {
    let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
    String::from_utf16_lossy(&raw[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: u32, parent: u32, exe: &str) -> ProcRow {
        ProcRow {
            pid,
            parent,
            exe: exe.into(),
        }
    }

    #[test]
    fn a_direct_parent_that_is_not_a_shell_is_the_cli() {
        let rows = [
            row(300, 200, "pixtuoid-hook.exe"),
            row(200, 100, "codex.exe"),
            row(100, 1, "WindowsTerminal.exe"),
        ];
        assert_eq!(first_cli_ancestor(300, &rows), Some(200));
    }

    /// The `cmd.exe /C` form — the trap this module exists for.
    #[test]
    fn the_transient_cmd_parent_is_skipped_for_the_cli_above_it() {
        let rows = [
            row(300, 250, "pixtuoid-hook.exe"),
            row(250, 200, "cmd.exe"),
            row(200, 100, "codex.exe"),
            row(100, 1, "WindowsTerminal.exe"),
        ];
        assert_eq!(first_cli_ancestor(300, &rows), Some(200));
    }

    /// A `.cmd` wrapper stacks a second shell; casing is the filesystem's.
    #[test]
    fn every_interposer_spelling_and_a_stacked_pair_are_skipped() {
        let rows = [
            row(300, 260, "pixtuoid-hook.exe"),
            row(260, 250, "CMD.EXE"),
            row(250, 200, "PowerShell.exe"),
            row(200, 100, "node.exe"),
            row(100, 1, "WindowsTerminal.exe"),
        ];
        assert_eq!(
            first_cli_ancestor(300, &rows),
            Some(200),
            "node.exe is the CLI — an interpreter name must not read as a shell"
        );
    }

    /// An exited parent leaves no row — stamp nothing, not someone else's pid.
    #[test]
    fn a_parent_missing_from_the_snapshot_is_no_answer() {
        let rows = [row(300, 250, "pixtuoid-hook.exe")];
        assert_eq!(first_cli_ancestor(300, &rows), None);
        assert_eq!(
            first_cli_ancestor(999, &rows),
            None,
            "our own row missing is no answer either"
        );
    }

    #[test]
    fn a_chain_of_nothing_but_shells_runs_out_rather_than_looping() {
        let all_shells: Vec<ProcRow> = (0..MAX_HOPS as u32 + 2)
            .map(|i| row(i, i + 1, "cmd.exe"))
            .collect();
        assert_eq!(first_cli_ancestor(0, &all_shells), None);

        let cycle = [row(300, 250, "cmd.exe"), row(250, 300, "cmd.exe")];
        assert_eq!(first_cli_ancestor(300, &cycle), None);
    }

    /// The real resolver against the real process table.
    #[test]
    fn the_live_resolver_names_a_real_ancestor() {
        let pid = cli_pid().expect("this process has a live parent");
        assert_ne!(pid, std::process::id(), "never our own pid");
        assert_ne!(pid, 0);
    }
}
