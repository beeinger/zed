//! GUI `AgentConnection` that tunnels ACP JSON-RPC over Envelope.
//!
//! Follows `AcpConnection`: this process owns `AcpThread` and applies
//! `session/update` via `AcpThread::handle_session_update`. It does **not**
//! hold `NativeAgent` entities (those live on the daemon App).

use std::{any::Any, rc::Rc};

use acp_thread::{AcpThread, AgentConnection};
use action_log::ActionLog;
use agent_client_protocol::schema::v1 as acp;
use anyhow::{Context as _, Result, anyhow};
use collections::HashMap;
use gpui::{App, AppContext as _, AsyncApp, Entity, SharedString, Task, WeakEntity};
use project::{AgentId, Project};
use rpc::{AnyProtoClient, TypedEnvelope, proto};
use session_protocol::{
    CatchUpResponse, EventSeq, INTERNAL_ERROR, JsonRpcMessage, error_response, methods,
    notification, request,
};
use util::path_list::PathList;

const ZED_AGENT_ID: &str = "Zed Agent";

/// ACP-over-Envelope connection. GUI death does not cancel the daemon turn;
/// reconnect uses `SessionSubscribe` + catch-up, then `session/load`.
#[derive(Clone)]
pub struct RemoteAgentConnection(Entity<RemoteAgentSession>);

struct RemoteAgentSession {
    proto: AnyProtoClient,
    sessions: HashMap<acp::SessionId, WeakEntity<AcpThread>>,
    next_rpc_id: i64,
    /// Last applied EventLog seq per ACP session (global log index, not a count).
    last_seq_by_session: HashMap<acp::SessionId, u64>,
}

impl RemoteAgentConnection {
    /// Register inbound `SessionAgentRpc` handlers and return a connection.
    pub fn connect(proto: AnyProtoClient, cx: &mut App) -> Rc<Self> {
        let session = cx.new(|_| RemoteAgentSession {
            proto: proto.clone(),
            sessions: HashMap::default(),
            next_rpc_id: 0,
            last_seq_by_session: HashMap::default(),
        });
        proto.add_request_handler(session.downgrade(), RemoteAgentSession::handle_inbound);
        Rc::new(Self(session))
    }

    /// Build a connection from a `Project::remote` facade.
    pub fn for_project(project: &Entity<Project>, cx: &mut App) -> Result<Rc<Self>> {
        let proto = project
            .read(cx)
            .remote_client()
            .context("project is not via remote server")?
            .read(cx)
            .proto_client();
        Ok(Self::connect(proto, cx))
    }

    fn next_id(&self, cx: &mut App) -> i64 {
        self.0.update(cx, |session, _| {
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
        let proto = self.0.read(cx).proto.clone();
        let json = match request(id, method, params) {
            Ok(json) => json,
            Err(error) => return Task::ready(Err(error)),
        };
        cx.spawn(async move |_| {
            let response = proto
                .request(proto::SessionAgentRpc { json, seq: 0 })
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
                });
            }
        };

        if incoming.method_name() == Some(methods::SESSION_UPDATE) {
            match incoming.params_as::<acp::SessionNotification>() {
                Ok(notification) => {
                    let seq = envelope.payload.seq;
                    let session_id = notification.session_id.clone();
                    let applied: Result<(), anyhow::Error> =
                        this.update(&mut cx, |session, cx| {
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
                                let last =
                                    session.last_seq_by_session.entry(session_id).or_insert(0);
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
            });
        }

        Ok(proto::SessionAgentRpc {
            json: error_response(
                incoming.id,
                session_protocol::METHOD_NOT_FOUND,
                "GUI only accepts session/update from the daemon",
            ),
            seq: 0,
        })
    }
}

impl AgentConnection for RemoteAgentConnection {
    fn agent_id(&self) -> AgentId {
        AgentId::new(ZED_AGENT_ID)
    }

    fn telemetry_id(&self) -> SharedString {
        "zed".into()
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
        true
    }

    fn load_session(
        self: Rc<Self>,
        session_id: acp::SessionId,
        project: Entity<Project>,
        work_dirs: PathList,
        _title: Option<SharedString>,
        cx: &mut App,
    ) -> Task<Result<Entity<AcpThread>>> {
        let cwd = work_dirs
            .ordered_paths()
            .next()
            .cloned()
            .unwrap_or_else(|| std::path::PathBuf::from("/"));
        let request = acp::LoadSessionRequest::new(session_id.clone(), cwd);
        let this = self.clone();
        cx.spawn(async move |cx| {
            let thread = cx.update(|cx| {
                this.open_local_thread(session_id.clone(), project, work_dirs, cx)
            })?;
            let load = cx.update(|cx| {
                this.rpc::<acp::LoadSessionResponse>(methods::SESSION_LOAD, request, cx)
            });
            load.await?;
            let last_seq = cx.update(|cx| {
                this.0
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
                this.0.update(cx, |session, _| {
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

    fn auth_methods(&self) -> &[acp::AuthMethod] {
        &[]
    }

    fn authenticate(&self, _method: acp::AuthMethodId, _cx: &mut App) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }

    fn prompt(
        &self,
        params: acp::PromptRequest,
        cx: &mut App,
    ) -> Task<Result<acp::PromptResponse>> {
        self.rpc(methods::SESSION_PROMPT, params, cx)
    }

    fn cancel(&self, session_id: &acp::SessionId, cx: &mut App) {
        let proto = self.0.read(cx).proto.clone();
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
                .request(proto::SessionAgentRpc { json, seq: 0 })
                .await
                .context("session/cancel")?;
            anyhow::Ok(())
        })
        .detach();
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
        self.0.update(cx, |session, _| {
            session.sessions.insert(session_id, thread.downgrade());
        });
        Ok(thread)
    }
}

impl RemoteAgentConnection {
    /// Subscribe to the daemon event log. Catch-up lines are `session/update` JSON.
    pub fn subscribe(&self, last_seq: u64, cx: &mut App) -> Task<Result<proto::SessionCatchUp>> {
        let proto = self.0.read(cx).proto.clone();
        cx.spawn(async move |_| {
            proto
                .request(proto::SessionSubscribe { last_seq })
                .await
                .context("SessionSubscribe")
        })
    }
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
}
