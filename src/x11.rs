//! X11 window details, mirroring `macos.rs`: keep the card above everything without taking focus,
//! place it on the monitor under the pointer, fade it, and focus the terminal behind a session.
//! The window itself is created by winit as override-redirect (see `overlay.rs`); everything here
//! talks to the X server over a second, pure-Rust connection.

use crate::config::Position;
use crate::geom::{self, Rect};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use winit::dpi::PhysicalPosition;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, EventMask, PropMode,
    StackMode,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

/// Shared libraries winit's X11 backend loads at runtime. It panics (an abort here) when one is
/// missing, so check first and let the caller fall back to a notification instead.
const NEEDED_LIBS: &[&str] = &[
    "libX11.so.6",
    "libX11-xcb.so.1",
    "libXcursor.so.1",
    "libXi.so.6",
    "libXrandr.so.2",
    "libxkbcommon.so.0",
    "libxkbcommon-x11.so.0",
];

pub fn missing_libs() -> Vec<&'static str> {
    NEEDED_LIBS
        .iter()
        .copied()
        .filter(|name| {
            let c = std::ffi::CString::new(*name).unwrap();
            let h = unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_LAZY) };
            if !h.is_null() {
                unsafe { libc::dlclose(h) };
            }
            h.is_null()
        })
        .collect()
}

struct X {
    conn: RustConnection,
    root: u32,
    opacity: u32,
    workarea: u32,
    current_desktop: u32,
    client_list: u32,
    wm_pid: u32,
    active_window: u32,
    /// A compositing manager owns the screen, so per-window opacity and ARGB visuals work.
    compositor: bool,
}

thread_local! {
    static CONN: RefCell<Option<Rc<X>>> = const { RefCell::new(None) };
}

fn atom(conn: &RustConnection, name: &str) -> Option<u32> {
    conn.intern_atom(false, name.as_bytes())
        .ok()?
        .reply()
        .ok()
        .map(|r| r.atom)
}

fn connect() -> Option<X> {
    let (conn, screen) = RustConnection::connect(None).ok()?;
    let root = conn.setup().roots.get(screen)?.root;
    let cm = atom(&conn, &format!("_NET_WM_CM_S{screen}"))?;
    let compositor = conn.get_selection_owner(cm).ok()?.reply().ok()?.owner != 0;
    Some(X {
        opacity: atom(&conn, "_NET_WM_WINDOW_OPACITY")?,
        workarea: atom(&conn, "_NET_WORKAREA")?,
        current_desktop: atom(&conn, "_NET_CURRENT_DESKTOP")?,
        client_list: atom(&conn, "_NET_CLIENT_LIST")?,
        wm_pid: atom(&conn, "_NET_WM_PID")?,
        active_window: atom(&conn, "_NET_ACTIVE_WINDOW")?,
        compositor,
        root,
        conn,
    })
}

/// The overlay is single-threaded for window calls, so one cached connection per thread is enough.
fn x() -> Option<Rc<X>> {
    CONN.with(|c| {
        let mut c = c.borrow_mut();
        if c.is_none() {
            match connect() {
                Some(x) => *c = Some(Rc::new(x)),
                None => crate::log::line("x11: cannot connect to the X server"),
            }
        }
        c.clone()
    })
}

/// True if a compositing manager is running, i.e. transparent windows will look right.
pub fn compositor_running() -> bool {
    x().is_some_and(|x| x.compositor)
}

fn window_id(w: &Window) -> Option<u32> {
    match w.window_handle().ok()?.as_raw() {
        RawWindowHandle::Xlib(h) => Some(h.window as u32),
        RawWindowHandle::Xcb(h) => Some(h.window.get()),
        _ => None,
    }
}

/// Maps the window at its slot, above everything. It is override-redirect, so no window manager
/// can give it focus, a frame, or a taskbar entry.
pub fn configure(w: &Window, width: f64, height: f64, slot: usize, pos: Position) {
    place(w, width, height, slot, pos);
    w.set_visible(true);
    raise(w);
}

fn raise(w: &Window) {
    let (Some(x), Some(id)) = (x(), window_id(w)) else {
        return;
    };
    let _ = x
        .conn
        .configure_window(id, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
    let _ = x.conn.flush();
}

/// Monitors, primary first; the whole screen if RandR has nothing to say.
fn monitors(x: &X) -> Vec<Rect> {
    let from_randr = || {
        let mut ms = x
            .conn
            .randr_get_monitors(x.root, true)
            .ok()?
            .reply()
            .ok()?
            .monitors;
        ms.sort_by_key(|m| !m.primary);
        Some(
            ms.iter()
                .map(|m| Rect {
                    x: m.x as i32,
                    y: m.y as i32,
                    w: m.width as i32,
                    h: m.height as i32,
                })
                .collect::<Vec<_>>(),
        )
    };
    from_randr().filter(|ms| !ms.is_empty()).unwrap_or_else(|| {
        x.conn
            .get_geometry(x.root)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|g| {
                vec![Rect {
                    x: 0,
                    y: 0,
                    w: g.width as i32,
                    h: g.height as i32,
                }]
            })
            .unwrap_or_default()
    })
}

/// The window manager's usable area (screen minus panels) on the current desktop.
fn workarea(x: &X) -> Option<Rect> {
    let value32 = |atom: u32, len: u32| {
        let reply = x
            .conn
            .get_property(false, x.root, atom, AtomEnum::CARDINAL, 0, len)
            .ok()?
            .reply()
            .ok()?;
        let values: Option<Vec<u32>> = reply.value32().map(|it| it.collect());
        values
    };
    let desktop = value32(x.current_desktop, 1)
        .and_then(|v| v.first().copied())
        .unwrap_or(0) as usize;
    let all = value32(x.workarea, 4 * 64)?;
    let r = all.chunks(4).nth(desktop).filter(|r| r.len() == 4)?;
    Some(Rect {
        x: r[0] as i32,
        y: r[1] as i32,
        w: r[2] as i32,
        h: r[3] as i32,
    })
}

/// The area to place cards in: the monitor under the pointer, minus panels.
fn target_area(x: &X) -> Option<Rect> {
    let pointer = x
        .conn
        .query_pointer(x.root)
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|p| (p.root_x as i32, p.root_y as i32))
        .unwrap_or((0, 0));
    let monitor = geom::pick_monitor(&monitors(x), pointer)?;
    Some(
        workarea(x)
            .and_then(|wa| monitor.intersect(&wa))
            .unwrap_or(monitor),
    )
}

/// Puts the window in stack slot `slot` of the corner on the monitor holding the pointer.
pub fn place(w: &Window, width: f64, height: f64, slot: usize, pos: Position) {
    let Some(x) = x() else { return };
    let Some(area) = target_area(&x) else { return };
    let (px, py) = geom::slot_origin(area, width, height, slot, pos, w.scale_factor());
    w.set_outer_position(PhysicalPosition::new(px, py));
}

/// Whole-window opacity for the fades. Needs a compositor; without one the card just appears and
/// disappears (and is drawn opaque, see `overlay.rs`).
pub fn set_alpha(w: &Window, alpha: f64) {
    let (Some(x), Some(id)) = (x(), window_id(w)) else {
        return;
    };
    if !x.compositor {
        return;
    }
    if alpha >= 1.0 {
        let _ = x.conn.delete_property(id, x.opacity);
    } else {
        let v = (alpha.clamp(0.0, 1.0) * u32::MAX as f64) as u32;
        let _ =
            x.conn
                .change_property32(PropMode::REPLACE, id, x.opacity, AtomEnum::CARDINAL, &[v]);
    }
    let _ = x.conn.flush();
}

/// Parent PID from the text of `/proc/<pid>/stat` (`pid (comm) state ppid ...`; `comm` may
/// contain spaces and parentheses, so split after the last `)`).
fn parse_ppid(stat: &str) -> Option<u32> {
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// Raises the window of the terminal or IDE hosting Claude Code process `pid`: the nearest
/// ancestor (or the process itself) that owns a top-level window (`_NET_WM_PID`). Terminals that
/// run all windows in one server process (gnome-terminal) are not found this way; then it is a
/// no-op. No-op when `pid` is 0.
pub fn focus_pid(pid: u32, _app: &str) {
    if pid == 0 {
        return;
    }
    let Some(x) = x() else { return };
    let windows: Vec<u32> = x
        .conn
        .get_property(false, x.root, x.client_list, AtomEnum::WINDOW, 0, 4096)
        .ok()
        .and_then(|c| c.reply().ok())
        .and_then(|r| Some(r.value32()?.collect()))
        .unwrap_or_default();
    let mut by_pid: HashMap<u32, u32> = HashMap::new();
    for win in windows {
        let owner = x
            .conn
            .get_property(false, win, x.wm_pid, AtomEnum::CARDINAL, 0, 1)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32()?.next());
        if let Some(p) = owner {
            by_pid.entry(p).or_insert(win);
        }
    }
    let mut cur = pid;
    for _ in 0..32 {
        if let Some(&win) = by_pid.get(&cur) {
            // Source indication 2 = "pager": the WM may switch desktops and unminimize.
            let ev = ClientMessageEvent::new(32, win, x.active_window, [2u32, 0, 0, 0, 0]);
            let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
            let _ = x.conn.send_event(false, x.root, mask, ev);
            let _ = x.conn.flush();
            return;
        }
        match std::fs::read_to_string(format!("/proc/{cur}/stat"))
            .ok()
            .as_deref()
            .and_then(parse_ppid)
        {
            Some(p) if p > 1 => cur = p,
            _ => break,
        }
    }
    crate::log::line(&format!("x11: no window found above pid {pid}"));
}

#[cfg(test)]
mod tests {
    use super::parse_ppid;

    #[test]
    fn ppid_survives_awkward_command_names() {
        assert_eq!(parse_ppid("1234 (bash) S 1200 1234 1234 0 -1"), Some(1200));
        assert_eq!(parse_ppid("77 (my (weird) app) R 42 77 77 0 -1"), Some(42));
        assert_eq!(parse_ppid("garbage"), None);
    }
}
