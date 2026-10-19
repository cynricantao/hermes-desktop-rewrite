//! Sandbox display (virtual desktop the agent drives): `display.*` RPCs and the
//! `display.*` notifications.
//!
//! A profile's "Bot Screen" is an Xvnc + Xfce session the agent (or a human taking over)
//! drives; the desktop's Screen pane paints a cached [`DisplayStatus`] and streams the
//! picture over its own socket. Sources: `tui_gateway/methods_display.py`,
//! `tui_gateway/methods_display_watch.py`, `tui_gateway/contracts/display.py`,
//! `tools/bot_desktop/{runtime,lease,thumbnail,install}.py`.
//!
//! # Profile scope
//! Every handler is `@profile_scoped`: without an explicit `profile` the gateway answers for
//! its **launch** home, so a pane looking at another bot must name it (the Electron desktop
//! routes through `requestGatewayForProfile`). Builders take `profile: &str` and omit the
//! param when it is empty, which selects the launch home.
//!
//! # Lease
//! `holder` is `"agent"` or `"human"`; `epoch` moves on every real transition, so a client can
//! tell that an in-flight result was produced under a lease that has since changed. The
//! holder's `viewer_id` is a capability and never leaves the gateway — [`DisplayLease`] carries
//! `viewer_id: null` plus a short `viewer_hash` so the holder can recognise itself.
//! [`display_observe`] mints that id (per connection); [`display_lease_acquire`] /
//! [`display_lease_release`] take it back as proof. An id this connection never minted is
//! refused with code 5300 / `viewer_mismatch`.
//!
//! Every display failure is JSON-RPC **5300**; the gateway also sets `error.data.code`
//! (`viewer_mismatch`, …), which [`crate::RpcError`] does not carry — a caller only sees
//! `RpcError::Server { code: 5300, message }`, so a pane must re-observe on any 5300 from a
//! lease call instead of matching the data code.
//!
//! # Streaming socket: `WS /api/display/ws` (wire format documented, not implemented here)
//! Raw RFB over a WebSocket, one socket per viewer (`hermes_cli/web_routers/display.py`); the
//! two transports in this crate do not carry it, so a UI needs its own socket client (the
//! Electron desktop uses noVNC's Websock plus `apps/desktop/src/plugins/hermes-bots/screen-*`).
//!
//! * **URL**: `ws(s)://<gateway>/api/display/ws?display_ticket=<ticket>` — the ticket comes
//!   from [`display_observe`], is single-use, expires after 30 s, is provider `bot-desktop`
//!   and carries the profile home, so it only works on the gateway that minted it. It rides in
//!   the query string because noVNC's Websock cannot set headers or a subprotocol; there is no
//!   `Authorization` header on this route.
//! * **Frames**: binary only, both directions. Server→client frames are the raw RFB byte
//!   stream (`0`…`64 KiB` chunks) spliced from that profile's Xvnc Unix socket
//!   (`<profile home>/bot-desktop/rfb.sock`) or from a `docker exec` / `ssh` relay whose stdio
//!   *is* the stream when the screen lives inside the terminal backend's sandbox. There is no
//!   JSON envelope, no subprotocol and no application-level ping: client→server frames are
//!   RFB bytes too, and a **text** frame closes the socket (1003).
//! * **Input gate**: client bytes pass through `tools.bot_desktop.rfb_filter.RfbClientFilter`;
//!   keyboard/pointer/clipboard reach Xvnc only while a human holds the lease *and* the
//!   ticket's viewer id is that holder. Refused input is dropped silently (the stream, i.e.
//!   the picture, is never gated — watchers always see the screen).
//! * **Close codes**: `4000` `control-taken` — another human took over and this viewer was
//!   evicted (drop to watch mode and re-observe); `4001` `Bot Desktop is not running`;
//!   `4401` ticket missing/expired/used (call [`display_observe`] again); `4403`
//!   host/origin/client-IP policy refused the handshake *before* accept (a browser surfaces
//!   that one as an opaque 403); `1003` protocol (non-binary frame or an RFB parse error);
//!   `1000`/`1001` — the viewer closed its window, which hands control back if it still held
//!   it. A **dropped** link (1006, laptop lid, Wi-Fi) keeps the human's exclusion: they may be
//!   mid-login and the agent must not resume into that screen.
//!
//! The server→client request `display.install.sudo` (the masked sudo password card for
//! [`display_install`], app-level with an empty session id) is answered by the terminal sudo
//! bridge, not by this module.
//!
//! # Notifications
//! [`DISPLAY_STATUS_EVENT`] fires when a screen starts or stops — including transitions made
//! outside `hermes serve` (the CLI, gateway auto-start, an Xvnc crash, the idle auto-stop;
//! `methods_display_watch.py` polls the runtime files and the lease file every 0.5 s), with the
//! same payload shape as [`display_status`]. [`DISPLAY_LEASE_EVENT`] fires on every takeover /
//! hand-back from any process. Both are broadcast to every connected client, so a pane must
//! filter on `profile_key` *and* the connection the event arrived on.
//! [`DISPLAY_INSTALL_LOG_EVENT`] streams package-manager lines and [`DISPLAY_INSTALL_DONE_EVENT`]
//! ends the run with a fresh status snapshot (`code`: 0 ok, -1 cancelled, -2 no sudo — the
//! command to run by hand was streamed).

use std::time::Duration;

use serde_json::{Value, json};

use crate::api::{ApiError, Call, call_of, decode};
use crate::rpc::Client;

/// Notification: this profile's screen started or stopped. Payload = [`DisplayStatus`].
pub const DISPLAY_STATUS_EVENT: &str = "display.status";
/// Notification: the takeover lease changed hands. Payload = [`DisplayLeasePayload`].
pub const DISPLAY_LEASE_EVENT: &str = "display.lease";
/// Notification: one line of package-manager output. Payload = [`DisplayInstallLogPayload`].
pub const DISPLAY_INSTALL_LOG_EVENT: &str = "display.install.log";
/// Notification: the install ended. Payload = [`DisplayInstallDonePayload`].
pub const DISPLAY_INSTALL_DONE_EVENT: &str = "display.install.done";

/// Path of the raw-RFB WebSocket ([`display_observe`] returns it as `path`).
pub const DISPLAY_WS_PATH: &str = "/api/display/ws";
/// Query parameter carrying the single-use observe ticket on [`DISPLAY_WS_PATH`].
pub const DISPLAY_WS_TICKET_PARAM: &str = "display_ticket";

/// Close code `4000`: another human took over; this viewer was evicted.
pub const DISPLAY_WS_CLOSE_CONTROL_TAKEN: u16 = 4000;
/// Close code `4001`: the profile's screen is not running (start it, then re-observe).
pub const DISPLAY_WS_CLOSE_DESKTOP_GONE: u16 = 4001;
/// Close code `4401`: ticket missing, expired or already used — call [`display_observe`] again.
pub const DISPLAY_WS_CLOSE_BAD_TICKET: u16 = 4401;
/// Close code `4403`: host/origin/client-IP policy refused the handshake before accept.
pub const DISPLAY_WS_CLOSE_NOT_ALLOWED: u16 = 4403;
/// Close code `1003`: protocol error (a text frame, or unparsable RFB).
pub const DISPLAY_WS_CLOSE_PROTOCOL: u16 = 1003;

/// Our budget for [`display_start`]: the RPC blocks until Xvnc is published, well past this
/// crate's 30 s gateway default on a cold host (Xvnc + Xfce, or a container pull for a screen
/// inside the terminal backend's sandbox).
pub const DISPLAY_START_TIMEOUT: Duration = Duration::from_secs(180);

/// The takeover lease as clients may see it. `viewer_id` is always `null` on the wire (the
/// real id is a capability); `viewer_hash` is the first 12 hex chars of its SHA-256, for the
/// holder to recognise itself.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct DisplayLease {
    /// `"agent"` or `"human"`.
    pub holder: String,
    /// Always `null` in the public view (see [`DisplayLease`]).
    pub viewer_id: Option<String>,
    /// Short hash of the holder's viewer id, so the holder can tell it is in control.
    pub viewer_hash: Option<String>,
    /// Unix seconds of the last real transition.
    pub since: f64,
    /// Bumped on every real transition; unchanged for a no-op (already yours, bare release
    /// while the agent holds, …).
    pub epoch: i64,
    /// The agent's reason for asking a human to take over (kept while they act).
    pub reason: String,
}

impl DisplayLease {
    /// True while a human holds the screen (the only comparison that matters).
    pub fn human_holds(&self) -> bool {
        self.holder == "human"
    }
}

/// A persisted Docker sandbox still on the previous default image; the user decides the switch
/// ([`display_switch_sandbox_image`]).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct DisplayImageSwitch {
    pub current_image: String,
    pub target_image: String,
    /// How many persisted containers would be recreated.
    pub containers: i64,
}

/// `runtime.DesktopStatus` + the lease + the profile it speaks for. This is also the payload of
/// [`DISPLAY_STATUS_EVENT`] and part of [`DisplayObserve`] / [`DisplayStop`] /
/// [`DisplaySwitchSandboxImage`].
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct DisplayStatus {
    pub profile: String,
    /// False on a non-Linux gateway host (the screen cannot run there).
    pub supported: bool,
    /// The desktop stack (Xvnc, Xfce, a headed browser) is present.
    pub installed: bool,
    /// Packages missing from the host, for the install card.
    pub missing: Vec<String>,
    pub running: bool,
    pub pid: Option<i64>,
    /// `:20`-style display number.
    pub display: Option<String>,
    /// RFB Unix socket path.
    pub socket: Option<String>,
    /// Xvnc geometry, e.g. `1440x900x24`.
    pub geometry: String,
    /// The distro install command, or `None` when no apt-get/dnf/pacman exists.
    pub install_command: Option<String>,
    /// Headed Chromium the dock and `agent-browser` share; `None` when there is none.
    pub browser: Option<String>,
    /// Why [`display_start`] would refuse right now (host memory); the pane shows this
    /// instead of a Start button.
    pub blocker: Option<String>,
    pub memory_available_mb: Option<i64>,
    pub memory_limit_mb: Option<i64>,
    /// `"gateway"` or `"terminal:<backend>"` — where Xvnc runs.
    pub placement: String,
    /// A pending default-image switch the pane may approve (Docker placement only).
    pub image_switch: Option<DisplayImageSwitch>,
    pub lease: DisplayLease,
    /// Home key of the profile this snapshot speaks for (what events are matched on).
    pub profile_key: String,
}

/// One JPEG grab of the screen. `data_url` is `None` while stopped and while a human holds
/// (`suppressed: "human_has_control"`), because the frame may show what they are typing.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct DisplayThumbnail {
    /// `data:image/jpeg;base64,…`
    pub data_url: Option<String>,
    /// `"human_has_control"` when the grab was withheld.
    pub suppressed: Option<String>,
}

/// [`display_stop`] reply: the status snapshot plus whether anything was actually stopped.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct DisplayStop {
    #[serde(flatten)]
    pub status: DisplayStatus,
    pub stopped: bool,
}

/// [`display_observe`] reply: a single-use ticket for [`DISPLAY_WS_PATH`], the path itself, the
/// viewer id this connection must present to the lease calls, and a status snapshot. `ticket`
/// and `viewer_id` are capabilities, so `Debug` prints only their lengths.
#[derive(Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct DisplayObserve {
    pub ticket: String,
    /// Always [`DISPLAY_WS_PATH`].
    pub path: String,
    pub viewer_id: String,
    #[serde(flatten)]
    pub status: DisplayStatus,
}

impl std::fmt::Debug for DisplayObserve {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DisplayObserve")
            .field("ticket", &format_args!("<{} chars>", self.ticket.len()))
            .field("path", &self.path)
            .field("viewer_id", &format_args!("<{} chars>", self.viewer_id.len()))
            .field("status", &self.status)
            .finish()
    }
}

/// [`display_install`] reply: the package manager started in the background. Progress arrives
/// as [`DISPLAY_INSTALL_LOG_EVENT`] / [`DISPLAY_INSTALL_DONE_EVENT`].
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct DisplayInstallStarted {
    pub started: bool,
    /// The install command being run (e.g. `sudo apt-get install -y …`).
    pub command: Option<String>,
    pub profile_key: String,
}

/// [`display_switch_sandbox_image`] reply: the image now pinned in `terminal.docker_image`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct DisplaySwitchSandboxImage {
    pub docker_image: String,
    #[serde(flatten)]
    pub status: DisplayStatus,
}

/// [`display_lease_acquire`] / [`display_lease_release`] reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct DisplayLeaseResult {
    pub lease: DisplayLease,
}

// ── Notification payloads ───────────────────────────────────────────────────────

/// [`DISPLAY_LEASE_EVENT`] payload: the lease changed hands for `profile_key`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct DisplayLeasePayload {
    pub profile_key: String,
    pub lease: DisplayLease,
}

/// [`DISPLAY_INSTALL_LOG_EVENT`] payload: one line of package-manager output.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct DisplayInstallLogPayload {
    pub profile_key: String,
    pub line: String,
}

/// [`DISPLAY_INSTALL_DONE_EVENT`] payload: the run ended. `code` is 0 (ok), -1 (cancelled) or
/// -2 (no sudo — the command to run by hand was streamed as log lines).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct DisplayInstallDonePayload {
    pub profile_key: String,
    pub code: i64,
    pub status: DisplayStatus,
}

/// Parse a [`DISPLAY_STATUS_EVENT`] payload; `None` for any other event type.
pub fn parse_display_status(kind: &str, payload: &Value) -> Option<DisplayStatus> {
    if kind != DISPLAY_STATUS_EVENT {
        return None;
    }
    decode(payload.clone()).ok()
}

/// Parse a [`DISPLAY_LEASE_EVENT`] payload; `None` for any other event type.
pub fn parse_display_lease(kind: &str, payload: &Value) -> Option<DisplayLeasePayload> {
    if kind != DISPLAY_LEASE_EVENT {
        return None;
    }
    decode(payload.clone()).ok()
}

/// Parse a [`DISPLAY_INSTALL_LOG_EVENT`] payload; `None` for any other event type.
pub fn parse_display_install_log(kind: &str, payload: &Value) -> Option<DisplayInstallLogPayload> {
    if kind != DISPLAY_INSTALL_LOG_EVENT {
        return None;
    }
    decode(payload.clone()).ok()
}

/// Parse a [`DISPLAY_INSTALL_DONE_EVENT`] payload; `None` for any other event type.
pub fn parse_display_install_done(kind: &str, payload: &Value) -> Option<DisplayInstallDonePayload> {
    if kind != DISPLAY_INSTALL_DONE_EVENT {
        return None;
    }
    decode(payload.clone()).ok()
}

// ── Builders ────────────────────────────────────────────────────────────────────
// Every handler is `@profile_scoped`; an empty profile omits the param (launch home).

fn params(profile: &str) -> Value {
    let mut p = json!({});
    if !profile.is_empty() {
        p["profile"] = json!(profile);
    }
    p
}

/// `display.status` — runtime + lease snapshot. Read-only.
pub fn display_status_call(profile: &str) -> Call {
    call_of("display.status", params(profile))
}

/// `display.thumbnail` — one JPEG grab. Read-only: never changes the lease.
pub fn display_thumbnail_call(profile: &str) -> Call {
    call_of("display.thumbnail", params(profile))
}

/// `display.start` — start Xvnc + Xfce (idempotent; blocks until published). **Spawns
/// processes** (see [`display_start`]).
pub fn display_start_call(profile: &str) -> Call {
    call_of("display.start", params(profile))
}

/// `display.stop` — stop the screen. `force` stops it even while a human holds (otherwise the
/// gateway refuses with 5300 / `viewer_mismatch`).
pub fn display_stop_call(profile: &str, force: bool) -> Call {
    let mut p = params(profile);
    if force {
        p["force"] = json!(true);
    }
    call_of("display.stop", p)
}

/// `display.observe` — mint a `/api/display/ws` ticket and this connection's viewer id.
/// `viewer_id` may repeat an id this connection minted earlier (keeps identity and lease
/// across a reconnect); anything else is replaced by a fresh id.
pub fn display_observe_call(profile: &str, viewer_id: &str) -> Call {
    let mut p = params(profile);
    if !viewer_id.is_empty() {
        p["viewer_id"] = json!(viewer_id);
    }
    call_of("display.observe", p)
}

/// `display.install` — run the distro package install on the gateway host. **Installs system
/// packages as root** (see [`display_install`]).
pub fn display_install_call(profile: &str) -> Call {
    call_of("display.install", params(profile))
}

/// `display.switchSandboxImage` — decide the pending default-image switch: `approve` true pins
/// the new image (containers are recreated on next use), false keeps the current one.
pub fn display_switch_sandbox_image_call(profile: &str, approve: bool) -> Call {
    let mut p = params(profile);
    p["approve"] = json!(approve);
    call_of("display.switchSandboxImage", p)
}

/// `display.lease.acquire` — a human (`viewer_id`) takes control (last writer wins).
pub fn display_lease_acquire_call(profile: &str, viewer_id: &str, reason: &str) -> Call {
    let mut p = params(profile);
    p["viewer_id"] = json!(viewer_id);
    if !reason.is_empty() {
        p["reason"] = json!(reason);
    }
    call_of("display.lease.acquire", p)
}

/// `display.lease.release` — hand control back. Without a `viewer_id` the release is refused
/// while a human holds unless `force`; with one, only that holder may release.
pub fn display_lease_release_call(profile: &str, viewer_id: &str, force: bool) -> Call {
    let mut p = params(profile);
    if !viewer_id.is_empty() {
        p["viewer_id"] = json!(viewer_id);
    }
    if force {
        p["force"] = json!(true);
    }
    call_of("display.lease.release", p)
}

// ── Wrappers ────────────────────────────────────────────────────────────────────

/// Send with a longer budget than the crate default (the RPC blocks until the screen is up).
async fn call_with_timeout(client: &Client, c: &Call, timeout: Duration) -> Result<Value, ApiError> {
    Ok(client.request_with_timeout(c.method, c.params.clone(), timeout).await?)
}

/// Runtime + lease snapshot for this profile's screen. Read-only; safe to poll.
///
/// Also the first call that installs the gateway's lease listener, which is what makes
/// [`DISPLAY_LEASE_EVENT`] arrive at all.
pub async fn display_status(client: &Client, profile: &str) -> Result<DisplayStatus, ApiError> {
    crate::api::call_as(client, &display_status_call(profile)).await
}

/// One JPEG grab of the screen (`data_url` is `None` while stopped or while a human holds).
/// **Read-only: never changes the lease**, so it is safe to call on a timer — but the frame is
/// the profile's live desktop, i.e. potentially private content; don't ship it anywhere.
pub async fn display_thumbnail(client: &Client, profile: &str) -> Result<DisplayThumbnail, ApiError> {
    crate::api::call_as(client, &display_thumbnail_call(profile)).await
}

/// Start this profile's Xvnc + Xfce (idempotent) and return the new snapshot.
///
/// **Spawns processes**: Xvnc, Xfce and (for a sandbox placement) a container. Blocks until
/// the display is published, hence [`DISPLAY_START_TIMEOUT`]; confirm with the user first.
pub async fn display_start(client: &Client, profile: &str) -> Result<DisplayStatus, ApiError> {
    decode(call_with_timeout(client, &display_start_call(profile), DISPLAY_START_TIMEOUT).await?)
}

/// Stop the screen and report whether anything was stopped.
///
/// **Kills the display under whoever is on it** — a human holding the lease is only stopped
/// with `force: true` (the gateway otherwise refuses 5300 / `viewer_mismatch`). Destructive:
/// unsaved work in that session is lost; confirm first.
pub async fn display_stop(client: &Client, profile: &str, force: bool) -> Result<DisplayStop, ApiError> {
    crate::api::call_as(client, &display_stop_call(profile, force)).await
}

/// Mint a single-use 30 s ticket for [`DISPLAY_WS_PATH`] plus the viewer id this connection
/// must present to the lease calls. Read-only (a ticket is inert until redeemed) but it does
/// install the gateway's lease listener; the returned `path` is `""`-safe — use
/// [`DISPLAY_WS_PATH`] if the backend omitted it.
pub async fn display_observe(client: &Client, profile: &str, viewer_id: &str) -> Result<DisplayObserve, ApiError> {
    crate::api::call_as(client, &display_observe_call(profile, viewer_id)).await
}

/// Start the distro package install on the gateway host in the background.
///
/// **Builder-only in practice; never call this from an automated path.** It runs a package
/// manager as root — the gateway asks for the sudo password through the masked
/// `display.install.sudo` server→client request (answered by the terminal sudo bridge, not by
/// this module) — and mutates the host's installed packages. Progress arrives as
/// [`DISPLAY_INSTALL_LOG_EVENT`] lines and [`DISPLAY_INSTALL_DONE_EVENT`]; keep the gateway
/// socket open across the run or the events are lost.
pub async fn display_install(client: &Client, profile: &str) -> Result<DisplayInstallStarted, ApiError> {
    crate::api::call_as(client, &display_install_call(profile)).await
}

/// Decide the pending default sandbox image switch (the Screen pane's answer to the offer).
///
/// **Writes persisted config** (`terminal.docker_image`) and pins the image that the *next*
/// container use will recreate persisted containers from — a destructive, profile-wide
/// decision. Refused when nothing is pending. Confirm first.
pub async fn display_switch_sandbox_image(
    client: &Client,
    profile: &str,
    approve: bool,
) -> Result<DisplaySwitchSandboxImage, ApiError> {
    crate::api::call_as(client, &display_switch_sandbox_image_call(profile, approve)).await
}

/// Take over: the human named by `viewer_id` controls the screen (last writer wins; another
/// viewer is evicted from the RFB socket).
///
/// **Takes control away from the agent or another human.** `viewer_id` must be an id this
/// connection minted via [`display_observe`]; anything else is refused 5300 / `viewer_mismatch`.
pub async fn display_lease_acquire(
    client: &Client,
    profile: &str,
    viewer_id: &str,
    reason: &str,
) -> Result<DisplayLease, ApiError> {
    let r: DisplayLeaseResult =
        crate::api::call_as(client, &display_lease_acquire_call(profile, viewer_id, reason)).await?;
    Ok(r.lease)
}

/// Hand control back to the agent.
///
/// **Changes who may drive the screen.** Without a `viewer_id` the release is refused while a
/// human holds unless `force: true`; with one, only that holder may release. Returns the lease
/// as it stands afterwards (read `human_holds()` to learn whether anything happened).
pub async fn display_lease_release(
    client: &Client,
    profile: &str,
    viewer_id: &str,
    force: bool,
) -> Result<DisplayLease, ApiError> {
    let r: DisplayLeaseResult =
        crate::api::call_as(client, &display_lease_release_call(profile, viewer_id, force)).await?;
    Ok(r.lease)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_debug_hides_capabilities() {
        let o = DisplayObserve {
            ticket: "tkt-SECRET".into(),
            viewer_id: "vw-SECRET".into(),
            ..Default::default()
        };
        let shown = format!("{o:?}");
        assert!(!shown.contains("SECRET"), "{shown}");
        assert!(shown.contains("<10 chars>") && shown.contains("<9 chars>"), "{shown}");
    }

    fn status_json() -> Value {
        json!({
            "profile": "ops",
            "supported": true,
            "installed": true,
            "missing": ["xdotool"],
            "running": true,
            "pid": 4242,
            "display": ":20",
            "socket": "/home/u/.hermes/profiles/ops/bot-desktop/rfb.sock",
            "geometry": "1440x900x24",
            "install_command": "sudo apt-get install -y xvnc xfce4",
            "browser": "/usr/bin/chromium",
            "blocker": null,
            "memory_available_mb": 5120,
            "memory_limit_mb": 2048,
            "placement": "gateway",
            "image_switch": {"current_image": "a:1", "target_image": "a:2", "containers": 2},
            "lease": {
                "holder": "human",
                "viewer_id": null,
                "viewer_hash": "9f2c1ab34de5",
                "since": 1770000000.5,
                "epoch": 7,
                "reason": "please log in"
            },
            "profile_key": "/home/u/.hermes/profiles/ops",
            "future_field": 1
        })
    }

    #[test]
    fn builders_send_profile_and_optional_params() {
        assert_eq!(display_status_call("ops").method, "display.status");
        assert_eq!(display_status_call("").params, json!({}));
        assert_eq!(display_status_call("ops").params, json!({"profile": "ops"}));
        assert_eq!(display_thumbnail_call("ops").method, "display.thumbnail");
        assert_eq!(display_thumbnail_call("ops").params, json!({"profile": "ops"}));
        assert_eq!(display_start_call("ops").method, "display.start");
        assert_eq!(display_start_call("").params, json!({}));

        assert_eq!(display_stop_call("ops", false).params, json!({"profile": "ops"}));
        assert_eq!(
            display_stop_call("ops", true).params,
            json!({"profile": "ops", "force": true})
        );

        assert_eq!(display_observe_call("ops", "").params, json!({"profile": "ops"}));
        assert_eq!(
            display_observe_call("ops", "v1").params,
            json!({"profile": "ops", "viewer_id": "v1"})
        );

        assert_eq!(display_install_call("ops").method, "display.install");
        assert_eq!(display_install_call("ops").params, json!({"profile": "ops"}));

        assert_eq!(
            display_switch_sandbox_image_call("ops", false).params,
            json!({"profile": "ops", "approve": false})
        );
        assert_eq!(
            display_switch_sandbox_image_call("", true).params,
            json!({"approve": true})
        );

        assert_eq!(
            display_lease_acquire_call("ops", "v1", "").params,
            json!({"profile": "ops", "viewer_id": "v1"})
        );
        assert_eq!(
            display_lease_acquire_call("", "v1", "please log in").params,
            json!({"viewer_id": "v1", "reason": "please log in"})
        );

        assert_eq!(
            display_lease_release_call("ops", "", false).params,
            json!({"profile": "ops"})
        );
        assert_eq!(
            display_lease_release_call("ops", "v1", true).params,
            json!({"profile": "ops", "viewer_id": "v1", "force": true})
        );
    }

    #[test]
    fn status_decodes_and_is_lenient() {
        let s: DisplayStatus = decode(status_json()).unwrap();
        assert_eq!(s.profile, "ops");
        assert!(s.supported && s.installed && s.running);
        assert_eq!(s.display.as_deref(), Some(":20"));
        assert_eq!(s.geometry, "1440x900x24");
        assert_eq!(s.missing, vec!["xdotool"]);
        assert_eq!(s.pid, Some(4242));
        assert!(s.blocker.is_none());
        assert_eq!(s.memory_limit_mb, Some(2048));
        assert_eq!(s.placement, "gateway");
        assert_eq!(s.image_switch.as_ref().unwrap().containers, 2);
        assert!(s.lease.human_holds());
        assert_eq!(s.lease.epoch, 7);
        assert!(s.lease.viewer_id.is_none());
        assert_eq!(s.profile_key, "/home/u/.hermes/profiles/ops");

        let empty: DisplayStatus = decode(json!({})).unwrap();
        assert_eq!(empty, DisplayStatus::default());
        assert_eq!(empty.placement, "");
        assert!(!empty.lease.human_holds());
        assert!(decode::<DisplayStatus>(json!([1])).is_err());
    }

    #[test]
    fn composite_replies_decode() {
        let thumb: DisplayThumbnail = decode(json!({"data_url": null, "suppressed": "human_has_control"})).unwrap();
        assert_eq!(thumb.suppressed.as_deref(), Some("human_has_control"));
        assert!(thumb.data_url.is_none());
        assert_eq!(
            decode::<DisplayThumbnail>(json!({"data_url": "data:image/jpeg;base64,AA"}))
                .unwrap()
                .data_url
                .as_deref(),
            Some("data:image/jpeg;base64,AA")
        );
        assert_eq!(
            decode::<DisplayThumbnail>(json!({})).unwrap(),
            DisplayThumbnail::default()
        );

        let mut stopped = status_json();
        stopped["stopped"] = json!(true);
        let stop: DisplayStop = decode(stopped).unwrap();
        assert!(stop.stopped);
        assert_eq!(stop.status.display.as_deref(), Some(":20"));
        assert_eq!(decode::<DisplayStop>(json!({})).unwrap(), DisplayStop::default());

        let mut observed = status_json();
        observed["ticket"] = json!("single-use");
        observed["path"] = json!("/api/display/ws");
        observed["viewer_id"] = json!("v1");
        let o: DisplayObserve = decode(observed).unwrap();
        assert_eq!(o.ticket, "single-use");
        assert_eq!(o.path, DISPLAY_WS_PATH);
        assert_eq!(o.viewer_id, "v1");
        assert_eq!(o.status.profile, "ops");
        assert_eq!(decode::<DisplayObserve>(json!({})).unwrap(), DisplayObserve::default());

        let inst: DisplayInstallStarted =
            decode(json!({"started": true, "command": "sudo apt-get install -y xvnc", "profile_key": "k"})).unwrap();
        assert!(inst.started);
        assert_eq!(inst.profile_key, "k");
        assert_eq!(
            decode::<DisplayInstallStarted>(json!({})).unwrap(),
            DisplayInstallStarted::default()
        );

        let mut switched = status_json();
        switched["docker_image"] = json!("nousresearch/hermes-sandbox:desktop");
        let sw: DisplaySwitchSandboxImage = decode(switched).unwrap();
        assert_eq!(sw.docker_image, "nousresearch/hermes-sandbox:desktop");
        assert!(sw.status.running);
        assert_eq!(
            decode::<DisplaySwitchSandboxImage>(json!({})).unwrap(),
            DisplaySwitchSandboxImage::default()
        );

        let lease: DisplayLeaseResult =
            decode(json!({"lease": {"holder": "agent", "epoch": 3, "since": 1.0}})).unwrap();
        assert!(!lease.lease.human_holds());
        assert_eq!(lease.lease.epoch, 3);
        assert_eq!(
            decode::<DisplayLeaseResult>(json!({})).unwrap(),
            DisplayLeaseResult::default()
        );
    }

    #[test]
    fn notification_payloads_parse() {
        let mut s = status_json();
        s["profile_key"] = json!("k1");
        let got = parse_display_status(DISPLAY_STATUS_EVENT, &s).unwrap();
        assert_eq!(got.profile_key, "k1");
        assert_eq!(got.lease.epoch, 7);
        assert!(parse_display_status(DISPLAY_LEASE_EVENT, &s).is_none());
        assert_eq!(
            parse_display_status(DISPLAY_STATUS_EVENT, &json!({})).unwrap(),
            DisplayStatus::default()
        );

        let p = parse_display_lease(
            DISPLAY_LEASE_EVENT,
            &json!({"profile_key": "k1", "lease": {"holder": "human", "epoch": 9, "since": 2.0,
             "viewer_hash": "abc123abc123"}}),
        )
        .unwrap();
        assert_eq!(p.profile_key, "k1");
        assert!(p.lease.human_holds());
        assert_eq!(p.lease.epoch, 9);
        assert_eq!(
            parse_display_lease(DISPLAY_LEASE_EVENT, &json!({})).unwrap(),
            DisplayLeasePayload::default()
        );
        assert!(parse_display_lease(DISPLAY_STATUS_EVENT, &json!({})).is_none());

        let log = parse_display_install_log(
            DISPLAY_INSTALL_LOG_EVENT,
            &json!({"profile_key": "k1", "line": "Reading package lists..."}),
        )
        .unwrap();
        assert_eq!(log.line, "Reading package lists...");
        assert_eq!(log.profile_key, "k1");
        assert!(parse_display_install_log(DISPLAY_INSTALL_DONE_EVENT, &json!({})).is_none());

        let done = parse_display_install_done(
            DISPLAY_INSTALL_DONE_EVENT,
            &json!({"profile_key": "k1", "code": -2, "status": status_json()}),
        )
        .unwrap();
        assert_eq!(done.code, -2);
        assert!(done.status.installed);
        assert_eq!(
            parse_display_install_done(DISPLAY_INSTALL_DONE_EVENT, &json!({})).unwrap(),
            DisplayInstallDonePayload::default()
        );
        assert!(parse_display_install_done(DISPLAY_INSTALL_LOG_EVENT, &json!({})).is_none());
    }

    #[test]
    fn ws_constants_match_the_backend() {
        assert_eq!(DISPLAY_WS_PATH, "/api/display/ws");
        assert_eq!(DISPLAY_WS_TICKET_PARAM, "display_ticket");
        assert_eq!(DISPLAY_WS_CLOSE_CONTROL_TAKEN, 4000);
        assert_eq!(DISPLAY_WS_CLOSE_DESKTOP_GONE, 4001);
        assert_eq!(DISPLAY_WS_CLOSE_BAD_TICKET, 4401);
        assert_eq!(DISPLAY_WS_CLOSE_NOT_ALLOWED, 4403);
        assert_eq!(DISPLAY_WS_CLOSE_PROTOCOL, 1003);
    }
}
