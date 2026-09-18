//! Local unix-socket `RemoteConnection` for the always-on session host.
//!
//! Production GUI uses the same `Project::remote` facade as SSH: spawn
//! `remote_server proxy --identifier …` on this machine, then speak Envelope
//! RPC over child stdio. The proxy child is `kill_on_drop`; Phase 1's attach
//! path keeps the `run` daemon alive. Tests keep `Project::local`.
//!
//! Cycle rule: `remote` does not depend on this crate. `init` registers
//! `LocalRemoteConnection::connect` into `ConnectionPool`.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Context as _, Result};
use async_trait::async_trait;
use collections::HashMap;
use futures::channel::mpsc::{Sender, UnboundedReceiver, UnboundedSender};
use gpui::{App, AppContext as _, AsyncApp, Task};
use remote::{
    CommandTemplate, Interactive, LocalConnectionOptions, RemoteArch, RemoteClientDelegate,
    RemoteConnection, RemoteConnectionOptions, RemoteOs, RemotePlatform,
    handle_rpc_messages_over_child_process_stdio, register_local_remote_connect,
};
use rpc::proto::Envelope;
use util::{
    command::{Stdio, new_command},
    paths::{PathStyle, RemotePathBuf},
    shell::{ShellKind, get_default_system_shell, get_system_shell},
};

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

    pub fn connection_options(&self) -> LocalConnectionOptions {
        LocalConnectionOptions {
            project_root: self.project_root.clone(),
            nickname: self.nickname.clone(),
        }
    }

    pub fn daemon_id(&self) -> String {
        self.connection_options().daemon_id()
    }

    pub fn into_remote_options(self) -> RemoteConnectionOptions {
        RemoteConnectionOptions::Local(self.connection_options())
    }
}

/// Registers `LocalRemoteConnection::connect` so `ConnectionPool` can open
/// `RemoteConnectionOptions::Local` without `remote` depending on this crate.
pub fn init(_cx: &mut App) {
    register_local_remote_connect(LocalRemoteConnection::connect);
}

/// Production GUI should open a folder through the local daemon with:
///
/// 1. `session_transport::init(cx)` once at app start.
/// 2. `remote::connect(local_daemon_connection_options(root, nickname), delegate, cx)`.
/// 3. `workspace::open_remote_project_with_new_connection(...)`.
///
/// Tests keep `Project::local`.
pub fn local_daemon_connection_options(
    project_root: PathBuf,
    nickname: Option<String>,
) -> RemoteConnectionOptions {
    RemoteConnectionOptions::Local(LocalConnectionOptions {
        project_root,
        nickname,
    })
}

pub struct LocalRemoteConnection {
    connection_options: LocalConnectionOptions,
    binary_path: PathBuf,
    platform: RemotePlatform,
    path_style: PathStyle,
    os_version: Option<String>,
    shell: String,
    default_system_shell: String,
    killed: AtomicBool,
}

impl LocalRemoteConnection {
    pub fn connect(
        options: LocalConnectionOptions,
        delegate: Arc<dyn RemoteClientDelegate>,
        cx: &mut AsyncApp,
    ) -> Result<Arc<dyn RemoteConnection>> {
        Ok(Arc::new(Self::new(options, delegate, cx)?) as Arc<dyn RemoteConnection>)
    }

    fn new(
        connection_options: LocalConnectionOptions,
        delegate: Arc<dyn RemoteClientDelegate>,
        cx: &mut AsyncApp,
    ) -> Result<Self> {
        delegate.set_status(Some("Locating local remote_server"), cx);
        let binary_path = find_remote_server_binary()?;
        log::info!(
            "using local remote_server binary at {}",
            binary_path.display()
        );

        delegate.set_status(Some("Connecting to local session host"), cx);

        Ok(Self {
            connection_options,
            binary_path,
            platform: current_remote_platform()?,
            path_style: PathStyle::local(),
            os_version: local_os_version(),
            shell: get_system_shell(),
            default_system_shell: get_default_system_shell(),
            killed: AtomicBool::new(false),
        })
    }
}

#[async_trait(?Send)]
impl RemoteConnection for LocalRemoteConnection {
    fn start_proxy(
        &self,
        unique_identifier: String,
        reconnect: bool,
        incoming_tx: UnboundedSender<Envelope>,
        outgoing_rx: UnboundedReceiver<Envelope>,
        connection_activity_tx: Sender<()>,
        delegate: Arc<dyn RemoteClientDelegate>,
        cx: &mut AsyncApp,
    ) -> Task<Result<i32>> {
        delegate.set_status(Some("Starting proxy"), cx);

        let mut command = new_command(&self.binary_path);
        command
            .arg("proxy")
            .arg("--identifier")
            .arg(&unique_identifier);
        if reconnect {
            command.arg("--reconnect");
        }

        let proxy_process = match command
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(process) => process,
            Err(error) => {
                return Task::ready(Err(anyhow::Error::new(error).context(format!(
                    "failed to spawn local remote_server proxy from {}",
                    self.binary_path.display()
                ))));
            }
        };

        handle_rpc_messages_over_child_process_stdio(
            proxy_process,
            incoming_tx,
            outgoing_rx,
            connection_activity_tx,
            cx,
        )
    }

    fn upload_directory(
        &self,
        src_path: PathBuf,
        dest_path: RemotePathBuf,
        cx: &App,
    ) -> Task<Result<()>> {
        let destination = PathBuf::from(dest_path.to_string());
        cx.background_spawn(async move {
            copy_directory_recursive(&src_path, &destination).with_context(|| {
                format!(
                    "failed to upload directory {} -> {}",
                    src_path.display(),
                    destination.display()
                )
            })
        })
    }

    async fn kill(&self) -> Result<()> {
        // Dropping the proxy task (kill_on_drop) stops the proxy child. The
        // `remote_server run` daemon is left running so a later proxy can attach.
        self.killed.store(true, Ordering::Release);
        Ok(())
    }

    fn has_been_killed(&self) -> bool {
        self.killed.load(Ordering::Acquire)
    }

    fn shares_network_interface(&self) -> bool {
        true
    }

    fn build_command(
        &self,
        program: Option<String>,
        args: &[String],
        env: &HashMap<String, String>,
        working_dir: Option<String>,
        port_forward: Option<(u16, String, u16)>,
        _interactive: Interactive,
    ) -> Result<CommandTemplate> {
        if port_forward.is_some() {
            anyhow::bail!("local connection shares the network interface with the host");
        }

        let is_windows = self.path_style.is_windows();
        let (program, args) = match program {
            Some(program) => (program, args.to_vec()),
            None => {
                let login_args = if is_windows {
                    Vec::new()
                } else {
                    vec!["-l".to_string()]
                };
                (self.shell.clone(), login_args)
            }
        };

        if let Some(working_dir) = working_dir {
            return local_command_in_working_directory(
                &self.shell,
                is_windows,
                program,
                args,
                env,
                &working_dir,
            );
        }

        Ok(CommandTemplate {
            program,
            args,
            env: env.clone(),
        })
    }

    fn build_forward_ports_command(
        &self,
        _forwards: Vec<(u16, String, u16)>,
    ) -> Result<CommandTemplate> {
        anyhow::bail!("local connection shares the network interface with the host")
    }

    fn connection_options(&self) -> RemoteConnectionOptions {
        RemoteConnectionOptions::Local(self.connection_options.clone())
    }

    fn path_style(&self) -> PathStyle {
        self.path_style
    }

    fn remote_platform(&self) -> RemotePlatform {
        self.platform
    }

    fn remote_os_version(&self) -> Option<String> {
        self.os_version.clone()
    }

    fn shell(&self) -> String {
        self.shell.clone()
    }

    fn default_system_shell(&self) -> String {
        self.default_system_shell.clone()
    }

    fn has_wsl_interop(&self) -> bool {
        false
    }
}

fn local_command_in_working_directory(
    shell: &str,
    is_windows: bool,
    program: String,
    args: Vec<String>,
    env: &HashMap<String, String>,
    working_dir: &str,
) -> Result<CommandTemplate> {
    let shell_kind = ShellKind::new(shell, is_windows);
    let quoted_dir = shell_kind
        .try_quote(working_dir)
        .context("shell quoting working directory")?;
    let quoted_program = shell_kind
        .try_quote_prefix_aware(&program)
        .context("shell quoting program")?;

    let mut command = quoted_program.into_owned();
    for arg in &args {
        let quoted_arg = shell_kind
            .try_quote(arg)
            .context("shell quoting argument")?;
        command.push(' ');
        command.push_str(&quoted_arg);
    }

    let script = if is_windows {
        format!("cd /d {quoted_dir} && {command}")
    } else {
        format!("cd {quoted_dir} && exec {command}")
    };

    let shell_args = if is_windows {
        vec!["/C".to_string(), script]
    } else {
        vec!["-c".to_string(), script]
    };

    Ok(CommandTemplate {
        program: shell.to_string(),
        args: shell_args,
        env: env.clone(),
    })
}

fn find_remote_server_binary() -> Result<PathBuf> {
    if let Some(from_environment) = std::env::var_os("ZED_REMOTE_SERVER") {
        let path = PathBuf::from(from_environment);
        if path.is_file() {
            return Ok(path);
        }
        log::warn!(
            "ZED_REMOTE_SERVER is set but is not a file: {}",
            path.display()
        );
    }

    let binary_name = remote_server_file_name();

    match std::env::current_exe() {
        Ok(current_exe) => {
            if let Some(directory) = current_exe.parent() {
                let sibling = directory.join(&binary_name);
                if sibling.is_file() {
                    return Ok(sibling);
                }
            }
        }
        Err(error) => {
            log::warn!(
                "failed to resolve current executable while locating remote_server: {error:#}"
            );
        }
    }

    if let Some(repo_root) = util::dev_repo_root() {
        let platform = current_remote_platform()?;
        for triple in local_target_triples(platform) {
            let candidate = repo_root
                .join("target")
                .join("remote_server")
                .join(&triple)
                .join("debug")
                .join(&binary_name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    anyhow::bail!(
        "could not find the remote_server binary; set ZED_REMOTE_SERVER or place remote_server next to the Zed executable"
    )
}

fn remote_server_file_name() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from("remote_server.exe")
    } else {
        PathBuf::from("remote_server")
    }
}

fn current_remote_platform() -> Result<RemotePlatform> {
    let os = match std::env::consts::OS {
        "linux" => RemoteOs::Linux,
        "macos" => RemoteOs::MacOs,
        "windows" => RemoteOs::Windows,
        other => anyhow::bail!("unsupported local OS for remote_server: {other}"),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => RemoteArch::X86_64,
        "aarch64" => RemoteArch::Aarch64,
        other => anyhow::bail!("unsupported local architecture for remote_server: {other}"),
    };
    Ok(RemotePlatform { os, arch })
}

fn local_target_triples(platform: RemotePlatform) -> Vec<String> {
    let arch = platform.arch.to_string();
    match platform.os {
        RemoteOs::Linux => vec![
            format!("{arch}-unknown-linux-gnu"),
            format!("{arch}-unknown-linux-musl"),
        ],
        RemoteOs::MacOs => vec![format!("{arch}-apple-darwin")],
        RemoteOs::Windows if cfg!(windows) => vec![format!("{arch}-pc-windows-msvc")],
        RemoteOs::Windows => vec![format!("{arch}-pc-windows-gnu")],
    }
}

fn local_os_version() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let content = std::fs::read_to_string("/etc/os-release").ok()?;
        return util::parse_os_release(&content);
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn copy_directory_recursive(source: &Path, destination: &Path) -> Result<()> {
    if !source.is_dir() {
        anyhow::bail!(
            "upload_directory source is not a directory: {}",
            source.display()
        );
    }
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let to = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_directory_recursive(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to).with_context(|| {
                format!(
                    "failed to copy {} -> {}",
                    entry.path().display(),
                    to.display()
                )
            })?;
        }
    }
    Ok(())
}

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

    #[test]
    fn nickname_is_not_part_of_daemon_id() {
        let mut project = LocalProject::new(PathBuf::from("/tmp/app"));
        project.nickname = Some("work".to_string());
        assert_eq!(
            project.daemon_id(),
            session_protocol::daemon_socket_id("local", "localhost", "/tmp/app")
        );
    }

    #[test]
    fn into_remote_options_is_local() {
        let options = LocalProject::new(PathBuf::from("/tmp/app")).into_remote_options();
        assert!(matches!(options, RemoteConnectionOptions::Local(_)));
        assert_eq!(options.connection_type(), "local");
        assert_eq!(options.host(), "localhost");
    }
}
