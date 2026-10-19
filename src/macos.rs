//! The only platform-specific window code: float above everything without taking focus.

use objc2::rc::Retained;
use objc2_app_kit::{NSEvent, NSScreen, NSView, NSWindow, NSWindowCollectionBehavior};
use objc2_foundation::{MainThreadMarker, NSPoint};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

const STATUS_WINDOW_LEVEL: isize = 25;

fn ns_window(w: &Window) -> Option<Retained<NSWindow>> {
    let RawWindowHandle::AppKit(h) = w.window_handle().ok()?.as_raw() else { return None };
    let view: &NSView = unsafe { h.ns_view.cast::<NSView>().as_ref() };
    view.window()
}

/// Pins the window above full-screen apps on every Space and moves it to the corner of the
/// screen holding the mouse pointer.
pub fn configure(w: &Window, width: f64, height: f64) {
    let (Some(win), Some(mtm)) = (ns_window(w), MainThreadMarker::new()) else { return };
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
    let mouse = unsafe { NSEvent::mouseLocation() };
    let screens = NSScreen::screens(mtm);
    let target = screens
        .iter()
        .find(|s| {
            let f = s.frame();
            mouse.x >= f.origin.x
                && mouse.x < f.origin.x + f.size.width
                && mouse.y >= f.origin.y
                && mouse.y < f.origin.y + f.size.height
        })
        .or_else(|| NSScreen::mainScreen(mtm));
    if let Some(screen) = target {
        let vf = screen.visibleFrame();
        let origin = NSPoint::new(
            vf.origin.x + vf.size.width - width - 16.0,
            vf.origin.y + vf.size.height - height - 12.0,
        );
        win.setFrameOrigin(origin);
    }
    win.orderFrontRegardless();
}

pub fn set_alpha(w: &Window, alpha: f64) {
    if let Some(win) = ns_window(w) {
        unsafe { win.setAlphaValue(alpha) };
    }
}
