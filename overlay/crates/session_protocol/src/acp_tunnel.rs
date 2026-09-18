//! JSON payload carried in the three Envelope messages at fields ≥ 2000.
//!
//! The wire format is ACP JSON-RPC (or a compatible JSON object) as a string.
//! Envelope stays tiny; this module owns the payload schema.

use crate::EventSeq;

/// One JSON-RPC line (no trailing newline) tunneled in `SessionAgentRpc`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonRpcLine(pub String);

impl JsonRpcLine {
    pub fn new(json: impl Into<String>) -> Self {
        Self(json.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Catch-up cursor sent on reconnect (`SessionSubscribe.last_seq`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubscribeRequest {
    pub last_seq: EventSeq,
}

/// Snapshot of log entries the client has not yet seen (`SessionCatchUp`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatchUpResponse {
    pub from_seq: EventSeq,
    pub to_seq: EventSeq,
    pub events_json: Vec<String>,
}

impl CatchUpResponse {
    pub fn empty(head: EventSeq) -> Self {
        Self {
            from_seq: head,
            to_seq: head,
            events_json: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.events_json.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_catch_up_has_no_events() {
        let head = EventSeq(7);
        let catch_up = CatchUpResponse::empty(head);
        assert_eq!(catch_up.from_seq, head);
        assert_eq!(catch_up.to_seq, head);
        assert!(catch_up.is_empty());
    }

    #[test]
    fn json_rpc_line_preserves_payload() {
        let line = JsonRpcLine::new(r#"{"jsonrpc":"2.0","id":1}"#);
        assert!(line.as_str().contains("jsonrpc"));
    }
}
