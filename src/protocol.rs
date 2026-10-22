//! Line-delimited JSON messages between the hook shim and the daemon.

use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Done,
    Permission,
    Idle,
    Subagent,
    Interrupted,
    #[serde(other)]
    Other,
}

impl Kind {
    pub fn status(self) -> &'static str {
        match self {
            Kind::Done => "Done",
            Kind::Permission => "Needs permission",
            Kind::Idle => "Waiting for you",
            Kind::Subagent => "Subagent done",
            Kind::Interrupted => "Interrupted",
            Kind::Other => "Claude needs you",
        }
    }
}

/// Hook-facing identity within a tool call. Options are validated by the adapter,
/// but never transmitted or displayed in desktop notifications.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Question {
    pub id: String,
    pub title: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Msg {
    SessionOpen {
        session_id: String,
        /// PID of the Claude Code process; 0 when unknown (no liveness check).
        #[serde(default)]
        pid: u32,
        #[serde(default)]
        cwd: String,
    },
    SessionClose {
        session_id: String,
    },
    Questions {
        session_id: String,
        turn_id: String,
        call_id: String,
        questions: Vec<Question>,
        asynchronous: bool,
        project: String,
        pid: u32,
        app: String,
        ts: u64,
    },
    /// Only a successful blocking question response provides confirmed answers.
    QuestionsResolved {
        session_id: String,
        turn_id: String,
        call_id: String,
        question_ids: Vec<String>,
    },
    /// Activity is not evidence that any question was answered.
    SessionActivity {
        session_id: String,
        pid: u32,
    },
    Alert {
        session_id: String,
        kind: Kind,
        /// Optional stable hook event identity; used to deduplicate Codex input prompts.
        #[serde(default)]
        event_id: String,
        #[serde(default)]
        project: String,
        #[serde(default)]
        text: String,
        /// Shim timestamp in ms since the epoch, for latency logging.
        #[serde(default)]
        ts: u64,
        /// Claude Code PID, so a session the daemon never saw open (or lost in a restart) can
        /// still focus its terminal on click.
        #[serde(default)]
        pid: u32,
        /// Bundle id of the terminal app the hook's environment points at; the click-to-focus
        /// fallback for sessions with no GUI app above them (background sessions).
        #[serde(default)]
        app: String,
        /// Which agent sent the alert ("codex"); empty means Claude Code. Picks the card sprite.
        #[serde(default)]
        agent: String,
    },
    /// Re-read config.toml.
    Reload,
    /// Exit now; sent by a newer binary that is taking over.
    Shutdown,
    #[serde(other)]
    Unknown,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Envelope {
    pub v: u32,
    #[serde(flatten)]
    pub msg: Msg,
}

pub fn encode(msg: &Msg) -> String {
    let env = Envelope {
        v: VERSION,
        msg: msg.clone(),
    };
    let mut line = serde_json::to_string(&env).unwrap_or_default();
    line.push('\n');
    line
}

pub fn decode(line: &str) -> Option<Msg> {
    serde_json::from_str::<Envelope>(line).ok().map(|e| e.msg)
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_alert() {
        let m = Msg::Alert {
            session_id: "abc1".into(),
            kind: Kind::Permission,
            event_id: String::new(),
            project: "clawdtoast".into(),
            text: "hi".into(),
            ts: 5,
            pid: 0,
            app: String::new(),
            agent: String::new(),
        };
        let back = decode(&encode(&m)).unwrap();
        assert!(matches!(
            back,
            Msg::Alert {
                kind: Kind::Permission,
                ..
            }
        ));
    }

    #[test]
    fn ignores_unknown_fields_and_types() {
        let m = decode(r#"{"v":9,"type":"session_close","session_id":"x","future":true}"#).unwrap();
        assert!(matches!(m, Msg::SessionClose { .. }));
        assert!(matches!(
            decode(r#"{"v":1,"type":"brand_new"}"#),
            Some(Msg::Unknown)
        ));
        let m = decode(r#"{"v":1,"type":"alert","session_id":"x","kind":"weird"}"#).unwrap();
        assert!(matches!(m, Msg::Alert { kind: Kind::Other, event_id, .. } if event_id.is_empty()));
    }

    #[test]
    fn garbage_is_none() {
        assert!(decode("not json").is_none());
    }
}
