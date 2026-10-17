//! Live probe checks against the PRIVATE test backend (`scripts/live-test-backend.sh`).
//!
//! The token is never printed: it comes from the same discovery `backend.rs` uses for
//! `HERMES_DESKTOP_PORT` — `GET /` and scrape `window.__HERMES_SESSION_TOKEN__`.
//!
//! Run (the token discovery needs a live backend; the wrong-service check spins up its
//! own throwaway HTTP helper on 127.0.0.1):
//!
//! ```text
//! terminal 1: scripts/live-test-backend.sh          # prints its port
//! terminal 2: HERMES_DESKTOP_PORT=<port> cargo test -p hermes-core --test live_remote -- --ignored --nocapture
//! ```
//!
//! Never point `HERMES_DESKTOP_PORT` at the everyday backend.

use std::time::Duration;

use hermes_core::backend;
use hermes_core::remote::{AuthMode, ProbeOptions, ProbeOutcome, ProbeRequest, probe};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// The batch's private test backend, used when the env var is not set.
const DEFAULT_TEST_PORT: u16 = 36763;

fn port() -> u16 {
    std::env::var("HERMES_DESKTOP_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(DEFAULT_TEST_PORT)
}

fn options() -> ProbeOptions {
    ProbeOptions {
        http_timeout: Duration::from_secs(5),
        connect_timeout: Duration::from_secs(5),
        ready_grace: Duration::from_millis(750),
    }
}

#[tokio::test]
#[ignore = "live: needs the private test backend (scripts/live-test-backend.sh)"]
async fn probe_ok_with_the_served_token() {
    let port = port();
    let token = backend::fetch_token("127.0.0.1", port)
        .await
        .expect("the test backend must serve a session token on /");
    let request = ProbeRequest::new(&format!("http://127.0.0.1:{port}"), AuthMode::Token, Some(token), &[]).unwrap();

    let outcome = probe(&request, options()).await;
    assert!(
        matches!(outcome, ProbeOutcome::Ok { ws_checked: true, .. }),
        "expected ok with a live ws leg, got {outcome:?}"
    );
    println!("live_remote: ok (HTTP status + live /api/ws upgrade) on port {port}");
}

#[tokio::test]
#[ignore = "live: needs the private test backend (scripts/live-test-backend.sh)"]
async fn probe_auth_failed_with_a_wrong_token() {
    let port = port();
    // Deliberately wrong: /api/status is public, so only the WS upgrade can reject it —
    // exactly the false-positive the live WS leg exists to catch.
    let request = ProbeRequest::new(
        &format!("http://127.0.0.1:{port}"),
        AuthMode::Token,
        Some("not-the-real-token-0000".to_string()),
        &[],
    )
    .unwrap();

    let outcome = probe(&request, options()).await;
    assert!(
        matches!(outcome, ProbeOutcome::AuthFailed { .. }),
        "expected auth-failed for a wrong token, got {outcome:?}"
    );
    println!("live_remote: wrong token rejected by the gateway as expected");
}

#[tokio::test]
#[ignore = "live: independent of the backend — a closed local port"]
async fn probe_unreachable_on_a_closed_port() {
    let request = ProbeRequest::new("http://127.0.0.1:1", AuthMode::Token, None, &[]).unwrap();
    let outcome = probe(&request, options()).await;
    assert!(
        matches!(outcome, ProbeOutcome::Unreachable { .. }),
        "expected unreachable, got {outcome:?}"
    );
    println!("live_remote: closed port reported unreachable");
}

#[tokio::test]
#[ignore = "live: spins up its own throwaway HTTP helper on 127.0.0.1"]
async fn probe_wrong_service_on_a_non_hermes_port() {
    // Something that is definitely not a Hermes gateway: one HTML page on a kernel-picked
    // loopback port, served by this test process only.
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let helper = tokio::spawn(async move {
        for _ in 0..4 {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut buf = vec![0u8; 4096];
            let _ = stream.read(&mut buf).await;
            let body = "<html><body>hello, not hermes</body></html>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });

    let request = ProbeRequest::new(&format!("http://127.0.0.1:{port}"), AuthMode::Token, None, &[]).unwrap();
    let outcome = probe(&request, options()).await;
    helper.abort();
    assert!(
        matches!(outcome, ProbeOutcome::WrongService { .. }),
        "expected wrong-service for an HTML page, got {outcome:?}"
    );
    println!("live_remote: an HTML port is reported as wrong service");
}
