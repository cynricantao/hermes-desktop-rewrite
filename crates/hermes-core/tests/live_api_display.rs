//! Live checks for `api::display` against a PRIVATE test backend (`HERMES_DESKTOP_PORT`).
//!
//! ```sh
//! scripts/live-test-backend.sh        # prints HERMES_DESKTOP_PORT
//! HERMES_DESKTOP_PORT=<port> cargo test -p hermes-core --test live_api_display -- --ignored --nocapture
//! ```
//!
//! Run live (all pure local reads; nothing to clean up):
//! * `display.status` — the runtime + lease snapshot. Read-only in the handler (`_display_snapshot`);
//!   the only side effect is installing the gateway's in-process lease listener, which is what makes
//!   `display.lease` events arrive at all.
//! * `display.thumbnail` — the handler reads `lease.json`, then `thumbnail_data_url()`, which returns
//!   `None` when `published_env()` has no `DISPLAY` (i.e. nothing is running). No process, no grab, no
//!   lease change; a running screen is only ever *read*.
//! * `display.observe` — refused with 5300 while nothing runs (`rfb_socket_path() is None and not
//!   sandbox_screen_running()`), before any ticket is minted. When a screen IS running we accept the
//!   minted ticket and never redeem it (a ticket is inert until the WebSocket uses it).
//!
//! Builder-only (not called live):
//! * `display.start` / `display.stop` — spawn / kill the Xvnc + Xfce process tree (stop also kills
//!   whatever a human has open on that screen).
//! * `display.install` — runs a distro package manager as root on the gateway host and asks for the
//!   sudo password through the `display.install.sudo` server request.
//! * `display.switchSandboxImage` — persists `terminal.docker_image` and decides which image the next
//!   container use recreates persisted containers from.
//! * `display.lease.acquire` / `display.lease.release` — flips the shared takeover lease that live
//!   agents fence on (and `acquire` needs a viewer id only `display.observe` can mint), so a live
//!   check would change who may drive the screen rather than exercise a throwaway item.
//!
//! The `/api/display/ws` socket is documented in the module, not exercised here: it carries raw RFB
//! binary frames and the crate's transports do not speak it.

use std::time::Duration;

use hermes_core::RpcError;
use hermes_core::api::ApiError;
use hermes_core::api::display;

mod common;

use common::{Checks, backend};

#[tokio::test]
#[ignore = "needs a private test backend: HERMES_DESKTOP_PORT"]
async fn display_status_thumbnail_observe() {
    let (_rest, client) = backend().await;
    let mut c = Checks::default();

    // ── display.status: read-only snapshot ──────────────────────────────────────
    let status = display::display_status(&client, "").await;
    // Summarise rather than print the whole snapshot: the lease carries a viewer id.
    c.check(
        "display.status",
        status.is_ok(),
        match &status {
            Ok(s) => format!(
                "supported={} installed={} running={}",
                s.supported, s.installed, s.running
            ),
            Err(e) => e.to_string(),
        },
    );
    let Some(status) = status.ok() else {
        c.finish();
        return;
    };
    c.check(
        "status.profile_key",
        !status.profile_key.is_empty(),
        format!("{:?}", status.profile_key),
    );
    c.check(
        "status.placement",
        status.placement == "gateway" || status.placement.starts_with("terminal:"),
        format!("{:?}", status.placement),
    );
    c.check(
        "status.lease.holder",
        status.lease.holder == "agent" || status.lease.holder == "human",
        format!("{:?} epoch={}", status.lease.holder, status.lease.epoch),
    );
    c.check(
        "status.lease.viewer_id_redacted",
        status.lease.viewer_id.is_none(),
        format!("{:?}", status.lease.viewer_id),
    );
    c.check(
        "status.running_matches_display",
        status.running || status.display.is_none(),
        format!("running={} display={:?}", status.running, status.display),
    );

    // ── display.thumbnail: read-only grab ───────────────────────────────────────
    let thumb = display::display_thumbnail(&client, "").await;
    // Never print the frame itself (a live screen grab): presence and size only.
    c.check(
        "display.thumbnail",
        thumb.is_ok(),
        match &thumb {
            Ok(t) => format!("data_url_len={:?}", t.data_url.as_ref().map(|d| d.len())),
            Err(e) => e.to_string(),
        },
    );
    if let Ok(t) = &thumb {
        let consistent = if status.running {
            true // a stopped lease holder can still get nothing, both outcomes are valid
        } else {
            t.data_url.is_none() && t.suppressed.is_none()
        };
        c.check(
            "thumbnail.stopped_is_null",
            consistent,
            format!(
                "running={} data_url={:?} suppressed={:?}",
                status.running,
                t.data_url.as_ref().map(|_| "<jpeg>"),
                t.suppressed
            ),
        );
    }

    // ── display.observe: refused while stopped, inert ticket while running ─────
    let observed = display::display_observe(&client, "", "").await;
    match (&observed, status.running) {
        (Ok(o), _) => {
            c.check(
                "display.observe.ticket",
                !o.ticket.is_empty() && !o.viewer_id.is_empty(),
                format!(
                    "path={:?} viewer_id_len={} ticket_len={}",
                    o.path,
                    o.viewer_id.len(),
                    o.ticket.len()
                ),
            );
            c.check(
                "display.observe.path",
                o.path == display::DISPLAY_WS_PATH,
                format!("{:?}", o.path),
            );
        }
        (Err(e), false) => {
            let shape = match e {
                ApiError::Rpc(RpcError::Server { code, message }) => *code == 5300 && message.contains("not running"),
                _ => false,
            };
            c.check("display.observe.refused_while_stopped", shape, format!("{e}"));
        }
        (Err(e), true) => {
            // A running screen should always mint; surface it rather than hiding it.
            c.check("display.observe", false, format!("{e}"));
        }
    }

    // The socket path constant is the one `display.observe` returns.
    c.check(
        "ws.path_constant",
        display::DISPLAY_WS_PATH == "/api/display/ws",
        display::DISPLAY_WS_PATH,
    );
    // Keep the client alive until the checks are done (drops cleanly at scope end).
    tokio::time::sleep(Duration::from_millis(10)).await;
    drop(client);
    c.finish();
}
