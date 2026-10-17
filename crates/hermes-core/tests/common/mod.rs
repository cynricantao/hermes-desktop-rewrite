//! Shared helpers for the `live_api_*` tests: they talk to a PRIVATE test backend only.
//!
//! ```sh
//! scripts/live-test-backend.sh        # prints HERMES_DESKTOP_PORT=<port>
//! HERMES_DESKTOP_PORT=<port> cargo test -p hermes-core --test live_api_<area> -- --ignored --nocapture
//! ```
//!
//! Never point these at the everyday backend: some checks create and delete throwaway
//! items (named with [`tag`]). Checks that would spend model calls, spawn processes, use
//! the network or reveal secrets are not run live at all.
#![allow(dead_code)]

use hermes_core::Client;
use hermes_core::http::Rest;

/// REST + RPC handles to the backend named by `HERMES_DESKTOP_PORT` (required).
pub async fn backend() -> (Rest, Client) {
    assert!(
        std::env::var_os("HERMES_DESKTOP_PORT").is_some(),
        "set HERMES_DESKTOP_PORT to a private test backend (scripts/live-test-backend.sh)"
    );
    let b = hermes_core::backend::discover_or_spawn(None).await.expect("backend");
    let e = &b.endpoint;
    let rest = Rest::new(e.host.clone(), e.port, e.token.clone());
    let (client, _events) = Client::connect(&e.ws_url()).await.expect("websocket");
    // The managed handle must outlive the test; an attached backend is never killed.
    std::mem::forget(b);
    (rest, client)
}

/// Unique suffix for throwaway items created by one run.
pub fn tag() -> String {
    format!("{}", std::process::id())
}

/// Collects named checks; `finish` prints a summary and fails the test if any failed.
#[derive(Default)]
pub struct Checks(Vec<(String, bool, String)>);

impl Checks {
    pub fn check(&mut self, name: &str, ok: bool, detail: impl Into<String>) {
        let detail = detail.into();
        println!("[{}] {name}: {detail}", if ok { "PASS" } else { "FAIL" });
        self.0.push((name.into(), ok, detail));
    }

    /// Record the outcome of a call whose only requirement is that it succeeds.
    pub fn ok<T: std::fmt::Debug, E: std::fmt::Display>(&mut self, name: &str, r: &Result<T, E>) -> bool {
        match r {
            Ok(v) => {
                let mut d = format!("{v:?}");
                if d.len() > 160 {
                    d.truncate(160);
                    d.push('…');
                }
                self.check(name, true, d);
                true
            }
            Err(e) => {
                self.check(name, false, e.to_string());
                false
            }
        }
    }

    pub fn finish(self) {
        let failed: Vec<&str> = self.0.iter().filter(|c| !c.1).map(|c| c.0.as_str()).collect();
        println!("\n{} checks, {} failed", self.0.len(), failed.len());
        assert!(failed.is_empty(), "failed: {failed:?}");
    }
}
