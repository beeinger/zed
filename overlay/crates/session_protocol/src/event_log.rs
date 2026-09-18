//! Append-only agent event log. Disconnect does not rewind or drop entries.

use crate::{CatchUpResponse, EventSeq};

/// Server-side log of ACP JSON-RPC notifications (typically `session/update`).
#[derive(Clone, Debug, Default)]
pub struct EventLog {
    events: Vec<String>,
}

impl EventLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn head(&self) -> EventSeq {
        EventSeq(self.events.len() as u64)
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Append one JSON line and return the new head seq (1-based count).
    pub fn append(&mut self, json: impl Into<String>) -> EventSeq {
        self.events.push(json.into());
        self.head()
    }

    /// Events after `last_seq`. `last_seq == 0` means the client has nothing.
    pub fn catch_up(&self, last_seq: EventSeq) -> CatchUpResponse {
        let start = last_seq.0 as usize;
        if start >= self.events.len() {
            return CatchUpResponse::empty(self.head());
        }
        CatchUpResponse {
            from_seq: last_seq.next(),
            to_seq: self.head(),
            events_json: self.events[start..].to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_catch_up_from_zero() {
        let log = EventLog::new();
        let catch_up = log.catch_up(EventSeq::ZERO);
        assert!(catch_up.is_empty());
        assert_eq!(catch_up.to_seq, EventSeq::ZERO);
    }

    #[test]
    fn catch_up_skips_seen_events() {
        let mut log = EventLog::new();
        log.append("one");
        log.append("two");
        log.append("three");
        let catch_up = log.catch_up(EventSeq(1));
        assert_eq!(catch_up.from_seq, EventSeq(2));
        assert_eq!(catch_up.to_seq, EventSeq(3));
        assert_eq!(
            catch_up.events_json,
            vec!["two".to_string(), "three".to_string()]
        );
    }

    #[test]
    fn persist_then_catch_up_from_zero() {
        let mut log = EventLog::new();
        let first = log.append(r#"{"jsonrpc":"2.0","method":"session/update"}"#);
        assert_eq!(first, EventSeq(1));
        let catch_up = log.catch_up(EventSeq::ZERO);
        assert_eq!(catch_up.events_json.len(), 1);
        assert_eq!(catch_up.to_seq, EventSeq(1));
    }
}
