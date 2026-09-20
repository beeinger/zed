//! JSON payload carried in the three Envelope messages at fields ≥ 2000.
//!
//! The wire format is ACP JSON-RPC (or a compatible JSON object) as a string.
//! Envelope stays tiny; this module owns the payload schema.

use anyhow::Result;

use crate::EventSeq;
use crate::jsonrpc::{JsonRpcMessage, methods};

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

/// `session/update` params from a catch-up log that belong to `session_id`.
///
/// Disconnect does not clear the log; a later catch-up returns only unseen lines,
/// and this filter drops updates for other ACP sessions.
pub fn catch_up_session_update_params(
    events_json: &[String],
    session_id: &str,
) -> Result<Vec<serde_json::Value>> {
    let mut params_list = Vec::new();
    for line in events_json {
        let incoming = JsonRpcMessage::parse(line)?;
        if incoming.method_name() != Some(methods::SESSION_UPDATE) {
            continue;
        }
        let Some(params) = incoming.params else {
            continue;
        };
        let matches = params
            .get("sessionId")
            .and_then(|value| value.as_str())
            == Some(session_id);
        if matches {
            params_list.push(params);
        }
    }
    Ok(params_list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonrpc::notification;
    use serde_json::json;

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

    #[test]
    fn catch_up_filters_other_sessions() {
        let mine = notification(
            methods::SESSION_UPDATE,
            json!({
                "sessionId": "keep",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": "hello" }
                }
            }),
        )
        .unwrap();
        let other = notification(
            methods::SESSION_UPDATE,
            json!({
                "sessionId": "other",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": "nope" }
                }
            }),
        )
        .unwrap();
        let params = catch_up_session_update_params(&[mine, other], "keep").unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0]["sessionId"], "keep");
        assert_eq!(params[0]["update"]["content"]["text"], "hello");
    }
}
