//! The only platform-specific window code on Windows: float above everything without taking focus.

use crate::config::Position;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    CreateRoundRectRgn, GetMonitorInfoW, MonitorFromPoint, SetWindowRgn, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetCursorPos, GetWindow, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, SetForegroundWindow, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, GWL_EXSTYLE, GW_OWNER, HWND_TOPMOST, LWA_ALPHA, SWP_NOACTIVATE, SWP_NOSIZE,
    SW_RESTORE, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
};

const MARGIN: f64 = 12.0;
const GAP: f64 = 8.0;
const CORNER: f64 = 14.0; // keep in step with render.rs

fn hwnd(w: &Window) -> Option<HWND> {
    match w.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as HWND),
        _ => None,
    }
}

/// Pins the window above everything, off the taskbar and unfocusable, then places it.
/// Fading uses a layered window's constant alpha; the rounded corners are a window region
/// (hard-edged: softbuffer's GDI path ignores per-pixel alpha).
pub fn configure(w: &Window, width: f64, height: f64, slot: usize, pos: Position) {
    let Some(h) = hwnd(w) else { return };
    unsafe {
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        let ex = ex | (WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST) as isize;
        SetWindowLongPtrW(h, GWL_EXSTYLE, ex);
    }
    place(w, width, height, slot, pos);
}

/// Puts the window in stack slot `slot` of the corner on the monitor holding the mouse pointer.
/// Top corners stack downward, bottom corners upward.
pub fn place(w: &Window, _width: f64, _height: f64, slot: usize, pos: Position) {
    let Some(h) = hwnd(w) else { return };
    unsafe {
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut mi) == 0 {
            return;
        }
        let work = mi.rcWork;
        // Move onto the target monitor first: the window's DPI (and so its size) follows it.
        SetWindowPos(h, HWND_TOPMOST, work.left, work.top, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
        let s = w.scale_factor();
        let size = w.outer_size();
        let (pw, ph) = (size.width as i32, size.height as i32);
        let margin = ((MARGIN + 4.0) * s) as i32;
        let offset = (slot as f64 * (ph as f64 / s + GAP) * s) as i32;
        let x = match pos {
            Position::TopLeft | Position::BottomLeft => work.left + margin,
            Position::TopRight | Position::BottomRight => work.right - pw - margin,
        };
        let y = match pos {
            Position::TopLeft | Position::TopRight => work.top + margin + offset,
            Position::BottomLeft | Position::BottomRight => work.bottom - ph - margin - offset,
        };
        SetWindowPos(h, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
        let r = (2.0 * CORNER * s) as i32;
        let rgn = CreateRoundRectRgn(0, 0, pw + 1, ph + 1, r, r);
        SetWindowRgn(h, rgn, 1); // the system owns the region from here
    }
}

pub fn set_alpha(w: &Window, alpha: f64) {
    if let Some(h) = hwnd(w) {
        unsafe { SetLayeredWindowAttributes(h, 0, (alpha.clamp(0.0, 1.0) * 255.0).round() as u8, LWA_ALPHA) };
    }
}

/// Brings forward the window of the app that hosts the Claude Code process `pid`: the nearest
/// ancestor (or the process itself) that owns a visible top-level window, i.e. the terminal or IDE.
/// No-op when `pid` is 0 or no ancestor has a window.
pub fn focus_pid(pid: u32, _app: &str) {
    if pid == 0 {
        crate::log::line("windows: card has no pid, nothing to focus");
        return;
    }
    let parents = crate::proc::table();
    let windows = top_level_windows();
    let mut cur = pid;
    for _ in 0..32 {
        if let Some(&(h, _)) = windows.iter().find(|(_, p)| *p == cur) {
            unsafe {
                if IsIconic(h) != 0 {
                    ShowWindow(h, SW_RESTORE);
                }
                if SetForegroundWindow(h) == 0 {
                    crate::log::line("windows: SetForegroundWindow refused");
                }
            }
            return;
        }
        match parents.get(&cur) {
            Some(&(p, _)) if p > 1 => cur = p,
            _ => break,
        }
    }
    crate::log::line(&format!("windows: no window found above pid {pid}"));
}

/// Visible, unowned top-level windows (the ones with a taskbar button) and their process ids.
fn top_level_windows() -> Vec<(HWND, u32)> {
    unsafe extern "system" fn each(h: HWND, lparam: LPARAM) -> BOOL {
        let out = &mut *(lparam as *mut Vec<(HWND, u32)>);
        if IsWindowVisible(h) != 0
            && GetWindow(h, GW_OWNER).is_null()
            && GetWindowLongPtrW(h, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW == 0
        {
            let mut r: RECT = std::mem::zeroed();
            let mut pid = 0u32;
            GetWindowRect(h, &mut r);
            GetWindowThreadProcessId(h, &mut pid);
            if r.right > r.left {
                out.push((h, pid));
            }
        }
        1
    }
    let mut v: Vec<(HWND, u32)> = Vec::new();
    unsafe { EnumWindows(Some(each), &mut v as *mut _ as LPARAM) };
    v
}
