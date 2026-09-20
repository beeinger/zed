# Upstream touchpoints

Every `FORK:<id>` marker in this repository must appear here. The check script
fails if a marker is missing from the named file, or if a `FORK:` hunk exists
in the tree that is not listed.

Convention: `APPLIED <id> <path>`

`FORK:end` closes a hunk and is not listed.

## Applied

APPLIED overlay-crates Cargo.toml
APPLIED daemon-attach crates/remote_server/src/server.rs
APPLIED no-idle-quit crates/remote_server/src/server.rs
APPLIED ignore-ui-shutdown crates/remote_server/src/headless_project.rs
APPLIED no-shutdown-on-quit crates/project/src/project.rs
APPLIED stable-id crates/remote/src/remote_client.rs
APPLIED stable-id crates/remote/Cargo.toml
APPLIED stable-id-open crates/workspace/src/workspace.rs
APPLIED loose-heartbeat crates/remote/src/remote_client.rs
APPLIED local-transport crates/remote/src/transport/local.rs
APPLIED local-transport crates/remote/src/transport.rs
APPLIED local-transport crates/remote/src/remote.rs
APPLIED local-transport crates/remote/src/remote_client.rs
APPLIED local-transport crates/remote/src/remote_identity.rs
APPLIED local-transport crates/title_bar/src/title_bar.rs
APPLIED local-transport crates/recent_projects/src/recent_projects.rs
APPLIED local-transport crates/recent_projects/src/remote_connections.rs
APPLIED local-transport crates/recent_projects/src/remote_servers.rs
APPLIED local-transport crates/remote_connection/src/remote_connection.rs
APPLIED local-transport crates/project/src/trusted_worktrees.rs
APPLIED local-transport crates/workspace/src/workspace.rs
APPLIED local-transport crates/workspace/src/persistence.rs
APPLIED local-transport crates/workspace/src/persistence/model.rs
APPLIED local-transport crates/sidebar/src/sidebar.rs
APPLIED local-transport crates/zed/src/main.rs
APPLIED local-transport crates/zed/Cargo.toml
APPLIED acp-envelope crates/proto/proto/ai.proto
APPLIED acp-envelope crates/proto/proto/zed.proto
APPLIED acp-envelope crates/proto/src/proto.rs
APPLIED session-host-init crates/project/src/project.rs
APPLIED session-host-init crates/remote_server/src/headless_project.rs
APPLIED session-host-init crates/remote_server/Cargo.toml
APPLIED session-host-init crates/gpui_tokio/src/gpui_tokio.rs
APPLIED session-update-sink crates/agent/src/agent.rs
APPLIED detached-prompts crates/settings_content/src/agent.rs
APPLIED detached-prompts assets/settings/default.json
APPLIED detached-prompt-advice crates/zed/src/zed.rs
APPLIED detached-prompt-advice crates/zed/Cargo.toml
APPLIED detach-external-acp crates/agent_servers/src/acp.rs
APPLIED detach-external-acp crates/agent_servers/Cargo.toml

## Planned (not yet in tree)

These ids will be added in later commits. They are documented so merges and
reviews share one vocabulary.

| id | file | function / site | intent |
| --- | --- | --- | --- |
| remote-native-agent | `crates/agent_ui/src/agent_ui.rs` | `Agent::server` | `RemoteAgentConnection` when via remote server |
| server-buffer-authority | `crates/project/src/buffer_store.rs` | `handle_close_buffer` | Client close does not drop agent-held buffers |
| zstd-remote | `crates/remote/src/protocol.rs` | `read_message` / `write_message` | Compress remote Envelope stream |

