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
- Permission and elicitation prompts are asked of the GUI. A missing window
  waits forever (default) or until `agent.disconnected_prompt_wait` times out:
  permission timeout denies that tool once (it does **not** `session/cancel`);
  elicitation timeout continues without the form. Unattended remote work should
  set `agent.detached_permissions` to `"permit_everything"` or a thorough
  `agent.tool_permissions` allow-list.
- External ACP children (Gemini, Claude, …) are spawned on the daemon. The GUI
  only tunnels ACP. Closing the window does not SIGHUP the child; reconnect
  uses `session/load` or `session/resume`.
- Native Zed Agent on a remote (or local-daemon) project uses
  `RemoteAgentConnection`. The GUI never constructs in-process `NativeAgent`
  for those projects.
- Production local open uses the unix-socket daemon for folders **and**
  file-only opens (parent directory is the daemon id). Empty windows and
  unsaved restore share `EMPTY_LOCAL_DAEMON_ROOT` so they reconnect to one
  host instead of `Project::local`. Opening a folder from that empty window
  (or cloning a repo into it) connects to **that folder's** daemon rather
  than adding a worktree to the empty host. Tests keep `Project::local`.
- The production GUI never constructs `NativeAgent` or stdio-spawns ACP
  (`session_client::init` marks the process as a window). Tests and
  `remote_server` still own those objects.
- External ACP spawn is resolved on the host `AgentServerStore` by agent
  id. The GUI does not supply a command path.
- Transport reconnect (`RemoteClientEvent::Reconnected`) is
  `SessionSubscribe` + catch-up on every live GUI thread, plus a session
  list refresh. Closing the GUI or dropping SSH does not cancel a run.
- Several GUIs can attach to one daemon at once (Envelope ids remapped).
  SSH/`run` is `setsid` + SIGHUP ignored so dropping the proxy does not
  kill the host. `serve` + systemd/launchd remains the service entry.
- The threads sidebar lists daemon sessions (native and ACP) with live
  generating status, including threads that are not open in this GUI.
  Switching workspace does not cancel daemon turns.
- Dirty buffers stay on the daemon after the GUI closes its replica, and are
  snapshotted under the server state dir.
- The daemon event log is snapshotted to jsonl so a process bounce can still
  catch a reconnecting GUI up (GUI close itself does not need disk: the live
  daemon already keeps the RAM log).
- The remote Envelope stream is zstd-compressed (legacy uncompressed frames
  still decode). Agent token deltas coalesce (~50ms / 64 chars). Heartbeats
  adapt to RTT and honor `SessionHeartbeat` tunables.
- LLM API keys pasted in the GUI are forwarded to the daemon credential file.
- Zed cloud tokens on the laptop are copied into that same file when a remote
  agent hub is created, so daemon LLM HTTP can authenticate.
- `remote_server serve --identifier` is the systemd/launchd-friendly entry
  (same sockets as `run`). See `overlay/systemd` and `overlay/launchd`.
- User-facing identity is **Peekado** so this fork can sit next to official
  Zed. Crate names stay `zed`. See `overlay/BRANDING.md`.

## Crates

| Crate | Role |
| --- | --- |
| `session_protocol` | Daemon ids, event sequence, disconnected prompt policy, ACP-in-Envelope codec |
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
