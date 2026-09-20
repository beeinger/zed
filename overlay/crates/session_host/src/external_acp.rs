//! External ACP children live on the daemon. GUI death must not SIGHUP them.

use std::rc::Rc;

use acp_thread::{AgentConnection as _, PermissionOptions};
use agent::{DaemonPromptHandler as _, DaemonPromptWait};
use agent_client_protocol::schema::v1 as acp;
use agent_servers::{AcpConnection, DetachedAcpIo};
use anyhow::{Context as _, Result, anyhow};
use gpui::{App, AsyncApp, Entity, SharedString, Task, WeakEntity};
use project::{AgentId, agent_server_store::AgentServerCommand};
use session_protocol::{AcpConnectRequest, AcpConnectResponse, methods};
use settings::AgentConfigOptionValue;
use util::path_list::PathList;

use crate::SessionHost;
use crate::gui_prompts::HostPromptHandler;

pub(crate) struct ExternalAgent {
    pub connection: Rc<AcpConnection>,
    telemetry_id: SharedString,
    agent_version: Option<SharedString>,
    auth_methods: Vec<acp::AuthMethod>,
    load_session: bool,
    resume_session: bool,
}

impl ExternalAgent {
    fn from_connection(connection: Rc<AcpConnection>) -> Self {
        Self {
            telemetry_id: connection.telemetry_id(),
            agent_version: connection.agent_version(),
            auth_methods: connection.auth_methods().to_vec(),
            load_session: connection.supports_load_session(),
            resume_session: connection.supports_resume_session(),
            connection,
        }
    }

    fn connect_response(&self) -> Result<AcpConnectResponse> {
        let auth_methods_json = self
            .auth_methods
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .context("serialize auth methods")?;
        Ok(AcpConnectResponse {
            telemetry_id: self.telemetry_id.to_string(),
            agent_version: self.agent_version.as_ref().map(|value| value.to_string()),
            auth_methods_json,
            load_session: self.load_session,
            resume_session: self.resume_session,
        })
    }
}

struct HostExternalIo {
    host: WeakEntity<SessionHost>,
    agent_id: AgentId,
}

impl DetachedAcpIo for HostExternalIo {
    fn on_session_update(
        &self,
        notification: acp::SessionNotification,
        persist: bool,
        cx: &mut App,
    ) {
        let agent_id = self.agent_id.to_string();
        let _ = self.host.update(cx, |host, cx| {
            host.emit_session_update(notification, persist, &agent_id, cx);
        });
    }

    fn request_permission(
        &self,
        request: acp::RequestPermissionRequest,
        cx: &mut App,
    ) -> Task<Result<acp::RequestPermissionResponse>> {
        let host = self.host.clone();
        let agent_id = self.agent_id.clone();
        let session_id = request.session_id.clone();
        let options = PermissionOptions::Flat(request.options.clone());
        let kind = acp_thread::AuthorizationKind::PermissionGrant;
        cx.spawn(async move |cx| {
            let permit = host.read_with(cx, |host, _cx| host.config.permit_tool_permissions())?;
            if permit && let Some(outcome) = permit_everything_permission_outcome(&options) {
                return Ok(acp::RequestPermissionResponse::new(
                    acp::RequestPermissionOutcome::Selected(outcome.into()),
                ));
            }
            let wait_task = cx.update(|cx| {
                HostPromptHandler(host.clone()).request_permission(
                    session_id.clone(),
                    request.tool_call,
                    options.clone(),
                    kind,
                    cx,
                )
            });
            match wait_task.await {
                DaemonPromptWait::Answered(outcome) => Ok(acp::RequestPermissionResponse::new(
                    acp::RequestPermissionOutcome::Selected(outcome.into()),
                )),
                DaemonPromptWait::TimedOut => {
                    let _ = host.update(cx, |host, cx| {
                        if let Some(agent) = host.external_agents.get(agent_id.as_ref()) {
                            agent.connection.cancel(&session_id, cx);
                        }
                    });
                    Ok(acp::RequestPermissionResponse::new(
                        acp::RequestPermissionOutcome::Cancelled,
                    ))
                }
            }
        })
    }

    fn request_elicitation(
        &self,
        request: acp::CreateElicitationRequest,
        cx: &mut App,
    ) -> Task<Result<acp::CreateElicitationResponse>> {
        let host = self.host.clone();
        cx.spawn(async move |cx| {
            let wait_task =
                cx.update(|cx| HostPromptHandler(host).request_elicitation(request, cx));
            Ok(match wait_task.await {
                DaemonPromptWait::Answered(response) => response,
                DaemonPromptWait::TimedOut => {
                    acp::CreateElicitationResponse::new(acp::ElicitationAction::Cancel)
                }
            })
        })
    }
}

impl SessionHost {
    pub(crate) async fn handle_external_json_rpc(
        this: Entity<Self>,
        agent_id: String,
        incoming: session_protocol::JsonRpcMessage,
        cx: &mut AsyncApp,
    ) -> String {
        let id = incoming.id.clone();
        let Some(method) = incoming.method_name().map(str::to_string) else {
            return session_protocol::error_response(
                id,
                session_protocol::INVALID_REQUEST,
                "missing method",
            );
        };

        match method.as_str() {
            methods::ACP_CONNECT => match incoming.params_as::<AcpConnectRequest>() {
                Ok(request) => match Self::acp_connect(this, agent_id, request, cx).await {
                    Ok(response) => success_or_internal(id, response),
                    Err(error) => session_protocol::error_response(
                        id,
                        session_protocol::INTERNAL_ERROR,
                        error.to_string(),
                    ),
                },
                Err(error) => session_protocol::error_response(
                    id,
                    session_protocol::INVALID_PARAMS,
                    error.to_string(),
                ),
            },
            methods::AUTHENTICATE => match incoming.params_as::<acp::AuthenticateRequest>() {
                Ok(request) => match Self::external_authenticate(this, &agent_id, request, cx) {
                    Ok(task) => match task.await {
                        Ok(()) => success_or_internal(id, serde_json::json!({})),
                        Err(error) => session_protocol::error_response(
                            id,
                            session_protocol::INTERNAL_ERROR,
                            error.to_string(),
                        ),
                    },
                    Err(error) => session_protocol::error_response(
                        id,
                        session_protocol::INTERNAL_ERROR,
                        error.to_string(),
                    ),
                },
                Err(error) => session_protocol::error_response(
                    id,
                    session_protocol::INVALID_PARAMS,
                    error.to_string(),
                ),
            },
            methods::SESSION_NEW => match incoming.params_as::<acp::NewSessionRequest>() {
                Ok(request) => match Self::external_session_new(this, &agent_id, request, cx).await
                {
                    Ok(response) => success_or_internal(id, response),
                    Err(error) => session_protocol::error_response(
                        id,
                        session_protocol::INTERNAL_ERROR,
                        error.to_string(),
                    ),
                },
                Err(error) => session_protocol::error_response(
                    id,
                    session_protocol::INVALID_PARAMS,
                    error.to_string(),
                ),
            },
            methods::SESSION_LOAD => match incoming.params_as::<acp::LoadSessionRequest>() {
                Ok(request) => {
                    match Self::external_session_load(this, &agent_id, request, false, cx).await {
                        Ok(response) => success_or_internal(id, response),
                        Err(error) => session_protocol::error_response(
                            id,
                            session_protocol::INTERNAL_ERROR,
                            error.to_string(),
                        ),
                    }
                }
                Err(error) => session_protocol::error_response(
                    id,
                    session_protocol::INVALID_PARAMS,
                    error.to_string(),
                ),
            },
            methods::SESSION_RESUME => match incoming.params_as::<acp::ResumeSessionRequest>() {
                Ok(request) => {
                    match Self::external_session_resume(this, &agent_id, request, cx).await {
                        Ok(response) => success_or_internal(id, response),
                        Err(error) => session_protocol::error_response(
                            id,
                            session_protocol::INTERNAL_ERROR,
                            error.to_string(),
                        ),
                    }
                }
                Err(error) => session_protocol::error_response(
                    id,
                    session_protocol::INVALID_PARAMS,
                    error.to_string(),
                ),
            },
            methods::SESSION_PROMPT => match incoming.params_as::<acp::PromptRequest>() {
                Ok(request) => {
                    match Self::external_session_prompt(this, &agent_id, request, cx).await {
                        Ok(response) => success_or_internal(id, response),
                        Err(error) => session_protocol::error_response(
                            id,
                            session_protocol::INTERNAL_ERROR,
                            error.to_string(),
                        ),
                    }
                }
                Err(error) => session_protocol::error_response(
                    id,
                    session_protocol::INVALID_PARAMS,
                    error.to_string(),
                ),
            },
            methods::SESSION_CANCEL => match incoming.params_as::<acp::CancelNotification>() {
                Ok(request) => {
                    if let Err(error) = Self::external_session_cancel(this, &agent_id, request, cx)
                    {
                        return session_protocol::error_response(
                            id,
                            session_protocol::INTERNAL_ERROR,
                            error.to_string(),
                        );
                    }
                    success_or_internal(id, serde_json::json!({}))
                }
                Err(error) => session_protocol::error_response(
                    id,
                    session_protocol::INVALID_PARAMS,
                    error.to_string(),
                ),
            },
            other => session_protocol::error_response(
                id,
                session_protocol::METHOD_NOT_FOUND,
                format!("method not found: {other}"),
            ),
        }
    }

    async fn acp_connect(
        this: Entity<Self>,
        agent_id: String,
        request: AcpConnectRequest,
        cx: &mut AsyncApp,
    ) -> Result<AcpConnectResponse> {
        if let Some(response) = this.read_with(cx, |host, _cx| {
            host.external_agents
                .get(&agent_id)
                .map(|agent| agent.connect_response())
        }) {
            return response;
        }

        let (project, agent_server_store) = this.read_with(cx, |host, cx| {
            (
                host.project.clone(),
                host.project.read(cx).agent_server_store().clone(),
            )
        });
        let default_mode = request.default_mode.map(acp::SessionModeId::new);
        let default_config_options = request
            .default_config_options
            .into_iter()
            .filter_map(|(key, value)| {
                serde_json::from_value::<AgentConfigOptionValue>(value)
                    .ok()
                    .map(|value| (key, value))
            })
            .collect();
        let command = AgentServerCommand {
            path: request.path.into(),
            args: request.args,
            env: Some(request.env.into_iter().collect()),
        };
        let io: Rc<dyn DetachedAcpIo> = Rc::new(HostExternalIo {
            host: this.downgrade(),
            agent_id: AgentId::new(agent_id.clone()),
        });
        AcpConnection::set_detached_io_for_next_spawn(io);
        let connection = AcpConnection::stdio(
            AgentId::new(agent_id.clone()),
            project,
            command,
            agent_server_store.downgrade(),
            default_mode,
            default_config_options,
            cx,
        )
        .await?;
        let connection = Rc::new(connection);
        let agent = ExternalAgent::from_connection(connection);
        let response = agent.connect_response()?;
        this.update(cx, |host, cx| {
            host.external_agents.insert(agent_id, agent);
            host.notify_session_list_changed(cx);
        });
        Ok(response)
    }

    fn external_connection(
        this: &Entity<Self>,
        agent_id: &str,
        cx: &AsyncApp,
    ) -> Result<Rc<AcpConnection>> {
        this.read_with(cx, |host, _cx| {
            host.external_agents
                .get(agent_id)
                .map(|agent| agent.connection.clone())
                .with_context(|| format!("external ACP agent `{agent_id}` is not connected"))
        })
    }

    fn external_authenticate(
        this: Entity<Self>,
        agent_id: &str,
        request: acp::AuthenticateRequest,
        cx: &mut AsyncApp,
    ) -> Result<Task<Result<()>>> {
        let connection = Self::external_connection(&this, agent_id, cx)?;
        Ok(cx.update(|cx| connection.authenticate(request.method_id, cx)))
    }

    async fn external_session_new(
        this: Entity<Self>,
        agent_id: &str,
        request: acp::NewSessionRequest,
        cx: &mut AsyncApp,
    ) -> Result<acp::NewSessionResponse> {
        let connection = Self::external_connection(&this, agent_id, cx)?;
        let work_dirs = PathList::new(&[request.cwd]);
        let (tx, rx) = futures::channel::oneshot::channel();
        this.update(cx, |host, cx| {
            let project = host.project.clone();
            let task = connection.new_session(project, work_dirs, cx);
            cx.spawn(async move |this, cx| {
                let result = task.await;
                if let Ok(thread) = &result {
                    let _ = this.update(cx, |host, cx| {
                        host.retain_daemon_thread(thread.clone(), cx);
                        host.notify_session_list_changed(cx);
                    });
                }
                let _ = tx.send(result);
            })
            .detach();
        });
        let thread = rx
            .await
            .map_err(|_| anyhow!("external ACP session/new dropped"))??;
        let session_id = this.read_with(cx, |_, cx| thread.read(cx).session_id().clone());
        Ok(acp::NewSessionResponse::new(session_id))
    }

    async fn external_session_load(
        this: Entity<Self>,
        agent_id: &str,
        request: acp::LoadSessionRequest,
        resume: bool,
        cx: &mut AsyncApp,
    ) -> Result<acp::LoadSessionResponse> {
        let session_id = request.session_id.clone();
        let already = this.read_with(cx, |host, _cx| {
            host.daemon_threads.contains_key(&session_id)
        });
        if already {
            return Ok(acp::LoadSessionResponse::new());
        }

        let connection = Self::external_connection(&this, agent_id, cx)?;
        let work_dirs = PathList::new(&[request.cwd]);
        let (tx, rx) = futures::channel::oneshot::channel();
        this.update(cx, |host, cx| {
            let project = host.project.clone();
            let task = if resume {
                connection.resume_session(session_id.clone(), project, work_dirs, None, cx)
            } else {
                connection.load_session(session_id.clone(), project, work_dirs, None, cx)
            };
            cx.spawn(async move |this, cx| {
                let result = task.await;
                if let Ok(thread) = &result {
                    let _ = this.update(cx, |host, cx| {
                        host.retain_daemon_thread(thread.clone(), cx);
                        host.notify_session_list_changed(cx);
                    });
                }
                let _ = tx.send(result);
            })
            .detach();
        });
        rx.await
            .map_err(|_| anyhow!("external ACP session/load dropped"))??;
        Ok(acp::LoadSessionResponse::new())
    }

    async fn external_session_resume(
        this: Entity<Self>,
        agent_id: &str,
        request: acp::ResumeSessionRequest,
        cx: &mut AsyncApp,
    ) -> Result<acp::ResumeSessionResponse> {
        Self::external_session_load(
            this,
            agent_id,
            acp::LoadSessionRequest::new(request.session_id, request.cwd),
            true,
            cx,
        )
        .await?;
        Ok(acp::ResumeSessionResponse::new())
    }

    async fn external_session_prompt(
        this: Entity<Self>,
        agent_id: &str,
        request: acp::PromptRequest,
        cx: &mut AsyncApp,
    ) -> Result<acp::PromptResponse> {
        let connection = Self::external_connection(&this, agent_id, cx)?;
        let (tx, rx) = futures::channel::oneshot::channel();
        this.update(cx, |_host, cx| {
            let task = connection.prompt(request, cx);
            cx.spawn(async move |_this, _cx| {
                let _ = tx.send(task.await);
            })
            .detach();
        });
        rx.await
            .map_err(|_| anyhow!("external ACP prompt dropped"))?
            .context("external ACP prompt")
    }

    fn external_session_cancel(
        this: Entity<Self>,
        agent_id: &str,
        request: acp::CancelNotification,
        cx: &mut AsyncApp,
    ) -> Result<()> {
        let connection = Self::external_connection(&this, agent_id, cx)?;
        this.update(cx, |_host, cx| {
            connection.cancel(&request.session_id, cx);
        });
        Ok(())
    }
}

fn permit_everything_permission_outcome(
    options: &PermissionOptions,
) -> Option<acp_thread::SelectedPermissionOutcome> {
    let option = options.first_option_of_kind(acp::PermissionOptionKind::AllowOnce)?;
    Some(acp_thread::SelectedPermissionOutcome::new(
        option.option_id.clone(),
        option.kind,
    ))
}

fn success_or_internal(id: Option<serde_json::Value>, value: impl serde::Serialize) -> String {
    session_protocol::success(id.clone(), value).unwrap_or_else(|error| {
        session_protocol::error_response(id, session_protocol::INTERNAL_ERROR, error.to_string())
    })
}
