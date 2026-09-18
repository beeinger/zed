//! Local transport for the always-on session host.
//!
//! Production GUI uses the same `Project::remote` facade as SSH. This crate
//! will implement `RemoteConnection` over unix sockets (Windows: follow
//! `remote_server`'s existing named-pipe/unix split). Tests keep
//! `Project::local`.
//!
//! Cycle rule: `remote` must not depend on this crate. A later phase registers
//! a connect callback from `session_transport::init` into `ConnectionPool`.

use std::path::PathBuf;

pub use session_protocol::daemon_socket_id;

/// Options for a same-machine daemon. The matching `RemoteConnectionOptions`
/// variant lives in `remote` so that crate does not import overlay.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LocalProject {
    pub project_root: PathBuf,
    pub nickname: Option<String>,
}

impl LocalProject {
    pub fn new(project_root: PathBuf) -> Self {
        Self {
            project_root,
            nickname: None,
        }
    }

    pub fn daemon_id(&self) -> String {
        session_protocol::daemon_socket_id(
            "local",
            "localhost",
            &self.project_root.to_string_lossy(),
        )
    }
}

/// Placeholder so production binaries can call `session_transport::init` at a
/// single, marked hook. Currently a no-op.
pub fn init() {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn daemon_id_matches_protocol() {
        let project = LocalProject::new(PathBuf::from("/tmp/app"));
        assert_eq!(
            project.daemon_id(),
            session_protocol::daemon_socket_id("local", "localhost", "/tmp/app")
        );
        assert_eq!(project.project_root.as_path(), Path::new("/tmp/app"));
    }
}
