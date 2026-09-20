//! Dispatch ACP JSON-RPC carried in `SessionAgentRpc`.
//!
//! Turns are spawned on the daemon `App` and are **not** tied to the proto
//! request lifetime: dropping the GUI cancels the waiter, not NativeAgent.

use std::rc::Rc;

use acp_thread::{AgentConnection as _, AgentModelId, AgentModelList, AgentSessionListRequest};
use agent::NativeAgentConnection;
use agent_client_protocol::schema::{
    ProtocolVersion,
    v1::{
        AgentCapabilities, AuthenticateRequest, CancelNotification, InitializeRequest,
        InitializeResponse, LoadSessionRequest, LoadSessionResponse, NewSessionRequest,
        NewSessionResponse, PromptRequest, ResumeSessionRequest, SessionCapabilities,
        SessionListCapabilities, SessionResumeCapabilities,
    },
};
use agent_client_protocol::schema::v1 as acp;
use anyhow::{Context as _, Result, anyhow};
use gpui::{AsyncApp, Entity};
use session_protocol::{
    DeleteSessionRequest, INTERNAL_ERROR, INVALID_PARAMS, JsonRpcMessage, METHOD_NOT_FOUND,
    ModelListWire, SessionListWire, SessionModelRequest, error_response, is_native_agent_id,
    methods, success,
};
use util::path_list::PathList;

use crate::SessionHost;

impl SessionHost {
    pub(crate) async fn handle_json_rpc(
        this: Entity<Self>,
        json: String,
        agent_id: String,
        cx: &mut AsyncApp,
    ) -> String {
        let incoming = match JsonRpcMessage::parse(&json) {
            Ok(incoming) => incoming,
            Err(error) => {
                return error_response(None, session_protocol::PARSE_ERROR, error.to_string());
            }
        };

        if incoming.method_name() == Some(methods::ACP_CONNECT) || !is_native_agent_id(&agent_id) {
            return Self::handle_external_json_rpc(this, agent_id, incoming, cx).await;
        }

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
            methods::SESSION_RESUME => match incoming.params_as::<ResumeSessionRequest>() {
                Ok(request) => {
                    let load = LoadSessionRequest::new(request.session_id, request.cwd);
                    match Self::session_load(this, load, cx).await {
                        Ok(_) => success_or_internal(id, LoadSessionResponse::new()),
                        Err(error) => error_response(id, INTERNAL_ERROR, error.to_string()),
                    }
                }
                Err(error) => error_response(id, INVALID_PARAMS, error.to_string()),
            },
            methods::SET_CREDENTIALS => match incoming
                .params_as::<session_protocol::SetCredentialsRequest>()
            {
                Ok(request) => {
                    let username = request
                        .username
                        .filter(|username| !username.is_empty())
                        .unwrap_or_else(|| "Bearer".to_string());
                    let task = this.update(cx, |_host, cx| {
                        crate::credentials::store_api_key(
                            request.url,
                            username,
                            request.api_key,
                            cx,
                        )
                    });
                    match task.await {
                        Ok(()) => success_or_internal(id, serde_json::json!({})),
                        Err(error) => error_response(id, INTERNAL_ERROR, error.to_string()),
                    }
                }
                Err(error) => error_response(id, INVALID_PARAMS, error.to_string()),
            },
            methods::SESSION_LIST => match Self::session_list(this, cx).await {
                Ok(response) => success_or_internal(id, response),
                Err(error) => error_response(id, INTERNAL_ERROR, error.to_string()),
            },
            methods::SESSION_DELETE => match incoming.params_as::<DeleteSessionRequest>() {
                Ok(request) => match Self::session_delete(this, request, cx).await {
                    Ok(()) => success_or_internal(id, serde_json::json!({})),
                    Err(error) => error_response(id, INTERNAL_ERROR, error.to_string()),
                },
                Err(error) => error_response(id, INVALID_PARAMS, error.to_string()),
            },
            methods::SESSION_DELETE_ALL => match Self::session_delete_all(this, cx).await {
                Ok(()) => success_or_internal(id, serde_json::json!({})),
                Err(error) => error_response(id, INTERNAL_ERROR, error.to_string()),
            },
            methods::LIST_MODELS => match incoming.params_as::<SessionModelRequest>() {
                Ok(request) => match Self::list_models(this, request, cx).await {
                    Ok(response) => success_or_internal(id, response),
                    Err(error) => error_response(id, INTERNAL_ERROR, error.to_string()),
                },
                Err(error) => error_response(id, INVALID_PARAMS, error.to_string()),
            },
            methods::SELECT_MODEL => match incoming.params_as::<SessionModelRequest>() {
                Ok(request) => match Self::select_model(this, request, cx).await {
                    Ok(()) => success_or_internal(id, serde_json::json!({})),
                    Err(error) => error_response(id, INTERNAL_ERROR, error.to_string()),
                },
                Err(error) => error_response(id, INVALID_PARAMS, error.to_string()),
            },
            methods::SELECTED_MODEL => match incoming.params_as::<SessionModelRequest>() {
                Ok(request) => match Self::selected_model(this, request, cx).await {
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

    async fn session_list(
        this: Entity<Self>,
        cx: &mut AsyncApp,
    ) -> Result<SessionListWire> {
        let task = this
            .update(cx, |host, cx| {
                NativeAgentConnection(host.agent.clone())
                    .session_list(cx)
                    .map(|list| list.list_sessions(AgentSessionListRequest::default(), cx))
            })
            .context("session list unavailable")?;
        let response = task.await.context("list native agent sessions")?;
        Ok(SessionListWire {
            sessions: response
                .sessions
                .into_iter()
                .map(|info| session_protocol::SessionInfoWire {
                    session_id: info.session_id.to_string(),
                    title: info.title.map(|title| title.to_string()),
                    work_dirs: info
                        .work_dirs
                        .map(|paths| {
                            paths
                                .ordered_paths()
                                .map(|path| path.display().to_string())
                                .collect()
                        })
                        .unwrap_or_default(),
                    updated_at: info.updated_at.map(|time| time.to_rfc3339()),
                })
                .collect(),
        })
    }

    async fn session_delete(
        this: Entity<Self>,
        request: DeleteSessionRequest,
        cx: &mut AsyncApp,
    ) -> Result<()> {
        let session_id = acp::SessionId::new(request.session_id);
        let task = this
            .update(cx, |host, cx| {
                NativeAgentConnection(host.agent.clone())
                    .session_list(cx)
                    .map(|list| list.delete_session(&session_id, cx))
            })
            .context("session delete unavailable")?;
        task.await.context("delete native agent session")
    }

    async fn session_delete_all(this: Entity<Self>, cx: &mut AsyncApp) -> Result<()> {
        let task = this
            .update(cx, |host, cx| {
                NativeAgentConnection(host.agent.clone())
                    .session_list(cx)
                    .map(|list| list.delete_sessions(cx))
            })
            .context("session delete-all unavailable")?;
        task.await.context("delete native agent sessions")
    }

    async fn list_models(
        this: Entity<Self>,
        request: SessionModelRequest,
        cx: &mut AsyncApp,
    ) -> Result<ModelListWire> {
        let session_id = acp::SessionId::new(request.session_id);
        let task = this.update(cx, |host, cx| {
            let selector = NativeAgentConnection(host.agent.clone())
                .model_selector(&session_id)
                .context("native agent has no model selector")?;
            anyhow::Ok(selector.list_models(cx))
        })?;
        let list = task.await.context("list models")?;
        Ok(model_list_wire(list))
    }

    async fn select_model(
        this: Entity<Self>,
        request: SessionModelRequest,
        cx: &mut AsyncApp,
    ) -> Result<()> {
        let session_id = acp::SessionId::new(request.session_id.clone());
        let model_id = request
            .model_id
            .context("select_model requires model_id")?;
        let task = this.update(cx, |host, cx| {
            let selector = NativeAgentConnection(host.agent.clone())
                .model_selector(&session_id)
                .context("native agent has no model selector")?;
            anyhow::Ok(selector.select_model(AgentModelId::new(model_id.as_str()), cx))
        })?;
        task.await.context("select model")
    }

    async fn selected_model(
        this: Entity<Self>,
        request: SessionModelRequest,
        cx: &mut AsyncApp,
    ) -> Result<session_protocol::ModelInfoWire> {
        let session_id = acp::SessionId::new(request.session_id);
        let task = this.update(cx, |host, cx| {
            let selector = NativeAgentConnection(host.agent.clone())
                .model_selector(&session_id)
                .context("native agent has no model selector")?;
            anyhow::Ok(selector.selected_model(cx))
        })?;
        let info = task.await.context("selected model")?;
        Ok(model_info_wire(info, None))
    }
}

fn success_or_internal(id: Option<serde_json::Value>, value: impl serde::Serialize) -> String {
    success(id.clone(), value)
        .unwrap_or_else(|error| error_response(id, INTERNAL_ERROR, error.to_string()))
}

fn model_list_wire(list: AgentModelList) -> ModelListWire {
    match list {
        AgentModelList::Flat(models) => ModelListWire {
            models: models
                .into_iter()
                .map(|model| model_info_wire(model, None))
                .collect(),
        },
        AgentModelList::Grouped(groups) => ModelListWire {
            models: groups
                .into_iter()
                .flat_map(|(group, models)| {
                    let group = group.0.to_string();
                    models
                        .into_iter()
                        .map(move |model| model_info_wire(model, Some(group.clone())))
                })
                .collect(),
        },
    }
}

fn model_info_wire(
    info: acp_thread::AgentModelInfo,
    group: Option<String>,
) -> session_protocol::ModelInfoWire {
    session_protocol::ModelInfoWire {
        id: info.id.to_string(),
        name: info.name.to_string(),
        description: info.description.map(|description| description.to_string()),
        group,
    }
}
