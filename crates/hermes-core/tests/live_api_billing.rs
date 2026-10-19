//! Live check for the billing area against a PRIVATE test backend.
//!
//! ```sh
//! scripts/live-test-backend.sh        # prints HERMES_DESKTOP_PORT=<port>
//! HERMES_DESKTOP_PORT=<port> cargo test -p hermes-core --test live_api_billing -- --ignored --nocapture
//! ```
//!
//! # What runs live, and why so little
//! Every `billing.*`, `subscription.*` and `free_tier.*` method except one reaches the Nous portal,
//! and several of them **spend money or change the plan**, so they are builder + unit test only
//! (`crates/hermes-core/src/api/billing.rs`). The same goes for `onboarding.set_run`, which writes
//! the root `config.yaml`.
//!
//! This file therefore calls exactly one billing-area RPC, chosen because its handler proves it is
//! local and side-effect free (`tui_gateway/methods_free_tier.py`: "``free_tier.status`` answers
//! from the auth store with zero network and zero side effects… an identity is never created as a
//! side effect of polling this"):
//!
//! * `free_tier.status` — local auth-store read only.
//!
//! Builder-only (reason): `billing.state`, `billing.charge`, `billing.charge_status`,
//! `billing.auto_reload`, `billing.step_up`, `subscription.state`, `subscription.preview`,
//! `subscription.change`, `subscription.resume`, `subscription.upgrade` (all round-trip the portal;
//! charge / auto_reload / change / resume / upgrade move money or the plan), `free_tier.provision`
//! (claims the welcome trial over the network), `free_tier.claim_nudge` / `free_tier.ack_notice` /
//! `free_tier.challenge_result` (one-shot local writes the test must not consume),
//! `onboarding.set_run` (writes config).
//!
//! `api::profiles_active` is called first as a harmless reachability check on the same backend.

use hermes_core::api::billing;

mod common;

#[tokio::test]
#[ignore]
async fn billing_area_live() {
    let (rest, client) = common::backend().await;
    let mut checks = common::Checks::default();

    checks.ok(
        "profiles_active (reachability)",
        &hermes_core::api::profiles_active(&rest).await,
    );

    match billing::free_tier_status(&client, "").await {
        Ok(s) => {
            // A fresh test backend has no guest identity, and polling must not create one.
            checks.check(
                "free_tier.status",
                !s.model.is_empty(),
                format!(
                    "has_guest={} enabled={} available={} notice_pending={} model={:?} label={:?} challenge={}",
                    s.has_guest,
                    s.enabled,
                    s.available,
                    s.notice_pending,
                    s.model,
                    s.label,
                    s.challenge.is_some()
                ),
            );
            if s.enabled && !s.has_guest {
                println!(
                    "[info] free_tier.status: claimable, last mint failure {:?} / {:?}",
                    s.error, s.error_code
                );
            }
        }
        Err(e) => checks.check("free_tier.status", false, e.to_string()),
    }

    checks.finish();
}
