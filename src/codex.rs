//! Codex lifecycle hook adapter. Uses the same daemon and cards as the Claude Code adapter.

use crate::protocol::{self, Kind, Msg};
use crate::{log, paths, proc, shim, text};
use serde_json::{json, Value};
use std::io::Read;
use std::path::PathBuf;

/// (Codex event, argument to `clawdtoast codex-hook`)
const HOOKS: [(&str, &str); 5] = [
    ("SessionStart", "session-start"),
    ("SessionEnd", "session-end"),
    ("Stop", "stop"),
    ("SubagentStop", "subagent-stop"),
    ("PermissionRequest", "permission-request"),
];

/// Codex hooks must never block or interrupt the agent.
pub fn run_hook(event: &str) {
    if let Err(e) = handle_hook(event) {
        log::line(&format!("codex hook {event}: {e}"));
    }
    // Both Stop and SubagentStop require JSON on stdout when returning success.
    // An empty object leaves Codex's flow untouched.
    if matches!(event, "stop" | "subagent-stop") {
        println!("{{}}");
    }
}

fn handle_hook(event: &str) -> Result<(), String> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| e.to_string())?;
    // Over SSH or a bare VT there is no screen to show a card on.
    #[cfg(target_os = "linux")]
    if std::env::var_os("CLAWDTOAST_HEADLESS").is_none()
        && !crate::backend::has_display(|k| std::env::var(k).ok())
    {
        return Ok(());
    }
    let value: Value = serde_json::from_str(&input).map_err(|e| e.to_string())?;
    let Some(msg) = translate(event, &value, codex_pid(), shim::terminal_app())? else {
        return Ok(());
    };
    if matches!(msg, Msg::SessionClose { .. }) {
        // A closed session should never start a new daemon.
        shim::send(&msg)
    } else {
        shim::ensure_daemon()?;
        shim::send(&msg)
    }
}

/// Pure translation of Codex's documented hook inputs into the existing IPC protocol.
fn translate(event: &str, v: &Value, pid: u32, app: String) -> Result<Option<Msg>, String> {
    let session = v["session_id"]
        .as_str()
        .ok_or("payload has no session_id")?;
    if session.is_empty() {
        return Err("payload has empty session_id".into());
    }
    // A Codex session cannot collide with a Claude session in the shared daemon.
    let session_id = format!("codex:{session}");
    let cwd = v["cwd"].as_str().unwrap_or("");
    let project = if cwd.is_empty() {
        "codex".into()
    } else {
        text::project_name(cwd)
    };
    let alert = |kind, body| Msg::Alert {
        session_id: session_id.clone(),
        kind,
        project: project.clone(),
        text: body,
        ts: protocol::now_ms(),
        pid,
        app: app.clone(),
    };
    Ok(Some(match event {
        "session-start" => Msg::SessionOpen {
            session_id,
            pid,
            cwd: cwd.into(),
        },
        "session-end" => Msg::SessionClose { session_id },
        "stop" => {
            // Codex supplies the assistant message directly. Its transcript format is unstable.
            let body = v["last_assistant_message"]
                .as_str()
                .map(text::first_sentence)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "Finished".into());
            alert(Kind::Done, body)
        }
        "subagent-stop" => alert(Kind::Subagent, "A Codex subagent finished".into()),
        "permission-request" => {
            let tool = v["tool_name"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("a tool");
            // Avoid including raw commands or other potentially sensitive tool arguments.
            alert(Kind::Permission, format!("Codex needs approval for {tool}"))
        }
        _ => return Ok(None),
    }))
}

fn codex_pid() -> u32 {
    if let Some(pid) = std::env::var("CLAWDTOAST_PID")
        .ok()
        .and_then(|p| p.parse().ok())
    {
        return pid;
    }
    proc::find_ancestor(|name| name.to_ascii_lowercase().contains("codex")).unwrap_or(0)
}

fn codex_dir() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths::home().join(".codex"))
}

fn command(exe: &str, arg: &str) -> String {
    // Windows Codex launches hooks through a shell which can misparse a quoted
    // executable as the first token (and report exit 1 before we ever run).
    // An unquoted, shell-safe executable path works in both cmd and PowerShell.
    if cfg!(windows) {
        format!("{exe} codex-hook {arg}")
    } else {
        format!("\"{exe}\" codex-hook {arg}")
    }
}

fn is_ours(cmd: &str) -> bool {
    cmd.contains("clawdtoast") && cmd.contains(" codex-hook ")
}

/// Merge into ~/.codex/hooks.json without disturbing unrelated hooks or metadata.
pub fn merge(settings: &mut Value, exe: &str) -> Result<usize, String> {
    let root = settings
        .as_object_mut()
        .ok_or("hooks.json must be a JSON object")?;
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().ok_or("hooks must be an object")?;
    let mut changed = 0;
    for (event, arg) in HOOKS {
        let wanted = command(exe, arg);
        let groups = hooks.entry(event).or_insert_with(|| json!([]));
        let groups = groups
            .as_array_mut()
            .ok_or_else(|| format!("hooks.{event} must be an array"))?;
        let mut found = false;
        for group in groups.iter_mut() {
            if let Some(entries) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                entries.retain_mut(|h| {
                    if !h["command"].as_str().is_some_and(is_ours) {
                        return true;
                    }
                    if found {
                        changed += 1; // remove duplicate registrations from earlier installs
                        return false;
                    }
                    found = true;
                    if h["command"] != wanted {
                        h["command"] = json!(wanted);
                        changed += 1;
                    }
                    true
                });
            }
        }
        groups.retain(|g| g["hooks"].as_array().is_none_or(|h| !h.is_empty()));
        if !found {
            groups.push(json!({ "hooks": [{ "type": "command", "command": wanted }] }));
            changed += 1;
        }
    }
    Ok(changed)
}

/// Remove only ClawdToast's Codex commands.
pub fn strip(settings: &mut Value) -> usize {
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else {
        return 0;
    };
    let mut removed = 0;
    for groups in hooks.values_mut().filter_map(Value::as_array_mut) {
        for group in groups.iter_mut() {
            if let Some(entries) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                let before = entries.len();
                entries.retain(|h| !h["command"].as_str().is_some_and(is_ours));
                removed += before - entries.len();
            }
        }
        groups.retain(|g| g["hooks"].as_array().is_none_or(|h| !h.is_empty()));
    }
    hooks.retain(|_, groups| groups.as_array().is_none_or(|g| !g.is_empty()));
    if hooks.is_empty() {
        if let Some(root) = settings.as_object_mut() {
            root.remove("hooks");
        }
    }
    removed
}

/// Install or uninstall just our hooks. CODEX_HOME is honored, with backups before changes.
pub fn install(add: bool) -> Result<(), String> {
    let dir = codex_dir();
    let path = dir.join("hooks.json");
    let (mut settings, existed) = match std::fs::read_to_string(&path) {
        Ok(src) => (
            serde_json::from_str::<Value>(&src).map_err(|e| format!("{}: {e}", path.display()))?,
            true,
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (json!({}), false),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let changed = if add {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let exe = exe.to_string_lossy().replace('\\', "/");
        if exe.contains('"') || exe.contains('\n') {
            return Err("binary path cannot be quoted safely for Codex hooks".into());
        }
        if cfg!(windows)
            && !exe
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '/' | '\\' | ':' | '.' | '_' | '-'))
        {
            return Err("Codex on Windows needs clawdtoast.exe in a path without spaces or shell metacharacters; move the executable to a simple path before installing hooks".into());
        }
        merge(&mut settings, &exe)?
    } else {
        strip(&mut settings)
    };
    if changed == 0 {
        println!("{}: nothing to change", path.display());
        return Ok(());
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    if existed {
        let backup = path.with_extension(format!("json.clawdtoast-bak-{}", protocol::now_ms()));
        std::fs::copy(&path, &backup).map_err(|e| e.to_string())?;
        println!("backup: {}", backup.display());
    }
    let tmp = path.with_extension("json.clawdtoast-tmp");
    let out = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())? + "\n";
    std::fs::write(&tmp, out)
        .and_then(|_| std::fs::rename(&tmp, &path))
        .map_err(|e| e.to_string())?;
    println!(
        "{}: {} hook entr{} {}",
        path.display(),
        changed,
        if changed == 1 { "y" } else { "ies" },
        if add { "added/updated" } else { "removed" }
    );
    if add {
        println!("Open /hooks in Codex to review and trust the new hook commands.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_codex_events_without_accessing_transcript() {
        let v = json!({ "session_id": "xyz", "cwd": "/work/alpha",
            "last_assistant_message": "Fixed tests. All good.", "tool_name": "Bash" });
        assert!(
            matches!(translate("session-start", &v, 42, String::new()).unwrap(),
            Some(Msg::SessionOpen { session_id, pid: 42, .. }) if session_id == "codex:xyz")
        );
        assert!(matches!(translate("stop", &v, 42, String::new()).unwrap(),
            Some(Msg::Alert { kind: Kind::Done, text, project, .. })
                if text == "Fixed tests." && project == "alpha"));
        assert!(
            matches!(translate("permission-request", &v, 42, String::new()).unwrap(),
            Some(Msg::Alert { kind: Kind::Permission, text, .. })
                if text == "Codex needs approval for Bash")
        );
        assert!(matches!(
            translate("subagent-stop", &v, 42, String::new()).unwrap(),
            Some(Msg::Alert {
                kind: Kind::Subagent,
                ..
            })
        ));
        assert!(
            matches!(translate("session-end", &v, 42, String::new()).unwrap(),
            Some(Msg::SessionClose { session_id }) if session_id == "codex:xyz")
        );
        assert!(translate("other", &v, 0, String::new()).unwrap().is_none());
        assert!(translate("stop", &json!({}), 0, String::new()).is_err());
    }

    #[test]
    fn merges_without_affecting_unrelated_hooks() {
        let original = json!({"description": "mine",
            "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "echo hello"}]}]}});
        let mut s = original.clone();
        assert_eq!(merge(&mut s, "/bin/clawdtoast").unwrap(), 5);
        assert_eq!(s["hooks"]["Stop"][0]["hooks"][0]["command"], "echo hello");
        assert_eq!(merge(&mut s, "/bin/clawdtoast").unwrap(), 0);
        assert_eq!(merge(&mut s, "/new/clawdtoast").unwrap(), 5);
        assert_eq!(strip(&mut s), 5);
        assert_eq!(s, original);
        assert_eq!(strip(&mut s), 0);
    }

    #[test]
    fn rejects_invalid_configuration() {
        assert!(merge(&mut json!([]), "/bin/clawdtoast").is_err());
        assert!(merge(&mut json!({"hooks": {"Stop": 1}}), "/bin/clawdtoast").is_err());
    }

    #[test]
    fn windows_hook_command_has_no_quoted_program_token() {
        let cmd = command("D:/apps/clawdtoast.exe", "stop");
        if cfg!(windows) {
            assert_eq!(cmd, "D:/apps/clawdtoast.exe codex-hook stop");
        } else {
            assert_eq!(cmd, "\"D:/apps/clawdtoast.exe\" codex-hook stop");
        }
    }
}
