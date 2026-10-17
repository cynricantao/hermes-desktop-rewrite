//! Live check for image attachments, against a PRIVATE test backend only:
//!
//! ```sh
//! scripts/live-test-backend.sh        # prints HERMES_DESKTOP_PORT=<port>
//! HERMES_DESKTOP_PORT=<port> cargo test -p hermes-core --test live_attachments -- --ignored --nocapture
//! ```
//!
//! Creates its own throwaway session, queues a 1×1 PNG with `image.attach_bytes` (the call
//! Ctrl+V makes), removes it with `image.detach`, then deletes the session. No prompt is
//! sent, so no model calls are spent. The desktop clipboard read itself (wl-paste/xclip)
//! is not exercised here: it needs the user's graphical session.

use hermes_core::Client;
use hermes_core::attachments::{self, AttachedImage, ClipboardImage};
use serde_json::{Value, json};

/// A valid 1×1 transparent PNG.
const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49,
    0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00,
    0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a private test backend (HERMES_DESKTOP_PORT)"]
async fn paste_image_round_trip() {
    assert!(
        std::env::var_os("HERMES_DESKTOP_PORT").is_some(),
        "set HERMES_DESKTOP_PORT to a private test backend (scripts/live-test-backend.sh)"
    );
    let backend = hermes_core::backend::discover_or_spawn(None).await.expect("backend");
    let (client, _events) = Client::connect(&backend.endpoint.ws_url()).await.expect("websocket");

    let created = client
        .request("session.create", json!({"source": "desktop"}))
        .await
        .expect("session.create");
    let sid = created["session_id"].as_str().expect("session_id").to_owned();
    println!("throwaway session {sid}");

    let img = ClipboardImage {
        bytes: PNG_1X1.to_vec(),
        mime: "image/png".into(),
    };
    let (m, p) = attachments::attach_bytes_params(&sid, &img);
    let reply = client.request(m, p).await.expect("image.attach_bytes");
    let attached: AttachedImage = serde_json::from_value(reply.clone()).expect("decode");
    println!("attach_bytes → {reply}");
    assert!(attached.attached, "image queued");
    assert_eq!(attached.count, Some(1), "one image queued for the next turn");
    assert!(attached.label().ends_with(".png"), "chip label {}", attached.label());

    let (m, p) = attachments::detach_params(&sid, attached.path.as_deref().expect("path"));
    let reply = client.request(m, p).await.expect("image.detach");
    println!("detach → {reply}");
    let remaining = reply.get("count").and_then(Value::as_u64);
    assert_eq!(remaining, Some(0), "queue empty after detach: {reply}");

    // Clean up. A session that never received a prompt is not written to the session
    // store, so session.delete may answer "not found"; that is fine.
    let stored = created["stored_session_id"].as_str().unwrap_or(&sid).to_owned();
    let _ = client.request("session.close", json!({"session_id": sid})).await;
    let deleted = client.request("session.delete", json!({"session_id": stored})).await;
    println!("session.delete → {deleted:?}");
    println!("PASS paste_image_round_trip");
    std::mem::forget(backend);
}
