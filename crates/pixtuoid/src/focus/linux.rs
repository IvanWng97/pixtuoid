//! Linux focus glue: `/proc/<pid>/stat` for the ancestor walk; activation
//! prefers a tiling-WM IPC when its env marker is present (sway/i3's
//! `$SWAYSOCK`/`$I3SOCK` → `swaymsg`, hyprland's
//! `$HYPRLAND_INSTANCE_SIGNATURE` → `hyprctl`) and otherwise falls to the
//! X11/EWMH `_NET_ACTIVE_WINDOW` protocol (the wmctrl mechanism) via x11rb.
//! GNOME Wayland forbids focus-steal by design — every channel simply fails
//! there → the caller's silent no-op, per the ONE failure rule.
//!
//! codecov-ignored glue; the walk logic is tested in `focus::tests`.

use super::ProcessTable;

pub(crate) struct OsProcessTable;

impl ProcessTable for OsProcessTable {
    fn ppid(&self, pid: i32) -> Option<i32> {
        // /proc/<pid>/stat field 4 is ppid; the comm field (2) can contain
        // spaces/parens, so parse AFTER the last ')'.
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after = stat.rsplit_once(')')?.1;
        after.split_whitespace().nth(1)?.parse().ok()
    }

    fn focusable(&self, pid: i32) -> bool {
        // Under a tiling-WM IPC we can address windows BY pid directly, so
        // the walk's job is only "is this pid a window owner" — which the
        // IPC/X11 activation answers implicitly. Treat any pid that owns an
        // X11 window (per _NET_WM_PID) as focusable; under pure Wayland IPC
        // WMs the activate step matches by pid anyway, so accepting the first
        // ancestor that activation can address keeps this cheap: we probe
        // lazily by attempting only at activate time and let the walk surface
        // ancestors in order. Cheap conservative test: an X11 window exists.
        x11_window_of(pid).is_some() || wm_ipc_available()
    }
}

fn wm_ipc_available() -> bool {
    std::env::var_os("SWAYSOCK").is_some()
        || std::env::var_os("I3SOCK").is_some()
        || std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

/// Find an X11 window whose `_NET_WM_PID` matches, via x11rb.
fn x11_window_of(pid: i32) -> Option<u32> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots[screen_num].root;
    let net_client_list = conn
        .intern_atom(false, b"_NET_CLIENT_LIST")
        .ok()?
        .reply()
        .ok()?
        .atom;
    let net_wm_pid = conn
        .intern_atom(false, b"_NET_WM_PID")
        .ok()?
        .reply()
        .ok()?
        .atom;
    let clients = conn
        .get_property(false, root, net_client_list, AtomEnum::WINDOW, 0, u32::MAX)
        .ok()?
        .reply()
        .ok()?;
    for win in clients.value32()? {
        if let Ok(Ok(prop)) = conn
            .get_property(false, win, net_wm_pid, AtomEnum::CARDINAL, 0, 1)
            .map(|c| c.reply())
        {
            if prop.value32().and_then(|mut v| v.next()) == Some(pid as u32) {
                return Some(win);
            }
        }
    }
    None
}

/// Activate `pid`'s window: tiling-WM IPC first (zero-setup, pid-addressed),
/// else EWMH `_NET_ACTIVE_WINDOW`.
pub(crate) fn activate_os(pid: i32) -> bool {
    if std::env::var_os("SWAYSOCK").is_some() || std::env::var_os("I3SOCK").is_some() {
        return std::process::Command::new("swaymsg")
            .arg(format!("[pid={pid}] focus"))
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    }
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        return std::process::Command::new("hyprctl")
            .args(["dispatch", "focuswindow", &format!("pid:{pid}")])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    }
    x11_activate(pid).unwrap_or(false)
}

fn x11_activate(pid: i32) -> Option<bool> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt, EventMask};
    let win = x11_window_of(pid)?;
    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots[screen_num].root;
    let net_active = conn
        .intern_atom(false, b"_NET_ACTIVE_WINDOW")
        .ok()?
        .reply()
        .ok()?
        .atom;
    // Source indication 2 = a pager/direct user action (the wmctrl value).
    let ev = ClientMessageEvent::new(32, win, net_active, [2, 0, 0, 0, 0]);
    conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        ev,
    )
    .ok()?;
    conn.flush().ok()?;
    Some(true)
}
