//! GUI-side adapter for the always-on session host.
//!
//! `RemoteAgentConnection` is `AgentConnection` over ACP-in-Envelope.
//! Production opens a folder through a local daemon instead of `Project::local`.
//! Tests keep `Project::local`.

mod remote_agent_connection;

pub use remote_agent_connection::{
    RemoteAgentConnection, SessionListUpdated, apply_catch_up, connect_external_agent,
};
pub use session_protocol::{SessionListWire, daemon_socket_id};

/// Register GUI-side hooks (credential forwarding to daemon-held agents).
pub fn init() {
    language_model::set_credential_forwarder(remote_agent_connection::forward_credentials);
}

/// Socket-name body for a daemon keyed by transport, host, and project root.
///
/// Release-channel prefixes are applied by `remote::ConnectionIdentifier`.
pub fn stable_daemon_id(transport: &str, host: &str, project_root: &str) -> String {
    session_protocol::daemon_socket_id(transport, host, project_root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reexports_protocol_id() {
        let identifier = stable_daemon_id("local", "localhost", "/tmp/app");
        assert_eq!(identifier.len(), session_protocol::DAEMON_ID_BODY_LEN);
    }
}
