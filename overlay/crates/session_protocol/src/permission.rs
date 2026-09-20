//! How the daemon waits for permission and elicitation when the GUI is gone.

use std::time::Duration;

/// ACP `_meta` key carrying Zed's `AuthorizationKind` across the tunnel.
pub const AUTHORIZATION_KIND_META: &str = "zed.dev/authorizationKind";

/// Default wait when `disconnected_prompt_wait` is `"timeout"` (five minutes).
pub const DEFAULT_PROMPT_TIMEOUT_MS: u64 = 300_000;

/// Shown when a GUI attaches to a remote/local daemon so unattended work is not
/// a surprise. Server settings own the policy (the laptop is only a window).
pub const DETACHED_WORK_ADVICE: &str = "Detached agent work waits for this window to approve tools and questions. For unattended remote runs, set agent.detached_permissions to \"permit_everything\" or a thorough agent.tool_permissions allow-list in server settings.";

/// Whether the daemon auto-allows tool permissions while the client is gone.
///
/// Elicitation is always asked: a form cannot be filled without a window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DetachedPermissions {
    /// Send permission and elicitation prompts to the GUI (or wait for one).
    #[default]
    Ask,
    /// Auto-allow tool permissions (permit everything). Still asks elicitation.
    PermitEverything,
}

/// How long to wait for a GUI answer when a prompt is outstanding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DisconnectedPromptWait {
    /// Wait until a client reconnects and answers.
    #[default]
    Forever,
    /// Wait this many milliseconds, then apply the timeout outcome.
    Timeout { millis: u64 },
}

impl DisconnectedPromptWait {
    pub fn duration(self) -> Option<Duration> {
        match self {
            Self::Forever => None,
            Self::Timeout { millis } => Some(Duration::from_millis(millis)),
        }
    }
}

/// Daemon policy for prompts while no GUI is attached (or one has not answered).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DisconnectedPromptPolicy {
    pub permissions: DetachedPermissions,
    pub wait: DisconnectedPromptWait,
}

impl DisconnectedPromptPolicy {
    pub fn permit_tool_permissions(self) -> bool {
        matches!(self.permissions, DetachedPermissions::PermitEverything)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_asks_and_waits_forever() {
        let policy = DisconnectedPromptPolicy::default();
        assert_eq!(policy.permissions, DetachedPermissions::Ask);
        assert_eq!(policy.wait, DisconnectedPromptWait::Forever);
        assert!(policy.wait.duration().is_none());
        assert!(!policy.permit_tool_permissions());
    }

    #[test]
    fn timeout_has_a_duration() {
        let wait = DisconnectedPromptWait::Timeout {
            millis: DEFAULT_PROMPT_TIMEOUT_MS,
        };
        assert_eq!(
            wait.duration(),
            Some(Duration::from_millis(DEFAULT_PROMPT_TIMEOUT_MS))
        );
    }

    #[test]
    fn advice_mentions_permit_everything_and_tool_permissions() {
        assert!(DETACHED_WORK_ADVICE.contains("permit_everything"));
        assert!(DETACHED_WORK_ADVICE.contains("tool_permissions"));
    }
}
