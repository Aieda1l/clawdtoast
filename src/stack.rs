//! Which cards are on screen and which wait. Pure bookkeeping; the overlay owns the windows.

use crate::core::Card;
use std::collections::VecDeque;

pub enum Offer {
    /// The session already has a visible card at this index: replace its content.
    Replace(usize),
    /// Show as a new card (appended to the visible list).
    Show,
    /// No room; waits in the queue (or replaced that session's queued card).
    Queued,
    Dismiss(usize),
    Ignore,
}

pub struct Stack {
    visible: Vec<String>,
    queue: VecDeque<Card>,
    max: usize,
}

impl Stack {
    pub fn new(max: usize) -> Stack {
        Stack {
            visible: Vec::new(),
            queue: VecDeque::new(),
            max: max.max(1),
        }
    }

    pub fn offer(&mut self, card: &Card) -> Offer {
        let key = card.slot();
        if let Some(i) = self.visible.iter().position(|s| *s == key) {
            if card.dismiss {
                return Offer::Dismiss(i);
            }
            return Offer::Replace(i);
        }
        if let Some(i) = self.queue.iter().position(|q| q.slot() == key) {
            if card.dismiss {
                self.queue.remove(i);
                return Offer::Ignore;
            }
            self.queue[i] = card.clone();
            return Offer::Queued;
        }
        if card.silent || card.dismiss {
            return Offer::Ignore;
        }
        if self.visible.len() < self.max {
            self.visible.push(key);
            Offer::Show
        } else {
            self.queue.push_back(card.clone());
            Offer::Queued
        }
    }

    /// Removes the card at `idx`; returns the next queued card, now appended to the visible list.
    pub fn remove(&mut self, idx: usize) -> Option<Card> {
        self.visible.remove(idx);
        let next = self.queue.pop_front()?;
        self.visible.push(next.slot());
        Some(next)
    }

    pub fn is_empty(&self) -> bool {
        self.visible.is_empty() && self.queue.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Kind;

    fn card(sid: &str, body: &str) -> Card {
        Card {
            session_id: sid.into(),
            kind: Kind::Done,
            project: "p".into(),
            body: body.into(),
            badge: None,
            ts: 0,
            pid: 0,
            app: String::new(),
            agent: String::new(),
            pending_count: 0,
            expires_at: 0,
            silent: false,
            dismiss: false,
        }
    }

    #[test]
    fn caps_visible_and_queues_the_rest() {
        let mut s = Stack::new(3);
        for sid in ["a", "b", "c"] {
            assert!(matches!(s.offer(&card(sid, "x")), Offer::Show));
        }
        assert!(matches!(s.offer(&card("d", "x")), Offer::Queued));
        assert!(matches!(s.offer(&card("e", "x")), Offer::Queued));
    }

    #[test]
    fn same_session_replaces_visible_or_queued() {
        let mut s = Stack::new(1);
        assert!(matches!(s.offer(&card("a", "1")), Offer::Show));
        assert!(matches!(s.offer(&card("a", "2")), Offer::Replace(0)));
        assert!(matches!(s.offer(&card("b", "1")), Offer::Queued));
        assert!(matches!(s.offer(&card("b", "2")), Offer::Queued));
        let next = s.remove(0).unwrap();
        assert_eq!(
            (next.session_id.as_str(), next.body.as_str()),
            ("b", "2"),
            "queued card was replaced, not duplicated"
        );
        assert!(s.remove(0).is_none());
        assert!(s.is_empty());
    }

    #[test]
    fn attention_coexists_with_done_and_silent_updates_never_resurrect_it() {
        let mut s = Stack::new(3);
        let mut attention = card("a", "Which option?");
        attention.kind = Kind::Idle;
        attention.pending_count = 2;
        assert!(matches!(s.offer(&attention), Offer::Show));
        assert!(matches!(s.offer(&card("a", "Finished")), Offer::Show));
        attention.silent = true;
        attention.pending_count = 1;
        assert!(matches!(s.offer(&attention), Offer::Replace(0)));
        attention.dismiss = true;
        attention.pending_count = 0;
        assert!(matches!(s.offer(&attention), Offer::Dismiss(0)));
        s.remove(0);
        attention.dismiss = false;
        attention.pending_count = 1;
        assert!(matches!(s.offer(&attention), Offer::Ignore));
        assert!(matches!(
            s.offer(&card("a", "Finished again")),
            Offer::Replace(0)
        ));
    }

    #[test]
    fn resolving_queued_attention_prevents_its_later_promotion() {
        let mut s = Stack::new(1);
        s.offer(&card("busy", "Finished"));
        let mut attention = card("a", "Which option?");
        attention.pending_count = 1;
        assert!(matches!(s.offer(&attention), Offer::Queued));
        attention.dismiss = true;
        attention.silent = true;
        attention.pending_count = 0;
        assert!(matches!(s.offer(&attention), Offer::Ignore));
        assert!(s.remove(0).is_none());
    }

    #[test]
    fn promoted_card_goes_last_and_indices_shift() {
        let mut s = Stack::new(2);
        s.offer(&card("a", "x"));
        s.offer(&card("b", "x"));
        s.offer(&card("c", "x"));
        assert_eq!(s.remove(0).unwrap().session_id, "c");
        // visible is now [b, c]
        assert!(matches!(s.offer(&card("c", "y")), Offer::Replace(1)));
        assert!(matches!(s.offer(&card("b", "y")), Offer::Replace(0)));
    }
}
