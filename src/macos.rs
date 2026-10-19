//! The only platform-specific window code: float above everything without taking focus.

use crate::config::Position;
use objc2::rc::Retained;
use objc2_app_kit::{NSEvent, NSScreen, NSView, NSWindow, NSWindowCollectionBehavior};
use objc2_foundation::{MainThreadMarker, NSPoint};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

const STATUS_WINDOW_LEVEL: isize = 25;
const MARGIN: f64 = 12.0;
const GAP: f64 = 8.0;

fn ns_window(w: &Window) -> Option<Retained<NSWindow>> {
    let RawWindowHandle::AppKit(h) = w.window_handle().ok()?.as_raw() else { return None };
    let view: &NSView = unsafe { h.ns_view.cast::<NSView>().as_ref() };
    view.window()
}

/// Pins the window above full-screen apps on every Space, then places it.
pub fn configure(w: &Window, width: f64, height: f64, slot: usize, pos: Position) {
    let Some(win) = ns_window(w) else { return };
    unsafe {
        win.setLevel(STATUS_WINDOW_LEVEL);
        win.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        win.setHidesOnDeactivate(false);
    }
    place(w, width, height, slot, pos);
    unsafe { win.orderFrontRegardless() };
}

/// Puts the window in stack slot `slot` of the corner on the screen holding the mouse pointer.
/// Top corners stack downward, bottom corners upward.
pub fn place(w: &Window, width: f64, height: f64, slot: usize, pos: Position) {
    let (Some(win), Some(mtm)) = (ns_window(w), MainThreadMarker::new()) else { return };
    let mouse = unsafe { NSEvent::mouseLocation() };
    let screens = NSScreen::screens(mtm);
    let under_mouse = screens.iter().find(|s| {
        let f = s.frame();
        mouse.x >= f.origin.x
            && mouse.x < f.origin.x + f.size.width
            && mouse.y >= f.origin.y
            && mouse.y < f.origin.y + f.size.height
    });
    let visible = match under_mouse {
        Some(s) => Some(s.visibleFrame()),
        None => NSScreen::mainScreen(mtm).map(|s| s.visibleFrame()),
    };
    let Some(vf) = visible else { return };
    let offset = slot as f64 * (height + GAP);
    let x = match pos {
        Position::TopLeft | Position::BottomLeft => vf.origin.x + MARGIN + 4.0,
        Position::TopRight | Position::BottomRight => vf.origin.x + vf.size.width - width - MARGIN - 4.0,
    };
    let y = match pos {
        Position::TopLeft | Position::TopRight => vf.origin.y + vf.size.height - height - MARGIN - offset,
        Position::BottomLeft | Position::BottomRight => vf.origin.y + MARGIN + offset,
    };
    unsafe { win.setFrameOrigin(NSPoint::new(x, y)) };
}

pub fn set_alpha(w: &Window, alpha: f64) {
    if let Some(win) = ns_window(w) {
        unsafe { win.setAlphaValue(alpha) };
    }
}
