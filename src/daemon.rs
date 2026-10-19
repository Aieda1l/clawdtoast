//! `clawdpet daemon`: owns the socket, the session registry and the overlay window.

use crate::core::{Card, Core};
use crate::protocol::{self, Msg};
use crate::{log, paths, render, sound};
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
use winit::window::{Window, WindowId, WindowLevel};

const FADE_IN: Duration = Duration::from_millis(150);
const FADE_OUT: Duration = Duration::from_millis(300);
const FRAME: Duration = Duration::from_millis(16);

pub fn run(headless: bool) {
    let listener = match bind() {
        Some(l) => l,
        None => return, // another daemon is already serving
    };
    log::line(&format!("daemon up pid={} headless={headless}", std::process::id()));
    if headless {
        run_headless(listener);
    } else {
        run_windowed(listener);
    }
    let _ = std::fs::remove_file(paths::socket());
    log::line("daemon exit");
}

fn bind() -> Option<UnixListener> {
    let dir = paths::support_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    let sock = paths::socket();
    let listener = match UnixListener::bind(&sock) {
        Ok(l) => l,
        Err(_) => {
            if UnixStream::connect(&sock).is_ok() {
                return None;
            }
            let _ = std::fs::remove_file(&sock); // stale socket from a dead daemon
            UnixListener::bind(&sock).ok()?
        }
    };
    let _ = std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600));
    Some(listener)
}

/// Accepts connections on a background thread and forwards parsed messages.
fn spawn_acceptor(listener: UnixListener, tx: impl Fn(Msg) -> bool + Send + 'static) {
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            stream.set_read_timeout(Some(Duration::from_millis(200))).ok();
            for line in BufReader::new(stream.take_limit()).lines().map_while(Result::ok) {
                match protocol::decode(&line) {
                    Some(m) => {
                        if !tx(m) {
                            return;
                        }
                    }
                    None => log::line("daemon: unparseable message"),
                }
            }
        }
    });
}

trait TakeLimit {
    fn take_limit(self) -> std::io::Take<UnixStream>;
}
impl TakeLimit for UnixStream {
    /// Messages are at most 4 KB; cap a connection so a bad client cannot flood memory.
    fn take_limit(self) -> std::io::Take<UnixStream> {
        std::io::Read::take(self, 64 * 1024)
    }
}

fn run_headless(listener: UnixListener) {
    let (tx, rx) = std::sync::mpsc::channel::<Msg>();
    spawn_acceptor(listener, move |m| tx.send(m).is_ok());
    let mut core = Core::new(Instant::now());
    loop {
        let now = Instant::now();
        let wait = core.next_deadline().saturating_duration_since(now);
        if let Ok(msg) = rx.recv_timeout(wait) {
            if let Some(card) = core.handle(msg, Instant::now()) {
                log::line(&format!("card: {} | {}", card.title(), card.body));
            }
        }
        if core.tick(Instant::now(), false) {
            return;
        }
    }
}

fn run_windowed(listener: UnixListener) {
    let event_loop = match EventLoop::<Msg>::with_user_event()
        .with_activation_policy(ActivationPolicy::Accessory)
        .with_default_menu(false)
        .build()
    {
        Ok(l) => l,
        Err(e) => {
            log::line(&format!("event loop: {e}"));
            return;
        }
    };
    let proxy = event_loop.create_proxy();
    spawn_acceptor(listener, move |m| proxy.send_event(m).is_ok());
    let mut app = App::new();
    if let Err(e) = event_loop.run_app(&mut app) {
        log::line(&format!("run_app: {e}"));
    }
}

struct Shown {
    window: Arc<Window>,
    surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
    card: Card,
    shown_at: Instant,
    hover_since: Option<Instant>,
}

struct App {
    core: Core,
    assets: Option<render::Assets>,
    shown: Option<Shown>,
    duration: Duration,
}

impl App {
    fn new() -> App {
        let secs = std::env::var("CLAWDPET_DURATION_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(5.0);
        App {
            core: Core::new(Instant::now()),
            assets: None,
            shown: None,
            duration: Duration::from_secs_f64(secs),
        }
    }

    fn show(&mut self, el: &ActiveEventLoop, card: Card) {
        sound::play(card.kind);
        let now = Instant::now();
        if let Some(s) = &mut self.shown {
            // A new alert replaces the visible card and restarts its timer.
            s.card = card;
            s.shown_at = now;
            s.hover_since = None;
            draw(self.assets.get_or_insert_with(render::Assets::load), s);
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("clawdpet")
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
        crate::macos::set_alpha(&window, 0.0);
        crate::macos::configure(&window, render::CARD_W, render::CARD_H);
        let mut shown = Shown { window, surface, card, shown_at: now, hover_since: None };
        draw(self.assets.get_or_insert_with(render::Assets::load), &mut shown);
        if shown.card.ts > 0 {
            log::line(&format!("latency_ms={}", protocol::now_ms().saturating_sub(shown.card.ts)));
        }
        self.shown = Some(shown);
    }

    /// Alpha for the current instant, or None once the card is gone.
    fn alpha(&self, now: Instant) -> Option<f64> {
        let s = self.shown.as_ref()?;
        let t = s.hover_since.unwrap_or(now).saturating_duration_since(s.shown_at);
        if t < FADE_IN {
            Some(t.as_secs_f64() / FADE_IN.as_secs_f64())
        } else if t < self.duration {
            Some(1.0)
        } else if t < self.duration + FADE_OUT {
            Some(1.0 - (t - self.duration).as_secs_f64() / FADE_OUT.as_secs_f64())
        } else {
            None
        }
    }

    fn dismiss(&mut self, why: &str) {
        log::line(&format!("dismiss: {why}"));
        self.shown = None; // dropping the window destroys it
    }
}

fn draw(assets: &render::Assets, s: &mut Shown) {
    let size = s.window.inner_size();
    let (Some(w), Some(h)) = (std::num::NonZeroU32::new(size.width), std::num::NonZeroU32::new(size.height)) else {
        return;
    };
    let pixels = render::render(&s.card, assets, size.width, size.height, s.window.scale_factor());
    if s.surface.resize(w, h).is_err() {
        return;
    }
    if let Ok(mut buf) = s.surface.buffer_mut() {
        let n = buf.len().min(pixels.len());
        buf[..n].copy_from_slice(&pixels[..n]);
        let _ = buf.present();
    }
}

impl ApplicationHandler<Msg> for App {
    fn resumed(&mut self, _: &ActiveEventLoop) {}

    fn user_event(&mut self, el: &ActiveEventLoop, msg: Msg) {
        if let Some(card) = self.core.handle(msg, Instant::now()) {
            self.show(el, card);
        }
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        use winit::event::{ElementState, MouseButton};
        match event {
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => self.dismiss("click"),
            WindowEvent::CursorEntered { .. } => {
                if let Some(s) = &mut self.shown {
                    s.hover_since = Some(Instant::now());
                }
            }
            WindowEvent::CursorLeft { .. } => {
                if let Some(s) = &mut self.shown {
                    if let Some(h) = s.hover_since.take() {
                        s.shown_at += h.elapsed(); // time spent hovering does not count
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                if let (Some(a), Some(s)) = (self.assets.as_ref(), self.shown.as_mut()) {
                    draw(a, s);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        let now = Instant::now();
        let mut next = self.core.next_deadline();
        if self.shown.is_some() {
            match self.alpha(now) {
                None => self.dismiss("timeout"),
                Some(a) => {
                    if let Some(s) = &self.shown {
                        crate::macos::set_alpha(&s.window, a);
                        let t = s.hover_since.unwrap_or(now).saturating_duration_since(s.shown_at);
                        if s.hover_since.is_some() && t >= FADE_IN && t < self.duration {
                            // Paused while hovered: wake only on pointer events (or the core timers).
                        } else if t < FADE_IN || t >= self.duration {
                            next = next.min(now + FRAME);
                        } else {
                            next = next.min(s.shown_at + self.duration);
                        }
                    }
                }
            }
        }
        if self.core.tick(now, self.shown.is_some()) {
            el.exit();
            return;
        }
        el.set_control_flow(ControlFlow::WaitUntil(next));
    }
}
