//! Server-side session host.
//!
//! Later phases construct `NativeAgent`, `LanguageModelRegistry`, and an
//! append-only agent event log inside `HeadlessProject`. The GUI never owns
//! those tasks: dropping a client is a no-op for in-flight turns.

use session_protocol::{DisconnectedPermissionPolicy, EventSeq};

/// How the daemon behaves with no attached GUI.
#[derive(Clone, Copy, Debug)]
pub struct HostConfig {
    pub disconnected_permissions: DisconnectedPermissionPolicy,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            disconnected_permissions: DisconnectedPermissionPolicy::AllowAccordingToTrust,
        }
    }
}

/// Placeholder so `HeadlessProject::new` can call `session_host::init` at a
/// single, marked hook. Currently a no-op.
pub fn init() {}

/// Empty event log cursor used until the host is wired to NativeAgent.
pub fn empty_log_head() -> EventSeq {
    EventSeq::ZERO
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_permissions_do_not_stall() {
        assert_eq!(
            HostConfig::default().disconnected_permissions,
            DisconnectedPermissionPolicy::AllowAccordingToTrust
        );
    }
}
