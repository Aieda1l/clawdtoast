//! Card placement maths shared by the Linux backends (the macOS code works in AppKit's own
//! coordinates). Pure, so it is unit-tested on every platform.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use crate::config::Position;

/// Space between a card and the screen edge, and between stacked cards (logical pixels).
pub const MARGIN: f64 = 16.0;
pub const GAP: f64 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let (x0, y0) = (self.x.max(o.x), self.y.max(o.y));
        let (x1, y1) = ((self.x + self.w).min(o.x + o.w), (self.y + self.h).min(o.y + o.h));
        (x1 > x0 && y1 > y0).then_some(Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }
}

/// The monitor holding the pointer, else the first (callers put the primary monitor first).
pub fn pick_monitor(monitors: &[Rect], pointer: (i32, i32)) -> Option<Rect> {
    monitors.iter().find(|m| m.contains(pointer.0, pointer.1)).or(monitors.first()).copied()
}

/// Top-left corner, in physical pixels, of stack slot `slot` inside `area`. Top corners stack
/// downward, bottom corners upward. `w`/`h` are the logical card size.
pub fn slot_origin(area: Rect, w: f64, h: f64, slot: usize, pos: Position, scale: f64) -> (i32, i32) {
    let (pw, ph, m) = ((w * scale).round(), (h * scale).round(), MARGIN * scale);
    let offset = slot as f64 * (h + GAP) * scale;
    let x = match pos {
        Position::TopLeft | Position::BottomLeft => area.x as f64 + m,
        Position::TopRight | Position::BottomRight => (area.x + area.w) as f64 - pw - m,
    };
    let y = match pos {
        Position::TopLeft | Position::TopRight => area.y as f64 + m + offset,
        Position::BottomLeft | Position::BottomRight => (area.y + area.h) as f64 - ph - m - offset,
    };
    (x.round() as i32, y.round() as i32)
}

/// Layer-shell margins `(top, right, bottom, left)` in logical pixels for stack slot `slot`, for a
/// surface anchored to the corner `pos`. Only the two anchored edges get a margin.
pub fn layer_margins(h: f64, slot: usize, pos: Position) -> (i32, i32, i32, i32) {
    let edge = MARGIN.round() as i32;
    let off = edge + (slot as f64 * (h + GAP)).round() as i32;
    match pos {
        Position::TopLeft => (off, 0, 0, edge),
        Position::TopRight => (off, edge, 0, 0),
        Position::BottomLeft => (0, 0, off, edge),
        Position::BottomRight => (0, edge, off, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect { x: 0, y: 0, w: 1920, h: 1080 };

    #[test]
    fn top_right_stacks_downward() {
        assert_eq!(slot_origin(SCREEN, 320.0, 96.0, 0, Position::TopRight, 1.0), (1920 - 320 - 16, 16));
        assert_eq!(slot_origin(SCREEN, 320.0, 96.0, 2, Position::TopRight, 1.0), (1584, 16 + 2 * 104));
    }

    #[test]
    fn bottom_left_stacks_upward() {
        assert_eq!(slot_origin(SCREEN, 320.0, 96.0, 0, Position::BottomLeft, 1.0), (16, 1080 - 96 - 16));
        assert_eq!(slot_origin(SCREEN, 320.0, 96.0, 1, Position::BottomLeft, 1.0), (16, 1080 - 96 - 16 - 104));
    }

    #[test]
    fn scale_applies_to_everything_and_offsets_stay_in_the_monitor() {
        let m = Rect { x: 1920, y: 100, w: 2560, h: 1440 };
        let (x, y) = slot_origin(m, 320.0, 96.0, 0, Position::TopRight, 2.0);
        assert_eq!((x, y), (1920 + 2560 - 640 - 32, 100 + 32));
    }

    #[test]
    fn pointer_picks_monitor_else_first() {
        let a = Rect { x: 0, y: 0, w: 100, h: 100 };
        let b = Rect { x: 100, y: 0, w: 100, h: 100 };
        assert_eq!(pick_monitor(&[a, b], (150, 10)), Some(b));
        assert_eq!(pick_monitor(&[a, b], (-5, 10)), Some(a));
        assert_eq!(pick_monitor(&[], (0, 0)), None);
    }

    #[test]
    fn intersect_clips_to_work_area() {
        let work = Rect { x: 0, y: 28, w: 1920, h: 1052 };
        assert_eq!(SCREEN.intersect(&work), Some(work));
        assert_eq!(SCREEN.intersect(&Rect { x: 5000, y: 0, w: 10, h: 10 }), None);
    }

    #[test]
    fn layer_margins_only_on_anchored_edges() {
        assert_eq!(layer_margins(96.0, 0, Position::TopRight), (16, 16, 0, 0));
        assert_eq!(layer_margins(96.0, 1, Position::BottomLeft), (0, 0, 120, 16));
    }
}
