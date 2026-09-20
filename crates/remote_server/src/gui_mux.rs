//! Concurrent GUI attach onto one HeadlessProject proto client.
//!
//! FORK:multi-gui
//! Unix sockets are still the stdin/stdout/stderr triple the proxy expects.
//! Connections are paired by peer pid so two proxies cannot mix streams.
//! Envelope ids are remapped (`session_protocol::GuiIdMap`) so request ids
//! from two GUIs do not collide on the shared `ChannelClient`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures::{
    AsyncWriteExt as _,
    FutureExt as _,
    StreamExt as _,
    channel::mpsc,
    select,
};
use gpui::{App, AppContext as _};
use net::async_net::UnixStream;
use remote::protocol::{read_message, write_message};
use rpc::proto::{self, Envelope, EnvelopedMessage};
use session_protocol::{EnvelopeIds, GuiIdMap, OutgoingRoute};
use smol::channel::Receiver;

use crate::ServerListeners;

#[derive(Clone)]
pub(crate) struct GuiMux {
    incoming_tx: mpsc::UnboundedSender<Envelope>,
    inner: Arc<Mutex<GuiMuxInner>>,
    stderr_txs: Arc<Mutex<Vec<mpsc::UnboundedSender<Vec<u8>>>>>,
}

struct GuiMuxInner {
    ids: GuiIdMap,
    stdout_txs: HashMap<u64, mpsc::UnboundedSender<Envelope>>,
    pending_out: Vec<Envelope>,
    next_conn: u64,
}

impl GuiMux {
    fn new(incoming_tx: mpsc::UnboundedSender<Envelope>) -> Self {
        Self {
            incoming_tx,
            inner: Arc::new(Mutex::new(GuiMuxInner {
                ids: GuiIdMap::new(),
                stdout_txs: HashMap::new(),
                pending_out: Vec::new(),
                next_conn: 0,
            })),
            stderr_txs: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GuiMuxInner> {
        self.inner.lock().unwrap_or_else(|poison| poison.into_inner())
    }
}

#[derive(Default)]
struct PartialTriple {
    stdin: Option<UnixStream>,
    stdout: Option<UnixStream>,
    stderr: Option<UnixStream>,
}

enum StreamKind {
    Stdin,
    Stdout,
    Stderr,
}

impl PartialTriple {
    fn insert(
        &mut self,
        kind: StreamKind,
        stream: UnixStream,
    ) -> Option<(UnixStream, UnixStream, UnixStream)> {
        match kind {
            StreamKind::Stdin => self.stdin = Some(stream),
            StreamKind::Stdout => self.stdout = Some(stream),
            StreamKind::Stderr => self.stderr = Some(stream),
        }
        match (
            self.stdin.take(),
            self.stdout.take(),
            self.stderr.take(),
        ) {
            (Some(stdin), Some(stdout), Some(stderr)) => Some((stdin, stdout, stderr)),
            (stdin, stdout, stderr) => {
                self.stdin = stdin;
                self.stdout = stdout;
                self.stderr = stderr;
                None
            }
        }
    }
}

pub(crate) fn run(
    listeners: ServerListeners,
    log_rx: Receiver<Vec<u8>>,
    incoming_tx: mpsc::UnboundedSender<Envelope>,
    outgoing_rx: mpsc::UnboundedReceiver<Envelope>,
    mut app_quit_rx: mpsc::UnboundedReceiver<()>,
    cx: &mut App,
) {
    let mux = GuiMux::new(incoming_tx);

    cx.spawn({
        let mux = mux.clone();
        async move |_| route_outgoing(mux, outgoing_rx).await
    })
    .detach();

    cx.spawn({
        let mux = mux.clone();
        async move |_| fanout_logs(mux, log_rx).await
    })
    .detach();

    cx.spawn(async move |cx| {
        let mut by_pid: HashMap<u32, PartialTriple> = HashMap::new();
        let mut unkeyed = PartialTriple::default();

        loop {
            let stdin_accept = listeners.stdin.accept().fuse();
            let stdout_accept = listeners.stdout.accept().fuse();
            let stderr_accept = listeners.stderr.accept().fuse();
            futures::pin_mut!(stdin_accept, stdout_accept, stderr_accept);

            let accepted = select! {
                result = stdin_accept => result.ok().map(|(stream, _)| (StreamKind::Stdin, stream)),
                result = stdout_accept => result.ok().map(|(stream, _)| (StreamKind::Stdout, stream)),
                result = stderr_accept => result.ok().map(|(stream, _)| (StreamKind::Stderr, stream)),
                _ = app_quit_rx.next() => {
                    log::info!("app quit requested");
                    break;
                }
            };

            let Some((kind, stream)) = accepted else {
                log::error!("failed to accept new connections");
                break;
            };

            let pid = unix_peer_pid(&stream);
            let complete = if let Some(pid) = pid {
                by_pid.entry(pid).or_default().insert(kind, stream)
            } else {
                unkeyed.insert(kind, stream)
            };

            if let Some((stdin, stdout, stderr)) = complete {
                if let Some(pid) = pid {
                    by_pid.remove(&pid);
                }
                log::info!("accepted GUI connection (pid={pid:?})");
                attach_gui(mux.clone(), stdin, stdout, stderr, cx);
            }
        }
        anyhow::Ok(())
    })
    .detach();
}

fn attach_gui(
    mux: GuiMux,
    stdin: UnixStream,
    stdout: UnixStream,
    stderr: UnixStream,
    cx: &mut gpui::AsyncApp,
) {
    let (stdout_tx, stdout_rx) = mpsc::unbounded::<Envelope>();
    let (stderr_tx, stderr_rx) = mpsc::unbounded::<Vec<u8>>();
    let conn_id = {
        let mut inner = mux.lock();
        inner.next_conn += 1;
        let conn_id = inner.next_conn;
        let is_first = inner.stdout_txs.is_empty();
        inner.stdout_txs.insert(conn_id, stdout_tx.clone());
        if is_first {
            for envelope in std::mem::take(&mut inner.pending_out) {
                stdout_tx.unbounded_send(envelope).ok();
            }
        } else {
            let started = proto::RemoteStarted {}.into_envelope(0, None, None);
            stdout_tx.unbounded_send(started).ok();
        }
        conn_id
    };
    mux.stderr_txs
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .push(stderr_tx);

    cx.background_spawn({
        let mux = mux.clone();
        async move {
            read_stdin(mux, conn_id, stdin).await;
        }
    })
    .detach();

    cx.background_spawn(async move {
        write_stdout(stdout, stdout_rx).await;
    })
    .detach();

    cx.background_spawn(async move {
        write_stderr(stderr, stderr_rx).await;
    })
    .detach();
}

async fn read_stdin(mux: GuiMux, conn_id: u64, mut stdin: UnixStream) {
    let mut input_buffer = Vec::new();
    loop {
        match read_message(&mut stdin, &mut input_buffer).await {
            Ok(mut message) => {
                message.ack_id = None;
                let remapped = mux.lock().ids.remap_incoming(
                    conn_id,
                    EnvelopeIds {
                        id: message.id,
                        responding_to: message.responding_to,
                    },
                );
                message.id = remapped.id;
                message.responding_to = remapped.responding_to;
                if mux.incoming_tx.unbounded_send(message).is_err() {
                    break;
                }
            }
            Err(error) => {
                log::info!("GUI stdin closed (conn={conn_id}): {error:?}");
                break;
            }
        }
    }
    let mut inner = mux.lock();
    inner.stdout_txs.remove(&conn_id);
    inner.ids.drop_connection(conn_id);
}

async fn write_stdout(
    mut stdout: UnixStream,
    mut stdout_rx: mpsc::UnboundedReceiver<Envelope>,
) {
    let mut output_buffer = Vec::new();
    while let Some(message) = stdout_rx.next().await {
        if write_message(&mut stdout, &mut output_buffer, message)
            .await
            .is_err()
        {
            break;
        }
        if stdout.flush().await.is_err() {
            break;
        }
    }
}

async fn write_stderr(
    mut stderr: UnixStream,
    mut stderr_rx: mpsc::UnboundedReceiver<Vec<u8>>,
) {
    while let Some(message) = stderr_rx.next().await {
        if stderr.write_all(&message).await.is_err() {
            break;
        }
        if stderr.flush().await.is_err() {
            break;
        }
    }
}

async fn route_outgoing(mux: GuiMux, mut outgoing_rx: mpsc::UnboundedReceiver<Envelope>) {
    while let Some(mut message) = outgoing_rx.next().await {
        let mut inner = mux.lock();
        let route = inner.ids.route_outgoing(EnvelopeIds {
            id: message.id,
            responding_to: message.responding_to,
        });
        if inner.stdout_txs.is_empty() {
            inner.pending_out.push(message);
            continue;
        }
        match route {
            OutgoingRoute::Broadcast => {
                for sender in inner.stdout_txs.values() {
                    sender.unbounded_send(message.clone()).ok();
                }
            }
            OutgoingRoute::Unicast {
                conn,
                responding_to,
            } => {
                message.responding_to = Some(responding_to);
                if let Some(sender) = inner.stdout_txs.get(&conn) {
                    sender.unbounded_send(message).ok();
                }
            }
        }
    }
}

async fn fanout_logs(mux: GuiMux, log_rx: Receiver<Vec<u8>>) {
    while let Ok(message) = log_rx.recv().await {
        let senders = mux
            .stderr_txs
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        for sender in senders {
            sender.unbounded_send(message.clone()).ok();
        }
    }
}

#[cfg(unix)]
fn unix_peer_pid(stream: &UnixStream) -> Option<u32> {
    use std::os::fd::AsRawFd;
    let fd = stream.as_raw_fd();
    #[cfg(target_os = "linux")]
    {
        let mut credentials = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let result = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut credentials as *mut _ as *mut libc::c_void,
                &mut length,
            )
        };
        if result == 0 {
            Some(credentials.pid as u32)
        } else {
            None
        }
    }
    #[cfg(target_os = "macos")]
    {
        let mut pid: libc::pid_t = 0;
        let mut length = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
        const SOL_LOCAL: libc::c_int = 0;
        const LOCAL_PEERPID: libc::c_int = 2;
        let result = unsafe {
            libc::getsockopt(
                fd,
                SOL_LOCAL,
                LOCAL_PEERPID,
                &mut pid as *mut _ as *mut libc::c_void,
                &mut length,
            )
        };
        if result == 0 {
            Some(pid as u32)
        } else {
            None
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = fd;
        None
    }
}

#[cfg(not(unix))]
fn unix_peer_pid(_stream: &UnixStream) -> Option<u32> {
    None
}
