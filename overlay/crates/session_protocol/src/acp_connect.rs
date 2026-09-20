//! Spawn/reuse an external ACP child on the daemon (`zed/acp_connect`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Native Zed Agent on the wire. Empty `SessionAgentRpc.agent_id` also means native.
pub const NATIVE_AGENT_ID: &str = "Zed Agent";

/// True when `SessionAgentRpc.agent_id` addresses NativeAgent rather than an ACP child.
pub fn is_native_agent_id(agent_id: &str) -> bool {
    agent_id.is_empty() || agent_id == NATIVE_AGENT_ID
}

/// GUI → daemon: run this command as a child on the host (no SSH stdio wrap).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AcpConnectRequest {
    pub path: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub default_mode: Option<String>,
    #[serde(default)]
    pub default_config_options: HashMap<String, serde_json::Value>,
}

/// Daemon → GUI: identity of a child that now lives on the host.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AcpConnectResponse {
    pub telemetry_id: String,
    pub agent_version: Option<String>,
    #[serde(default)]
    pub auth_methods_json: Vec<serde_json::Value>,
    pub load_session: bool,
    pub resume_session: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_zed_agent_are_native() {
        assert!(is_native_agent_id(""));
        assert!(is_native_agent_id(NATIVE_AGENT_ID));
        assert!(!is_native_agent_id("gemini"));
    }

    #[test]
    fn connect_request_roundtrip() {
        let request = AcpConnectRequest {
            path: "/usr/bin/gemini".into(),
            args: vec!["--acp".into()],
            env: HashMap::from([("NO_BROWSER".into(), "1".into())]),
            default_mode: Some("default".into()),
            default_config_options: HashMap::new(),
        };
        let json = serde_json::to_string(&request).expect("serialize");
        let parsed: AcpConnectRequest = serde_json::from_str(&json).expect("parse");
        assert_eq!(parsed, request);
    }
}
