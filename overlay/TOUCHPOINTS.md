# Upstream touchpoints

Every `FORK:<id>` marker in this repository must appear here. The check script
fails if a marker is missing from the named file, or if a `FORK:` hunk exists
in the tree that is not listed.

Convention: `APPLIED <id> <path>`

`FORK:end` closes a hunk and is not listed.

## Applied

APPLIED overlay-crates Cargo.toml

## Planned (not yet in tree)

These ids will be added in later commits. They are documented so merges and
reviews share one vocabulary.

| id | file | function / site | intent |
| --- | --- | --- | --- |
| daemon-attach | `crates/remote_server/src/server.rs` | `execute_proxy` | Attach to a live daemon; do not kill |
| no-idle-quit | `crates/remote_server/src/server.rs` | `start_server` | Do not quit after 10 minutes idle |
| ignore-ui-shutdown | `crates/remote_server/src/headless_project.rs` | `handle_shutdown_remote_server` | GUI quit is not daemon quit |
| no-shutdown-on-quit | `crates/project/src/project.rs` | `on_app_quit` / `release` | Do not send `ShutdownRemoteServer` |
| stable-id | `crates/remote/src/remote_client.rs` | `ConnectionIdentifier` | Hash of transport+host+root, not window id |
| stable-id-open | `crates/workspace/src/workspace.rs` | `open_remote_project_with_new_connection` | Use `ConnectionIdentifier::stable` |
| loose-heartbeat | `crates/remote/src/remote_client.rs` | heartbeat constants | Survive high-RTT SSH |
| local-transport | `crates/remote/src/remote_client.rs` | `RemoteConnectionOptions` + `ConnectionPool` | Local unix `RemoteConnection` |
| session-host-init | `crates/remote_server/src/headless_project.rs` | `HeadlessProject::new` | Native agent + LLM on the daemon |
| acp-envelope | `crates/proto/proto/{ai,zed}.proto` | Envelope fields ≥ 2000 | Tunnel ACP; do not grow a full agent API |
| remote-native-agent | `crates/agent_ui/src/agent_ui.rs` | `Agent::server` | `RemoteAgentConnection` when via remote server |
| detach-external-acp | `crates/agent_servers/src/acp.rs` | `AcpConnection::stdio` | Spawn ACP children on the daemon |
| server-buffer-authority | `crates/project/src/buffer_store.rs` | `handle_close_buffer` | Client close does not drop agent-held buffers |
| zstd-remote | `crates/remote/src/protocol.rs` | `read_message` / `write_message` | Compress remote Envelope stream |
