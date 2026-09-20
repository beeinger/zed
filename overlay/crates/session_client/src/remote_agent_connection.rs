//! GUI `AgentConnection` that tunnels ACP JSON-RPC over Envelope.
//!
//! Follows `AcpConnection`: this process owns `AcpThread` and applies
//! `session/update` via `AcpThread::handle_session_update`. It does **not**
//! hold `NativeAgent` entities (those live on the daemon App).

use std::{any::Any, collections::HashMap as StdHashMap, rc::Rc};

use acp_thread::{AcpThread, AgentConnection, ElicitationStore};
use action_log::ActionLog;
use agent_client_protocol::schema::v1 as acp;
use anyhow::{Context as _, Result, anyhow};
use collections::HashMap;
use gpui::{
    App, AppContext as _, AsyncApp, Entity, EntityId, Global, SharedString, Task, WeakEntity,
};
use project::{AgentId, Project, agent_server_store::AgentServerCommand};
use rpc::{AnyProtoClient, TypedEnvelope, proto};
use session_protocol::{
    AUTHORIZATION_KIND_META, AcpConnectRequest, AcpConnectResponse, CatchUpResponse, EventSeq,
    INTERNAL_ERROR, INVALID_PARAMS, JsonRpcMessage, NATIVE_AGENT_ID, error_response, methods,
    notification, request,
};
use util::path_list::PathList;

/// ACP-over-Envelope connection. GUI death does not cancel the daemon turn;
/// reconnect uses `SessionSubscribe` + catch-up, then `session/load` or `session/resume`.
#[derive(Clone)]
pub struct RemoteAgentConnection {
    session: Entity<RemoteAgentSession>,
    agent_id: AgentId,
    telemetry_id: SharedString,
    agent_version: Option<SharedString>,
    auth_methods: Vec<acp::AuthMethod>,
    load_session: bool,
    resume_session: bool,
    request_elicitations: Entity<ElicitationStore>,
}

#[derive(Default)]
struct RemoteAgentHub {
    by_remote_client: HashMap<EntityId, Entity<RemoteAgentSession>>,
}

impl Global for RemoteAgentHub {}

struct RemoteAgentSession {
    proto: AnyProtoClient,
    sessions: HashMap<acp::SessionId, WeakEntity<AcpThread>>,
    next_rpc_id: i64,
    /// Last applied EventLog seq per ACP session (global log index, not a count).
    last_seq_by_session: HashMap<acp::SessionId, u64>,
    request_elicitations: Entity<ElicitationStore>,
}

fn session_for_remote(
    remote_client_id: EntityId,
    proto: AnyProtoClient,
    cx: &mut App,
) -> Entity<RemoteAgentSession> {
    if let Some(existing) = cx
        .try_global::<RemoteAgentHub>()
        .and_then(|hub| hub.by_remote_client.get(&remote_client_id).cloned())
    {
        return existing;
    }
    let session = new_remote_session(proto, cx);
    cx.default_global::<RemoteAgentHub>()
        .by_remote_client
        .insert(remote_client_id, session.clone());
    session
}

fn new_remote_session(proto: AnyProtoClient, cx: &mut App) -> Entity<RemoteAgentSession> {
    let request_elicitations = cx.new(|_| ElicitationStore::default());
    let session = cx.new(|_| RemoteAgentSession {
        proto: proto.clone(),
        sessions: HashMap::default(),
        next_rpc_id: 0,
        last_seq_by_session: HashMap::default(),
        request_elicitations,
    });
    proto.add_request_handler(session.downgrade(), RemoteAgentSession::handle_inbound);
    session
}

impl RemoteAgentConnection {
    /// Register inbound `SessionAgentRpc` handlers and return a native-agent connection.
    pub fn connect(proto: AnyProtoClient, cx: &mut App) -> Rc<Self> {
        let session = new_remote_session(proto, cx);
        Rc::new(Self::native(session, cx))
    }

    /// Build a native-agent connection from a `Project::remote` facade.
    pub fn for_project(project: &Entity<Project>, cx: &mut App) -> Result<Rc<Self>> {
        let remote = project
            .read(cx)
            .remote_client()
            .context("project is not via remote server")?;
        let remote_id = remote.entity_id();
        let proto = remote.read(cx).proto_client();
        let session = session_for_remote(remote_id, proto, cx);
        Ok(Rc::new(Self::native(session, cx)))
    }

    fn native(session: Entity<RemoteAgentSession>, cx: &App) -> Self {
        let request_elicitations = session.read(cx).request_elicitations.clone();
        Self {
            session,
            agent_id: AgentId::new(NATIVE_AGENT_ID),
            telemetry_id: "zed".into(),
            agent_version: None,
            auth_methods: Vec::new(),
            load_session: true,
            resume_session: true,
            request_elicitations,
        }
    }

    fn wire_agent_id(&self) -> String {
        if self.agent_id.as_ref() == NATIVE_AGENT_ID {
            String::new()
        } else {
            self.agent_id.to_string()
        }
    }

    fn next_id(&self, cx: &mut App) -> i64 {
        self.session.update(cx, |session, _| {
            session.next_rpc_id += 1;
            session.next_rpc_id
        })
    }

    fn rpc<T>(
        &self,
        method: &'static str,
        params: impl serde::Serialize + 'static,
        cx: &mut App,
    ) -> Task<Result<T>>
    where
        T: serde::de::DeserializeOwned + 'static,
    {
        let id = self.next_id(cx);
        let proto = self.session.read(cx).proto.clone();
        let agent_id = self.wire_agent_id();
        let json = match request(id, method, params) {
            Ok(json) => json,
            Err(error) => return Task::ready(Err(error)),
        };
        cx.spawn(async move |_| {
            let response = proto
                .request(proto::SessionAgentRpc {
                    json,
                    seq: 0,
                    agent_id,
                })
                .await
                .context("SessionAgentRpc")?;
            let parsed = JsonRpcMessage::parse(&response.json)?;
            parsed.result_as()
        })
    }
}

impl RemoteAgentSession {
    async fn handle_inbound(
        this: Entity<Self>,
        envelope: TypedEnvelope<proto::SessionAgentRpc>,
        mut cx: AsyncApp,
    ) -> Result<proto::SessionAgentRpc> {
        let incoming = match JsonRpcMessage::parse(&envelope.payload.json) {
            Ok(incoming) => incoming,
            Err(error) => {
                return Ok(proto::SessionAgentRpc {
                    json: error_response(None, session_protocol::PARSE_ERROR, error.to_string()),
                    seq: 0,
                    agent_id: String::new(),
                });
            }
        };

        if incoming.method_name() == Some(methods::SESSION_UPDATE) {
            match incoming.params_as::<acp::SessionNotification>() {
                Ok(notification) => {
                    let seq = envelope.payload.seq;
                    let session_id = notification.session_id.clone();
                    let applied: Result<(), anyhow::Error> = this.update(&mut cx, |session, cx| {
                        let Some(thread) = session
                            .sessions
                            .get(&session_id)
                            .and_then(|thread| thread.upgrade())
                        else {
                            return Ok(());
                        };
                        thread.update(cx, |thread, cx| {
                            thread
                                .handle_session_update(notification.update, cx)
                                .map_err(|error| anyhow!("{error}"))
                        })?;
                        if seq > 0 {
                            let last = session.last_seq_by_session.entry(session_id).or_insert(0);
                            *last = (*last).max(seq);
                        }
                        Ok(())
                    });
                    if let Err(error) = applied {
                        log::debug!("session/update for unknown GUI thread: {error}");
                    }
                }
                Err(error) => {
                    log::warn!("invalid session/update params: {error:#}");
                }
            }
            return Ok(proto::SessionAgentRpc {
                json: session_protocol::success(incoming.id, serde_json::json!({}))
                    .unwrap_or_else(|_| error_response(None, INTERNAL_ERROR, "serialize")),
                seq: 0,
                agent_id: String::new(),
            });
        }

        if incoming.method_name() == Some(methods::SESSION_REQUEST_PERMISSION) {
            return Self::handle_request_permission(this, incoming, &mut cx).await;
        }
        if incoming.method_name() == Some(methods::ELICITATION_CREATE) {
            return Self::handle_create_elicitation(this, incoming, &mut cx).await;
        }

        Ok(proto::SessionAgentRpc {
            json: error_response(
                incoming.id,
                session_protocol::METHOD_NOT_FOUND,
                "GUI only accepts session/update, session/request_permission, and elicitation/create from the daemon",
            ),
            seq: 0,
            agent_id: String::new(),
        })
    }

    async fn handle_request_permission(
        this: Entity<Self>,
        incoming: JsonRpcMessage,
        cx: &mut AsyncApp,
    ) -> Result<proto::SessionAgentRpc> {
        let request = match incoming.params_as::<acp::RequestPermissionRequest>() {
            Ok(request) => request,
            Err(error) => {
                return Ok(json_rpc_error(
                    incoming.id,
                    INVALID_PARAMS,
                    error.to_string(),
                ));
            }
        };
        let kind = authorization_kind_from_meta(&request.meta);
        let outcome_task = this.update(cx, |session, cx| -> Result<_> {
            let thread = session
                .sessions
                .get(&request.session_id)
                .and_then(|thread| thread.upgrade())
                .context("GUI thread not registered for permission request")?;
            Ok(thread.update(cx, |thread, cx| {
                thread.request_tool_call_authorization(
                    request.tool_call,
                    acp_thread::PermissionOptions::Flat(request.options),
                    kind,
                    cx,
                )
            })?)
        });
        match outcome_task {
            Ok(task) => {
                let outcome = task.await;
                json_rpc_success(
                    incoming.id,
                    acp::RequestPermissionResponse::new(outcome.into()),
                )
            }
            Err(error) => Ok(json_rpc_error(
                incoming.id,
                INTERNAL_ERROR,
                error.to_string(),
            )),
        }
    }

    async fn handle_create_elicitation(
        this: Entity<Self>,
        incoming: JsonRpcMessage,
        cx: &mut AsyncApp,
    ) -> Result<proto::SessionAgentRpc> {
        let request = match incoming.params_as::<acp::CreateElicitationRequest>() {
            Ok(request) => request,
            Err(error) => {
                return Ok(json_rpc_error(
                    incoming.id,
                    INVALID_PARAMS,
                    error.to_string(),
                ));
            }
        };
        let response_task = this.update(cx, |session, cx| -> Result<Task<_>> {
            match request.scope() {
                acp::ElicitationScope::Session(scope) => {
                    let thread = session
                        .sessions
                        .get(&scope.session_id)
                        .and_then(|thread| thread.upgrade())
                        .context("GUI thread not registered for elicitation")?;
                    thread
                        .update(cx, |thread, cx| thread.request_elicitation(request, cx))
                        .map_err(|error| anyhow!("{error}"))
                }
                _ => session
                    .request_elicitations
                    .update(cx, |store, cx| store.request_elicitation(request, cx))
                    .map_err(|error| anyhow!("{error}")),
            }
        });
        match response_task {
            Ok(task) => json_rpc_success(incoming.id, task.await),
            Err(error) => Ok(json_rpc_error(
                incoming.id,
                INTERNAL_ERROR,
                error.to_string(),
            )),
        }
    }
}

fn json_rpc_success(
    id: Option<serde_json::Value>,
    value: impl serde::Serialize,
) -> Result<proto::SessionAgentRpc> {
    Ok(proto::SessionAgentRpc {
        json: session_protocol::success(id.clone(), value)
            .unwrap_or_else(|_| error_response(id, INTERNAL_ERROR, "serialize")),
        seq: 0,
        agent_id: String::new(),
    })
}

fn json_rpc_error(
    id: Option<serde_json::Value>,
    code: i64,
    message: impl Into<String>,
) -> proto::SessionAgentRpc {
    proto::SessionAgentRpc {
        json: error_response(id, code, message),
        seq: 0,
        agent_id: String::new(),
    }
}

fn authorization_kind_from_meta(meta: &Option<acp::Meta>) -> acp_thread::AuthorizationKind {
    match meta
        .as_ref()
        .and_then(|meta| meta.get(AUTHORIZATION_KIND_META))
        .and_then(|value| value.as_str())
    {
        Some("action_choice") => acp_thread::AuthorizationKind::ActionChoice,
        _ => acp_thread::AuthorizationKind::PermissionGrant,
    }
}

impl AgentConnection for RemoteAgentConnection {
    fn agent_id(&self) -> AgentId {
        self.agent_id.clone()
    }

    fn telemetry_id(&self) -> SharedString {
        self.telemetry_id.clone()
    }

    fn agent_version(&self) -> Option<SharedString> {
        self.agent_version.clone()
    }

    fn new_session(
        self: Rc<Self>,
        project: Entity<Project>,
        work_dirs: PathList,
        cx: &mut App,
    ) -> Task<Result<Entity<AcpThread>>> {
        let cwd = work_dirs
            .ordered_paths()
            .next()
            .cloned()
            .unwrap_or_else(|| std::path::PathBuf::from("/"));
        let request = acp::NewSessionRequest::new(cwd);
        let this = self.clone();
        let create = self.rpc::<acp::NewSessionResponse>(methods::SESSION_NEW, request, cx);
        cx.spawn(async move |cx| {
            let response = create.await?;
            cx.update(|cx| this.open_local_thread(response.session_id, project, work_dirs, cx))
        })
    }

    fn supports_load_session(&self) -> bool {
        self.load_session
    }

    fn load_session(
        self: Rc<Self>,
        session_id: acp::SessionId,
        project: Entity<Project>,
        work_dirs: PathList,
        _title: Option<SharedString>,
        cx: &mut App,
    ) -> Task<Result<Entity<AcpThread>>> {
        self.open_existing_session(methods::SESSION_LOAD, session_id, project, work_dirs, cx)
    }

    fn supports_resume_session(&self) -> bool {
        self.resume_session
    }

    fn resume_session(
        self: Rc<Self>,
        session_id: acp::SessionId,
        project: Entity<Project>,
        work_dirs: PathList,
        _title: Option<SharedString>,
        cx: &mut App,
    ) -> Task<Result<Entity<AcpThread>>> {
        self.open_existing_session(methods::SESSION_RESUME, session_id, project, work_dirs, cx)
    }

    fn auth_methods(&self) -> &[acp::AuthMethod] {
        &self.auth_methods
    }

    fn authenticate(&self, method: acp::AuthMethodId, cx: &mut App) -> Task<Result<()>> {
        if self.agent_id.as_ref() == NATIVE_AGENT_ID {
            return Task::ready(Ok(()));
        }
        let task = self.rpc::<serde_json::Value>(
            methods::AUTHENTICATE,
            acp::AuthenticateRequest::new(method),
            cx,
        );
        cx.spawn(async move |_| {
            task.await?;
            Ok(())
        })
    }

    fn prompt(
        &self,
        params: acp::PromptRequest,
        cx: &mut App,
    ) -> Task<Result<acp::PromptResponse>> {
        self.rpc(methods::SESSION_PROMPT, params, cx)
    }

    fn cancel(&self, session_id: &acp::SessionId, cx: &mut App) {
        let proto = self.session.read(cx).proto.clone();
        let agent_id = self.wire_agent_id();
        let json = match notification(
            methods::SESSION_CANCEL,
            acp::CancelNotification::new(session_id.clone()),
        ) {
            Ok(json) => json,
            Err(error) => {
                log::error!("serialize session/cancel: {error:#}");
                return;
            }
        };
        cx.spawn(async move |_| {
            proto
                .request(proto::SessionAgentRpc {
                    json,
                    seq: 0,
                    agent_id,
                })
                .await
                .context("session/cancel")?;
            anyhow::Ok(())
        })
        .detach();
    }

    fn request_elicitations(&self) -> Option<Entity<ElicitationStore>> {
        Some(self.request_elicitations.clone())
    }

    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }
}

impl RemoteAgentConnection {
    fn open_local_thread(
        &self,
        session_id: acp::SessionId,
        project: Entity<Project>,
        work_dirs: PathList,
        cx: &mut App,
    ) -> Result<Entity<AcpThread>> {
        let connection: Rc<dyn AgentConnection> = Rc::new(self.clone());
        let action_log = cx.new(|_| ActionLog::new(project.clone()));
        let thread = cx.new(|cx| {
            AcpThread::new(
                None,
                None,
                Some(work_dirs),
                connection,
                project,
                action_log,
                session_id.clone(),
                watch::Receiver::constant(acp::PromptCapabilities::new()),
                cx,
            )
        });
        self.session.update(cx, |session, _| {
            session.sessions.insert(session_id, thread.downgrade());
        });
        Ok(thread)
    }

    fn open_existing_session(
        self: Rc<Self>,
        method: &'static str,
        session_id: acp::SessionId,
        project: Entity<Project>,
        work_dirs: PathList,
        cx: &mut App,
    ) -> Task<Result<Entity<AcpThread>>> {
        let cwd = work_dirs
            .ordered_paths()
            .next()
            .cloned()
            .unwrap_or_else(|| std::path::PathBuf::from("/"));
        let this = self.clone();
        cx.spawn(async move |cx| {
            let thread =
                cx.update(|cx| this.open_local_thread(session_id.clone(), project, work_dirs, cx))?;
            let params = if method == methods::SESSION_RESUME {
                serde_json::to_value(acp::ResumeSessionRequest::new(session_id.clone(), cwd))?
            } else {
                serde_json::to_value(acp::LoadSessionRequest::new(session_id.clone(), cwd))?
            };
            let load = cx.update(|cx| this.rpc::<serde_json::Value>(method, params, cx));
            load.await?;
            let last_seq = cx.update(|cx| {
                this.session
                    .read(cx)
                    .last_seq_by_session
                    .get(&session_id)
                    .copied()
                    .unwrap_or(0)
            });
            let subscribe = cx.update(|cx| this.subscribe(last_seq, cx));
            let catch_up = subscribe.await?;
            cx.update(|cx| {
                apply_catch_up(&thread, &catch_up_from_proto(&catch_up), cx)?;
                this.session.update(cx, |session, _| {
                    let last = session
                        .last_seq_by_session
                        .entry(session_id.clone())
                        .or_insert(0);
                    *last = (*last).max(catch_up.to_seq);
                });
                anyhow::Ok(())
            })?;
            Ok(thread)
        })
    }

    /// Subscribe to the daemon event log. Catch-up lines are `session/update` JSON.
    pub fn subscribe(&self, last_seq: u64, cx: &mut App) -> Task<Result<proto::SessionCatchUp>> {
        let proto = self.session.read(cx).proto.clone();
        cx.spawn(async move |_| {
            proto
                .request(proto::SessionSubscribe { last_seq })
                .await
                .context("SessionSubscribe")
        })
    }
}

/// GUI remote path: ask the daemon to spawn/reuse the child and tunnel ACP.
pub async fn connect_external_agent(
    agent_id: AgentId,
    project: Entity<Project>,
    command: AgentServerCommand,
    default_mode: Option<acp::SessionModeId>,
    default_config_options: StdHashMap<String, serde_json::Value>,
    cx: &mut AsyncApp,
) -> Result<Rc<dyn AgentConnection>> {
    let mut connection = cx.update(|cx| -> Result<RemoteAgentConnection> {
        let remote = project
            .read(cx)
            .remote_client()
            .context("project is not via remote server")?;
        let remote_id = remote.entity_id();
        let proto = remote.read(cx).proto_client();
        let session = session_for_remote(remote_id, proto, cx);
        let request_elicitations = session.read(cx).request_elicitations.clone();
        Ok(RemoteAgentConnection {
            session,
            agent_id: agent_id.clone(),
            telemetry_id: agent_id.to_string().into(),
            agent_version: None,
            auth_methods: Vec::new(),
            load_session: false,
            resume_session: false,
            request_elicitations,
        })
    })?;
    let request = AcpConnectRequest {
        path: command.path.display().to_string(),
        args: command.args,
        env: command.env.unwrap_or_default().into_iter().collect(),
        default_mode: default_mode.map(|mode| mode.to_string()),
        default_config_options,
    };
    let response = cx
        .update(|cx| connection.rpc::<AcpConnectResponse>(methods::ACP_CONNECT, request, cx))
        .await?;
    connection.telemetry_id = response.telemetry_id.into();
    connection.agent_version = response.agent_version.map(SharedString::from);
    connection.auth_methods = response
        .auth_methods_json
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect();
    connection.load_session = response.load_session;
    connection.resume_session = response.resume_session;
    Ok(Rc::new(connection) as _)
}

/// Apply catch-up JSON lines to a GUI thread (reconnect path).
pub fn apply_catch_up(
    thread: &Entity<AcpThread>,
    catch_up: &CatchUpResponse,
    cx: &mut App,
) -> Result<()> {
    let session_id = thread.read(cx).session_id().clone();
    for line in &catch_up.events_json {
        let incoming = JsonRpcMessage::parse(line)?;
        if incoming.method_name() != Some(methods::SESSION_UPDATE) {
            continue;
        }
        let notification: acp::SessionNotification = incoming.params_as()?;
        if notification.session_id != session_id {
            continue;
        }
        thread.update(cx, |thread, cx| {
            thread
                .handle_session_update(notification.update, cx)
                .map_err(|error| anyhow!("{error}"))
        })?;
    }
    Ok(())
}

fn catch_up_from_proto(catch_up: &proto::SessionCatchUp) -> CatchUpResponse {
    CatchUpResponse {
        from_seq: EventSeq(catch_up.from_seq),
        to_seq: EventSeq(catch_up.to_seq),
        events_json: catch_up.events_json.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proto_catch_up_maps_fields() {
        let proto_catch_up = proto::SessionCatchUp {
            from_seq: 1,
            to_seq: 2,
            events_json: vec!["{}".into()],
        };
        let catch_up = catch_up_from_proto(&proto_catch_up);
        assert_eq!(catch_up.from_seq, EventSeq(1));
        assert_eq!(catch_up.to_seq, EventSeq(2));
        assert_eq!(catch_up.events_json.len(), 1);
    }

    #[test]
    fn authorization_kind_defaults_to_permission_grant() {
        assert_eq!(
            authorization_kind_from_meta(&None),
            acp_thread::AuthorizationKind::PermissionGrant
        );
        let mut meta = acp::Meta::new();
        meta.insert(
            AUTHORIZATION_KIND_META.to_string(),
            serde_json::json!("action_choice"),
        );
        assert_eq!(
            authorization_kind_from_meta(&Some(meta)),
            acp_thread::AuthorizationKind::ActionChoice
        );
    }
}
