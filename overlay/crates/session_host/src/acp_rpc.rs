//! Dispatch ACP JSON-RPC carried in `SessionAgentRpc`.
//!
//! Turns are spawned on the daemon `App` and are **not** tied to the proto
//! request lifetime: dropping the GUI cancels the waiter, not NativeAgent.

use std::rc::Rc;

use acp_thread::AgentConnection as _;
use agent::NativeAgentConnection;
use agent_client_protocol::schema::{
    ProtocolVersion,
    v1::{
        AgentCapabilities, AuthenticateRequest, CancelNotification, InitializeRequest,
        InitializeResponse, LoadSessionRequest, LoadSessionResponse, NewSessionRequest,
        NewSessionResponse, PromptRequest, SessionCapabilities, SessionListCapabilities,
        SessionResumeCapabilities,
    },
};
use anyhow::{Context as _, Result, anyhow};
use gpui::{AsyncApp, Entity};
use session_protocol::{
    INTERNAL_ERROR, INVALID_PARAMS, JsonRpcMessage, METHOD_NOT_FOUND, error_response, methods,
    success,
};
use util::path_list::PathList;

use crate::SessionHost;

impl SessionHost {
    pub(crate) async fn handle_json_rpc(
        this: Entity<Self>,
        json: String,
        cx: &mut AsyncApp,
    ) -> String {
        let incoming = match JsonRpcMessage::parse(&json) {
            Ok(incoming) => incoming,
            Err(error) => {
                return error_response(None, session_protocol::PARSE_ERROR, error.to_string());
            }
        };

        let id = incoming.id.clone();
        let Some(method) = incoming.method_name().map(str::to_string) else {
            if incoming.is_response() {
                return success(id, serde_json::json!({})).unwrap_or_else(|_| {
                    error_response(None, INTERNAL_ERROR, "serialize empty result")
                });
            }
            return error_response(id, session_protocol::INVALID_REQUEST, "missing method");
        };

        match method.as_str() {
            methods::INITIALIZE => {
                let _ = incoming.params_as::<InitializeRequest>();
                success_or_internal(id, Self::initialize())
            }
            methods::AUTHENTICATE => match incoming.params_as::<AuthenticateRequest>() {
                Ok(_) => success_or_internal(id, serde_json::json!({})),
                Err(error) => error_response(id, INVALID_PARAMS, error.to_string()),
            },
            methods::SESSION_NEW => match incoming.params_as::<NewSessionRequest>() {
                Ok(request) => match Self::session_new(this, request, cx).await {
                    Ok(response) => success_or_internal(id, response),
                    Err(error) => error_response(id, INTERNAL_ERROR, error.to_string()),
                },
                Err(error) => error_response(id, INVALID_PARAMS, error.to_string()),
            },
            methods::SESSION_LOAD => match incoming.params_as::<LoadSessionRequest>() {
                Ok(request) => match Self::session_load(this, request, cx).await {
                    Ok(response) => success_or_internal(id, response),
                    Err(error) => error_response(id, INTERNAL_ERROR, error.to_string()),
                },
                Err(error) => error_response(id, INVALID_PARAMS, error.to_string()),
            },
            methods::SESSION_PROMPT => match incoming.params_as::<PromptRequest>() {
                Ok(request) => match Self::session_prompt(this, request, cx).await {
                    Ok(response) => success_or_internal(id, response),
                    Err(error) => error_response(id, INTERNAL_ERROR, error.to_string()),
                },
                Err(error) => error_response(id, INVALID_PARAMS, error.to_string()),
            },
            methods::SESSION_CANCEL => match incoming.params_as::<CancelNotification>() {
                Ok(request) => {
                    if let Err(error) = Self::session_cancel(this, request, cx) {
                        return error_response(id, INTERNAL_ERROR, error.to_string());
                    }
                    success_or_internal(id, serde_json::json!({}))
                }
                Err(error) => error_response(id, INVALID_PARAMS, error.to_string()),
            },
            other => error_response(id, METHOD_NOT_FOUND, format!("method not found: {other}")),
        }
    }

    fn initialize() -> InitializeResponse {
        InitializeResponse::new(ProtocolVersion::V1).agent_capabilities(
            AgentCapabilities::new()
                .load_session(true)
                .session_capabilities(
                    SessionCapabilities::new()
                        .list(SessionListCapabilities::new())
                        .resume(SessionResumeCapabilities::new()),
                ),
        )
    }

    async fn session_new(
        this: Entity<Self>,
        request: NewSessionRequest,
        cx: &mut AsyncApp,
    ) -> Result<NewSessionResponse> {
        let work_dirs = PathList::new(&[request.cwd]);
        let thread = this
            .update(cx, |host, cx| {
                let connection = Rc::new(NativeAgentConnection(host.agent.clone()));
                connection.new_session(host.project.clone(), work_dirs, cx)
            })
            .await?;
        let session_id = this.update(cx, |host, cx| host.retain_daemon_thread(thread, cx));
        Ok(NewSessionResponse::new(session_id))
    }

    async fn session_load(
        this: Entity<Self>,
        request: LoadSessionRequest,
        cx: &mut AsyncApp,
    ) -> Result<LoadSessionResponse> {
        let thread = this
            .update(cx, |host, cx| {
                let connection = Rc::new(NativeAgentConnection(host.agent.clone()));
                connection.load_session(
                    request.session_id.clone(),
                    host.project.clone(),
                    PathList::default(),
                    None,
                    cx,
                )
            })
            .await?;
        this.update(cx, |host, cx| {
            host.retain_daemon_thread(thread, cx);
        });
        Ok(LoadSessionResponse::new())
    }

    async fn session_prompt(
        this: Entity<Self>,
        request: PromptRequest,
        cx: &mut AsyncApp,
    ) -> Result<agent_client_protocol::schema::v1::PromptResponse> {
        let (tx, rx) = futures::channel::oneshot::channel();
        this.update(cx, |host, cx| {
            let connection = NativeAgentConnection(host.agent.clone());
            let task = connection.prompt(request, cx);
            cx.spawn(async move |_this, _cx| {
                let result = task.await;
                let _ = tx.send(result);
            })
            .detach();
        });
        rx.await
            .map_err(|_| anyhow!("native agent prompt task dropped"))?
            .context("native agent prompt")
    }

    fn session_cancel(
        this: Entity<Self>,
        request: CancelNotification,
        cx: &mut AsyncApp,
    ) -> Result<()> {
        this.update(cx, |host, cx| {
            NativeAgentConnection(host.agent.clone()).cancel(&request.session_id, cx);
        });
        Ok(())
    }
}

fn success_or_internal(id: Option<serde_json::Value>, value: impl serde::Serialize) -> String {
    success(id.clone(), value)
        .unwrap_or_else(|error| error_response(id, INTERNAL_ERROR, error.to_string()))
}
