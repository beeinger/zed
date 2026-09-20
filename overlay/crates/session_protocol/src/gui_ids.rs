//! Remap Envelope ids so several GUIs can share one daemon proto client.
//!
//! Each GUI assigns `Envelope.id` from its own counter. Merging two stdin
//! streams without remapping would collide request ids. This map gives every
//! inbound message a daemon-unique id and remembers which connection must
//! receive the matching response.

use std::collections::HashMap;

/// Ids copied off an `Envelope` for remapping. The rest of the payload is
/// untouched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnvelopeIds {
    pub id: u32,
    pub responding_to: Option<u32>,
}

/// How a daemon-originated Envelope should be written to GUI connections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutgoingRoute {
    /// Server-initiated request or notification: every attached GUI.
    Broadcast,
    /// Response to a GUI request: only the originator, with its original id.
    Unicast { conn: u64, responding_to: u32 },
}

/// Connection-scoped Envelope id map.
#[derive(Debug, Default)]
pub struct GuiIdMap {
    next_id: u32,
    /// Remapped inbound id → (connection, GUI's original id).
    requests: HashMap<u32, (u64, u32)>,
}

impl GuiIdMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Give `ids` a daemon-unique `id`. Keep `responding_to` (those are
    /// daemon-assigned ids on GUI responses to server requests).
    pub fn remap_incoming(&mut self, conn: u64, ids: EnvelopeIds) -> EnvelopeIds {
        self.next_id = self.next_id.saturating_add(1);
        if self.next_id == 0 {
            self.next_id = 1;
        }
        let remapped = self.next_id;
        if ids.responding_to.is_none() {
            self.requests.insert(remapped, (conn, ids.id));
        }
        EnvelopeIds {
            id: remapped,
            responding_to: ids.responding_to,
        }
    }

    /// Route a daemon-originated Envelope. Consumes the request map entry
    /// when this is a response, so ids do not leak.
    pub fn route_outgoing(&mut self, ids: EnvelopeIds) -> OutgoingRoute {
        let Some(responding_to) = ids.responding_to else {
            return OutgoingRoute::Broadcast;
        };
        match self.requests.remove(&responding_to) {
            Some((conn, original)) => OutgoingRoute::Unicast {
                conn,
                responding_to: original,
            },
            None => OutgoingRoute::Broadcast,
        }
    }

    /// Drop pending request ids for a GUI that disconnected.
    pub fn drop_connection(&mut self, conn: u64) {
        self.requests.retain(|_, (owner, _)| *owner != conn);
    }

    pub fn pending_request_count(&self) -> usize {
        self.requests.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(id: u32) -> EnvelopeIds {
        EnvelopeIds {
            id,
            responding_to: None,
        }
    }

    fn response(id: u32, responding_to: u32) -> EnvelopeIds {
        EnvelopeIds {
            id,
            responding_to: Some(responding_to),
        }
    }

    #[test]
    fn two_guis_with_the_same_request_id_are_unicast_apart() {
        let mut map = GuiIdMap::new();
        let first = map.remap_incoming(1, request(1));
        let second = map.remap_incoming(2, request(1));
        assert_ne!(first.id, second.id);

        match map.route_outgoing(response(10, first.id)) {
            OutgoingRoute::Unicast {
                conn,
                responding_to,
            } => {
                assert_eq!(conn, 1);
                assert_eq!(responding_to, 1);
            }
            other => panic!("expected unicast, got {other:?}"),
        }
        match map.route_outgoing(response(11, second.id)) {
            OutgoingRoute::Unicast {
                conn,
                responding_to,
            } => {
                assert_eq!(conn, 2);
                assert_eq!(responding_to, 1);
            }
            other => panic!("expected unicast, got {other:?}"),
        }
        assert_eq!(map.pending_request_count(), 0);
    }

    #[test]
    fn server_initiated_messages_broadcast() {
        let mut map = GuiIdMap::new();
        assert_eq!(
            map.route_outgoing(request(42)),
            OutgoingRoute::Broadcast
        );
    }

    #[test]
    fn gui_response_to_server_keeps_responding_to() {
        let mut map = GuiIdMap::new();
        let remapped = map.remap_incoming(3, response(9, 42));
        assert_eq!(remapped.responding_to, Some(42));
        assert_ne!(remapped.id, 9);
        assert_eq!(map.pending_request_count(), 0);
    }

    #[test]
    fn drop_connection_forgets_pending_requests() {
        let mut map = GuiIdMap::new();
        map.remap_incoming(1, request(1));
        map.remap_incoming(2, request(1));
        map.drop_connection(1);
        assert_eq!(map.pending_request_count(), 1);
    }
}
