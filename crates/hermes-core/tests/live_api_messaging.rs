//! Live checks for the messaging area against a PRIVATE test backend.
//!
//! ```sh
//! scripts/live-test-backend.sh          # prints the port
//! HERMES_DESKTOP_PORT=<port> cargo test -p hermes-core --test live_api_messaging -- --ignored --nocapture
//! ```
//!
//! Only safe calls run here: platform list (default + a named profile), the no-op
//! platform update (an empty body mutates nothing), pairing status, webhook list,
//! and the two onboarding status reads for an unknown pairing id (a purely local
//! 404 — no network). Creating/enabling/testing a platform, the Telegram/WhatsApp
//! onboarding flows and the webhook enable route are builder-only: they use the
//! network, write credentials or spawn a gateway restart.
//!
//! The webhook round trip (create → toggle → delete) only runs when the webhook
//! platform is already enabled on the test backend, since creating while disabled
//! is refused with 400 and enabling it restarts the gateway.

mod common;

use common::{Checks, backend, tag};
use hermes_core::api::ApiError;
use hermes_core::api::messaging::{self, WebhookCreate};
use hermes_core::http::HttpError;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a private test backend (HERMES_DESKTOP_PORT)"]
async fn messaging_live() {
    let (rest, _client) = backend().await;
    let mut checks = Checks::default();

    // ── Platform catalogue (read-only) ─────────────────────────────
    let platforms = messaging::messaging_platforms(&rest, "").await;
    checks.ok("platforms list (default profile)", &platforms);
    if let Ok(p) = &platforms {
        checks.check(
            "platforms: cards decoded",
            !p.platforms.is_empty() && p.platforms.iter().all(|c| !c.id.is_empty()),
            format!(
                "{} platforms, env_path={}, start={}",
                p.platforms.len(),
                p.env_path,
                p.gateway_start_command
            ),
        );
        let named: Vec<&str> = p
            .platforms
            .iter()
            .filter(|c| !c.env_vars.is_empty())
            .map(|c| c.id.as_str())
            .take(8)
            .collect();
        checks.check(
            "platforms: env-var cards decoded",
            !named.is_empty(),
            format!("with env vars: {}", named.join(", ")),
        );
        let enabled = p.platforms.iter().filter(|c| c.enabled).count();
        println!(
            "note: {} of {} platforms enabled, states: {}",
            enabled,
            p.platforms.len(),
            p.platforms
                .iter()
                .map(|c| c.state.as_str())
                .collect::<Vec<_>>()
                .join(",")
        );
    }

    let scoped = messaging::messaging_platforms(&rest, "swe").await;
    checks.ok("platforms list (profile=swe)", &scoped);
    if let (Ok(a), Ok(b)) = (&platforms, &scoped) {
        checks.check(
            "platforms: profile scope answers",
            !b.platforms.is_empty(),
            format!(
                "default={} swe={} (env_path={})",
                a.platforms.len(),
                b.platforms.len(),
                b.env_path
            ),
        );
    }

    // A platform update with nothing in it: the handler writes no env key, does
    // not touch the enable flag, and only answers ok. Safe to run live.
    let noop = messaging::messaging_platform_update(&rest, "telegram", "", None, &[], &[]).await;
    checks.ok("platform update (empty body, no-op write)", &noop);
    if let Ok(r) = &noop {
        checks.check(
            "platform update: echoes platform + ok",
            r.ok && r.platform == "telegram",
            format!("ok={} platform={} hot_served={:?}", r.ok, r.platform, r.hot_served),
        );
    }

    // ── Pairing (read-only) ────────────────────────────────────────
    let pairing = messaging::pairing(&rest, "").await;
    checks.ok("pairing status (default profile)", &pairing);
    if let Ok(p) = &pairing {
        checks.check(
            "pairing: pending/approved decoded",
            p.pending.iter().all(|u| !u.platform.is_empty()) && p.approved.iter().all(|u| !u.platform.is_empty()),
            format!("{} pending, {} approved", p.pending.len(), p.approved.len()),
        );
    }
    let pairing_swe = messaging::pairing(&rest, "swe").await;
    checks.ok("pairing status (profile=swe)", &pairing_swe);

    // ── Webhooks (read-only, then a round trip when the platform is on) ──
    let state = messaging::webhooks(&rest, "").await;
    checks.ok("webhooks list", &state);
    let mut webhooks_enabled = false;
    if let Ok(s) = &state {
        webhooks_enabled = s.enabled;
        checks.check(
            "webhooks: base_url + routes decoded",
            !s.base_url.is_empty() && s.subscriptions.iter().all(|r| !r.name.is_empty()),
            format!(
                "enabled={} base_url={} {} subscriptions: {}",
                s.enabled,
                s.base_url,
                s.subscriptions.len(),
                s.subscriptions
                    .iter()
                    .map(|r| r.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    }

    if webhooks_enabled {
        let name = format!("api-test-{}", tag());
        let mut body = WebhookCreate::new(&name);
        body.description = Some("hermes-desktop-rs live api test".into());
        body.events = vec!["live-test".into()];
        let created = messaging::webhook_create(&rest, "", &body).await;
        checks.ok("webhook create", &created);
        let secret_len = created.as_ref().map(|c| c.secret.len()).unwrap_or(0);
        checks.check(
            "webhook create: one-time secret returned",
            created
                .as_ref()
                .is_ok_and(|c| !c.secret.is_empty() && c.route.name == name),
            format!("secret of {secret_len} chars"),
        );

        // Independent read proves it landed in the store.
        let after = messaging::webhooks(&rest, "").await;
        checks.check(
            "webhook create: visible in a fresh list",
            after
                .as_ref()
                .is_ok_and(|s| s.subscriptions.iter().any(|r| r.name == name)),
            format!("{after:?}"),
        );

        let off = messaging::webhook_set_enabled(&rest, &name, false, "").await;
        checks.ok("webhook disable", &off);
        let disabled = messaging::webhooks(&rest, "").await;
        checks.check(
            "webhook disable: persisted",
            disabled
                .as_ref()
                .is_ok_and(|s| s.subscriptions.iter().any(|r| r.name == name && !r.enabled)),
            format!("{disabled:?}"),
        );
        let on = messaging::webhook_set_enabled(&rest, &name, true, "").await;
        checks.ok("webhook re-enable", &on);

        let deleted = messaging::webhook_delete(&rest, &name, "").await;
        checks.ok("webhook delete (explicit ?profile=default)", &deleted);
        let gone = messaging::webhooks(&rest, "").await;
        checks.check(
            "webhook delete: gone",
            gone.as_ref()
                .is_ok_and(|s| !s.subscriptions.iter().any(|r| r.name == name)),
            format!("{gone:?}"),
        );
    } else {
        // Refused before any write, so this exercises the route without mutating
        // the store. The full round trip is not run: enabling the platform is
        // POST /api/webhooks/enable, which restarts the gateway (spawns a process).
        let refused = messaging::webhook_create(&rest, "", &WebhookCreate::new(format!("api-test-{}", tag()))).await;
        checks.check(
            "webhook create refused while platform disabled",
            matches!(
                &refused,
                Err(ApiError::Http(HttpError::Status { status: 400, body })) if body.contains("not enabled")
            ),
            format!("{refused:?}"),
        );
        println!(
            "note: webhook create/toggle/delete round trip NOT RUN — the test backend's webhook platform is \
             disabled and enabling it (POST /api/webhooks/enable) restarts the gateway, which the live rules forbid"
        );
    }

    // ── Onboarding status for an unknown id: a local 404, no network ──
    match messaging::telegram_onboarding_status(&rest, "no-such-pairing", "").await {
        Err(ApiError::Http(HttpError::Status { status, body })) => checks.check(
            "telegram onboarding status unknown id -> 404",
            status == 404,
            format!("{status}: {body}"),
        ),
        other => checks.check(
            "telegram onboarding status unknown id -> 404",
            false,
            format!("{other:?}"),
        ),
    }
    match messaging::telegram_onboarding_cancel(&rest, "no-such-pairing", "").await {
        Ok(r) => checks.check("telegram onboarding cancel unknown id -> ok", r.ok, format!("{r:?}")),
        Err(e) => checks.check("telegram onboarding cancel unknown id -> ok", false, e.to_string()),
    }
    match messaging::whatsapp_onboarding_status(&rest, "no-such-pairing", "").await {
        Err(ApiError::Http(HttpError::Status { status, body })) => checks.check(
            "whatsapp onboarding status unknown id -> 404",
            status == 404,
            format!("{status}: {body}"),
        ),
        other => checks.check(
            "whatsapp onboarding status unknown id -> 404",
            false,
            format!("{other:?}"),
        ),
    }
    match messaging::whatsapp_onboarding_cancel(&rest, "no-such-pairing", "").await {
        Ok(r) => checks.check("whatsapp onboarding cancel unknown id -> ok", r.ok, format!("{r:?}")),
        Err(e) => checks.check("whatsapp onboarding cancel unknown id -> ok", false, e.to_string()),
    }

    checks.finish();
}
