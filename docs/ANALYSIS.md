# Phase 0 — Analysis of the existing Hermes Desktop and backend

Machine: Omarchy (Arch, Linux 7.2, Wayland/Hyprland). Hermes source checkout at
`~/.hermes/hermes-agent` (commit `4957bbd0a0f` at time of writing). All facts below
were verified on this machine on 2026-10-15 unless marked *(from source)*.

## 1. Correction to the brief: the backend is Python, not Rust

The brief says the backend is "written in Rust". It is not: the Hermes backend is
Python (`hermes_cli/`, `tui_gateway/`, `agent/`, `gateway/`, FastAPI/Starlette under
`hermes_cli/web_server*.py`, venv under `~/.hermes/hermes-agent/venv`, Python 3.11).
That does not change the plan: the rewrite talks to it **over its existing wire
protocol**, so the backend language does not matter, and no backend change is
needed or made.

## 2. Components on this machine

| Piece | Location | Notes |
|---|---|---|
| CLI launcher | `~/.local/bin/hermes` → `~/.hermes/hermes-agent/.hermes/bin/hermes` | shell shim |
| Electron desktop | `~/.hermes/hermes-agent/apps/desktop` (React 19 + Vite renderer, Electron main) | ~3.6k TS/TSX files |
| Packaged launcher | `/usr/bin/hermes-desktop` (bash; runs `apps/desktop/release/linux-unpacked/Hermes` or `/opt/hermes-desktop/Hermes`) | user's notes say it is currently broken; dev launcher is `hermes desktop --skip-build` |
| Shared protocol | `apps/shared/src/` — `json-rpc-gateway.ts`, `json-rpc-channel.ts`, **`gateway-contract.openrpc.json`** (generated, authoritative) | |
| Ink TUI | `ui-tui/` (Node) talking to `python -m tui_gateway.entry` over stdio | same JSON-RPC surface |
| Messaging gateway | separate process (`gateway.pid`, `gateway.sock`) — Telegram etc. | managed through RPC/REST, not dialled directly by the desktop |

## 3. How the desktop talks to the backend

### 3.1 Process model
The Electron main process spawns its own headless backend *(from source:
`electron/backend-command.ts`)*:

```
hermes [--profile <slug>] serve --host 127.0.0.1 --port 0
```

(legacy fallback: `dashboard --no-open`). Readiness is a stdout sentinel
`HERMES_BACKEND_READY port=<n>` (also `HERMES_DASHBOARD_READY port=<n>`), matched on a
token boundary because output can be spliced. **Verified:** spawning `hermes serve`
printed `HERMES_BACKEND_READY port=45125` in ≈3 s.

### 3.2 Auth
Loopback mode: the server injects `window.__HERMES_SESSION_TOKEN__ = "<token>"` into
the HTML served at `/`. The client reads it and dials `/api/ws?token=<token>`
(constant-time compared server-side, `hermes_cli/web_server_chat.py::_ws_auth_reason`).
Gated/remote mode adds `?ticket=` (single-use, 30 s TTL, minted via OAuth) or
`?internal=`; close codes `4401` bad auth, `4403` disabled/not allowed. Remote
connections additionally use bearer tokens for REST. The rewrite implements loopback
token auth now; remote/ticket auth is tracked in PARITY.md.

### 3.3 Transports
| Transport | Path | Used for |
|---|---|---|
| **WebSocket JSON-RPC** | `ws://127.0.0.1:<port>/api/ws` | everything chat/session/tools/config: 259 methods, 77 event types, server→client requests |
| REST (FastAPI) | `/api/status`, `/api/config`, `/api/profiles`, `/api/skills`, `/api/cron`, `/api/providers/*`, `/api/messaging/*`, … (`apps/desktop/src/api/*.ts`) | settings pages, catalogs, OAuth flows |
| PTY WebSocket | `/api/pty` | embedded terminal panel |
| Event fan-out | `/api/pub`, `/api/events` | TUI sidecar feeds (not needed by desktop chat) |

### 3.4 Wire format (verified with a live session)
* One JSON object per text frame; newline-delimited batches possible (the server
  coalesces token deltas on a short timer — `tui_gateway/ws.py`).
* First frame after accept: `{"jsonrpc":"2.0","method":"event","params":{"type":"gateway.ready","payload":{"skin":{…},"change_events":true,"replay_epoch":"…"}}}`.
* Client requests: `{"jsonrpc":"2.0","id":<int>,"method":"session.create","params":{…}}`.
* Responses: `{"jsonrpc":"2.0","id":<int>,"result":{…}}` or `"error":{"code":4007,"message":"session not found"}`.
* Notifications: `method:"event"`, `params:{type, session_id?, seq?, payload}`; `seq` is a
  per-session monotonic counter used for lossless reconnect via `session.events.since`.
* Server→client requests: string `id`, `method` ∈ {`approval`, `clarify`, `secret`,
  `display.install.sudo`, `preview.act`, `preview.read`, …}; the client must answer.
* Heartbeat: client sends `gateway.ping` every 15 s; reply `{"ok":true}`; any inbound
  frame counts as liveness; 45 s deadline.
* Timeouts used by the Electron client: default 30 s, `prompt.submit` 1800 s,
  `approval.respond` 300 s.

### 3.5 Streaming — observed event order for one turn
`session.create` → `prompt.submit` (ACK `{"status":"streaming","user_row_id":…}`) then:

```
sessions.changed, session.info, message.start, session.title, thinking.delta,
reasoning.delta ×N, message.delta ×N, reasoning.available, message.complete
```

* `message.delta {text}` — answer tokens.
* `reasoning.delta {text}` — model reasoning (Thinking block).
* `thinking.delta {text}` — spinner/status chatter (kaomoji, provider waits); **not**
  transcript content.
* `tool.generating {name}` → `tool.start {tool_id,name,context,args,preview}` →
  `tool.complete {tool_id,summary,result_text,inline_diff,duration_s,todos}`.
* `message.complete {text,usage,status,reasoning,error,…}` ends the turn; its `text` is
  authoritative.
* `agent.terminal.output {process_id,chunk}` streams background process output.

### 3.6 Config, data and paths
| Path | Content |
|---|---|
| `$HERMES_HOME` (default `~/.hermes`) | root; profiles under `profiles/<name>/` with same layout |
| `config.yaml` | settings (edited via `config.set` RPC / `hermes config set`, never by hand) |
| `.env` | secrets only |
| `auth.json` | OAuth tokens / credential pools |
| `state.db` | SQLite session store (FTS5) — **desktop never reads it directly; it goes through RPC** |
| `skins/`, `desktop-plugins/`, `pets/`, `cron/`, `memories/`, `skills/` | feature data |
| `~/.config/Hermes` *(from source)* | Electron app state (window bounds, localStorage) |

The rewrite stores its own UI prefs in `$XDG_CONFIG_HOME/hermes-desktop-rs/prefs.json`
and never touches backend files directly.

## 4. Proof of the protocol from Rust
`crates/hermes-core/examples/probe.rs` spawns `hermes serve`, reads the token, dials
the WebSocket, creates a session, submits a prompt and reduces the stream with
`hermes_core::transcript`. Real output (2026-10-15):

```
backend ready on :34217 (owned=true) in 2.9459902s
session d35864db model=deepseek/deepseek-v4.1-flash
submit ack: {"status":"streaming","user_row_id":58924}
events: ["gateway.ready","session.info","message.start","session.title","thinking.delta",
         "thinking.delta","message.delta","thinking.delta","reasoning.available","message.complete"]
User: Reply with exactly: pong
Assistant: pong
```

## 5. Existing desktop resource usage (baseline for the rewrite)
Measured with `ps` on this machine while the Electron desktop and TUIs were running.
Electron numbers are recorded in README "Benchmarks" once the rewrite is measured
under identical conditions; the backend (`hermes serve`, Python) is shared by both
and is excluded from the client comparison.

## 6. Feature inventory and design study
* Full feature/settings checklist: [`PARITY.md`](PARITY.md).
* Claude Desktop visual/motion spec: [`DESIGN.md`](DESIGN.md).

## 7. Consequences for the architecture
1. `hermes-core` speaks JSON-RPC/WS + a small REST helper; types are lenient serde
   structs mirroring the OpenRPC contract (unknown fields ignored, so newer backends work).
2. The app owns its backend process exactly like Electron (`hermes serve --port 0`), or
   attaches to an existing one via `HERMES_DESKTOP_PORT` / `HERMES_DESKTOP_TOKEN`.
3. All I/O lives on tokio; the UI receives `ClientEvent`s over a channel and applies
   them through the pure `Transcript` reducer, which reports row-level changes so the
   virtualised list updates in place.
