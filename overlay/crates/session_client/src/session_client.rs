//! GUI-side adapter for the always-on session host.
//!
//! `RemoteAgentConnection` is `AgentConnection` over ACP-in-Envelope.
//! Production opens a folder through a local daemon instead of `Project::local`.
//! Tests keep `Project::local`.

use std::sync::atomic::{AtomicBool, Ordering};

mod remote_agent_connection;

pub use remote_agent_connection::{
    RemoteAgentConnection, SessionListUpdated, apply_catch_up, connect_external_agent,
};
pub use session_protocol::{SessionListWire, daemon_socket_id};

static GUI_IS_WINDOW: AtomicBool = AtomicBool::new(false);

/// Register GUI-side hooks (credential forwarding to daemon-held agents).
///
/// Marks this process as a window: it must not construct `NativeAgent` or
/// stdio-spawn ACP children. The daemon binary never calls this.
pub fn init() {
    GUI_IS_WINDOW.store(true, Ordering::SeqCst);
    language_model::set_credential_forwarder(remote_agent_connection::forward_credentials);
}

/// True in the production GUI after [`init`]. False in tests and on `remote_server`.
pub fn gui_is_window() -> bool {
    GUI_IS_WINDOW.load(Ordering::SeqCst)
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

    #[test]
    fn gui_is_window_stays_off_without_init() {
        assert!(
            !gui_is_window(),
            "tests and remote_server must still be allowed to own NativeAgent / ACP stdio"
        );
    }
}
