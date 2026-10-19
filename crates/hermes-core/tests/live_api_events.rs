//! Live checks for the events area, against the PRIVATE test backend only
//! (`scripts/live-test-backend.sh`, then `HERMES_DESKTOP_PORT=<port>`).
//!
//! What can be checked live, and why the rest cannot:
//!
//! * **`gateway.ready`** — the contract says it is the first frame after accept, so the test
//!   connects, reads events for a few seconds and parses whatever startup notification
//!   arrives through [`events::KnownEvent`]. A backend that sends none is reported, not
//!   failed: older builds open the socket silently.
//! * **`gateway.ping`** — the harmless read that proves the socket still works after the
//!   event loop ran. Nothing is mutated.
//! * **Every other notification** is *builder-only*: emitting one requires a model turn
//!   (`subagent.*`, `moa.*`, `message.interim`, `reasoning.available`), a live recorder
//!   (`voice.*`, `wake.detected`), an agent-owned process (`terminal.*`), a connector
//!   operation (`connection.*`), a config/skin change (`*.changed`), or a user prompt
//!   (`tip.show`, `pane.reveal`, `setup.ready`). Triggering them on a shared backend means
//!   model calls or state changes, so they are covered by the fixture tests in
//!   `crates/hermes-core/src/api/events.rs` (one realistic payload per event).
//! * **Server->client requests** (`setup_beat`, `theme.apply`, `theme.pick`, `vault.code`,
//!   `vault.save_login`, `terminal.read`, `window.read`) are builder-only for the same
//!   reason plus one more: they are *sent by* the backend mid-turn and answered by the
//!   client, so there is nothing to call from a test. The parse + reply builders are unit
//!   tested.

mod common;

use std::time::Duration;

use hermes_core::Client;
use hermes_core::api::events::{self, KnownEvent};
use hermes_core::rpc::ClientEvent;
use serde_json::json;
use tokio::time::{Instant, timeout};

#[tokio::test]
#[ignore]
async fn startup_notifications_and_harmless_read() {
    assert!(
        std::env::var_os("HERMES_DESKTOP_PORT").is_some(),
        "set HERMES_DESKTOP_PORT to a private test backend (scripts/live-test-backend.sh)"
    );
    let mut checks = common::Checks::default();

    let b = hermes_core::backend::discover_or_spawn(None).await.expect("backend");
    let (client, mut rx) = Client::connect(&b.endpoint.ws_url()).await.expect("websocket");
    // The managed handle must outlive the test; an attached backend is never killed.
    std::mem::forget(b);

    let mut ready = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    while ready.is_none() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match timeout(left, rx.recv()).await {
            Ok(Some(ClientEvent::Event(ev))) => {
                if ev.kind == events::GATEWAY_READY {
                    ready = KnownEvent::parse(&ev.kind, &ev.payload);
                } else {
                    println!("startup event: {} seq={:?}", ev.kind, ev.seq);
                }
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => break,
        }
    }

    match ready {
        Some(KnownEvent::GatewayReady(p)) => checks.check(
            "gateway.ready",
            true,
            format!(
                "change_events={} heartbeat={:?} replay_epoch={:?} skin={:?}",
                p.change_events, p.heartbeat, p.replay_epoch, p.skin.name
            ),
        ),
        Some(other) => checks.check("gateway.ready", false, format!("unexpected variant: {other:?}")),
        None => println!(
            "NOTE: no gateway.ready within 5 s; the backend opened the socket silently. \
             Event payloads remain fixture-tested (unit tests in api::events)."
        ),
    }

    // A harmless read over the same socket after the event loop ran.
    let ping = client.request("gateway.ping", json!({})).await;
    checks.ok("gateway.ping", &ping);

    checks.finish();
}
