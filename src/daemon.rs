//! `clawdtoast daemon`: owns the socket and the session registry. It draws nothing itself; when a
//! card is due it feeds `clawdtoast overlay` (a short-lived child process), so the daemon stays small.

use crate::config::Config;
use crate::core::{Card, Core, QuestionCheckpoint};
use crate::ipc::{self, Listener};
use crate::protocol::{self, Msg};
use crate::{log, paths, sound};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Build time of this binary (mtime of the executable, in seconds). A newer binary replaces a
/// daemon with a smaller stamp; see `shim::ensure_daemon`.
pub fn stamp() -> u64 {
    std::env::current_exe()
        .and_then(|p| std::fs::metadata(p)?.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

pub fn run(headless: bool) {
    let Some(listener) = bind() else { return }; // another daemon is already serving
    let _ = std::fs::create_dir_all(paths::support_dir()); // on Linux the socket lives elsewhere
    let _ = std::fs::write(paths::daemon_stamp(), stamp().to_string());
    log::line(&format!(
        "daemon up pid={} headless={headless}",
        std::process::id()
    ));
    run_loop(listener, headless);
    let _ = std::fs::remove_file(paths::daemon_stamp());
    let _ = std::fs::remove_file(paths::socket());
    log::line("daemon exit");
}

fn bind() -> Option<Listener> {
    let dir = paths::runtime_dir();
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
            stream
                .set_read_timeout(Some(Duration::from_millis(200)))
                .ok();
            // Each hook sends exactly one newline-delimited message. Don't wait for
            // a second line or a read timeout after processing it: on Windows the
            // connection probes and successive hooks otherwise stall the acceptor.
            let mut line = String::new();
            if BufReader::new(stream.take(64 * 1024))
                .read_line(&mut line)
                .is_ok()
                && !line.is_empty()
            {
                match protocol::decode(line.trim_end()) {
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
        cmd.arg("overlay")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
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
        let Ok(mut line) = serde_json::to_string(card) else {
            return false;
        };
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|_| self.stdin.flush())
            .is_ok()
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
    let checkpoint_path = paths::support_dir().join("codex-questions.json");
    ipc::secure_dir(&paths::support_dir());
    if let Ok(file) = std::fs::File::open(&checkpoint_path) {
        // Treat a corrupt/oversized checkpoint as absent; never prevent daemon startup.
        if let Ok(saved) =
            serde_json::from_reader::<_, QuestionCheckpoint>(file.take(4 * 1024 * 1024))
        {
            core.restore(saved, Instant::now(), protocol::now_ms());
        }
    }
    let mut saved_revision = core.revision;
    let mut overlay: Option<Overlay> = None;
    loop {
        if overlay.as_mut().is_some_and(|o| !o.alive()) {
            overlay = None;
        }
        let mut wait = core
            .next_deadline()
            .saturating_duration_since(Instant::now());
        if overlay.is_some() {
            wait = wait.min(Duration::from_secs(1)); // notice the overlay exiting
        }
        if let Ok(msg) = rx.recv_timeout(wait) {
            if matches!(msg, Msg::Shutdown) {
                log::line("daemon: replaced by a newer binary");
                return;
            }
            if let Some(card) = core.handle(msg, Instant::now()) {
                if !card.silent {
                    sound::play(card.kind, &core.config.sound);
                }
                if headless {
                    if card.silent {
                        log::line(&format!("attention update: {}", card.title()));
                    } else {
                        log::line(&format!("card: {} | {}", card.title(), card.body));
                    }
                } else if card.silent {
                    if let Some(o) = overlay.as_mut() {
                        o.send(&card);
                    }
                } else {
                    show(&mut overlay, &card);
                }
            }
        }
        let exit = core.tick(Instant::now(), overlay.is_some());
        while let Some(card) = core.take_effect() {
            if headless {
                log::line(&format!(
                    "attention update: {} dismiss={}",
                    card.title(),
                    card.dismiss
                ));
            } else if let Some(o) = overlay.as_mut() {
                // Never spawn an overlay just to remove or update an old card.
                o.send(&card);
            }
        }
        if core.revision != saved_revision {
            let saved = core.checkpoint(Instant::now(), protocol::now_ms());
            let tmp = checkpoint_path.with_extension("json.tmp");
            let result = serde_json::to_vec(&saved)
                .map_err(std::io::Error::other)
                .and_then(|bytes| std::fs::write(&tmp, bytes))
                .and_then(|_| std::fs::rename(&tmp, &checkpoint_path));
            if let Err(e) = result {
                log::line(&format!("question checkpoint: {e}"));
            }
            saved_revision = core.revision;
        }
        if exit {
            return;
        }
    }
}
