//! Native agent + LLM HTTP hosted inside the daemon `App`.
//!
//! GUI death is a no-op for in-flight turns: this crate holds `NativeAgent`
//! and a `Project` that **shares** `HeadlessProject` stores. Tools do not RPC
//! back to the GUI.

mod acp_rpc;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use acp_thread::AcpThread;
use agent::{NativeAgent, Templates, ThreadStore};
use agent_client_protocol::schema::v1 as acp;
use anyhow::Result;
use client::{Client, RefreshLlmTokenListener, UserStore};
use clock::RealSystemClock;
use fs::Fs;
use gpui::{App, AppContext as _, AsyncApp, Entity, Global};
use http_client::{HttpClient, HttpClientWithUrl};
use language::LanguageRegistry;
use node_runtime::NodeRuntime;
use project::{HeadlessProjectStores, Project};
use rpc::{AnyProtoClient, TypedEnvelope, proto};
use session_protocol::{DisconnectedPermissionPolicy, EventLog, EventSeq, methods, notification};

/// How the daemon behaves with no attached GUI.
#[derive(Clone, Copy, Debug)]
pub struct HostConfig {
    pub disconnected_permissions: DisconnectedPermissionPolicy,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            disconnected_permissions: DisconnectedPermissionPolicy::AllowAccordingToTrust,
        }
    }
}

/// Inputs taken from `HeadlessProject::new` after local stores exist.
pub struct SessionHostInit {
    pub session: AnyProtoClient,
    pub fs: Arc<dyn Fs>,
    pub http_client: Arc<dyn HttpClient>,
    pub node_runtime: NodeRuntime,
    pub languages: Arc<LanguageRegistry>,
    pub stores: HeadlessProjectStores,
}

/// Lives on the daemon `App`. Held as a GPUI global so GUI disconnect does not drop it.
pub struct SessionHost {
    pub project: Entity<Project>,
    pub agent: Entity<NativeAgent>,
    pub config: HostConfig,
    /// Proto client used to push `session/update` when a GUI is attached.
    session: AnyProtoClient,
    log: EventLog,
    /// Strong handles so `observe_release` does not drop NativeAgent sessions.
    daemon_threads: HashMap<acp::SessionId, Entity<AcpThread>>,
}

struct GlobalSessionHost(Entity<SessionHost>);

impl Global for GlobalSessionHost {}

impl SessionHost {
    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalSessionHost>()
            .map(|global| global.0.clone())
    }

    pub fn log_head(&self) -> EventSeq {
        self.log.head()
    }

    fn start(init: SessionHostInit, cx: &mut App) -> Result<Entity<Self>> {
        language_model::init(cx);
        gpui_tokio::init(cx);
        cx.set_global(acp_thread::HeadlessTerminal(true));

        let http = Arc::new(HttpClientWithUrl::new(
            init.http_client,
            "https://zed.dev",
            None,
        ));
        let client = Client::new(Arc::new(RealSystemClock), http, cx);
        let user_store = cx.new(|cx| UserStore::new(client.clone(), cx));
        RefreshLlmTokenListener::register(client.clone(), user_store.clone(), cx);
        language_models::init(user_store.clone(), client.clone(), cx);

        let project = Project::from_headless_stores(
            init.stores,
            client,
            init.node_runtime,
            user_store,
            init.languages,
            init.fs.clone(),
            cx,
        );
        let thread_store = cx.new(|cx| ThreadStore::new(cx));
        let agent = NativeAgent::new(thread_store, Templates::new(), init.fs, cx);

        let host = cx.new(|_| Self {
            project,
            agent,
            config: HostConfig::default(),
            session: init.session.clone(),
            log: EventLog::new(),
            daemon_threads: HashMap::new(),
        });

        host.update(cx, |host, cx| {
            let weak_host = cx.weak_entity();
            let auto_resolve = match host.config.disconnected_permissions {
                DisconnectedPermissionPolicy::AllowAccordingToTrust => Some(true),
                DisconnectedPermissionPolicy::Deny => Some(false),
                DisconnectedPermissionPolicy::Queue => None,
            };
            host.agent.update(cx, |agent, _cx| {
                agent.set_auto_resolve_permissions(auto_resolve);
                agent.set_session_notification_sink(Rc::new(move |notification, persist, cx| {
                    let _ = weak_host.update(cx, |host, cx| {
                        host.emit_session_update(notification, persist, cx);
                    });
                }));
            });
        });

        init.session
            .add_request_handler(host.downgrade(), Self::handle_subscribe);
        init.session
            .add_request_handler(host.downgrade(), Self::handle_rpc);

        Ok(host)
    }

    fn emit_session_update(
        &mut self,
        session_notification: acp::SessionNotification,
        persist: bool,
        cx: &mut App,
    ) {
        let json = match notification(methods::SESSION_UPDATE, &session_notification) {
            Ok(json) => json,
            Err(error) => {
                log::error!("serialize session/update: {error:#}");
                return;
            }
        };
        if persist {
            let seq = self.log.append(json.clone()).0;
            let proto = self.session.clone();
            cx.spawn(async move |_| {
                if let Err(error) = proto.request(proto::SessionAgentRpc { json, seq }).await {
                    log::debug!("session/update push skipped (no GUI?): {error:#}");
                }
                anyhow::Ok(())
            })
            .detach();
        }
    }

    fn retain_daemon_thread(&mut self, thread: Entity<AcpThread>, cx: &App) -> acp::SessionId {
        let session_id = thread.read(cx).session_id().clone();
        self.daemon_threads.insert(session_id.clone(), thread);
        session_id
    }

    async fn handle_subscribe(
        this: Entity<Self>,
        envelope: TypedEnvelope<proto::SessionSubscribe>,
        mut cx: AsyncApp,
    ) -> Result<proto::SessionCatchUp> {
        let last_seq = EventSeq(envelope.payload.last_seq);
        Ok(this.update(&mut cx, |host, _cx| {
            let catch_up = host.log.catch_up(last_seq);
            proto::SessionCatchUp {
                from_seq: catch_up.from_seq.0,
                to_seq: catch_up.to_seq.0,
                events_json: catch_up.events_json,
            }
        }))
    }

    async fn handle_rpc(
        this: Entity<Self>,
        envelope: TypedEnvelope<proto::SessionAgentRpc>,
        mut cx: AsyncApp,
    ) -> Result<proto::SessionAgentRpc> {
        let json = Self::handle_json_rpc(this, envelope.payload.json, &mut cx).await;
        Ok(proto::SessionAgentRpc { json, seq: 0 })
    }
}

/// Construct NativeAgent on the daemon and register Envelope handlers.
/// Failures are logged so HeadlessProject still starts (existing remote tests).
pub fn init(init: SessionHostInit, cx: &mut App) {
    match SessionHost::start(init, cx) {
        Ok(host) => {
            cx.set_global(GlobalSessionHost(host));
            log::info!("session host started (native agent on daemon)");
        }
        Err(error) => {
            log::error!("session host failed to start: {error:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_permissions_do_not_stall() {
        assert_eq!(
            HostConfig::default().disconnected_permissions,
            DisconnectedPermissionPolicy::AllowAccordingToTrust
        );
    }
}
