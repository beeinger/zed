//! Shared types for the always-on session host.
//!
//! Agent traffic stays on ACP JSON-RPC. A later phase tunnels that JSON inside
//! a handful of Envelope messages rather than growing `zed.proto` with a full
//! agent API.

mod acp_connect;
mod acp_tunnel;
mod coalesce;
mod daemon_id;
mod event_log;
mod event_seq;
mod jsonrpc;
mod permission;

pub use acp_connect::{
    AcpConnectRequest, AcpConnectResponse, DeleteSessionRequest, ModelInfoWire, ModelListWire,
    NATIVE_AGENT_ID, SessionInfoWire, SessionListWire, SessionModelRequest, SetCredentialsRequest,
    is_native_agent_id,
};
pub use acp_tunnel::{
    CatchUpResponse, JsonRpcLine, SubscribeRequest, catch_up_session_update_params,
};
pub use coalesce::CoalesceConfig;
pub use daemon_id::{DAEMON_ID_BODY_LEN, daemon_socket_id};
pub use event_log::EventLog;
pub use event_seq::EventSeq;
pub use jsonrpc::{
    INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, JSONRPC_VERSION, JsonRpcError, JsonRpcMessage,
    METHOD_NOT_FOUND, PARSE_ERROR, error_response, methods, notification, request, success,
};
pub use permission::{
    AUTHORIZATION_KIND_META, DEFAULT_PROMPT_TIMEOUT_MS, DETACHED_WORK_ADVICE, DetachedPermissions,
    DisconnectedPromptPolicy, DisconnectedPromptWait,
};

/// Placeholder so production binaries can call `session_protocol::init` at a
/// single, marked hook. Currently a no-op.
pub fn init() {}
