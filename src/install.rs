//! `clawdtoast install` / `uninstall`: add or remove clawdtoast's hooks in `~/.claude/settings.json`
//! without touching anyone else's. (The Claude Code plugin does the same job without editing settings.)

use serde_json::{json, Value};
use std::path::PathBuf;

/// (Claude Code event, `clawdtoast hook` argument, notification matcher)
const HOOKS: [(&str, &str, Option<&str>); 5] = [
    ("SessionStart", "session-start", None),
    ("SessionEnd", "session-end", None),
    ("Stop", "stop", None),
    ("SubagentStop", "subagent-stop", None),
    ("Notification", "notification", Some("permission_prompt|idle_prompt")),
];

fn is_ours(cmd: &str) -> bool {
    cmd.contains("clawdtoast") && cmd.contains(" hook ")
}

fn command_for(exe: &str, arg: &str) -> String {
    format!("\"{exe}\" hook {arg}")
}

fn group_hooks(group: &mut Value) -> Option<&mut Vec<Value>> {
    group.get_mut("hooks")?.as_array_mut()
}

/// Adds (or re-points) clawdtoast's hooks. Returns how many events changed.
pub fn merge(settings: &mut Value, exe: &str) -> Result<usize, String> {
    let root = settings.as_object_mut().ok_or("settings.json is not a JSON object")?;
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().ok_or("\"hooks\" is not an object")?;
    let mut changed = 0;
    for (event, arg, matcher) in HOOKS {
        let cmd = command_for(exe, arg);
        let arr = hooks.entry(event).or_insert_with(|| json!([]));
        let arr = arr.as_array_mut().ok_or_else(|| format!("hooks.{event} is not an array"))?;
        let mut found = false;
        for group in arr.iter_mut() {
            for h in group_hooks(group).into_iter().flatten() {
                if h["command"].as_str().is_some_and(is_ours) {
                    found = true;
                    if h["command"] != json!(cmd) {
                        h["command"] = json!(cmd);
                        changed += 1;
                    }
                }
            }
        }
        if !found {
            let mut group = json!({ "hooks": [{ "type": "command", "command": cmd }] });
            if let Some(m) = matcher {
                group["matcher"] = json!(m);
            }
            arr.push(group);
            changed += 1;
        }
    }
    Ok(changed)
}

/// Removes only clawdtoast's hook entries, pruning groups and events left empty. Returns the count removed.
pub fn strip(settings: &mut Value) -> usize {
    let mut removed = 0;
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else { return 0 };
    for arr in hooks.values_mut().filter_map(Value::as_array_mut) {
        for group in arr.iter_mut() {
            if let Some(hs) = group_hooks(group) {
                let before = hs.len();
                hs.retain(|h| !h["command"].as_str().is_some_and(is_ours));
                removed += before - hs.len();
            }
        }
        arr.retain(|g| g["hooks"].as_array().is_none_or(|h| !h.is_empty()));
    }
    hooks.retain(|_, v| v.as_array().is_none_or(|a| !a.is_empty()));
    if hooks.is_empty() {
        if let Some(o) = settings.as_object_mut() {
            o.remove("hooks");
        }
    }
    removed
}

fn claude_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return PathBuf::from(d);
    }
    crate::paths::home().join(".claude")
}

fn plugin_installed(dir: &std::path::Path) -> bool {
    std::fs::read_to_string(dir.join("plugins/installed_plugins.json")).is_ok_and(|s| s.contains("clawdtoast@"))
}

pub fn run(install: bool, force: bool) -> Result<(), String> {
    let dir = claude_dir();
    let path = dir.join("settings.json");
    if install && !force && plugin_installed(&dir) {
        return Err("the clawdtoast plugin is installed; its hooks would fire twice. \
            Uninstall the plugin, or pass --force."
            .into());
    }
    let (mut settings, existed) = match std::fs::read_to_string(&path) {
        Ok(src) => (serde_json::from_str::<Value>(&src).map_err(|e| format!("{}: {e}", path.display()))?, true),
        Err(_) => (json!({}), false),
    };
    let n = if install {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        // Claude Code runs hook commands through a POSIX shell, even on Windows (Git Bash), where
        // backslashes in a path would be eaten.
        merge(&mut settings, &exe.to_string_lossy().replace('\\', "/"))?
    } else {
        strip(&mut settings)
    };
    if n == 0 {
        println!("{}: nothing to change", path.display());
        return Ok(());
    }
    if existed {
        let backup = path.with_extension(format!("json.clawdtoast-bak-{}", crate::protocol::now_ms() / 1000));
        std::fs::copy(&path, &backup).map_err(|e| e.to_string())?;
        println!("backup: {}", backup.display());
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.clawdtoast-tmp");
    let out = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())? + "\n";
    std::fs::write(&tmp, out).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| e.to_string())?;
    println!("{}: {} hook entr{} {}", path.display(), n, if n == 1 { "y" } else { "ies" }, if install { "added/updated" } else { "removed" });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "/opt/bin/clawdtoast";

    fn user_settings() -> Value {
        json!({
            "model": "opus",
            "hooks": {
                "Stop": [{ "hooks": [{ "type": "command", "command": "say done" }] }],
                "PreToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "lint.sh" }] }]
            }
        })
    }

    #[test]
    fn merge_keeps_existing_hooks_and_is_idempotent() {
        let mut s = user_settings();
        assert_eq!(merge(&mut s, EXE).unwrap(), 5);
        assert_eq!(s["model"], "opus");
        assert_eq!(s["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(s["hooks"]["Stop"][0]["hooks"][0]["command"], "say done");
        assert_eq!(s["hooks"]["Notification"][0]["matcher"], "permission_prompt|idle_prompt");
        assert_eq!(merge(&mut s, EXE).unwrap(), 0, "second run changes nothing");
    }

    #[test]
    fn merge_repoints_a_moved_binary() {
        let mut s = json!({});
        merge(&mut s, EXE).unwrap();
        assert_eq!(merge(&mut s, "/new/clawdtoast").unwrap(), 5);
        assert_eq!(s["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert_eq!(s["hooks"]["Stop"][0]["hooks"][0]["command"], "\"/new/clawdtoast\" hook stop");
    }

    #[test]
    fn strip_restores_the_original() {
        let original = user_settings();
        let mut s = original.clone();
        merge(&mut s, EXE).unwrap();
        assert_eq!(strip(&mut s), 5);
        assert_eq!(s, original);
        assert_eq!(strip(&mut s), 0);
    }

    #[test]
    fn strip_drops_empty_hooks_object() {
        let mut s = json!({ "a": 1 });
        merge(&mut s, EXE).unwrap();
        strip(&mut s);
        assert_eq!(s, json!({ "a": 1 }));
    }

    #[test]
    fn rejects_malformed_settings() {
        assert!(merge(&mut json!([]), EXE).is_err());
        assert!(merge(&mut json!({ "hooks": { "Stop": 3 } }), EXE).is_err());
    }
}
