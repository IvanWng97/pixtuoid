//! Linux focus glue: `/proc/<pid>/stat` for the ancestor walk; window
//! ownership and activation ride ONE channel per environment — sway's IPC
//! when `$SWAYSOCK` is present (`swaymsg`), hyprland's when
//! `$HYPRLAND_INSTANCE_SIGNATURE` is (`hyprctl`), else the X11/EWMH
//! `_NET_ACTIVE_WINDOW` protocol (the wmctrl mechanism) via x11rb — which
//! covers i3 too (X11-native; `$I3SOCK` deliberately does NOT route to
//! `swaymsg`). `focusable` asks the SAME channel "does this pid own a
//! window?" (compositor tree / `_NET_WM_PID`), so the walk surfaces the
//! terminal emulator — the agent process itself owns no surface. GNOME
//! Wayland forbids focus-steal by design — every channel simply fails there
//! → the caller's silent no-op, per the ONE failure rule.
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
        if std::env::var_os("SWAYSOCK").is_some() {
            return tree_lists_pid("swaymsg", &["-t", "get_tree"], pid);
        }
        if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
            return tree_lists_pid("hyprctl", &["clients", "-j"], pid);
        }
        x11_window_of(pid).is_some()
    }
}

/// Whether the compositor's JSON tree (`swaymsg -t get_tree` / `hyprctl
/// clients -j`) lists a node with `"pid": <pid>` — the IPC answer to "does
/// this pid own a window". Full serde parse + recursive scan rather than a
/// substring match: both tools vary pretty-vs-compact output by tty.
fn tree_lists_pid(cmd: &str, args: &[&str], pid: i32) -> bool {
    let Ok(out) = std::process::Command::new(cmd).args(args).output() else {
        return false;
    };
    serde_json::from_slice::<serde_json::Value>(&out.stdout)
        .is_ok_and(|v| json_tree_has_pid(&v, i64::from(pid)))
}

fn json_tree_has_pid(v: &serde_json::Value, pid: i64) -> bool {
    match v {
        serde_json::Value::Object(m) => {
            m.get("pid").and_then(serde_json::Value::as_i64) == Some(pid)
                || m.values().any(|c| json_tree_has_pid(c, pid))
        }
        serde_json::Value::Array(a) => a.iter().any(|c| json_tree_has_pid(c, pid)),
        _ => false,
    }
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

/// Activate `pid`'s window on the same channel `focusable` matched it on:
/// sway/hyprland IPC (pid-addressed) when the env marker is present, else
/// EWMH `_NET_ACTIVE_WINDOW`.
pub(crate) fn activate_os(pid: i32) -> bool {
    if std::env::var_os("SWAYSOCK").is_some() {
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
