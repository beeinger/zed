# Always-on session host

This directory is the product fork. Official Zed already splits the **IDE**
across a GUI process and `remote_server` (LSP, worktrees, git, tasks). It does
**not** yet run the agent loop or LLM HTTP on that daemon, and a disconnect
still tears the session down.

The mission:

- The daemon is the brain. Native agent turns, LLM requests, file I/O, LSP,
  git, and agent terminals live there whether or not a GUI is connected.
- The GUI is a window. It may connect, disconnect, or die at any time. That
  must be a no-op for in-flight work.
- Reconnect is subscribe + catch-up, not a new session.
- Local and remote use the same architecture (unix socket vs SSH).
- Upstream merges stay mechanical: overlay crates plus tiny `FORK:` hooks.

## Crates

| Crate | Role |
| --- | --- |
| `session_protocol` | Daemon ids, event sequence, disconnected permission policy, ACP-in-Envelope codec |
| `session_host` | Native agent + LLM + event log inside `HeadlessProject` |
| `session_client` | `RemoteAgentConnection` and the production “open as daemon” helper |
| `session_transport` | Local unix-socket `RemoteConnection` (no SSH) |

Do **not** put fork logic in new files under `crates/editor`, `crates/workspace`,
or `crates/agent_ui`. Those churn every upstream week. Touch them only with a
marked one-line hook.

## Rules

- Closing the GUI, dropping SSH, or sleeping the laptop does not stop the
  daemon, an agent turn, LLM HTTP, or agent-owned terminals.
- Tests keep in-process `Project::local` (`FakeFs` + gpui). Production GUI
  talks to a daemon.
- GPUI stays on the server. `Entity` / `App` / `Task` are the runtime, not
  just widgets.
- Do not grow `zed.proto` with a full agent API. Tunnel ACP in a handful of
  Envelope messages at field numbers ≥ 2000.
- `collab` (zed.dev multiplayer) and `cli` (launcher IPC) stay out of this
  fork’s core.

## Markers

Every upstream hunk is wrapped:

```rust
// FORK:some-id
...
// FORK:end
```

`TOUCHPOINTS.md` lists each applied marker. `scripts/check-touchpoints.sh`
fails if a marker vanished. `scripts/merge-upstream.sh` fetches
`zed-industries/zed` and merges it.
