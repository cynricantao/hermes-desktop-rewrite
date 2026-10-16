//! Live end-to-end probe against a real backend.
//!
//! `cargo run -p hermes-core --example probe -- "Reply with exactly: pong"`
//!
//! Spawns (or attaches via `HERMES_DESKTOP_PORT`) a backend, creates a session, sends one
//! prompt, streams it through the transcript reducer and prints the result. The probe
//! session is closed afterwards. Note: this consumes a (tiny) model call.

use hermes_core::backend;
use hermes_core::protocol::SessionCreateResult;
use hermes_core::transcript::{Block, Transcript};
use hermes_core::{Client, ClientEvent};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let prompt = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Reply with exactly: pong".into());
    let t0 = std::time::Instant::now();
    let be = backend::discover_or_spawn(None).await?;
    println!(
        "backend ready on :{} (owned={}) in {:?}",
        be.endpoint.port,
        be.is_owned(),
        t0.elapsed()
    );
    let (client, mut rx) = Client::connect(&be.endpoint.ws_url()).await?;
    let created: SessionCreateResult = client.call("session.create", json!({"source": "desktop"})).await?;
    println!("session {} model={}", created.session_id, created.info.model);
    let sid = created.session_id.clone();
    let mut t = Transcript::new();
    t.push_user(&prompt);
    let ack = client
        .request("prompt.submit", json!({"session_id": sid, "text": prompt}))
        .await?;
    println!("submit ack: {ack}");
    let mut kinds = Vec::new();
    while let Some(ev) = rx.recv().await {
        if let ClientEvent::Event(e) = ev {
            if e.session_id.as_deref().is_some_and(|s| s != sid) {
                continue;
            }
            kinds.push(e.kind.clone());
            t.apply(&e);
            if e.kind == "message.complete" {
                break;
            }
        }
    }
    println!("events: {kinds:?}");
    for b in &t.blocks {
        if let Block::Text { role, text, .. } = b {
            println!("{role:?}: {text}");
        }
    }
    let _ = client.request("session.close", json!({"session_id": sid})).await;
    Ok(())
}
