# Peekado identity (keep this small)

Official Zed and Peekado must run on the same machine. Crate names, proto
fields, `zed://schemas/`, ACP/MCP client ids, and project-local `.zed/` stay
upstream so merges remain mechanical.

Change **only** the surfaces below. They live behind `FORK:branding` plus
`paths::APP_NAME` (the fork point Zed already documented).

## What differs from Zed

| Surface | Zed | Peekado |
| --- | --- | --- |
| App / Dock / menu | Zed | Peekado |
| macOS bundle id | `dev.zed.Zed*` | `dev.peekado.Peekado*` |
| GUI binary | `zed` | `peekado` |
| CLI | `/usr/local/bin/zed` | `/usr/local/bin/peekado` |
| Config | `~/.config/zed` | `~/.config/peekado` |
| Data (macOS) | `~/Library/Application Support/Zed` | `~/Library/Application Support/Peekado` |
| Logs (macOS) | `~/Library/Logs/Zed/Zed.log` | `~/Library/Logs/Peekado/Peekado.log` |
| OS URL scheme | `zed://` | `peekado://` (still parses `zed://`) |
| CLI IPC scheme | `zed-cli://` | `peekado-cli://` |
| SSH server dir | `~/.zed_server` | `~/.peekado_server` |
| Remote binary prefix | `zed-remote-server-*` | `peekado-remote-server-*` |
| Linux IPC socket | `$XDG_DATA_HOME/zed/zed-*.sock` | `$XDG_DATA_HOME/peekado/peekado-*.sock` |

Uninstall (`peekado --uninstall`) only removes Peekado paths. It never
touches `Zed.app` or `~/Library/Application Support/Zed`.

Auto-update stays off on the `dev` channel, so Peekado will not overwrite
itself with an official Zed build from zed.dev.

## Debug logs for testers

Dev-channel Peekado always:

- Writes `~/Library/Logs/Peekado/Peekado.log` (and `.log.old`)
- Enables `debug` for overlay/daemon crates (`session_*`, `remote`, `agent*`)
- Sets `RUST_BACKTRACE=1` unless you already exported it

Override with `ZED_LOG` / `RUST_LOG` (same env names as Zed, so we do not
fork the logger crate). Example: `ZED_LOG=trace`.

When filing a bug, send:

1. `Peekado.log` and `Peekado.log.old`
2. About Peekado (version + commit)
3. What you did, including local vs SSH and whether the GUI was closed

## macOS install (unsigned tester build)

1. Copy `Peekado Dev.app` into `/Applications`
2. Right-click → Open the first time (or `xattr -cr "/Applications/Peekado Dev.app"`)
3. Optional CLI: Peekado Dev → Install CLI (`peekado` on PATH)
4. Official Zed can stay installed. They do not share config, sockets, or
   the `zed://` handler.

Build on a Mac: `overlay/scripts/bundle-macos.sh`. CI:
`.github/workflows/peekado-macos.yml` (`workflow_dispatch`).
