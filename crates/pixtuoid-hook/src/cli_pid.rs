//! The spawning agent CLI's pid — the `_pid` field of the hook envelope, which
//! feeds the daemon's liveness watch and the TUI's focus-jump.
//!
//! On Unix that is plain `getppid`: a runner either direct-execs the shim
//! (Hermes, grok) or shells it through `sh -c`, which EXECs the command —
//! either way the shim's parent IS the CLI.
//!
//! Codex's and Reasonix's own hook runners shell via `cmd.exe /C` — that is
//! upstream's doing, not ours; `install::hook_cmd` writes the BARE form
//! precisely BECAUSE cmd will parse it — and a `subprocess` / `exec.Command`
//! shell call compiles to the same thing for the rest. The
//! interposed cmd.exe exits the moment the shim does, so a raw ppid names a
//! dead, soon-recycled process, which is why Windows used to send no pid at all
//! (#528). The channel here is an ancestor WALK instead: over ONE process
//! snapshot, skip the shells a hook runner interposes and stamp the first
//! ancestor that is the CLI itself.
//!
//! The list can stay short because being wrong degrades rather than misfires. A
//! runner interposing some OTHER shell leaves us stamping that transient pid,
//! and a dead pid owns no window, so the focus walk finds nothing to activate —
//! the pre-#528 silent no-op, not a wrong window. Over-skipping needs a CLI that
//! runs AS one of the listed shells; no shim-stamp source does today, and a
//! `pwsh`-hosted one would land on the terminal, which the focus walk would
//! have reached anyway.
//!
//! The walk and its vocabulary compile on EVERY platform (the `hook_cmd`
//! precedent) so the decision unit-tests on the machine this is developed on;
//! only the snapshot FFI is `cfg(windows)`, hence the module-wide dead-code
//! allowance off Windows.
#![cfg_attr(not(windows), allow(dead_code))]

/// One process-table row — the walk's whole input, so it reads a snapshot
/// rather than the OS once per hop.
struct ProcRow {
    pid: u32,
    parent: u32,
    /// The image NAME with no directory (Toolhelp32's `szExeFile`).
    exe: String,
}

/// The shells a Windows hook runner interposes between the CLI and the shim.
/// A CLI shipped as a `.cmd`/`.bat` wrapper adds another of the same.
///
/// By NAME because the snapshot carries nothing structural to key on: the
/// semantic signal (an ancestor whose command line holds OUR exe path) needs
/// `NtQueryInformationProcess` plus a cross-process read, and "skip an ancestor
/// created within N ms of us" — cheap now that `GetProcessTimes` sits next door
/// — skips the CLI itself at session_start, when it is milliseconds old.
const INTERPOSER_SHELLS: &[&str] = &["cmd.exe", "powershell.exe", "pwsh.exe"];

/// Hop ceiling for the walk. A real chain interposes one shell, two with a
/// `.cmd` wrapper — this is the terminator for a corrupt or racing snapshot
/// (whose rows can form a cycle), not a tuning knob.
const MAX_HOPS: usize = 8;

fn is_interposer(exe: &str) -> bool {
    INTERPOSER_SHELLS
        .iter()
        .any(|shell| exe.eq_ignore_ascii_case(shell))
}

/// Walk up from `start` (the shim) to the first ancestor that isn't an
/// interposed shell. `None` when the chain leaves the snapshot — an exited
/// parent is absent from it, though a RECYCLED pid is present and the walk
/// cannot tell — or outruns [`MAX_HOPS`].
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

/// Slice of the send bound the walk may spend. Derived from `WRITE_TIMEOUT`
/// rather than named separately so the two cannot drift: the walk is the one
/// step another process can make slow (an AV/EDR filter hooks Toolhelp32), and
/// it must not eat the budget the ENVELOPE needs. Losing `_pid` costs a focus
/// jump; losing the envelope costs the sprite, since for a mid-attached session
/// the hook is the only proof of life.
#[cfg(windows)]
const WALK_BUDGET: std::time::Duration = crate::transport::WRITE_TIMEOUT.checked_div(4).unwrap();

#[cfg(windows)]
pub(crate) fn cli_pid() -> Option<u32> {
    let (tx, rx) = std::sync::mpsc::channel();
    // Off-thread so a stalled snapshot is ABANDONED rather than waited on; the
    // orphan dies with the process. A spawn failure degrades to no pid, like
    // the watchdog's own `Builder::spawn`.
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

/// Every live process as `(pid, parent, image name)`, from ONE Toolhelp32
/// snapshot — a point-in-time view, so the whole walk sees one consistent tree
/// instead of racing a fresh snapshot per hop. Empty when it cannot be opened
/// or walked at all; a mid-enumeration failure truncates it instead, which the
/// walk reads the same way — no answer.
///
/// A SECOND Toolhelp32 enumeration on purpose: `focus::windows::OsProcessTable`
/// reads the same table, but this shim stays dependency-light (invariant #5) so
/// sharing would cost a crate. They differ deliberately — that one answers one
/// pid per call and re-snapshots per hop; this one snapshots once for the whole
/// walk. Fix a Toolhelp32 bug in BOTH.
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

    /// The exec-form chain (Hermes, and CC's Windows install): the CLI spawns
    /// the shim directly, so the parent already IS the answer.
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

    /// A CLI installed as a `.cmd` wrapper stacks a second shell, and the
    /// PowerShell runner spells its shell differently. Casing is the filesystem's
    /// to choose, not ours.
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

    /// A parent that exited leaves no row, and the shim must stamp nothing
    /// rather than a pid that may already belong to someone else.
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

    /// The real resolver, against the real process table: this test binary was
    /// spawned by the harness, not through a shell, so it must name a pid that
    /// is neither ours nor the root.
    #[test]
    fn the_live_resolver_names_a_real_ancestor() {
        let pid = cli_pid().expect("this process has a live parent");
        assert_ne!(pid, std::process::id(), "never our own pid");
        assert_ne!(pid, 0);
    }
}
