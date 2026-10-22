//! Turning hook payloads and transcripts into the one line a card shows.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

const TAIL_BYTES: u64 = 256 * 1024;
const MAX_CHARS: usize = 200;

/// First sentence of `s`, markdown noise stripped, capped at `MAX_CHARS`.
pub fn first_sentence(s: &str) -> String {
    let s = s.trim().trim_start_matches(|c: char| matches!(c, '#' | '*' | '-' | '>' | '`' | ' '));
    let mut end = s.len();
    let mut prev_end = false;
    for (i, c) in s.char_indices() {
        if c == '\n' {
            end = i;
            break;
        }
        if prev_end && c == ' ' {
            end = i;
            break;
        }
        prev_end = matches!(c, '.' | '!' | '?' | ':');
    }
    let out: String = s[..end].trim().trim_end_matches(':').chars().take(MAX_CHARS).collect();
    out.replace('`', "").replace("**", "")
}

/// A prompt to show on a card. Keep its punctuation, including colons and
/// periods inside filenames/versions, rather than treating it as a summary.
pub fn question_text(s: &str) -> String {
    let s = s.trim().trim_start_matches(|c: char| matches!(c, '#' | '*' | '-' | '>' | '`' | ' '));
    s.lines().next().unwrap_or("").trim().chars().take(MAX_CHARS).collect::<String>()
        .replace('`', "").replace("**", "")
}

/// Text of the last assistant message in a Claude Code transcript (JSONL), read from the end.
pub fn last_assistant_text(path: &str) -> Option<String> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = String::new();
    f.take(TAIL_BYTES).read_to_string(&mut buf).ok()?;
    let mut lines: Vec<&str> = buf.lines().collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0); // probably cut mid-line
    }
    for line in lines.iter().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let msg = &v["message"];
        if msg["role"] != "assistant" {
            continue;
        }
        let text = match &msg["content"] {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Array(blocks) => blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            _ => continue,
        };
        if !text.trim().is_empty() {
            return Some(text);
        }
    }
    None
}

pub fn project_name(cwd: &str) -> String {
    std::path::Path::new(cwd)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "claude".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentences() {
        assert_eq!(first_sentence("Wrote the doc. Then did more."), "Wrote the doc.");
        assert_eq!(first_sentence("## Done!\nbody"), "Done!");
        assert_eq!(first_sentence("Use `cargo build` now"), "Use cargo build now");
        assert_eq!(first_sentence("v1.2 shipped"), "v1.2 shipped");
        assert_eq!(first_sentence("  "), "");
        assert_eq!(first_sentence(&"a".repeat(500)).chars().count(), 200);
    }

    #[test]
    fn questions_keep_internal_punctuation() {
        assert_eq!(question_text("Which environment: staging or production?"), "Which environment: staging or production?");
        assert_eq!(question_text("Can I use Node.js and config.toml with v1.2?"), "Can I use Node.js and config.toml with v1.2?");
        assert_eq!(question_text("**Which option?**"), "Which option?");
        assert_eq!(question_text(&"a".repeat(500)).chars().count(), 200);
    }

    #[test]
    fn transcript_tail() {
        let dir = std::env::temp_dir().join(format!("clawdtoast-t-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.jsonl");
        std::fs::write(
            &p,
            concat!(
                r#"{"message":{"role":"assistant","content":[{"type":"text","text":"Old answer."}]}}"#, "\n",
                r#"{"message":{"role":"user","content":"hi"}}"#, "\n",
                r#"{"message":{"role":"assistant","content":[{"type":"text","text":"Fixed the bug. More."},{"type":"tool_use","name":"x"}]}}"#, "\n",
                r#"{"message":{"role":"assistant","content":[{"type":"tool_use","name":"x"}]}}"#, "\n",
            ),
        )
        .unwrap();
        let t = last_assistant_text(p.to_str().unwrap()).unwrap();
        assert_eq!(first_sentence(&t), "Fixed the bug.");
        assert!(last_assistant_text("/nonexistent").is_none());
    }

    #[test]
    fn project() {
        assert_eq!(project_name("/Users/me/ws/clawdtoast"), "clawdtoast");
        assert_eq!(project_name(""), "claude");
    }
}
