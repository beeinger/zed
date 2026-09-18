//! What the daemon does with agent permission prompts when no GUI is attached.

/// Policy for tool permission prompts while the client is gone.
///
/// The agent loop must not stall waiting for a window. Default for this product
/// is to allow according to project trust.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DisconnectedPermissionPolicy {
    /// Honor the project's trust state (trusted worktrees allow, others deny).
    #[default]
    AllowAccordingToTrust,
    /// Reject every prompt until a client reconnects.
    Deny,
    /// Queue prompts until a client reconnects. Avoid this as the product
    /// default: a long queue blocks the turn the same way a missing window did.
    Queue,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_does_not_stall() {
        assert_eq!(
            DisconnectedPermissionPolicy::default(),
            DisconnectedPermissionPolicy::AllowAccordingToTrust
        );
    }
}
