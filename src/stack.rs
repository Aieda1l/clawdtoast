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
}

pub struct Stack {
    visible: Vec<String>,
    queue: VecDeque<Card>,
    max: usize,
}

impl Stack {
    pub fn new(max: usize) -> Stack {
        Stack { visible: Vec::new(), queue: VecDeque::new(), max: max.max(1) }
    }

    pub fn offer(&mut self, card: &Card) -> Offer {
        if let Some(i) = self.visible.iter().position(|s| *s == card.session_id) {
            return Offer::Replace(i);
        }
        if let Some(q) = self.queue.iter_mut().find(|q| q.session_id == card.session_id) {
            *q = card.clone();
            return Offer::Queued;
        }
        if self.visible.len() < self.max {
            self.visible.push(card.session_id.clone());
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
        self.visible.push(next.session_id.clone());
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
        Card { session_id: sid.into(), kind: Kind::Done, project: "p".into(), body: body.into(), badge: None, ts: 0 }
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
        assert_eq!((next.session_id.as_str(), next.body.as_str()), ("b", "2"), "queued card was replaced, not duplicated");
        assert!(s.remove(0).is_none());
        assert!(s.is_empty());
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
