//! Daemon state that does not depend on the UI: session registry, liveness, exit rules.

use crate::protocol::{Kind, Msg};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const IDLE_BACKSTOP: Duration = Duration::from_secs(24 * 3600);

pub struct Session {
    pub pid: u32,
}

#[derive(Debug, Clone)]
pub struct Card {
    pub session_id: String,
    pub kind: Kind,
    pub project: String,
    pub body: String,
    /// Short session tag, set only when more than one session is open.
    pub badge: Option<String>,
    pub ts: u64,
}

impl Card {
    pub fn title(&self) -> String {
        match &self.badge {
            Some(b) => format!("{} · {} #{}", self.kind.status(), self.project, b),
            None => format!("{} · {}", self.kind.status(), self.project),
        }
    }
}

pub struct Core {
    pub sessions: HashMap<String, Session>,
    empty_since: Option<Instant>,
    last_activity: Instant,
    next_liveness: Instant,
    grace: Duration,
    liveness: Duration,
    subagent_alerts: bool,
}

fn env_secs(name: &str, default: u64) -> Duration {
    Duration::from_secs(std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default))
}

impl Core {
    pub fn new(now: Instant) -> Core {
        let liveness = env_secs("CLAWDPET_LIVENESS_SECS", 30);
        Core {
            sessions: HashMap::new(),
            empty_since: Some(now),
            last_activity: now,
            next_liveness: now + liveness,
            grace: env_secs("CLAWDPET_GRACE_SECS", 10),
            liveness,
            subagent_alerts: std::env::var_os("CLAWDPET_SUBAGENT").is_some(),
        }
    }

    /// Applies a message; returns a card to show for alerts.
    pub fn handle(&mut self, msg: Msg, now: Instant) -> Option<Card> {
        self.last_activity = now;
        match msg {
            Msg::SessionOpen { session_id, pid, .. } => {
                self.sessions.insert(session_id, Session { pid });
                self.empty_since = None;
                None
            }
            Msg::SessionClose { session_id } => {
                self.sessions.remove(&session_id);
                self.note_empty(now);
                None
            }
            Msg::Alert { session_id, kind, project, text, ts } => {
                if kind == Kind::Subagent && !self.subagent_alerts {
                    return None;
                }
                // An alert from a session we never saw open (plugin installed mid-session).
                if !self.sessions.contains_key(&session_id) {
                    self.sessions.insert(session_id.clone(), Session { pid: 0 });
                    self.empty_since = None;
                }
                let badge = (self.sessions.len() > 1).then(|| session_id.chars().take(4).collect());
                Some(Card { session_id, kind, project, body: text, badge, ts })
            }
            Msg::Unknown => None,
        }
    }

    fn note_empty(&mut self, now: Instant) {
        if self.sessions.is_empty() && self.empty_since.is_none() {
            self.empty_since = Some(now);
        }
    }

    /// Runs due timers. Returns true when the daemon should exit.
    pub fn tick(&mut self, now: Instant, card_visible: bool) -> bool {
        if now >= self.next_liveness {
            self.next_liveness = now + self.liveness;
            self.sessions.retain(|_, s| s.pid == 0 || pid_alive(s.pid));
            self.note_empty(now);
        }
        if now.duration_since(self.last_activity) > IDLE_BACKSTOP {
            return true;
        }
        match self.empty_since {
            Some(t) => !card_visible && now.duration_since(t) >= self.grace,
            None => false,
        }
    }

    /// Next instant `tick` needs to run, ignoring the UI's own timers.
    pub fn next_deadline(&self) -> Instant {
        let mut d = self.next_liveness;
        if let Some(t) = self.empty_since {
            d = d.min(t + self.grace);
        }
        d
    }
}

fn pid_alive(pid: u32) -> bool {
    let r = unsafe { libc::kill(pid as i32, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alert(sid: &str, kind: Kind) -> Msg {
        Msg::Alert { session_id: sid.into(), kind, project: "p".into(), text: "t".into(), ts: 0 }
    }

    #[test]
    fn badge_only_with_multiple_sessions() {
        let now = Instant::now();
        let mut c = Core::new(now);
        c.handle(Msg::SessionOpen { session_id: "aaaa1".into(), pid: 0, cwd: "".into() }, now);
        assert!(c.handle(alert("aaaa1", Kind::Done), now).unwrap().badge.is_none());
        c.handle(Msg::SessionOpen { session_id: "bbbb2".into(), pid: 0, cwd: "".into() }, now);
        assert_eq!(c.handle(alert("bbbb2", Kind::Done), now).unwrap().badge.as_deref(), Some("bbbb"));
    }

    #[test]
    fn subagent_alerts_off_by_default() {
        let now = Instant::now();
        let mut c = Core::new(now);
        assert!(c.handle(alert("s", Kind::Subagent), now).is_none());
    }

    #[test]
    fn exits_after_grace_when_empty() {
        let t0 = Instant::now();
        let mut c = Core::new(t0);
        c.handle(Msg::SessionOpen { session_id: "s".into(), pid: 0, cwd: "".into() }, t0);
        assert!(!c.tick(t0 + Duration::from_secs(60), false));
        c.handle(Msg::SessionClose { session_id: "s".into() }, t0);
        assert!(!c.tick(t0 + Duration::from_secs(5), false));
        assert!(!c.tick(t0 + Duration::from_secs(11), true), "card still visible");
        assert!(c.tick(t0 + Duration::from_secs(11), false));
    }

    #[test]
    fn drops_dead_pids() {
        let t0 = Instant::now();
        let mut c = Core::new(t0);
        // PID far above any real one: kill(pid, 0) fails with ESRCH.
        c.handle(Msg::SessionOpen { session_id: "s".into(), pid: 4_000_000, cwd: "".into() }, t0);
        c.tick(t0 + Duration::from_secs(31), false);
        assert!(c.sessions.is_empty());
    }
}
