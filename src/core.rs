//! Daemon state that does not depend on the UI: session registry, liveness, exit rules.

use crate::config::Config;
use crate::protocol::{Kind, Msg};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

const IDLE_BACKSTOP: Duration = Duration::from_secs(24 * 3600);
const UNIDENTIFIED_PROMPT_DEBOUNCE: Duration = Duration::from_secs(2);

struct WaitingPrompt {
    id: String,
    text: String,
    at: Instant,
}

/// Arrival-ordered observations, including answered tombstones for out-of-order hooks.
struct PendingQuestion {
    turn: String,
    call: String,
    id: String,
    title: String,
    asynchronous: bool,
    resolved: bool,
    at: Instant,
}

/// A bounded restart ledger. Never persist question text, options, commands or answers.
#[derive(Serialize, Deserialize)]
pub struct QuestionCheckpoint {
    version: u32,
    entries: Vec<SavedQuestion>,
}

#[derive(Serialize, Deserialize)]
struct SavedQuestion {
    session: String,
    pid: u32,
    turn: String,
    call: String,
    id: String,
    asynchronous: bool,
    resolved: bool,
    expires_at: u64,
}

fn waiting_turn(id: &str) -> Option<&str> {
    let (_, rest) = id.split_once(':')?;
    let (turn, _) = rest.split_once(':')?;
    (!turn.is_empty()).then_some(turn)
}

pub struct Session {
    pub pid: u32,
    last_seen: Instant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Card {
    pub session_id: String,
    pub kind: Kind,
    pub project: String,
    pub body: String,
    /// Short session tag, set only when more than one session is open.
    pub badge: Option<String>,
    pub ts: u64,
    /// PID of the Claude Code process, used to focus its terminal on click; 0 when unknown.
    #[serde(default)]
    pub pid: u32,
    /// Bundle id to focus when `pid` leads to no app; empty when unknown.
    #[serde(default)]
    pub app: String,
    /// Agent that sent the alert ("codex"); empty for Claude Code. Picks the card sprite.
    #[serde(default)]
    pub agent: String,
    /// Observed questions without confirmed resolution, not an authoritative client count.
    #[serde(default)]
    pub pending_count: usize,
    /// Attention cards use an absolute deadline that hover cannot extend.
    #[serde(default)]
    pub expires_at: u64,
    /// Silent update/removal controls only affect an existing attention card.
    #[serde(default)]
    pub silent: bool,
    #[serde(default)]
    pub dismiss: bool,
}

impl Card {
    pub fn title(&self) -> String {
        let title = match &self.badge {
            Some(b) => format!("{} · {} #{}", self.kind.status(), self.project, b),
            None => format!("{} · {}", self.kind.status(), self.project),
        };
        if self.pending_count > 0 {
            format!("{title} ({} unconfirmed)", self.pending_count)
        } else {
            title
        }
    }

    pub fn hold(&self, normal: Duration) -> Duration {
        if self.expires_at == 0 {
            normal
        } else {
            Duration::from_millis(self.expires_at.saturating_sub(self.ts))
        }
    }

    pub fn expired(&self, now_ms: u64) -> bool {
        self.expires_at > 0 && now_ms >= self.expires_at
    }

    pub fn slot(&self) -> String {
        if self.pending_count > 0
            || self.silent
            || (self.kind == Kind::Idle && self.agent == "codex")
        {
            format!("{}:attention", self.session_id)
        } else {
            self.session_id.clone()
        }
    }
}

pub struct Core {
    pub sessions: HashMap<String, Session>,
    /// Recent Codex prompt identities per session; keep retries from chiming again.
    last_waiting: HashMap<String, VecDeque<WaitingPrompt>>,
    pending: HashMap<String, VecDeque<PendingQuestion>>,
    effects: VecDeque<Card>,
    attention_cards: HashMap<String, Card>,
    closed: HashMap<String, Instant>,
    recent_alerts: HashMap<String, VecDeque<WaitingPrompt>>,
    pub revision: u64,
    empty_since: Option<Instant>,
    last_activity: Instant,
    next_liveness: Instant,
    grace: Duration,
    liveness: Duration,
    pub config: Config,
}

fn env_secs(name: &str, default: u64) -> Duration {
    Duration::from_secs(
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default),
    )
}

impl Core {
    pub fn new(now: Instant, config: Config) -> Core {
        let liveness = env_secs("CLAWDTOAST_LIVENESS_SECS", 30);
        Core {
            sessions: HashMap::new(),
            last_waiting: HashMap::new(),
            pending: HashMap::new(),
            effects: VecDeque::new(),
            attention_cards: HashMap::new(),
            closed: HashMap::new(),
            recent_alerts: HashMap::new(),
            revision: 0,
            empty_since: Some(now),
            last_activity: now,
            next_liveness: now + liveness,
            grace: env_secs("CLAWDTOAST_GRACE_SECS", 10),
            liveness,
            config,
        }
    }

    /// Applies a message; returns a card to show for alerts.
    pub fn handle(&mut self, msg: Msg, now: Instant) -> Option<Card> {
        self.last_activity = now;
        match msg {
            Msg::SessionOpen {
                session_id, pid, ..
            } => {
                self.revision += 1;
                self.closed.remove(&session_id);
                // A replayed SessionStart/resume is not a question resolution.
                self.sessions.insert(
                    session_id,
                    Session {
                        pid,
                        last_seen: now,
                    },
                );
                self.empty_since = None;
                None
            }
            Msg::SessionClose { session_id } => {
                self.close_session(&session_id, now);
                self.note_empty(now);
                None
            }
            Msg::SessionActivity { session_id, pid } => {
                self.closed.remove(&session_id);
                let s = self.sessions.entry(session_id).or_insert(Session {
                    pid,
                    last_seen: now,
                });
                s.last_seen = now;
                if pid != 0 {
                    s.pid = pid;
                }
                self.empty_since = None;
                None
            }
            Msg::Questions {
                session_id,
                turn_id,
                call_id,
                questions,
                asynchronous,
                project,
                pid,
                app,
                ts,
            } => {
                if self.closed.contains_key(&session_id) {
                    return None;
                }
                let pending = self.pending.entry(session_id.clone()).or_default();
                let mut titles = Vec::new();
                for q in questions.into_iter().take(64) {
                    if pending
                        .iter()
                        .any(|old| old.turn == turn_id && old.call == call_id && old.id == q.id)
                    {
                        continue;
                    }
                    titles.push(q.title.clone());
                    if pending.len() >= 512 {
                        pending.pop_front();
                    }
                    pending.push_back(PendingQuestion {
                        turn: turn_id.clone(),
                        call: call_id.clone(),
                        id: q.id,
                        title: q.title,
                        asynchronous,
                        resolved: false,
                        at: now,
                    });
                }
                if !titles.is_empty() {
                    self.revision += 1;
                }
                let s = self.sessions.entry(session_id.clone()).or_insert(Session {
                    pid,
                    last_seen: now,
                });
                s.last_seen = now;
                if pid != 0 {
                    s.pid = pid;
                }
                self.empty_since = None;
                let enabled = self.config.events.idle_prompt
                    && if asynchronous {
                        self.config.events.async_questions
                    } else {
                        self.config.events.blocking_questions
                    };
                if titles.is_empty() || !enabled {
                    return None;
                }
                let body = crate::text::question_text(&titles.join(" · "));
                self.handle(
                    Msg::Alert {
                        session_id,
                        kind: Kind::Idle,
                        event_id: format!("input:{turn_id}:{call_id}"),
                        project,
                        text: body,
                        ts,
                        pid,
                        app,
                        agent: "codex".into(),
                    },
                    now,
                )
            }
            Msg::QuestionsResolved {
                session_id,
                turn_id,
                call_id,
                question_ids,
            } => {
                if self.closed.contains_key(&session_id) {
                    return None;
                }
                self.revision += 1;
                let pending = self.pending.entry(session_id.clone()).or_default();
                for id in question_ids.into_iter().take(64) {
                    if let Some(q) = pending.iter_mut().find(|q| {
                        q.turn == turn_id && q.call == call_id && q.id == id && !q.asynchronous
                    }) {
                        q.resolved = true;
                    } else {
                        if pending.len() >= 512 {
                            pending.pop_front();
                        }
                        pending.push_back(PendingQuestion {
                            turn: turn_id.clone(),
                            call: call_id.clone(),
                            id,
                            title: String::new(),
                            asynchronous: false,
                            resolved: true,
                            at: now,
                        });
                    }
                }
                self.update_attention(&session_id);
                None
            }
            Msg::Alert {
                session_id,
                kind,
                event_id,
                project,
                text,
                ts,
                pid,
                app,
                agent,
            } => {
                if self.closed.contains_key(&session_id) {
                    return None;
                }
                // A final assistant message cannot re-alert a structured question.
                // Compare turn identities, not truncated text or matching punctuation.
                if agent == "codex"
                    && kind == Kind::Idle
                    && event_id.starts_with("stop:")
                    && self.pending.get(&session_id).is_some_and(|qs| {
                        qs.iter()
                            .any(|q| waiting_turn(&event_id).is_some_and(|turn| turn == q.turn))
                    })
                {
                    return None;
                }
                let e = &self.config.events;
                let enabled = match kind {
                    Kind::Done => e.stop,
                    Kind::Permission => e.permission_prompt,
                    Kind::Idle => e.idle_prompt,
                    Kind::Subagent => e.subagent_stop,
                    Kind::Interrupted => e.interrupted,
                    Kind::Other => true,
                };
                if !enabled {
                    return None;
                }
                if agent == "codex" && kind != Kind::Idle {
                    let recent = self.recent_alerts.entry(session_id.clone()).or_default();
                    if recent.iter().any(|old| {
                        (!event_id.is_empty() && old.id == event_id)
                            || (kind == Kind::Permission
                                && old.text == text
                                && now.saturating_duration_since(old.at)
                                    < UNIDENTIFIED_PROMPT_DEBOUNCE)
                    }) {
                        return None;
                    }
                    if recent.len() >= 128 {
                        recent.pop_front();
                    }
                    recent.push_back(WaitingPrompt {
                        id: event_id.clone(),
                        text: text.clone(),
                        at: now,
                    });
                }
                let mut silent = false;
                if kind == Kind::Idle && agent == "codex" {
                    let seen = self.last_waiting.entry(session_id.clone()).or_default();
                    // Async command hooks may finish out of order. Upgrade an
                    // earlier heuristic card in place, without a second ping.
                    silent = event_id.starts_with("input:")
                        && seen.iter().any(|old| {
                            old.id.starts_with("stop:")
                                && waiting_turn(&old.id)
                                    .zip(waiting_turn(&event_id))
                                    .is_some_and(|(a, b)| a == b)
                        })
                        && !seen.iter().any(|old| {
                            old.id.starts_with("input:")
                                && waiting_turn(&old.id)
                                    .zip(waiting_turn(&event_id))
                                    .is_some_and(|(a, b)| a == b)
                        });
                    let duplicate = seen.iter().any(|old| {
                        let same_id = !event_id.is_empty() && old.id == event_id;
                        let same_question = old.text == text;
                        let same_turn = waiting_turn(&old.id)
                            .zip(waiting_turn(&event_id))
                            .is_some_and(|(a, b)| a == b);
                        let different_signal =
                            old.id.starts_with("input:") != event_id.starts_with("input:");
                        let missing_id = old.id.is_empty() || event_id.is_empty();
                        same_id
                            || (same_question
                                && ((same_turn && different_signal)
                                    || (missing_id
                                        && now.saturating_duration_since(old.at)
                                            < UNIDENTIFIED_PROMPT_DEBOUNCE)))
                    });
                    if duplicate && !silent {
                        return None;
                    }
                    if seen.len() == 16 {
                        seen.pop_front();
                    }
                    seen.push_back(WaitingPrompt {
                        id: event_id,
                        text: text.clone(),
                        at: now,
                    });
                }
                // An alert from a session we never saw open (plugin installed mid-session).
                let s = self.sessions.entry(session_id.clone()).or_insert(Session {
                    pid: 0,
                    last_seen: now,
                });
                s.last_seen = now;
                if s.pid == 0 {
                    s.pid = pid;
                }
                let pid = s.pid;
                self.empty_since = None;
                let badge = (self.sessions.len() > 1).then(|| {
                    session_id
                        .trim_start_matches("codex:")
                        .chars()
                        .take(4)
                        .collect()
                });
                let pending_count = if kind == Kind::Idle && agent == "codex" {
                    self.pending_count(&session_id)
                } else {
                    0
                };
                let body = if kind == Kind::Idle
                    && agent == "codex"
                    && !self.config.attention.show_question_text
                {
                    "Codex has requested your input".into()
                } else {
                    text
                };
                let expires_at = if pending_count > 0 && self.config.attention.persistent {
                    ts + self
                        .config
                        .attention
                        .max_age_secs
                        .min(self.config.attention.question_ttl_secs)
                        * 1000
                } else {
                    0
                };
                let card = Card {
                    session_id: session_id.clone(),
                    kind,
                    project,
                    body,
                    badge,
                    ts,
                    pid,
                    app,
                    agent,
                    pending_count,
                    expires_at,
                    silent,
                    dismiss: false,
                };
                if pending_count > 0 {
                    self.attention_cards.insert(session_id, card.clone());
                }
                Some(card)
            }
            Msg::Reload => {
                self.config = Config::load();
                let sessions: Vec<String> = self.attention_cards.keys().cloned().collect();
                for id in sessions {
                    self.update_attention(&id);
                }
                None
            }
            Msg::Shutdown | Msg::Unknown => None,
        }
    }

    pub fn pending_count(&self, session: &str) -> usize {
        if !self.config.events.idle_prompt {
            return 0;
        }
        self.pending.get(session).map_or(0, |qs| {
            qs.iter()
                .filter(|q| {
                    !q.resolved
                        && if q.asynchronous {
                            self.config.events.async_questions
                        } else {
                            self.config.events.blocking_questions
                        }
                })
                .count()
        })
    }

    pub fn checkpoint(&self, now: Instant, wall_ms: u64) -> QuestionCheckpoint {
        let ttl = Duration::from_secs(self.config.attention.question_ttl_secs);
        let entries = self
            .pending
            .iter()
            .flat_map(|(session, qs)| {
                qs.iter().filter_map(|q| {
                    let remaining = ttl.checked_sub(now.saturating_duration_since(q.at))?;
                    Some(SavedQuestion {
                        session: session.clone(),
                        pid: self.sessions.get(session).map_or(0, |s| s.pid),
                        turn: q.turn.clone(),
                        call: q.call.clone(),
                        id: q.id.clone(),
                        asynchronous: q.asynchronous,
                        resolved: q.resolved,
                        expires_at: wall_ms.saturating_add(remaining.as_millis() as u64),
                    })
                })
            })
            .take(4096)
            .collect();
        QuestionCheckpoint {
            version: 1,
            entries,
        }
    }

    pub fn restore(&mut self, checkpoint: QuestionCheckpoint, now: Instant, wall_ms: u64) {
        if checkpoint.version != 1 {
            return;
        }
        let ttl = Duration::from_secs(self.config.attention.question_ttl_secs);
        for q in checkpoint.entries.into_iter().take(4096) {
            if q.expires_at <= wall_ms
                || (q.pid != 0 && !pid_alive(q.pid))
                || q.session.len() > 256
                || q.turn.len() > 256
                || q.call.len() > 256
                || q.id.len() > 128
            {
                continue;
            }
            let remaining = Duration::from_millis(q.expires_at.saturating_sub(wall_ms)).min(ttl);
            let pending = self.pending.entry(q.session.clone()).or_default();
            if pending.len() >= 512
                || pending
                    .iter()
                    .any(|old| old.turn == q.turn && old.call == q.call && old.id == q.id)
            {
                continue;
            }
            self.sessions.entry(q.session).or_insert(Session {
                pid: q.pid,
                last_seen: now,
            });
            pending.push_back(PendingQuestion {
                turn: q.turn,
                call: q.call,
                id: q.id,
                title: "Previously observed question".into(),
                asynchronous: q.asynchronous,
                resolved: q.resolved,
                at: now.checked_sub(ttl - remaining).unwrap_or(now),
            });
            self.empty_since = None;
        }
        // No UI or sounds on restoration: saved observations aren't a fresh request.
    }

    /// Lifecycle updates never play sound and never resurrect a dismissed card.
    pub fn take_effect(&mut self) -> Option<Card> {
        self.effects.pop_front()
    }

    fn update_attention(&mut self, session: &str) {
        let count = self.pending_count(session);
        if let Some(mut card) = self.attention_cards.get(session).cloned() {
            let body = if self.config.attention.show_question_text {
                self.pending
                    .get(session)
                    .and_then(|qs| {
                        qs.iter().find(|q| {
                            !q.resolved
                                && if q.asynchronous {
                                    self.config.events.async_questions
                                } else {
                                    self.config.events.blocking_questions
                                }
                        })
                    })
                    .map_or_else(|| card.body.clone(), |q| q.title.clone())
            } else {
                "Codex has requested your input".into()
            };
            if count == card.pending_count && body == card.body {
                return;
            }
            card.pending_count = count;
            card.silent = true;
            card.dismiss = count == 0;
            card.body = body;
            if card.dismiss {
                self.attention_cards.remove(session);
            } else {
                self.attention_cards.insert(session.into(), card.clone());
            }
            self.effects.push_back(card);
        }
    }

    fn close_session(&mut self, session: &str, now: Instant) {
        self.revision += 1;
        self.pending.remove(session);
        self.update_attention(session);
        self.last_waiting.remove(session);
        self.recent_alerts.remove(session);
        self.sessions.remove(session);
        self.closed.insert(session.into(), now);
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
            let dead: Vec<String> = self
                .sessions
                .iter()
                .filter(|(_, s)| {
                    (s.pid != 0 && !pid_alive(s.pid))
                        || now.saturating_duration_since(s.last_seen) >= IDLE_BACKSTOP
                })
                .map(|(id, _)| id.clone())
                .collect();
            for id in dead {
                self.close_session(&id, now);
            }
            let ttl = Duration::from_secs(self.config.attention.question_ttl_secs);
            let mut changed = Vec::new();
            for (id, qs) in &mut self.pending {
                let before = qs.len();
                qs.retain(|q| now.saturating_duration_since(q.at) < ttl);
                if qs.len() != before {
                    changed.push(id.clone());
                    self.revision += 1;
                }
            }
            for id in changed {
                self.update_attention(&id);
            }
            self.pending.retain(|_, qs| !qs.is_empty());
            self.closed
                .retain(|_, at| now.saturating_duration_since(*at) < Duration::from_secs(300));
            self.last_waiting
                .retain(|id, _| self.sessions.contains_key(id));
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
    crate::proc::alive(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn questions(sid: &str, call: &str, asynchronous: bool, count: usize) -> Msg {
        Msg::Questions {
            session_id: sid.into(),
            turn_id: "turn".into(),
            call_id: call.into(),
            questions: (0..count)
                .map(|i| crate::protocol::Question {
                    id: i.to_string(),
                    title: format!("Question {i}?"),
                })
                .collect(),
            asynchronous,
            project: "project".into(),
            pid: 0,
            app: "terminal".into(),
            ts: 1000,
        }
    }

    fn resolve(sid: &str, call: &str, ids: &[&str]) -> Msg {
        Msg::QuestionsResolved {
            session_id: sid.into(),
            turn_id: "turn".into(),
            call_id: call.into(),
            question_ids: ids.iter().map(|s| (*s).into()).collect(),
        }
    }

    #[test]
    fn outstanding_questions_survive_work_and_do_not_duplicate() {
        let now = Instant::now();
        let mut c = Core::new(now, Config::default());
        let card = c
            .handle(questions("codex:a", "call1", true, 2), now)
            .unwrap();
        assert_eq!(
            (card.kind, card.pending_count, card.pid),
            (Kind::Idle, 2, 0)
        );
        assert!(card.title().contains("2 unconfirmed"));
        assert_eq!(card.expires_at, 0);
        assert!(c
            .handle(questions("codex:a", "call1", true, 2), now)
            .is_none());
        c.handle(
            Msg::SessionActivity {
                session_id: "codex:a".into(),
                pid: 0,
            },
            now,
        );
        assert_eq!(
            c.handle(alert("codex:a", Kind::Done), now).unwrap().kind,
            Kind::Done
        );
        assert_eq!(c.pending_count("codex:a"), 2);
        assert!(c
            .handle(questions("codex:a", "call1", true, 2), now)
            .is_none());
        assert_eq!(
            c.handle(questions("codex:a", "call2", true, 1), now)
                .unwrap()
                .pending_count,
            3
        );
        assert_eq!(
            c.handle(questions("codex:b", "call1", true, 1), now)
                .unwrap()
                .pending_count,
            1
        );
        assert_eq!(c.pending_count("codex:a"), 3);
        // An answer for a blocking call cannot accidentally resolve an async call.
        c.handle(resolve("codex:a", "call1", &["0"]), now);
        assert_eq!(c.pending_count("codex:a"), 3);
    }

    #[test]
    fn confirmed_answers_resolve_individually_and_late_pre_hooks_do_not_reopen() {
        let now = Instant::now();
        let mut c = Core::new(now, Config::default());
        c.handle(questions("s", "blocking", false, 2), now);
        c.handle(resolve("s", "blocking", &["0"]), now);
        assert_eq!(c.pending_count("s"), 1);
        let update = c.take_effect().unwrap();
        assert!(update.silent && !update.dismiss);
        assert_eq!(update.pending_count, 1);
        assert_eq!(update.body, "Question 1?");
        c.handle(resolve("s", "blocking", &["1"]), now);
        assert!(c.take_effect().unwrap().dismiss);
        assert!(c
            .handle(questions("s", "blocking", false, 2), now)
            .is_none());
        c.handle(resolve("s", "raced", &["0"]), now);
        assert!(c.handle(questions("s", "raced", false, 1), now).is_none());
        assert_eq!(c.pending_count("s"), 0);
    }

    #[test]
    fn persistent_attention_is_bounded_and_cleaned_on_close_death_and_expiry() {
        let now = Instant::now();
        let mut config = Config::default();
        config.attention.persistent = true;
        config.attention.max_age_secs = 60;
        config.attention.question_ttl_secs = 90;
        let mut c = Core::new(now, config);
        let card = c.handle(questions("s", "call", true, 2), now).unwrap();
        assert_eq!(card.expires_at, 61000);
        assert!(card.expired(61000));
        assert!(!card.expired(60999));
        c.handle(
            Msg::SessionClose {
                session_id: "s".into(),
            },
            now,
        );
        assert_eq!(c.pending_count("s"), 0);
        assert!(c.take_effect().unwrap().dismiss);
        assert!(c.handle(questions("s", "late", true, 1), now).is_none());
        c.handle(questions("other", "call", true, 1), now);
        c.tick(now + Duration::from_secs(91), false);
        assert_eq!(c.pending_count("other"), 0);
        assert!(c.take_effect().unwrap().dismiss);
        c.handle(questions("dead", "call", true, 1), now);
        c.sessions.get_mut("dead").unwrap().pid = 4_000_000;
        c.tick(now + Duration::from_secs(122), false);
        assert!(!c.sessions.contains_key("dead"));
        assert!(c.take_effect().unwrap().dismiss);
    }

    #[test]
    fn question_categories_privacy_and_structured_stop_overlap() {
        let now = Instant::now();
        let mut c = Core::new(now, Config::default());
        c.config.events.async_questions = false;
        assert!(c.handle(questions("s", "async", true, 1), now).is_none());
        c.config.attention.show_question_text = false;
        let card = c.handle(questions("s", "block", false, 1), now).unwrap();
        assert_eq!(card.body, "Codex has requested your input");
        let stop = Msg::Alert {
            session_id: "s".into(),
            kind: Kind::Idle,
            event_id: "stop:turn:".into(),
            project: "p".into(),
            text: "Different wording of the same question?".into(),
            ts: 0,
            pid: 0,
            app: String::new(),
            agent: "codex".into(),
        };
        assert!(c.handle(stop, now).is_none());
        c.config.events.blocking_questions = false;
        assert!(c.handle(questions("s", "block2", false, 1), now).is_none());
        c.config.events.async_questions = true;
        c.config.events.idle_prompt = false;
        assert!(c.handle(questions("s", "async2", true, 1), now).is_none());
        assert!(c.handle(alert("s", Kind::Interrupted), now).is_none());
        c.config.events.interrupted = true;
        assert_eq!(
            c.handle(alert("s", Kind::Interrupted), now).unwrap().kind,
            Kind::Interrupted
        );
    }

    #[test]
    fn restart_ledger_retains_identities_without_realerting_or_persisting_text() {
        let now = Instant::now();
        let mut c = Core::new(now, Config::default());
        c.handle(questions("s", "async", true, 2), now);
        c.handle(questions("s", "block", false, 1), now);
        c.handle(resolve("s", "block", &["0"]), now);
        let wire = serde_json::to_string(&c.checkpoint(now, 1000)).unwrap();
        assert!(!wire.contains("Question 0?"));
        let saved = serde_json::from_str(&wire).unwrap();
        let mut recovered = Core::new(now, Config::default());
        recovered.restore(saved, now, 2000);
        assert_eq!(recovered.pending_count("s"), 2);
        assert!(recovered.take_effect().is_none());
        assert!(recovered
            .handle(questions("s", "async", true, 2), now)
            .is_none());
        assert!(recovered
            .handle(questions("s", "block", false, 1), now)
            .is_none());
        let mut stale = Core::new(now, Config::default());
        stale.restore(serde_json::from_str(&wire).unwrap(), now, 3_601_001);
        assert_eq!(stale.pending_count("s"), 0);
    }

    #[test]
    fn late_structured_hook_updates_earlier_stop_without_another_sound() {
        let now = Instant::now();
        let mut c = Core::new(now, Config::default());
        let stop = Msg::Alert {
            session_id: "s".into(),
            kind: Kind::Idle,
            event_id: "stop:turn:".into(),
            project: "p".into(),
            text: "Could you choose the approach?".into(),
            ts: 1000,
            pid: 0,
            app: String::new(),
            agent: "codex".into(),
        };
        let first = c.handle(stop, now).unwrap();
        let structured = c.handle(questions("s", "call", true, 2), now).unwrap();
        assert!(structured.silent);
        assert_eq!(structured.pending_count, 2);
        assert_eq!(first.slot(), structured.slot());
        assert!(
            !c.handle(questions("s", "second-call", true, 1), now)
                .unwrap()
                .silent
        );
    }

    #[test]
    fn privacy_update_silently_hides_existing_question_body() {
        let now = Instant::now();
        let mut c = Core::new(now, Config::default());
        c.handle(questions("s", "call", true, 1), now);
        c.config.attention.show_question_text = false;
        c.update_attention("s");
        let card = c.take_effect().unwrap();
        assert!(card.silent && !card.dismiss);
        assert_eq!(card.body, "Codex has requested your input");
    }

    fn alert(sid: &str, kind: Kind) -> Msg {
        Msg::Alert {
            session_id: sid.into(),
            kind,
            event_id: String::new(),
            project: "p".into(),
            text: "t".into(),
            ts: 0,
            pid: 0,
            app: String::new(),
            agent: String::new(),
        }
    }

    #[test]
    fn badge_only_with_multiple_sessions() {
        let now = Instant::now();
        let mut c = Core::new(now, Config::default());
        c.handle(
            Msg::SessionOpen {
                session_id: "aaaa1".into(),
                pid: 0,
                cwd: "".into(),
            },
            now,
        );
        assert!(c
            .handle(alert("aaaa1", Kind::Done), now)
            .unwrap()
            .badge
            .is_none());
        c.handle(
            Msg::SessionOpen {
                session_id: "bbbb2".into(),
                pid: 0,
                cwd: "".into(),
            },
            now,
        );
        assert_eq!(
            c.handle(alert("bbbb2", Kind::Done), now)
                .unwrap()
                .badge
                .as_deref(),
            Some("bbbb")
        );
    }

    #[test]
    fn disabled_events_are_dropped() {
        let now = Instant::now();
        let mut c = Core::new(now, Config::default());
        assert!(c.handle(alert("s", Kind::Subagent), now).is_none());
        c.config.events.stop = false;
        assert!(c.handle(alert("s", Kind::Done), now).is_none());
        assert!(c.handle(alert("s", Kind::Idle), now).is_some());
    }

    #[test]
    fn repeated_codex_questions_are_deduplicated_per_session_and_prompt() {
        let now = Instant::now();
        let mut core = Core::new(now, Config::default());
        let question = |sid: &str, id: &str| Msg::Alert {
            session_id: sid.into(),
            kind: Kind::Idle,
            event_id: id.into(),
            project: "p".into(),
            text: "Which option?".into(),
            ts: 0,
            pid: 0,
            app: String::new(),
            agent: "codex".into(),
        };
        assert!(core.handle(question("a", "input:t:call1"), now).is_some());
        assert!(core.handle(question("a", "input:t:call1"), now).is_none());
        assert!(core.handle(question("b", "input:t:call1"), now).is_some());
        assert!(core.handle(question("a", "input:t:call2"), now).is_some());
        assert!(core.handle(question("a", "input:t:call2"), now).is_none());
        assert!(core.handle(question("a", "input:t:call1"), now).is_none());
        core.handle(alert("a", Kind::Done), now);
        assert!(core.handle(question("a", "input:t:call2"), now).is_none());
        core.handle(
            Msg::SessionClose {
                session_id: "a".into(),
            },
            now,
        );
        assert!(core.handle(question("a", "input:t:call2"), now).is_none());
        core.handle(
            Msg::SessionOpen {
                session_id: "a".into(),
                pid: 0,
                cwd: String::new(),
            },
            now,
        );
        assert!(core.handle(question("a", "input:t:call2"), now).is_some());
    }

    #[test]
    fn codex_question_from_tool_and_stop_chimes_once_per_turn() {
        let now = Instant::now();
        let mut core = Core::new(now, Config::default());
        let question = |id: &str| Msg::Alert {
            session_id: "codex:s".into(),
            kind: Kind::Idle,
            event_id: id.into(),
            project: "p".into(),
            text: "Which option?".into(),
            ts: 0,
            pid: 0,
            app: String::new(),
            agent: "codex".into(),
        };
        assert!(core.handle(question("input:t1:tool1"), now).is_some());
        assert!(core
            .handle(question("stop:t1:"), now + Duration::from_secs(5))
            .is_none());
        // Distinct tool calls and turns can each need an answer, even with the same text.
        assert!(core
            .handle(question("input:t1:tool2"), now + Duration::from_secs(6))
            .is_some());
        assert!(core
            .handle(question("stop:t2:"), now + Duration::from_secs(7))
            .is_some());
    }

    #[test]
    fn unidentified_questions_debounce_briefly_but_can_recur() {
        let now = Instant::now();
        let mut core = Core::new(now, Config::default());
        let question = || Msg::Alert {
            session_id: "codex:s".into(),
            kind: Kind::Idle,
            event_id: String::new(),
            project: "p".into(),
            text: "Which option?".into(),
            ts: 0,
            pid: 0,
            app: String::new(),
            agent: "codex".into(),
        };
        assert!(core.handle(question(), now).is_some());
        assert!(core
            .handle(question(), now + Duration::from_millis(500))
            .is_none());
        assert!(core
            .handle(question(), now + Duration::from_secs(3))
            .is_some());
        core.config.events.idle_prompt = false;
        assert!(core
            .handle(question(), now + Duration::from_secs(6))
            .is_none());
    }

    #[test]
    fn exits_after_grace_when_empty() {
        let t0 = Instant::now();
        let mut c = Core::new(t0, Config::default());
        c.handle(
            Msg::SessionOpen {
                session_id: "s".into(),
                pid: 0,
                cwd: "".into(),
            },
            t0,
        );
        assert!(!c.tick(t0 + Duration::from_secs(60), false));
        c.handle(
            Msg::SessionClose {
                session_id: "s".into(),
            },
            t0,
        );
        assert!(!c.tick(t0 + Duration::from_secs(5), false));
        assert!(
            !c.tick(t0 + Duration::from_secs(11), true),
            "card still visible"
        );
        assert!(c.tick(t0 + Duration::from_secs(11), false));
    }

    #[test]
    fn drops_dead_pids() {
        let t0 = Instant::now();
        let mut c = Core::new(t0, Config::default());
        // PID far above any real one: kill(pid, 0) fails with ESRCH.
        c.handle(
            Msg::SessionOpen {
                session_id: "s".into(),
                pid: 4_000_000,
                cwd: "".into(),
            },
            t0,
        );
        c.tick(t0 + Duration::from_secs(31), false);
        assert!(c.sessions.is_empty());
    }
}
