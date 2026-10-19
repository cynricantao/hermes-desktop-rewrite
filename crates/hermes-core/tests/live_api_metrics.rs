//! Live checks for the shared-metrics area (`shared_metrics.*` RPCs) against a PRIVATE test
//! backend.
//!
//! ```sh
//! scripts/live-test-backend.sh    # prints HERMES_DESKTOP_PORT=<port>
//! HERMES_DESKTOP_PORT=<port> cargo test -p hermes-core --test live_api_metrics -- --ignored --nocapture
//! ```
//!
//! Only `shared_metrics.status` runs live: it is a pure read of the profile's `config.yaml`.
//! Everything else is builder/unit-tested only:
//!
//! * `shared_metrics.set` — **changes the user's telemetry consent** in `config.yaml` (and, when
//!   turning collection off, purges onboarding latches and pending upload receipts).
//! * `shared_metrics.desktop_*`, `shared_metrics.slash_command`, `shared_metrics.startup_latency`,
//!   `shared_metrics.update_run` — record telemetry that a later daily upload can carry off the
//!   machine. The handlers gate every write on the profile's collection flag
//!   (`shared_metrics_events._emit` → `relay_shared_metrics.enabled()`), so on an opted-out test
//!   backend they are no-ops — but the area's live scope is the consent read only, so they stay
//!   builder/unit-tested rather than exercised here.
//!
//! The `status` check never prints secrets (the reply is four booleans) and never writes.

mod common;

use common::{Checks, backend};
use hermes_core::api::metrics;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a private test backend (HERMES_DESKTOP_PORT)"]
async fn metrics_area_live() {
    let (_rest, client) = backend().await;
    let mut checks = Checks::default();

    // ── Consent state: a pure read. `send` must never be true while `enabled` is false (the
    // backend forces send off with collection); `decided`/`reask` are independent flags.
    match metrics::status(&client, "").await {
        Ok(c) => {
            checks.check(
                "RPC shared_metrics.status",
                !c.send || c.enabled,
                format!(
                    "enabled={} send={} decided={} reask={}",
                    c.enabled, c.send, c.decided, c.reask
                ),
            );
            // A reask is by definition not a settled answer; the UI relies on this pair.
            checks.check(
                "RPC shared_metrics.status flags",
                !(c.decided && c.reask),
                format!("decided={} reask={}", c.decided, c.reask),
            );
        }
        Err(e) => checks.check("RPC shared_metrics.status", false, e.to_string()),
    }

    checks.finish();
}
