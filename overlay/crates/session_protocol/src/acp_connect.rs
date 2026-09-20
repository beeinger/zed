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

/// GUI → daemon: persist credentials in the daemon store.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetCredentialsRequest {
    pub url: String,
    /// Keychain username. `"Bearer"` for LLM API keys; Zed cloud uses the user id.
    #[serde(default)]
    pub username: Option<String>,
    pub api_key: Option<String>,
}

/// Daemon thread archive row (ACP `session/list` payload we control).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionInfoWire {
    pub session_id: String,
    pub title: Option<String>,
    #[serde(default)]
    pub work_dirs: Vec<String>,
    pub updated_at: Option<String>,
    /// Empty or omitted means the native Zed Agent.
    #[serde(default)]
    pub agent_id: Option<String>,
    /// True while a turn is in flight on the daemon, even with no GUI attached.
    #[serde(default)]
    pub generating: bool,
}

/// GUI → daemon: summarize a native (or daemon-held) session for @-mentions.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadSummaryRequest {
    pub session_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SessionListWire {
    #[serde(default)]
    pub sessions: Vec<SessionInfoWire>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeleteSessionRequest {
    pub session_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelInfoWire {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub group: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ModelListWire {
    #[serde(default)]
    pub models: Vec<ModelInfoWire>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionModelRequest {
    pub session_id: String,
    pub model_id: Option<String>,
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

    #[test]
    fn set_credentials_roundtrip() {
        let request = SetCredentialsRequest {
            url: "https://api.example".into(),
            username: Some("Bearer".into()),
            api_key: Some("sk-test".into()),
        };
        let json = serde_json::to_string(&request).expect("serialize");
        let parsed: SetCredentialsRequest = serde_json::from_str(&json).expect("parse");
        assert_eq!(parsed, request);
    }

    #[test]
    fn session_info_wire_defaults_generating_false() {
        let json = r#"{"session_id":"s1"}"#;
        let parsed: SessionInfoWire = serde_json::from_str(json).expect("parse");
        assert!(!parsed.generating);
        assert!(parsed.agent_id.is_none());
    }
}
