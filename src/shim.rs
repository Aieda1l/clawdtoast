//! `clawdtoast hook <event>`: runs inside Claude Code's hook machinery. Always exits 0, prints nothing.

use crate::protocol::{self, Kind, Msg};
use crate::{log, paths, text};
use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::time::{Duration, Instant};

pub fn run(event: &str) {
    if let Err(e) = handle(event) {
        log::line(&format!("shim {event}: {e}"));
    }
}

fn handle(event: &str) -> Result<(), String> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).map_err(|e| e.to_string())?;
    // Over SSH or on a bare VT there is no screen to draw on: do nothing, start no daemon.
    #[cfg(target_os = "linux")]
    if std::env::var_os("CLAWDTOAST_HEADLESS").is_none() && !crate::backend::has_display(|k| std::env::var(k).ok()) {
        return Ok(());
    }
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
    Msg::Alert { session_id, kind, project, text, ts: protocol::now_ms() }
}

pub fn send(msg: &Msg) -> Result<(), String> {
    let mut s = UnixStream::connect(paths::socket()).map_err(|e| e.to_string())?;
    s.set_write_timeout(Some(Duration::from_millis(50))).ok();
    s.write_all(protocol::encode(msg).as_bytes()).map_err(|e| e.to_string())
}

pub fn connect_ok() -> bool {
    UnixStream::connect(paths::socket()).is_ok()
}

/// Connect, or start the daemon under an exclusive lock so concurrent sessions spawn only one.
pub fn ensure_daemon() -> Result<(), String> {
    if connect_ok() {
        return Ok(());
    }
    let _ = std::fs::create_dir_all(paths::runtime_dir());
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .open(paths::lock())
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_millis(200);
    // Held until `lock` drops at the end of this function.
    while unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        if Instant::now() > deadline {
            return Err("lock timeout".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
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
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// PID of the Claude Code process: the nearest ancestor whose command name mentions "claude".
/// 0 (no liveness check) when none is found; the 24h idle backstop still applies.
fn claude_pid() -> u32 {
    if let Some(p) = std::env::var("CLAWDTOAST_PID").ok().and_then(|p| p.parse().ok()) {
        return p;
    }
    let Ok(out) = std::process::Command::new("ps").args(["-axo", "pid=,ppid=,comm="]).output() else {
        return 0;
    };
    let table: std::collections::HashMap<u32, (u32, String)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let ppid = it.next()?.parse().ok()?;
            Some((pid, (ppid, it.collect::<Vec<_>>().join(" "))))
        })
        .collect();
    let mut cur = unsafe { libc::getppid() } as u32;
    for _ in 0..16 {
        let Some((ppid, comm)) = table.get(&cur) else { break };
        let base = comm.rsplit('/').next().unwrap_or("");
        if base.to_lowercase().contains("claude") {
            return cur;
        }
        if *ppid <= 1 {
            break;
        }
        cur = *ppid;
    }
    0
}
