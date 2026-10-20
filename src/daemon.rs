//! `clawdtoast daemon`: owns the socket and the session registry. It draws nothing itself; when a
//! card is due it feeds `clawdtoast overlay` (a short-lived child process), so the daemon stays small.

use crate::config::Config;
use crate::core::{Card, Core};
use crate::ipc::{self, Listener};
use crate::protocol::{self, Msg};
use crate::{log, paths, sound};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn run(headless: bool) {
    let Some(listener) = bind() else { return }; // another daemon is already serving
    log::line(&format!("daemon up pid={} headless={headless}", std::process::id()));
    run_loop(listener, headless);
    let _ = std::fs::remove_file(paths::socket());
    log::line("daemon exit");
}

fn bind() -> Option<Listener> {
    let dir = paths::support_dir();
    let _ = std::fs::create_dir_all(&dir);
    ipc::secure_dir(&dir);
    let sock = paths::socket();
    if ipc::connect(&sock).is_ok() {
        return None; // another daemon is serving
    }
    let _ = std::fs::remove_file(&sock); // stale socket from a dead daemon
    ipc::bind(&sock).ok()
}

/// Accepts connections on a background thread and forwards parsed messages.
fn spawn_acceptor(listener: Listener, tx: std::sync::mpsc::Sender<Msg>) {
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            stream.set_read_timeout(Some(Duration::from_millis(200))).ok();
            // Messages are at most 4 KB; cap a connection so a bad client cannot flood memory.
            for line in BufReader::new(stream.take(64 * 1024)).lines().map_while(Result::ok) {
                match protocol::decode(&line) {
                    Some(m) => {
                        if tx.send(m).is_err() {
                            return;
                        }
                    }
                    None => log::line("daemon: unparseable message"),
                }
            }
        }
    });
}

struct Overlay {
    child: Child,
    stdin: ChildStdin,
}

impl Overlay {
    fn spawn() -> Option<Overlay> {
        let exe = std::env::current_exe().ok()?;
        let mut cmd = Command::new(exe);
        cmd.arg("overlay").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut cmd, CREATE_NO_WINDOW);
        let mut child = cmd.spawn().ok()?;
        let stdin = child.stdin.take()?;
        Some(Overlay { child, stdin })
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn send(&mut self, card: &Card) -> bool {
        let Ok(mut line) = serde_json::to_string(card) else { return false };
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).and_then(|_| self.stdin.flush()).is_ok()
    }
}

/// Shows `card`, starting the overlay if needed (retrying once if it just exited).
fn show(overlay: &mut Option<Overlay>, card: &Card) {
    for _ in 0..2 {
        if overlay.as_mut().is_some_and(|o| !o.alive()) {
            *overlay = None;
        }
        if overlay.is_none() {
            *overlay = Overlay::spawn();
        }
        let Some(o) = overlay.as_mut() else { break };
        if o.send(card) {
            return;
        }
        *overlay = None;
    }
    log::line("daemon: could not show card");
}

fn run_loop(listener: Listener, headless: bool) {
    let (tx, rx) = std::sync::mpsc::channel::<Msg>();
    spawn_acceptor(listener, tx);
    let mut core = Core::new(Instant::now(), Config::load());
    let mut overlay: Option<Overlay> = None;
    loop {
        if overlay.as_mut().is_some_and(|o| !o.alive()) {
            overlay = None;
        }
        let mut wait = core.next_deadline().saturating_duration_since(Instant::now());
        if overlay.is_some() {
            wait = wait.min(Duration::from_secs(1)); // notice the overlay exiting
        }
        if let Ok(msg) = rx.recv_timeout(wait) {
            if let Some(card) = core.handle(msg, Instant::now()) {
                sound::play(card.kind, &core.config.sound);
                if headless {
                    log::line(&format!("card: {} | {}", card.title(), card.body));
                } else {
                    show(&mut overlay, &card);
                }
            }
        }
        if core.tick(Instant::now(), overlay.is_some()) {
            return;
        }
    }
}
