//! Shared types for the always-on session host.
//!
//! Agent traffic stays on ACP JSON-RPC. A later phase tunnels that JSON inside
//! a handful of Envelope messages rather than growing `zed.proto` with a full
//! agent API.

mod coalesce;
mod daemon_id;
mod event_seq;
mod permission;

pub use coalesce::CoalesceConfig;
pub use daemon_id::{DAEMON_ID_BODY_LEN, daemon_socket_id};
pub use event_seq::EventSeq;
pub use permission::DisconnectedPermissionPolicy;

/// Placeholder so production binaries can call `session_protocol::init` at a
/// single, marked hook. Currently a no-op.
pub fn init() {}
