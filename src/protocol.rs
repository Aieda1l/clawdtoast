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
            Kind::Other => "Claude needs you",
        }
    }
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
    Alert {
        session_id: String,
        kind: Kind,
        #[serde(default)]
        project: String,
        #[serde(default)]
        text: String,
        /// Shim timestamp in ms since the epoch, for latency logging.
        #[serde(default)]
        ts: u64,
    },
    /// Re-read config.toml.
    Reload,
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
    let env = Envelope { v: VERSION, msg: msg.clone() };
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
            project: "clawdtoast".into(),
            text: "hi".into(),
            ts: 5,
        };
        let back = decode(&encode(&m)).unwrap();
        assert!(matches!(back, Msg::Alert { kind: Kind::Permission, .. }));
    }

    #[test]
    fn ignores_unknown_fields_and_types() {
        let m = decode(r#"{"v":9,"type":"session_close","session_id":"x","future":true}"#).unwrap();
        assert!(matches!(m, Msg::SessionClose { .. }));
        assert!(matches!(decode(r#"{"v":1,"type":"brand_new"}"#), Some(Msg::Unknown)));
        let m = decode(r#"{"v":1,"type":"alert","session_id":"x","kind":"weird"}"#).unwrap();
        assert!(matches!(m, Msg::Alert { kind: Kind::Other, .. }));
    }

    #[test]
    fn garbage_is_none() {
        assert!(decode("not json").is_none());
    }
}
