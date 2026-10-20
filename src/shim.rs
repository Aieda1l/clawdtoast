//! `clawdtoast hook <event>`: runs inside Claude Code's hook machinery. Always exits 0, prints nothing.

use crate::protocol::{self, Kind, Msg};
use crate::{ipc, log, paths, proc, text};
use serde_json::Value;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

pub fn run(event: &str) {
    if let Err(e) = handle(event) {
        log::line(&format!("shim {event}: {e}"));
    }
}

fn handle(event: &str) -> Result<(), String> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
    let session_id = s("session_id");
    let cwd = s("cwd");
    let project = text::project_name(&cwd);
    if session_id.is_empty() {
        return Err("payload has no session_id".into());
    }

    let msg = match event {
        "session-start" => Msg::SessionOpen { session_id, pid: claude_pid(), cwd },
        "session-end" => Msg::SessionClose { session_id },
        "stop" => {
            let body = text::last_assistant_text(&s("transcript_path"))
                .map(|t| text::first_sentence(&t))
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "Finished".into());
            alert(session_id, Kind::Done, project, body)
        }
        "subagent-stop" => alert(session_id, Kind::Subagent, project, "A subagent finished".into()),
        "notification" => {
            let kind = match v["notification_type"].as_str() {
                Some("permission_prompt") => Kind::Permission,
                Some("idle_prompt") => Kind::Idle,
                Some(_) => return Ok(()),
                None => Kind::Other,
            };
            let body = text::first_sentence(v["message"].as_str().unwrap_or(""));
            let body = if body.is_empty() { "Claude needs you".into() } else { body };
            alert(session_id, kind, project, body)
        }
        other => return Err(format!("unknown event {other}")),
    };

    if event == "session-end" {
        // Never start a daemon just to tell it a session closed.
        return send(&msg).map(|_| ());
    }
    ensure_daemon()?;
    send(&msg).map(|_| ())
}

fn alert(session_id: String, kind: Kind, project: String, text: String) -> Msg {
    Msg::Alert { session_id, kind, project, text, ts: protocol::now_ms(), pid: claude_pid() }
}

pub fn send(msg: &Msg) -> Result<(), String> {
    let mut s = ipc::connect(&paths::socket()).map_err(|e| e.to_string())?;
    s.set_write_timeout(Some(Duration::from_millis(50))).ok();
    s.write_all(protocol::encode(msg).as_bytes()).map_err(|e| e.to_string())
}

pub fn connect_ok() -> bool {
    ipc::connect(&paths::socket()).is_ok()
}

/// Connect, or start the daemon under an exclusive lock so concurrent sessions spawn only one.
pub fn ensure_daemon() -> Result<(), String> {
    if connect_ok() {
        if !older_daemon_running() {
            return Ok(());
        }
        // A newer binary than the running daemon: replace it so only the newest one serves.
        log::line("shim: replacing older daemon");
        let _ = send(&Msg::Shutdown);
        let gone = Instant::now() + Duration::from_millis(1000);
        while connect_ok() && Instant::now() < gone {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let _ = std::fs::create_dir_all(paths::support_dir());
    let deadline = Instant::now() + Duration::from_millis(200);
    // Held until the guard drops at the end of this function.
    let _lock = lock_exclusive(deadline)?;
    if connect_ok() {
        return Ok(());
    }
    spawn_daemon()?;
    while Instant::now() < deadline + Duration::from_millis(300) {
        if connect_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Err("daemon did not come up".into())
}

/// True when the running daemon was built before this binary. A daemon with no stamp file (from
/// before stamps existed) cannot be asked to exit, so it is left alone.
fn older_daemon_running() -> bool {
    let theirs = std::fs::read_to_string(paths::daemon_stamp()).ok().and_then(|s| s.trim().parse::<u64>().ok());
    theirs.is_some_and(|t| t < crate::daemon::stamp())
}

/// Exclusive lock on the lock file, so concurrent sessions spawn only one daemon.
#[cfg(unix)]
fn lock_exclusive(deadline: Instant) -> Result<std::fs::File, String> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .open(paths::lock())
        .map_err(|e| e.to_string())?;
    while unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        if Instant::now() > deadline {
            return Err("lock timeout".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(lock)
}

/// Windows: opening with share mode 0 fails for everyone else while we hold the handle.
#[cfg(windows)]
fn lock_exclusive(deadline: Instant) -> Result<std::fs::File, String> {
    use std::os::windows::fs::OpenOptionsExt;
    loop {
        match std::fs::OpenOptions::new().create(true).write(true).truncate(false).share_mode(0).open(paths::lock()) {
            Ok(f) => return Ok(f),
            Err(e) if Instant::now() > deadline => return Err(format!("lock timeout: {e}")),
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

fn spawn_daemon() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if std::env::var_os("CLAWDTOAST_HEADLESS").is_some() {
        cmd.arg("--headless");
    }
    #[cfg(unix)]
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(&mut cmd, || {
            libc::setsid();
            Ok(())
        });
    }
    // Detached from our console and process group, so it outlives the hook and Claude Code.
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0000_0008 | 0x0000_0200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// PID of the Claude Code process: the nearest ancestor whose command name mentions "claude".
/// 0 (no liveness check) when none is found; the 24h idle backstop still applies.
fn claude_pid() -> u32 {
    if let Some(p) = std::env::var("CLAWDTOAST_PID").ok().and_then(|p| p.parse().ok()) {
        return p;
    }
    proc::find_ancestor(|name| name.to_lowercase().contains("claude")).unwrap_or(0)
}
