//! `clawdtoast overlay`: short-lived UI process. The daemon spawns it when a card is due and feeds
//! it one JSON card per line on stdin. It exits shortly after the last card is gone, so the
//! always-running daemon never carries AppKit's memory.

use crate::config::{Config, Position};
use crate::core::Card;
use crate::stack::{Offer, Stack};
use crate::{log, protocol, render};
use std::io::BufRead;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
#[cfg(target_os = "macos")]
use crate::macos as platform;
#[cfg(windows)]
use crate::windows as platform;
#[cfg(target_os = "macos")]
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
#[cfg(windows)]
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{Window, WindowId, WindowLevel};

const FADE_IN: Duration = Duration::from_millis(150);
const FADE_OUT: Duration = Duration::from_millis(300);
const FRAME: Duration = Duration::from_millis(16);
const WAVE_FRAME: Duration = Duration::from_millis(125); // 8 fps
const IDLE_EXIT: Duration = Duration::from_secs(3);

enum Ev {
    Card(Card),
    Eof,
}

pub fn run() {
    let mut builder = EventLoop::<Ev>::with_user_event();
    #[cfg(target_os = "macos")]
    builder
        .with_activation_policy(ActivationPolicy::Accessory)
        .with_default_menu(false)
        // winit otherwise activates the app at launch, which steals focus from whatever you're using.
        .with_activate_ignoring_other_apps(false);
    let event_loop = match builder.build() {
        Ok(l) => l,
        Err(e) => return log::line(&format!("overlay: event loop: {e}")),
    };
    let proxy = event_loop.create_proxy();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            match serde_json::from_str::<Card>(&line) {
                Ok(card) => {
                    if proxy.send_event(Ev::Card(card)).is_err() {
                        return;
                    }
                }
                Err(e) => log::line(&format!("overlay: bad card: {e}")),
            }
        }
        let _ = proxy.send_event(Ev::Eof);
    });
    let cfg = Config::load();
    let mut app = App {
        stack: Stack::new(cfg.max_stack),
        duration: Duration::from_secs_f64(cfg.duration_secs),
        position: cfg.position,
        shown: Vec::new(),
        assets: None,
        eof: false,
        idle_since: None,
    };
    if let Err(e) = event_loop.run_app(&mut app) {
        log::line(&format!("overlay: run_app: {e}"));
    }
}

struct Shown {
    window: Arc<Window>,
    surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
    card: Card,
    shown_at: Instant,
    hover_since: Option<Instant>,
    frame: usize,
}

impl Shown {
    /// Time on screen so far, frozen while the pointer hovers.
    fn age(&self, now: Instant) -> Duration {
        self.hover_since.unwrap_or(now).saturating_duration_since(self.shown_at)
    }

    /// Opacity now, or None once the card has faded out.
    fn alpha(&self, now: Instant, hold: Duration) -> Option<f64> {
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
}

struct App {
    stack: Stack,
    duration: Duration,
    position: Position,
    shown: Vec<Shown>,
    assets: Option<render::Assets>,
    eof: bool,
    idle_since: Option<Instant>,
}

fn draw(assets: &render::Assets, s: &mut Shown) {
    let size = s.window.inner_size();
    let (Some(w), Some(h)) = (std::num::NonZeroU32::new(size.width), std::num::NonZeroU32::new(size.height)) else {
        return;
    };
    let pixels = render::render(&s.card, assets, s.frame, size.width, size.height, s.window.scale_factor());
    if s.surface.resize(w, h).is_err() {
        return;
    }
    if let Ok(mut buf) = s.surface.buffer_mut() {
        let n = buf.len().min(pixels.len());
        buf[..n].copy_from_slice(&pixels[..n]);
        let _ = buf.present();
    }
}

impl App {
    fn open(&mut self, el: &ActiveEventLoop, card: Card) {
        let slot = self.shown.len();
        let attrs = Window::default_attributes();
        #[cfg(windows)]
        let attrs = attrs.with_skip_taskbar(true);
        let attrs = attrs
            .with_title("clawdtoast")
            .with_inner_size(LogicalSize::new(render::CARD_W, render::CARD_H))
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_active(false)
            .with_window_level(WindowLevel::AlwaysOnTop);
        let window = match el.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => return log::line(&format!("create_window: {e}")),
        };
        let surface = match softbuffer::Context::new(window.clone())
            .and_then(|ctx| softbuffer::Surface::new(&ctx, window.clone()))
        {
            Ok(s) => s,
            Err(e) => return log::line(&format!("softbuffer: {e}")),
        };
        platform::set_alpha(&window, 0.0);
        platform::configure(&window, render::CARD_W, render::CARD_H, slot, self.position);
        let mut s = Shown { window, surface, card, shown_at: Instant::now(), hover_since: None, frame: 0 };
        draw(self.assets.get_or_insert_with(render::Assets::load), &mut s);
        if s.card.ts > 0 {
            log::line(&format!("latency_ms={}", protocol::now_ms().saturating_sub(s.card.ts)));
        }
        self.shown.push(s);
    }

    /// Removes the card at `idx`, promotes a queued card if any, and closes the gap.
    fn remove(&mut self, el: &ActiveEventLoop, idx: usize, why: &str) {
        log::line(&format!("dismiss: {why}"));
        self.shown.remove(idx); // dropping the window destroys it
        if let Some(next) = self.stack.remove(idx) {
            self.open(el, next);
        }
        for (i, s) in self.shown.iter().enumerate().skip(idx) {
            platform::place(&s.window, render::CARD_W, render::CARD_H, i, self.position);
        }
    }

    fn index_of(&self, id: WindowId) -> Option<usize> {
        self.shown.iter().position(|s| s.window.id() == id)
    }
}

impl ApplicationHandler<Ev> for App {
    fn resumed(&mut self, _: &ActiveEventLoop) {}

    fn user_event(&mut self, el: &ActiveEventLoop, ev: Ev) {
        let card = match ev {
            Ev::Eof => {
                self.eof = true;
                return;
            }
            Ev::Card(c) => c,
        };
        self.idle_since = None;
        match self.stack.offer(&card) {
            Offer::Show => self.open(el, card),
            Offer::Replace(i) => {
                let s = &mut self.shown[i];
                s.card = card;
                s.shown_at = Instant::now();
                s.hover_since = None;
                s.frame = 0;
                draw(self.assets.get_or_insert_with(render::Assets::load), s);
            }
            Offer::Queued => {}
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(i) = self.index_of(id) else { return };
        match event {
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                platform::focus_pid(self.shown[i].card.pid, &self.shown[i].card.app);
                self.remove(el, i, "click")
            }
            WindowEvent::CursorEntered { .. } => self.shown[i].hover_since = Some(Instant::now()),
            WindowEvent::CursorLeft { .. } => {
                let s = &mut self.shown[i];
                if let Some(h) = s.hover_since.take() {
                    s.shown_at += h.elapsed(); // time spent hovering does not count
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(a) = self.assets.as_ref() {
                    draw(a, &mut self.shown[i]);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        let now = Instant::now();
        let hold = self.duration;

        let expired: Vec<usize> =
            (0..self.shown.len()).filter(|&i| self.shown[i].alpha(now, hold).is_none()).collect();
        for i in expired.into_iter().rev() {
            self.remove(el, i, "timeout");
        }

        let mut next = now + Duration::from_secs(3600);
        let assets = self.assets.get_or_insert_with(render::Assets::load);
        for s in self.shown.iter_mut() {
            let Some(a) = s.alpha(now, hold) else { continue };
            platform::set_alpha(&s.window, a);
            let t = s.age(now);
            let frame = (t.as_millis() / WAVE_FRAME.as_millis()) as usize % assets.frames();
            if frame != s.frame {
                s.frame = frame;
                draw(assets, s);
            }
            if s.hover_since.is_some() && t >= FADE_IN && t < hold {
                continue; // paused while hovered: wake only on pointer events
            }
            if t < FADE_IN || t >= hold {
                next = next.min(now + FRAME);
            } else {
                let to_frame = WAVE_FRAME - Duration::from_millis((t.as_millis() % WAVE_FRAME.as_millis()) as u64);
                next = next.min(now + to_frame).min(s.shown_at + hold);
            }
        }

        if self.shown.is_empty() && self.stack.is_empty() {
            let since = *self.idle_since.get_or_insert(now);
            if self.eof || now.duration_since(since) >= IDLE_EXIT {
                el.exit();
                return;
            }
            next = next.min(since + IDLE_EXIT);
        }
        el.set_control_flow(ControlFlow::WaitUntil(next));
    }
}
