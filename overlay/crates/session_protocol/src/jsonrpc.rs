//! JSON-RPC 2.0 helpers for ACP tunneled in `SessionAgentRpc.json`.

use anyhow::{Context as _, Result, anyhow};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// JSON-RPC 2.0 version literal.
pub const JSONRPC_VERSION: &str = "2.0";

/// ACP method names we dispatch on the daemon.
pub mod methods {
    pub const INITIALIZE: &str = "initialize";
    pub const AUTHENTICATE: &str = "authenticate";
    pub const SESSION_NEW: &str = "session/new";
    pub const SESSION_LOAD: &str = "session/load";
    pub const SESSION_PROMPT: &str = "session/prompt";
    pub const SESSION_CANCEL: &str = "session/cancel";
    pub const SESSION_RESUME: &str = "session/resume";
    pub const SESSION_UPDATE: &str = "session/update";
    pub const SESSION_REQUEST_PERMISSION: &str = "session/request_permission";
    pub const ELICITATION_CREATE: &str = "elicitation/create";
    /// Spawn or reuse a daemon-held external ACP child. Not an upstream ACP method.
    pub const ACP_CONNECT: &str = "zed/acp_connect";
}

/// Wire object carried in `SessionAgentRpc.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonRpcMessage {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC error object.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JsonRpcMessage {
    pub fn parse(json: &str) -> Result<Self> {
        serde_json::from_str(json).context("invalid JSON-RPC payload")
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self).context("serialize JSON-RPC payload")
    }

    pub fn is_request(&self) -> bool {
        self.method.is_some() && self.id.is_some()
    }

    pub fn is_notification(&self) -> bool {
        self.method.is_some() && self.id.is_none()
    }

    pub fn is_response(&self) -> bool {
        self.method.is_none() && (self.result.is_some() || self.error.is_some())
    }

    pub fn method_name(&self) -> Option<&str> {
        self.method.as_deref()
    }

    pub fn params_as<T: DeserializeOwned>(&self) -> Result<T> {
        let params = self
            .params
            .clone()
            .unwrap_or(serde_json::Value::Object(Default::default()));
        serde_json::from_value(params).context("invalid JSON-RPC params")
    }

    pub fn result_as<T: DeserializeOwned>(&self) -> Result<T> {
        if let Some(error) = &self.error {
            anyhow::bail!("JSON-RPC error {}: {}", error.code, error.message);
        }
        let result = self
            .result
            .clone()
            .ok_or_else(|| anyhow!("JSON-RPC response missing result"))?;
        serde_json::from_value(result).context("invalid JSON-RPC result")
    }
}

/// Build a JSON-RPC request line.
pub fn request(
    id: impl Into<serde_json::Value>,
    method: &str,
    params: impl Serialize,
) -> Result<String> {
    JsonRpcMessage {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: Some(id.into()),
        method: Some(method.to_string()),
        params: Some(serde_json::to_value(params)?),
        result: None,
        error: None,
    }
    .to_json()
}

/// Build a JSON-RPC notification line (no id; disconnect is a no-op for the sender).
pub fn notification(method: &str, params: impl Serialize) -> Result<String> {
    JsonRpcMessage {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: None,
        method: Some(method.to_string()),
        params: Some(serde_json::to_value(params)?),
        result: None,
        error: None,
    }
    .to_json()
}

/// Build a JSON-RPC success response.
pub fn success(id: Option<serde_json::Value>, result: impl Serialize) -> Result<String> {
    JsonRpcMessage {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id,
        method: None,
        params: None,
        result: Some(serde_json::to_value(result)?),
        error: None,
    }
    .to_json()
}

/// Build a JSON-RPC error response.
pub fn error_response(
    id: Option<serde_json::Value>,
    code: i64,
    message: impl Into<String>,
) -> String {
    JsonRpcMessage {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id,
        method: None,
        params: None,
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.into(),
            data: None,
        }),
    }
    .to_json()
    .unwrap_or_else(|_| {
        r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"internal error"}}"#.to_string()
    })
}

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_roundtrip() {
        let line = request(1, methods::SESSION_NEW, json!({"cwd": "/"})).unwrap();
        let parsed = JsonRpcMessage::parse(&line).unwrap();
        assert!(parsed.is_request());
        assert_eq!(parsed.method_name(), Some(methods::SESSION_NEW));
        assert_eq!(parsed.id, Some(json!(1)));
    }

    #[test]
    fn notification_has_no_id() {
        let line = notification(methods::SESSION_CANCEL, json!({"sessionId": "s"})).unwrap();
        let parsed = JsonRpcMessage::parse(&line).unwrap();
        assert!(parsed.is_notification());
        assert!(!parsed.is_request());
    }

    #[test]
    fn success_result_deserializes() {
        let line = success(Some(json!(2)), json!({"sessionId": "abc"})).unwrap();
        let parsed = JsonRpcMessage::parse(&line).unwrap();
        let value: serde_json::Value = parsed.result_as().unwrap();
        assert_eq!(value["sessionId"], "abc");
    }

    #[test]
    fn error_response_is_json() {
        let line = error_response(Some(json!(3)), METHOD_NOT_FOUND, "nope");
        let parsed = JsonRpcMessage::parse(&line).unwrap();
        assert!(parsed.error.is_some());
        assert!(parsed.result_as::<serde_json::Value>().is_err());
    }

    #[test]
    fn session_update_notification_roundtrip() {
        let line = notification(
            methods::SESSION_UPDATE,
            json!({
                "sessionId": "abc",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": "hi" }
                }
            }),
        )
        .unwrap();
        let parsed = JsonRpcMessage::parse(&line).unwrap();
        assert!(parsed.is_notification());
        assert_eq!(parsed.method_name(), Some(methods::SESSION_UPDATE));
        let params = parsed.params.unwrap();
        assert_eq!(params["sessionId"], "abc");
        assert_eq!(params["update"]["sessionUpdate"], "agent_message_chunk");
    }

    #[test]
    fn permission_and_elicitation_method_names_match_acp() {
        assert_eq!(
            methods::SESSION_REQUEST_PERMISSION,
            "session/request_permission"
        );
        assert_eq!(methods::ELICITATION_CREATE, "elicitation/create");
        assert_eq!(methods::ACP_CONNECT, "zed/acp_connect");
        assert_eq!(methods::SESSION_RESUME, "session/resume");
    }
}
