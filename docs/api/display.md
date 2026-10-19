# Display API (`api::display`)

Typed client for the Bot Screen — a profile's headless Xvnc + Xfce desktop that the agent
drives and a human can take over. UI-free; the controller maps these into the Screen pane's
hero thumbnail, viewer, takeover buttons and install card.

Sources of truth: `tui_gateway/methods_display.py`, `tui_gateway/methods_display_watch.py`,
`tui_gateway/contracts/display.py`, `hermes_cli/web_routers/display.py`,
`tools/bot_desktop/{runtime,lease,thumbnail,install}.py`, and the Electron calls in
`apps/desktop/src/plugins/hermes-bots/screen-{connection,pane,hero,install}*.ts*`.

All methods are `@profile_scoped`: the builders take `profile: &str` and omit the param when it
is empty, which answers for the gateway's launch home. A pane watching another bot must pass
its profile name, or every bot shows the same screen.

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Runtime + lease snapshot | `display_status_call` / `display_status` | RPC `display.status` `{profile?}` | Read-only. `DisplayStatus`: `supported`, `installed`, `missing[]`, `running`, `pid`, `display` (`":20"`), `socket`, `geometry`, `install_command`, `browser`, `blocker` (why start would refuse), `memory_available_mb` / `memory_limit_mb`, `placement` (`gateway` \| `terminal:<backend>`), `image_switch`, `lease`, `profile_key`. Also installs the gateway's lease listener, which is what makes `display.lease` events arrive. |
| Thumbnail | `display_thumbnail_call` / `display_thumbnail` | RPC `display.thumbnail` `{profile?}` | Read-only, never changes the lease. `data_url` = `data:image/jpeg;base64,…`; `null` while stopped, and while a human holds (`suppressed: "human_has_control"`, also when a takeover races the framebuffer read). |
| Start | `display_start_call` / `display_start` | RPC `display.start` `{profile?}` | **Spawns processes** (Xvnc + Xfce, or a container for a sandbox placement). Idempotent; blocks until published — `DISPLAY_START_TIMEOUT` (180 s) instead of the 30 s default. |
| Stop | `display_stop_call` / `display_stop` | RPC `display.stop` `{profile?, force?}` | **Destructive**: kills the screen under whoever is on it. Refused 5300 / `viewer_mismatch` while a human holds unless `force: true`. Reply adds `stopped`. |
| Observe / viewer ticket | `display_observe_call` / `display_observe` | RPC `display.observe` `{profile?, viewer_id?}` | Refused 5300 while nothing runs. Reply `{ticket, path, viewer_id}` + status. Ticket is single-use, 30 s, provider `bot-desktop`, bound to that profile home. `viewer_id` is server-minted per connection; repeating your own id keeps identity + lease across a reconnect. |
| Lease: take over | `display_lease_acquire_call` / `display_lease_acquire` | RPC `display.lease.acquire` `{profile?, viewer_id, reason?}` | **Changes who may drive the screen** (last writer wins; the previous viewer's socket is closed 4000). `viewer_id` must have been minted by this connection, else 5300 / `viewer_mismatch`. Returns `DisplayLease`. |
| Lease: hand back | `display_lease_release_call` / `display_lease_release` | RPC `display.lease.release` `{profile?, viewer_id?, force?}` | With a `viewer_id` only that holder may release; without one it is refused while a human holds unless `force: true` (5300 / `viewer_mismatch`). Returns the lease as it stands. |
| Install desktop stack | `display_install_call` / `display_install` | RPC `display.install` `{profile?}` | **Builder-only**: runs `apt-get`/`dnf`/`pacman` as root on the gateway host. The sudo password is asked through the `display.install.sudo` server→client request (app-level, empty session; answered by the terminal sudo bridge, not this module). Reply `{started, command, profile_key}`; progress is evented. Refused while one runs, on non-Linux hosts, and when the screen lives inside a terminal sandbox. |
| Sandbox image switch | `display_switch_sandbox_image_call` / `display_switch_sandbox_image` | RPC `display.switchSandboxImage` `{profile?, approve}` | **Writes persisted config** (`terminal.docker_image`); `approve: true` pins the new image and the next container use recreates the persisted containers from it. Refused when nothing is pending (`DisplayStatus.image_switch` is the offer). Reply adds `docker_image`. |

`DisplayLease`: `holder` (`agent` \| `human`), `viewer_id` (always `null` on the wire — the id
is a capability), `viewer_hash` (12 hex chars of its SHA-256, so the holder recognises itself),
`since`, `epoch` (bumped on every real transition; unchanged for a no-op), `reason`.
`DisplayLease::human_holds()` is the only comparison that matters.

Every failure here is JSON-RPC **5300**. The gateway's `error.data.code` (`viewer_mismatch`, …)
is not carried by `RpcError` (only `code` + `message`), so the pane must re-observe on any 5300
from a lease call rather than matching the data code.

## Notifications

| Event | Parser | Payload |
|---|---|---|
| `DISPLAY_STATUS_EVENT` (`display.status`) | `parse_display_status` | `DisplayStatus` — screen started/stopped, including transitions made outside `hermes serve` (CLI, gateway auto-start, Xvnc crash, idle auto-stop). |
| `DISPLAY_LEASE_EVENT` (`display.lease`) | `parse_display_lease` | `{profile_key, lease}` — every takeover / hand-back, from any process (a 0.5 s file watcher covers other processes). |
| `DISPLAY_INSTALL_LOG_EVENT` (`display.install.log`) | `parse_display_install_log` | `{profile_key, line}`. |
| `DISPLAY_INSTALL_DONE_EVENT` (`display.install.done`) | `parse_display_install_done` | `{profile_key, code, status}` — `code` 0 ok, -1 cancelled, -2 no sudo (the command to run by hand was streamed). |

All four are **global** broadcasts: filter on `profile_key` *and* on the gateway connection the
event arrived on (two hosts can share a `~/.hermes` path). Keep the socket open across an
install or the events are lost.

## Streaming socket `WS /api/display/ws` (documented, not implemented)

Raw RFB, one socket per viewer. URL `ws(s)://<gateway>/api/display/ws?display_ticket=<ticket>`
(ticket from `display_observe`; single-use, 30 s, query string because noVNC's Websock cannot
set headers). Binary frames only, both ways: server→client is the Xvnc byte stream spliced from
`<profile home>/bot-desktop/rfb.sock` or from a `docker exec`/`ssh` relay for a sandbox screen;
client→server bytes pass `RfbClientFilter`, so input reaches Xvnc only from the human currently
holding the lease — watchers always get the picture, never the keyboard. A text frame closes
1003. Close codes: `4000` control-taken (evicted by another human), `4001` not running, `4401`
ticket missing/expired/used (re-observe), `4403` host/origin policy before accept, `1000`/`1001`
viewer closed (hands control back if it still held it); a dropped link keeps the human's
exclusion.

Constants: `DISPLAY_WS_PATH`, `DISPLAY_WS_TICKET_PARAM`,
`DISPLAY_WS_CLOSE_{CONTROL_TAKEN,DESKTOP_GONE,BAD_TICKET,NOT_ALLOWED,PROTOCOL}`,
`DISPLAY_START_TIMEOUT`.

## Run live (private test backend)

`crates/hermes-core/tests/live_api_display.rs` — 10 checks against `HERMES_DESKTOP_PORT=40361`
(`scripts/live-test-backend.sh`): `display.status` (decodes; `profile_key` non-empty;
`placement` valid; `holder` ∈ {agent, human}; `viewer_id` redacted; `running` ⇒ `display`),
`display.thumbnail` (decodes; `data_url` null while stopped), `display.observe` (refused
5300 / "not running" while stopped). Builder-only: `start`, `stop`, `install`,
`switchSandboxImage`, `lease.acquire`, `lease.release` — see the test's header for why.
