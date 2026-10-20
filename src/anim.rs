//! Card timing shared by the winit and Wayland overlays: fade in, hold (paused while hovered),
//! fade out, plus the wave animation clock.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::time::{Duration, Instant};

pub const FADE_IN: Duration = Duration::from_millis(150);
pub const FADE_OUT: Duration = Duration::from_millis(300);
pub const FRAME: Duration = Duration::from_millis(16);
pub const WAVE_FRAME: Duration = Duration::from_millis(125); // 8 fps
pub const IDLE_EXIT: Duration = Duration::from_secs(3);

pub struct Timing {
    pub shown_at: Instant,
    pub hover_since: Option<Instant>,
}

impl Timing {
    pub fn new() -> Timing {
        Timing { shown_at: Instant::now(), hover_since: None }
    }

    pub fn restart(&mut self) {
        *self = Timing::new();
    }

    pub fn hover(&mut self, on: bool) {
        if on {
            self.hover_since.get_or_insert_with(Instant::now);
        } else if let Some(h) = self.hover_since.take() {
            self.shown_at += h.elapsed(); // time spent hovering does not count
        }
    }

    /// Time on screen so far, frozen while the pointer hovers.
    pub fn age(&self, now: Instant) -> Duration {
        self.hover_since.unwrap_or(now).saturating_duration_since(self.shown_at)
    }

    /// Opacity now, or None once the card has faded out.
    pub fn alpha(&self, now: Instant, hold: Duration) -> Option<f64> {
        let t = self.age(now);
        if t < FADE_IN {
            Some(t.as_secs_f64() / FADE_IN.as_secs_f64())
        } else if t < hold {
            Some(1.0)
        } else if t < hold + FADE_OUT {
            Some(1.0 - (t - hold).as_secs_f64() / FADE_OUT.as_secs_f64())
        } else {
            None
        }
    }

    /// Wave frame to show now, out of `frames`.
    pub fn wave_frame(&self, now: Instant, frames: usize) -> usize {
        (self.age(now).as_millis() / WAVE_FRAME.as_millis()) as usize % frames.max(1)
    }

    /// Earliest instant this card needs a redraw or removal, or None while paused by hover.
    pub fn next_wake(&self, now: Instant, hold: Duration) -> Option<Instant> {
        let t = self.age(now);
        if self.hover_since.is_some() && t >= FADE_IN && t < hold {
            return None; // paused while hovered: wake only on pointer events
        }
        if t < FADE_IN || t >= hold {
            Some(now + FRAME)
        } else {
            let to_frame = WAVE_FRAME - Duration::from_millis((t.as_millis() % WAVE_FRAME.as_millis()) as u64);
            Some((now + to_frame).min(self.shown_at + hold))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fades_in_holds_fades_out() {
        let t = Timing::new();
        let hold = Duration::from_secs(5);
        let at = |ms| t.shown_at + Duration::from_millis(ms);
        assert!(t.alpha(at(75), hold).unwrap() > 0.4 && t.alpha(at(75), hold).unwrap() < 0.6);
        assert_eq!(t.alpha(at(1000), hold), Some(1.0));
        assert!(t.alpha(at(5150), hold).unwrap() < 0.6);
        assert_eq!(t.alpha(at(5300), hold), None);
    }

    #[test]
    fn hover_freezes_age() {
        let mut t = Timing::new();
        t.hover(true);
        let frozen = t.age(t.shown_at + Duration::from_secs(100));
        assert_eq!(frozen, t.hover_since.unwrap() - t.shown_at);
    }
}
