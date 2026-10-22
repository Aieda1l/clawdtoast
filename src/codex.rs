//! Codex lifecycle hook adapter. Uses the same daemon and cards as the Claude Code adapter.

use crate::protocol::{self, Kind, Msg, Question};
use crate::{log, paths, proc, shim, text};
use serde_json::{json, Value};
use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// (Codex event, argument to `clawdtoast codex-hook`)
const HOOKS: [(&str, &str); 9] = [
    ("SessionStart", "session-start"),
    ("SessionEnd", "session-end"),
    ("Stop", "stop"),
    ("SubagentStop", "subagent-stop"),
    ("PermissionRequest", "permission-request"),
    // Codex has no user-input-requested hook. Observe the local function tool
    // before it blocks waiting for the answer (when that tool uses hook dispatch).
    ("PreToolUse", "pre-tool-use"),
    ("PostToolUse", "post-tool-use"),
    ("UserPromptSubmit", "user-prompt-submit"),
    ("Interrupt", "interrupt"),
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
    let value = read_payload()?;
    // Over SSH or a bare VT there is no screen to show a card on.
    #[cfg(target_os = "linux")]
    if std::env::var_os("CLAWDTOAST_HEADLESS").is_none()
        && !crate::backend::has_display(|k| std::env::var(k).ok())
    {
        return Ok(());
    }
    let Some(msg) = translate(event, &value, codex_pid(), shim::terminal_app())? else {
        return Ok(());
    };
    if matches!(
        msg,
        Msg::SessionClose { .. } | Msg::QuestionsResolved { .. } | Msg::SessionActivity { .. }
    ) {
        // A closed session should never start a new daemon.
        shim::send(&msg)
    } else {
        shim::ensure_daemon()?;
        shim::send(&msg)
    }
}

/// Codex may keep the hook's stdin pipe open after writing JSON. Read only
/// bytes already available on the pipe, parse as soon as an object is complete,
/// and exit within a bounded time even if the writer never closes the pipe.
fn read_payload() -> Result<Value, String> {
    const MAX_INPUT: usize = 1024 * 1024;
    let deadline = Instant::now() + Duration::from_millis(1000);
    let mut stdin = std::io::stdin();
    let mut input = Vec::new();
    loop {
        if !input.is_empty() {
            match serde_json::from_slice::<Value>(&input) {
                Ok(value) => return Ok(value),
                Err(e) if !e.is_eof() => return Err(format!("invalid hook JSON: {e}")),
                Err(_) => {}
            }
        }
        if input.len() >= MAX_INPUT {
            return Err("hook JSON exceeds 1 MiB".into());
        }
        if Instant::now() >= deadline {
            return Err("hook input timed out waiting for complete JSON".into());
        }

        let available = stdin_available()?;
        if available == 0 {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }
        let mut chunk = [0u8; 8192];
        let to_read = available.min(chunk.len()).min(MAX_INPUT - input.len());
        let n = stdin
            .read(&mut chunk[..to_read])
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("hook stdin closed before a complete JSON payload".into());
        }
        input.extend_from_slice(&chunk[..n]);
    }
}

#[cfg(windows)]
fn stdin_available() -> Result<usize, String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{GetFileType, FILE_TYPE_DISK};
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;

    let handle = std::io::stdin().as_raw_handle();
    let mut available = 0u32;
    let ok = unsafe {
        PeekNamedPipe(
            handle,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut available,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        // Windows also permits hooks with stdin redirected from a regular file.
        // These handles cannot be peeked, but reads never wait for a writer.
        if unsafe { GetFileType(handle) } == FILE_TYPE_DISK {
            return Ok(8192);
        }
        return Err(format!(
            "hook stdin pipe: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(available as usize)
}

#[cfg(unix)]
fn stdin_available() -> Result<usize, String> {
    use std::os::fd::AsRawFd;
    let mut fd = libc::pollfd {
        fd: std::io::stdin().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let ready = unsafe { libc::poll(&mut fd, 1, 10) };
    if ready < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // Unix pipe reads can return fewer bytes than requested; read one or more
    // available bytes when poll says the pipe has data or has closed.
    Ok(if ready == 0 { 0 } else { 8192 })
}

/// Pure translation of Codex's documented hook inputs into the existing IPC protocol.
fn translate(event: &str, v: &Value, pid: u32, app: String) -> Result<Option<Msg>, String> {
    let session = v["session_id"]
        .as_str()
        .ok_or("payload has no session_id")?;
    if session.is_empty() || session.len() > 250 {
        return Err("payload has empty or oversized session_id".into());
    }
    if ["turn_id", "tool_use_id"]
        .iter()
        .any(|key| v[key].as_str().is_some_and(|s| s.len() > 256))
    {
        return Err("hook identity is too long".into());
    }
    // A Codex session cannot collide with a Claude session in the shared daemon.
    let session_id = format!("codex:{session}");
    let cwd = v["cwd"].as_str().unwrap_or("");
    let project = if cwd.is_empty() {
        "codex".into()
    } else {
        text::project_name(cwd)
    };
    let alert = |kind, body, event_id| Msg::Alert {
        session_id: session_id.clone(),
        kind,
        event_id,
        project: project.clone(),
        text: body,
        ts: protocol::now_ms(),
        pid,
        app: app.clone(),
        agent: "codex".into(),
    };
    Ok(Some(match event {
        "session-start" => Msg::SessionOpen {
            session_id,
            pid,
            cwd: cwd.into(),
        },
        "session-end" => Msg::SessionClose { session_id },
        "user-prompt-submit" => Msg::SessionActivity { session_id, pid },
        "interrupt" => alert(
            Kind::Interrupted,
            "Codex was interrupted".into(),
            event_id(v, "interrupt"),
        ),
        "stop" => {
            // Codex supplies the assistant message directly. Its transcript format is unstable.
            let message = v["last_assistant_message"].as_str().unwrap_or("");
            if let Some(question) = final_question(message) {
                alert(Kind::Idle, question, event_id(v, "stop"))
            } else {
                let body = text::first_sentence(message);
                alert(
                    Kind::Done,
                    if body.is_empty() {
                        "Finished".into()
                    } else {
                        body
                    },
                    event_id(v, "stop"),
                )
            }
        }
        "subagent-stop" => alert(
            Kind::Subagent,
            "A Codex subagent finished".into(),
            String::new(),
        ),
        "permission-request" => {
            let tool = v["tool_name"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("a tool");
            // Avoid including raw commands or other potentially sensitive tool arguments.
            let tool: String = tool
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))
                .take(80)
                .collect();
            // PermissionRequest currently has no call id. A turn alone cannot
            // identify repeated approvals for the same tool; debounce briefly.
            let id = if v["tool_use_id"].as_str().is_some_and(|s| !s.is_empty()) {
                event_id(v, "permission")
            } else {
                String::new()
            };
            alert(
                Kind::Permission,
                format!("Codex needs approval for {tool}"),
                id,
            )
        }
        "pre-tool-use" => {
            // Only the question tool is an input pause. Other PreToolUse events
            // (including approvals) must not create waiting notifications.
            if question_tool(v) != Some(false) {
                return Ok(None);
            }
            // Auto-resolving/nonblocking questionnaires do not require a typed reply.
            if v["tool_input"]["is_blocking"] == false
                || v["tool_input"]["auto_resolution_ms"].as_u64().is_some()
            {
                return Ok(None);
            }
            let Some(questions) = parse_questions(v, false) else {
                return Ok(None);
            };
            Msg::Questions {
                session_id,
                turn_id: string(v, "turn_id"),
                call_id: question_call_id(v),
                questions,
                asynchronous: false,
                project,
                pid,
                app,
                ts: protocol::now_ms(),
            }
        }
        "post-tool-use" => {
            let Some(asynchronous) = question_tool(v) else {
                return Ok(None);
            };
            let Some(response) = tool_response(v) else {
                return Ok(None);
            };
            if response["success"] == false
                || response["is_error"] == true
                || response.get("error").is_some_and(|e| !e.is_null())
            {
                return Ok(None);
            }
            if asynchronous {
                if response["accepted"] != true {
                    return Ok(None);
                }
                let Some(questions) = parse_questions(v, true) else {
                    return Ok(None);
                };
                // The schema requires tool_use_id. Without it separate identical calls
                // cannot be distinguished reliably; do not fabricate accepted identities.
                let call_id = string(v, "tool_use_id");
                if call_id.is_empty() {
                    return Ok(None);
                }
                Msg::Questions {
                    session_id,
                    turn_id: string(v, "turn_id"),
                    call_id,
                    questions,
                    asynchronous,
                    project,
                    pid,
                    app,
                    ts: protocol::now_ms(),
                }
            } else {
                let Some(answers) = response["answers"].as_object() else {
                    return Ok(None);
                };
                let question_ids: Vec<String> = answers
                    .iter()
                    .filter(|(_, answer)| {
                        answer["answers"].as_array().is_some_and(|a| {
                            !a.is_empty()
                                && a.iter()
                                    .all(|s| s.as_str().is_some_and(|s| !s.trim().is_empty()))
                        })
                    })
                    .map(|(id, _)| id.clone())
                    .collect();
                if question_ids.is_empty() {
                    return Ok(None);
                }
                Msg::QuestionsResolved {
                    session_id,
                    turn_id: string(v, "turn_id"),
                    call_id: question_call_id(v),
                    question_ids,
                }
            }
        }
        _ => return Ok(None),
    }))
}

fn string(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or("").to_string()
}

fn question_tool(v: &Value) -> Option<bool> {
    match v["tool_name"]
        .as_str()?
        .strip_prefix("functions.")
        .unwrap_or(v["tool_name"].as_str()?)
    {
        "request_user_input" => Some(false),
        "request_user_input_async" => Some(true),
        _ => None,
    }
}

fn tool_response(v: &Value) -> Option<Value> {
    let response = &v["tool_response"];
    let response = match response.as_str() {
        Some(s) => serde_json::from_str(s).ok()?,
        None => response.clone(),
    };
    response.is_object().then_some(response)
}

fn parse_questions(v: &Value, asynchronous: bool) -> Option<Vec<Question>> {
    let input = &v["tool_input"]["questions"];
    let questions = input.as_array()?;
    // Bounded so the one-message IPC frame stays comfortably below 64 KiB.
    if questions.is_empty() || questions.len() > 64 {
        return None;
    }
    let mut ids = std::collections::HashSet::new();
    questions
        .iter()
        .enumerate()
        .map(|(index, q)| {
            let title = q[if asynchronous { "title" } else { "question" }].as_str()?;
            if title.trim().is_empty() {
                return None;
            }
            if let Some(options) = q.get("options").filter(|o| !o.is_null()) {
                let options = options.as_array()?;
                if options.is_empty()
                    || options.iter().any(|o| {
                        let label = if asynchronous {
                            o.as_str()
                        } else {
                            o["label"].as_str()
                        };
                        label.is_none_or(|s| s.trim().is_empty())
                    })
                {
                    return None;
                }
            }
            let id = if asynchronous {
                index.to_string()
            } else {
                q["id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| index.to_string())
            };
            if !ids.insert(id.clone()) {
                return None;
            }
            if id.len() > 128 {
                return None;
            }
            let title = text::question_text(title);
            if title.is_empty() {
                return None;
            }
            Some(Question { id, title })
        })
        .collect()
}

fn question_call_id(v: &Value) -> String {
    let call = string(v, "tool_use_id");
    if !call.is_empty() {
        return call;
    }
    // Compatibility for older blocking payloads without call ids. This is only
    // a debounce identity, never a claimed upstream question identifier.
    let input = v["tool_input"].to_string();
    let hash = input.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    });
    format!("compat-{hash:016x}")
}

/// Identity scoped to the current question/turn, so a retried hook can't chime twice.
fn event_id(v: &Value, prefix: &str) -> String {
    let turn = v["turn_id"].as_str().unwrap_or("");
    let tool = v["tool_use_id"].as_str().unwrap_or("");
    if turn.is_empty() && tool.is_empty() {
        return String::new();
    }
    format!("{prefix}:{turn}:{tool}")
}

/// Stop can be a plain-text request for a reply rather than a completed task.
/// Be conservative: only a final, direct question addressed to the user counts.
fn final_question(message: &str) -> Option<String> {
    let mut fenced = false;
    let mut last = "";
    for line in message.lines().filter(|line| !line.trim().is_empty()) {
        if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
            fenced = !fenced;
            last = line;
        } else {
            last = line;
        }
    }
    if fenced || last.trim_start().starts_with(['>', '`', '"', '\'']) {
        return None;
    }
    let end = message
        .trim()
        .trim_end_matches(['*', '`', '_', '"', '\'', ')', ' ', '\n']);
    if !end.ends_with('?') {
        return None;
    }
    let last_line = end.rsplit('\n').next()?.trim();
    // A sentence boundary needs whitespace after the punctuation. Periods
    // inside Node.js, config.toml, or v1.2 are part of the question itself.
    let mut start = 0;
    for (i, ch) in last_line[..last_line.len() - 1].char_indices() {
        if matches!(ch, '.' | '!' | '?')
            && last_line[i + ch.len_utf8()..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
        {
            start = i + ch.len_utf8();
        }
    }
    let question = last_line[start..]
        .trim()
        .trim_start_matches(['-', '*', '>', '#', ' ']);
    let lower = question.to_ascii_lowercase();
    let first = lower.split_whitespace().next()?;
    let interrogative = matches!(
        first,
        "what"
            | "which"
            | "when"
            | "where"
            | "how"
            | "who"
            | "why"
            | "is"
            | "are"
            | "do"
            | "does"
            | "did"
            | "can"
            | "could"
            | "would"
            | "will"
            | "should"
            | "shall"
            | "have"
            | "has"
    );
    let directed = [" you ", " your ", " i ", " me ", " we ", " us "]
        .iter().any(|s| format!(" {lower} ").contains(s))
        // Elliptical follow-ups such as "Which database?" or "What format?"
        // often omit pronouns. Exclude broad explanatory "What is ...?" questions.
        || (first == "which" && lower.split_whitespace().count() <= 10)
        || (first == "what" && lower.split_whitespace().count() <= 5
            && !matches!(lower.split_whitespace().nth(1), Some("is" | "are" | "was" | "were" | "do" | "does" | "did" | "if")));
    if interrogative && directed {
        Some(text::question_text(question))
    } else {
        None
    }
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
        format!("'{}' codex-hook {arg}", exe.replace('\'', "'\\''"))
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
        let before = groups.clone();
        // Move our entries into a dedicated group. Updating a shared group's
        // matcher would change the behavior of unrelated commands.
        for group in groups.iter_mut() {
            if let Some(entries) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                entries.retain(|h| !h["command"].as_str().is_some_and(is_ours));
            }
        }
        groups.retain(|g| g["hooks"].as_array().is_none_or(|h| !h.is_empty()));
        let mut hook = json!({"type":"command", "command":wanted, "timeout":2});
        if !matches!(event, "SessionStart" | "SessionEnd" | "UserPromptSubmit") {
            hook["async"] = json!(true);
        }
        let mut group = json!({"hooks":[hook]});
        match event {
            "PreToolUse" => group["matcher"] = json!(r"^(functions\.)?request_user_input$"),
            "PostToolUse" => {
                group["matcher"] = json!(r"^(functions\.)?request_user_input(_async)?$")
            }
            _ => {}
        }
        groups.push(group);
        if *groups != before {
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
    fn accepted_async_questions_use_post_tool_result() {
        for name in [
            "request_user_input_async",
            "functions.request_user_input_async",
        ] {
            for response in [json!({"accepted": true}), json!(r#"{"accepted":true}"#)] {
                let v = json!({"session_id":"s", "turn_id":"t", "tool_use_id":"call",
                    "tool_name":name, "tool_input":{"questions":[
                        {"title":"Which café?", "options":["One", "Two"]},
                        {"title":"When should we ship?"}]}, "tool_response":response});
                assert!(translate("pre-tool-use", &v, 0, String::new())
                    .unwrap()
                    .is_none());
                let msg = translate("post-tool-use", &v, 42, "terminal".into())
                    .unwrap()
                    .expect("accepted async questions need an attention message");
                let wire = serde_json::to_value(msg).unwrap();
                assert_eq!(wire["type"], "questions");
                assert_eq!(wire["questions"].as_array().unwrap().len(), 2);
                assert_eq!(wire["questions"][0]["title"], "Which café?");
                assert_eq!(wire["asynchronous"], true);
                assert_eq!(wire["pid"], 42);
            }
        }
    }

    #[test]
    fn async_requires_valid_acceptance_and_questions() {
        let base = json!({"session_id":"s", "tool_use_id":"call", "tool_name":"request_user_input_async",
            "tool_input":{"questions":[{"title":"Which?"}]}, "tool_response":{"accepted":true}});
        for response in [
            json!({"accepted":false}),
            json!({"accepted":1}),
            json!({"accepted":"true"}),
            json!("invalid"),
            json!(null),
            json!({"accepted":true,"success":false}),
            json!({"accepted":true,"is_error":true}),
        ] {
            let mut v = base.clone();
            v["tool_response"] = response;
            assert!(translate("post-tool-use", &v, 0, String::new())
                .unwrap()
                .is_none());
        }
        for questions in [
            json!([]),
            json!(null),
            json!([{}]),
            json!([{"title":" "}]),
            json!([{"title":1}]),
            json!([{"title":"OK?","options":[]}]),
            json!([{"title":"OK?","options":[1]}]),
            json!([{"title":"OK?"},{}]),
        ] {
            let mut v = base.clone();
            v["tool_input"]["questions"] = questions;
            assert!(translate("post-tool-use", &v, 0, String::new())
                .unwrap()
                .is_none());
        }
        let mut v = base;
        v["tool_name"] = json!("exec_command");
        assert!(translate("post-tool-use", &v, 0, String::new())
            .unwrap()
            .is_none());
    }

    #[test]
    fn stop_ignores_quoted_questions_and_code_examples() {
        for message in [
            "Example:\n```text\nCan you deploy?\n```",
            "Example:\n```text\nCan you deploy?",
            "> Can you deploy?",
            "\"Can you deploy?\"",
            "`Can you deploy?`",
        ] {
            assert!(final_question(message).is_none(), "{message}");
        }
    }

    #[test]
    fn blocking_post_response_resolves_only_confirmed_answer_ids() {
        let v = json!({"session_id":"s", "turn_id":"t", "tool_use_id":"call", "tool_name":"functions.request_user_input",
            "tool_response":r#"{"answers":{"a":{"answers":["yes"]},"b":{"answers":[]},"c":{"answers":[1]}}}"#});
        assert!(
            matches!(translate("post-tool-use", &v, 0, String::new()).unwrap(),
            Some(Msg::QuestionsResolved { question_ids, call_id, .. }) if question_ids == ["a"] && call_id == "call")
        );
    }

    #[test]
    fn translates_codex_events_without_accessing_transcript() {
        let v = json!({ "session_id": "xyz", "cwd": "/work/alpha",
            "last_assistant_message": "Fixed tests. All good.", "tool_name": "Bash",
            "tool_input": {"command":"TOKEN=PRIVATE_SECRET_SENTINEL command", "description":"PRIVATE_DESCRIPTION_SENTINEL"} });
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
        let permission = translate("permission-request", &v, 42, String::new())
            .unwrap()
            .unwrap();
        assert!(!serde_json::to_string(&permission)
            .unwrap()
            .contains("SENTINEL"));
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
    fn notifies_for_structured_questions_only() {
        let payload = json!({ "session_id": "s", "cwd": "/work/app", "turn_id": "turn1",
            "tool_use_id": "call1", "tool_name": "request_user_input",
            "tool_input": { "questions": [{ "question": "Which database should I use?",
                "options": [{ "label": "SQLite" }, { "label": "Postgres" }] }] } });
        assert!(
            matches!(translate("pre-tool-use", &payload, 15, String::new()).unwrap(),
            Some(Msg::Questions { questions, turn_id, call_id, asynchronous: false, .. })
            if questions[0].title == "Which database should I use?" && turn_id == "turn1" && call_id == "call1")
        );
        let mut with_colon = payload.clone();
        with_colon["tool_input"]["questions"][0]["question"] =
            json!("Which environment: staging or production?");
        assert!(
            matches!(translate("pre-tool-use", &with_colon, 15, String::new()).unwrap(),
            Some(Msg::Questions { questions, .. })
            if questions[0].title == "Which environment: staging or production?")
        );
        for tool in ["Bash", "apply_patch", "update_plan", "mcp__server__tool"] {
            let mut other = payload.clone();
            other["tool_name"] = json!(tool);
            assert!(translate("pre-tool-use", &other, 0, String::new())
                .unwrap()
                .is_none());
        }
        let mut no_question = payload.clone();
        no_question["tool_input"] = json!({ "questions": [] });
        assert!(translate("pre-tool-use", &no_question, 0, String::new())
            .unwrap()
            .is_none());
        for input in [
            json!({"is_blocking": false}),
            json!({"auto_resolution_ms": 500}),
        ] {
            let mut auto = payload.clone();
            for (key, val) in input.as_object().unwrap() {
                auto["tool_input"][key] = val.clone();
            }
            assert!(translate("pre-tool-use", &auto, 0, String::new())
                .unwrap()
                .is_none());
        }
    }

    #[test]
    fn stop_distinguishes_follow_up_from_completion() {
        for (message, expected) in [
            (
                "I have two options. Which approach should I use?",
                "Which approach should I use?",
            ),
            (
                "Can you clarify the target platform?",
                "Can you clarify the target platform?",
            ),
            (
                "**Would you like me to continue?**",
                "Would you like me to continue?",
            ),
            ("Which database?", "Which database?"),
            ("What file format?", "What file format?"),
            ("Can I use Node.js?", "Can I use Node.js?"),
            ("Can we ship v1.2?", "Can we ship v1.2?"),
            ("Should I edit config.toml?", "Should I edit config.toml?"),
            (
                "I checked README.md. Can I use Node.js?",
                "Can I use Node.js?",
            ),
            (
                "Which environment: staging or production?",
                "Which environment: staging or production?",
            ),
        ] {
            let v = json!({ "session_id": "s", "turn_id": "t", "last_assistant_message": message });
            assert!(matches!(translate("stop", &v, 0, String::new()).unwrap(),
                Some(Msg::Alert { kind: Kind::Idle, text, event_id, .. })
                if text == expected && event_id == "stop:t:"));
        }
        for message in [
            "Finished implementing the parser.",
            "What is 2+2?",
            "What is a socket? Here is the answer.",
            "",
            "Use `help?` to learn more.",
        ] {
            let v = json!({ "session_id": "s", "last_assistant_message": message });
            assert!(matches!(
                translate("stop", &v, 0, String::new()).unwrap(),
                Some(Msg::Alert {
                    kind: Kind::Done,
                    ..
                })
            ));
        }
    }

    #[test]
    fn merges_without_affecting_unrelated_hooks() {
        let original = json!({"description": "mine",
        "hooks": {
            "Stop": [{"hooks": [{"type": "command", "command": "echo hello"}]}],
            "PreToolUse": [{"matcher": "^Bash$", "hooks": [{"type": "command", "command": "echo other"}]}]
        }});
        let mut s = original.clone();
        assert_eq!(merge(&mut s, "/bin/clawdtoast").unwrap(), 9);
        assert_eq!(s["hooks"]["PreToolUse"][0]["matcher"], "^Bash$");
        assert_eq!(
            s["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "echo other"
        );
        assert_eq!(
            s["hooks"]["PreToolUse"][1]["matcher"],
            "^(functions\\.)?request_user_input$"
        );
        assert_eq!(s["hooks"]["Stop"][0]["hooks"][0]["command"], "echo hello");
        assert_eq!(merge(&mut s, "/bin/clawdtoast").unwrap(), 0);
        assert_eq!(merge(&mut s, "/new/clawdtoast").unwrap(), 9);
        assert_eq!(strip(&mut s), 9);
        assert_eq!(s, original);
        assert_eq!(strip(&mut s), 0);
    }

    #[test]
    fn upgrade_moves_our_hooks_out_of_shared_matcher_groups() {
        let mut s = json!({"hooks":{"PreToolUse":[{"matcher":"^request_user_input$", "hooks":[
            {"type":"command", "command":"old/clawdtoast codex-hook pre-tool-use"},
            {"type":"command", "command":"echo other"}]}]}});
        merge(&mut s, "/bin/clawdtoast").unwrap();
        assert_eq!(
            s["hooks"]["PreToolUse"][0]["matcher"],
            "^request_user_input$"
        );
        assert_eq!(
            s["hooks"]["PreToolUse"][0]["hooks"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            s["hooks"]["PreToolUse"][1]["matcher"],
            "^(functions\\.)?request_user_input$"
        );
        assert_eq!(
            s["hooks"]["PostToolUse"][0]["matcher"],
            "^(functions\\.)?request_user_input(_async)?$"
        );
        assert_eq!(s["hooks"]["PostToolUse"][0]["hooks"][0]["async"], true);
        assert_eq!(merge(&mut s, "/bin/clawdtoast").unwrap(), 0);
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
            assert_eq!(cmd, "'D:/apps/clawdtoast.exe' codex-hook stop");
        }
    }
}
