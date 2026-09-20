//! Native agent + LLM HTTP hosted inside the daemon `App`.
//!
//! GUI death is a no-op for in-flight turns: this crate holds `NativeAgent`
//! and a `Project` that **shares** `HeadlessProject` stores. Tools do not RPC
//! back to the GUI.

mod acp_rpc;
mod external_acp;
mod gui_prompts;

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
use session_protocol::{
    DEFAULT_PROMPT_TIMEOUT_MS, DetachedPermissions, DisconnectedPromptWait, EventLog, EventSeq,
    methods, notification,
};
use settings::{
    DetachedPermissionsContent, DisconnectedPromptWaitContent, Settings, SettingsContent,
    SettingsStore,
};

use crate::external_acp::ExternalAgent;
use crate::gui_prompts::HostPromptHandler;

/// How the daemon behaves with no attached GUI.
#[derive(Clone, Copy, Debug)]
pub struct HostConfig {
    pub detached_permissions: DetachedPermissions,
    pub disconnected_prompt_wait: DisconnectedPromptWait,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            detached_permissions: DetachedPermissions::Ask,
            disconnected_prompt_wait: DisconnectedPromptWait::Forever,
        }
    }
}

impl HostConfig {
    fn from_app(cx: &App) -> Self {
        SessionHostSettings::try_get(cx)
            .copied()
            .map(Self::from)
            .unwrap_or_default()
    }

    pub(crate) fn permit_tool_permissions(self) -> bool {
        matches!(
            self.detached_permissions,
            DetachedPermissions::PermitEverything
        )
    }
}

impl From<SessionHostSettings> for HostConfig {
    fn from(settings: SessionHostSettings) -> Self {
        Self {
            detached_permissions: settings.detached_permissions,
            disconnected_prompt_wait: settings.disconnected_prompt_wait,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct SessionHostSettings {
    detached_permissions: DetachedPermissions,
    disconnected_prompt_wait: DisconnectedPromptWait,
}

impl Settings for SessionHostSettings {
    fn from_settings(content: &SettingsContent) -> Self {
        let agent = content.agent.as_ref();
        let wait = agent
            .and_then(|agent| agent.disconnected_prompt_wait)
            .unwrap_or(DisconnectedPromptWaitContent::Forever);
        let timeout_ms = agent
            .and_then(|agent| agent.disconnected_prompt_timeout_ms)
            .unwrap_or(DEFAULT_PROMPT_TIMEOUT_MS);
        Self {
            detached_permissions: match agent
                .and_then(|agent| agent.detached_permissions)
                .unwrap_or(DetachedPermissionsContent::Ask)
            {
                DetachedPermissionsContent::Ask => DetachedPermissions::Ask,
                DetachedPermissionsContent::PermitEverything => {
                    DetachedPermissions::PermitEverything
                }
            },
            disconnected_prompt_wait: match wait {
                DisconnectedPromptWaitContent::Forever => DisconnectedPromptWait::Forever,
                DisconnectedPromptWaitContent::Timeout => {
                    DisconnectedPromptWait::Timeout { millis: timeout_ms }
                }
            },
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
    pub(crate) external_agents: HashMap<String, ExternalAgent>,
    gui_attached_tx: watch::Sender<u64>,
    /// Kept so `gui_attached_tx.send` always has a receiver.
    _gui_attached_rx: watch::Receiver<u64>,
    gui_generation: u64,
    next_rpc_id: i64,
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
        if cx.has_global::<SettingsStore>() {
            SessionHostSettings::register(cx);
        }
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
        let (gui_attached_tx, gui_attached_rx) = watch::channel(0);
        let config = HostConfig::from_app(cx);

        let host = cx.new(|_| Self {
            project,
            agent,
            config,
            session: init.session.clone(),
            log: EventLog::new(),
            daemon_threads: HashMap::new(),
            external_agents: HashMap::new(),
            gui_attached_tx,
            _gui_attached_rx: gui_attached_rx,
            gui_generation: 0,
            next_rpc_id: 0,
        });

        host.update(cx, |host, cx| {
            let weak_host = cx.weak_entity();
            let permit = host.config.permit_tool_permissions();
            host.agent.update(cx, |agent, _cx| {
                agent.set_daemon_prompt_handler(
                    Rc::new(HostPromptHandler(weak_host.clone())),
                    permit,
                );
                agent.set_session_notification_sink(Rc::new(move |notification, persist, cx| {
                    let _ = weak_host.update(cx, |host, cx| {
                        host.emit_session_update(notification, persist, "", cx);
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

    pub(crate) fn emit_session_update(
        &mut self,
        session_notification: acp::SessionNotification,
        persist: bool,
        agent_id: &str,
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
            let agent_id = agent_id.to_string();
            cx.spawn(async move |_| {
                if let Err(error) = proto
                    .request(proto::SessionAgentRpc {
                        json,
                        seq,
                        agent_id,
                    })
                    .await
                {
                    log::debug!("session/update push skipped (no GUI?): {error:#}");
                }
                anyhow::Ok(())
            })
            .detach();
        }
    }

    pub(crate) fn retain_daemon_thread(
        &mut self,
        thread: Entity<AcpThread>,
        cx: &App,
    ) -> acp::SessionId {
        let session_id = thread.read(cx).session_id().clone();
        self.daemon_threads.insert(session_id.clone(), thread);
        session_id
    }

    fn refresh_prompt_policy(&mut self, cx: &mut gpui::Context<Self>) {
        self.config = HostConfig::from_app(cx);
        let permit = self.config.permit_tool_permissions();
        let handler: Rc<dyn agent::DaemonPromptHandler> =
            Rc::new(HostPromptHandler(cx.weak_entity()));
        self.agent.update(cx, |agent, _cx| {
            agent.set_daemon_prompt_handler(handler, permit);
        });
    }

    fn notify_gui_attached(&mut self) {
        self.gui_generation = self.gui_generation.wrapping_add(1);
        let _ = self.gui_attached_tx.send(self.gui_generation);
    }

    async fn handle_subscribe(
        this: Entity<Self>,
        envelope: TypedEnvelope<proto::SessionSubscribe>,
        mut cx: AsyncApp,
    ) -> Result<proto::SessionCatchUp> {
        let last_seq = EventSeq(envelope.payload.last_seq);
        Ok(this.update(&mut cx, |host, cx| {
            host.refresh_prompt_policy(cx);
            host.notify_gui_attached();
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
        let agent_id = envelope.payload.agent_id;
        let json =
            Self::handle_json_rpc(this, envelope.payload.json, agent_id.clone(), &mut cx).await;
        Ok(proto::SessionAgentRpc {
            json,
            seq: 0,
            agent_id,
        })
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
    fn default_asks_and_waits_forever() {
        let config = HostConfig::default();
        assert_eq!(config.detached_permissions, DetachedPermissions::Ask);
        assert_eq!(
            config.disconnected_prompt_wait,
            DisconnectedPromptWait::Forever
        );
        assert!(!config.permit_tool_permissions());
    }
}
