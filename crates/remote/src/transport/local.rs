// FORK:local-transport
use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
};

use anyhow::Result;
use gpui::AsyncApp;
use parking_lot::Mutex;

use crate::remote_client::{RemoteClientDelegate, RemoteConnection};

/// Same-machine daemon. `nickname` is display-only and is not part of
/// [`crate::RemoteConnectionIdentity`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct LocalConnectionOptions {
    pub project_root: PathBuf,
    pub nickname: Option<String>,
}

impl LocalConnectionOptions {
    pub fn new(project_root: PathBuf) -> Self {
        Self {
            project_root,
            nickname: None,
        }
    }

    /// Socket-name body. Release-channel prefixes are applied by
    /// `ConnectionIdentifier::stable`.
    pub fn daemon_id(&self) -> String {
        session_protocol::daemon_socket_id("local", "localhost", &self.identity_project_root())
    }

    /// Project root used for persistence identity (trailing slashes stripped).
    pub fn identity_project_root(&self) -> String {
        let text = self.project_root.to_string_lossy();
        if text.len() <= 1 {
            text.into_owned()
        } else {
            text.trim_end_matches(['/', '\\']).to_string()
        }
    }
}

pub type LocalRemoteConnectFn = fn(
    LocalConnectionOptions,
    Arc<dyn RemoteClientDelegate>,
    &mut AsyncApp,
) -> Result<Arc<dyn RemoteConnection>>;

static LOCAL_REMOTE_CONNECT: OnceLock<Mutex<Option<LocalRemoteConnectFn>>> = OnceLock::new();

fn connector_slot() -> &'static Mutex<Option<LocalRemoteConnectFn>> {
    LOCAL_REMOTE_CONNECT.get_or_init(|| Mutex::new(None))
}

/// Called from `session_transport::init`. `remote` must not import overlay.
pub fn register_local_remote_connect(connect: LocalRemoteConnectFn) {
    *connector_slot().lock() = Some(connect);
}

pub(crate) fn connect(
    options: LocalConnectionOptions,
    delegate: Arc<dyn RemoteClientDelegate>,
    cx: &mut AsyncApp,
) -> Result<Arc<dyn RemoteConnection>> {
    let Some(connect) = *connector_slot().lock() else {
        anyhow::bail!(
            "local remote connector is not registered; call session_transport::init before connecting"
        );
    };
    connect(options, delegate, cx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_id_matches_protocol_and_ignores_nickname() {
        let options = LocalConnectionOptions {
            project_root: PathBuf::from("/tmp/app/"),
            nickname: Some("work".to_string()),
        };
        assert_eq!(
            options.daemon_id(),
            session_protocol::daemon_socket_id("local", "localhost", "/tmp/app")
        );
        assert_eq!(options.identity_project_root(), "/tmp/app");
    }
}
// FORK:end
