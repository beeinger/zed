# Merging official Zed

Keep `zed-industries/zed` as git remote `upstream` and this fork as `origin`
(`beeinger/peekado`).

## Procedure

```sh
overlay/scripts/merge-upstream.sh
```

The script:

1. Requires a clean worktree.
2. Fetches `upstream/main`.
3. Merges it into the current branch.
4. Runs `overlay/scripts/check-touchpoints.sh`.

If the merge stops on conflicts, resolve the files below first, then re-run
the touchpoint check.

## Expected conflict sites

These are the recurring, acceptable conflicts. Re-apply by locating `FORK:`
markers; if a marker disappeared, stop and restore it from `TOUCHPOINTS.md`.

| File | Why |
| --- | --- |
| Root `Cargo.toml` | Overlay workspace members and `workspace.dependencies` |
| `crates/proto/proto/zed.proto` | Envelope oneof field numbers |
| `crates/proto/proto/ai.proto` | Session agent messages |
| `crates/proto/src/proto.rs` | `messages!` / `request_messages!` |
| `crates/rpc/src/rpc.rs` | `PROTOCOL_VERSION` |
| `crates/remote/src/remote_client.rs` | Connection enum, heartbeats, identifier |
| `crates/remote/src/protocol.rs` | zstd on the remote stream |
| `crates/remote_server/src/server.rs` | Attach-not-kill, no idle quit |
| `crates/remote_server/src/headless_project.rs` | `session_host::init` hook, ignore UI shutdown |
| `crates/project/src/project.rs` | No `ShutdownRemoteServer` on GUI quit |
| `crates/workspace/src/workspace.rs` | Stable daemon identifier, local same-host |
| `crates/workspace/src/persistence.rs` | Persist `RemoteConnectionKind::Local` |
| `crates/remote/src/transport.rs` | `pub mod local`, pub stdio helper |
| `crates/zed/src/main.rs` | `session_transport::init`, `session_client::init`, open-via-daemon |
| `crates/agent_ui/src/agent_ui.rs` | Remote native-agent connection |
| `crates/language_model/src/api_key.rs` | Forward GUI API keys to the daemon |
| `crates/project/src/buffer_store.rs` | Retain dirty buffers after GUI close |
| `crates/remote_server/src/server.rs` | Attach-not-kill, no idle quit, `serve` |
| `crates/remote_server/Cargo.toml` | `session_host` / `agent` prod deps |
| `crates/zed/Cargo.toml` | `session_client` / `session_transport` |

## After a merge

1. `overlay/scripts/check-touchpoints.sh`
2. `cargo test -p session_protocol -p session_host -p session_client -p session_transport`
3. Existing `remote_server` tests (`crates/remote_server/src/remote_editing_tests.rs`)
4. Overlay reconnect tests: event log catch-up after a gap (`session_protocol`)
5. Do **not** silently absorb upstream refactors into overlay crates. If they
   moved a hook, update `TOUCHPOINTS.md` only.

AI-assisted upgrades stay easy because the semantic diff of this fork is:
daemon lifetime, local transport, agent host, three proto messages, one
`AgentConnection` impl.
