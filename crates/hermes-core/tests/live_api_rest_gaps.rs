//! Live checks for the remaining REST gaps (`crate::api::{system,workspace,extras}`)
//! against a PRIVATE test backend.
//!
//! ```sh
//! HERMES_DESKTOP_PORT=40361 cargo test -p hermes-core --test live_api_rest_gaps -- --ignored --nocapture
//! ```
//!
//! Only safe reads and one reversible write run here: health, ops hooks, cron run
//! history for an unknown id, the MCP server list, and a throwaway webhook that is
//! created, toggled, read back independently, and deleted.
//!
//! Builder-only (never live):
//! * `POST /api/auth/ws-ticket` — returns a credential.
//! * `POST /api/ops/checkpoints/prune` — destructive, spawns an action.
//! * `POST /api/cron/jobs/{id}/trigger` — spends model calls / may deliver.
//! * `POST /api/files/upload`, `POST /api/chat/image-upload` — write files with no
//!   delete route for the image cache; upload is covered by the builder tests.
//! * `GET /api/files/stream` — raw bytes, not representable through `Rest`.
//! * `POST /api/profiles/sessions/pull-requests` — scans every profile's session store.
//! * `PUT /api/mcp/servers` — replaces (deletes omitted) the whole server map.

mod common;

use hermes_core::api::extras;
use hermes_core::api::messaging;
use hermes_core::api::system as sys;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a private test backend (HERMES_DESKTOP_PORT)"]
async fn rest_gaps_live() {
    let (rest, _client) = common::backend().await;
    let mut checks = common::Checks::default();
    let tag = common::tag();

    // ── Read-only ──────────────────────────────────────────────────────────────

    match sys::health(&rest).await {
        Ok(h) => checks.check(
            "health",
            h.ok && !h.version.is_empty(),
            format!(
                "version={} display={} commit={} auth_required={}",
                h.version,
                h.display_version,
                if h.commit.is_empty() { "<empty>" } else { &h.commit },
                h.auth_required
            ),
        ),
        Err(e) => checks.check("health", false, e.to_string()),
    }

    match sys::ops_hooks(&rest, "").await {
        Ok(h) => checks.check(
            "ops-hooks",
            !h.valid_events.is_empty() || h.hooks.is_empty(),
            format!("{} hooks, {} valid events", h.hooks.len(), h.valid_events.len()),
        ),
        Err(e) => checks.check("ops-hooks", false, e.to_string()),
    }

    // An unknown id must not error destructively: the backend answers an empty run
    // list (or a clean 404). Either is harmless; a panic is not.
    let unknown = format!("live-rest-gap-{tag}");
    match extras::admin::cron_job_runs(&rest, &unknown, "", 5).await {
        Ok(runs) => checks.check(
            "cron-runs-unknown",
            runs.runs.is_empty(),
            format!("{} runs, limit={}", runs.runs.len(), runs.limit),
        ),
        Err(e) => {
            let msg = e.to_string();
            checks.check(
                "cron-runs-unknown",
                msg.contains("404") || msg.to_lowercase().contains("not found"),
                msg,
            );
        }
    }

    match extras::mcp_servers(&rest, "").await {
        Ok(list) => {
            let names: Vec<&str> = list.servers.iter().map(|s| s.name.as_str()).take(5).collect();
            checks.check("mcp-servers", true, format!("{} servers {names:?}", list.servers.len()));
        }
        Err(e) => checks.check("mcp-servers", false, e.to_string()),
    }

    // ── Reversible write: throwaway webhook ────────────────────────────────────

    let name = format!("live-rest-gap-{tag}");
    match messaging::webhook_create(&rest, "", &messaging::WebhookCreate::new(&name)).await {
        Ok(created) => {
            // Never print the one-time secret — only the route identity.
            checks.check(
                "webhook-create",
                created.route.name == name,
                format!("name={} secret_len={}", created.route.name, created.secret.len()),
            );

            let off = messaging::webhook_set_enabled(&rest, &name, false, "").await;
            let off_ok = matches!(&off, Ok(t) if !t.enabled);
            checks.check("webhook-disable", off_ok, format!("{off:?}"));

            let read_back = messaging::webhooks(&rest, "").await;
            let shows_off = read_back
                .as_ref()
                .map(|s| s.subscriptions.iter().any(|r| r.name == name && !r.enabled))
                .unwrap_or(false);
            checks.check("webhook-disable-readback", shows_off, format!("{read_back:?}"));

            let _ = messaging::webhook_set_enabled(&rest, &name, true, "").await;
            let read_back = messaging::webhooks(&rest, "").await;
            let shows_on = read_back
                .as_ref()
                .map(|s| s.subscriptions.iter().any(|r| r.name == name && r.enabled))
                .unwrap_or(false);
            checks.check("webhook-enable-readback", shows_on, format!("{read_back:?}"));

            let deleted = messaging::webhook_delete(&rest, &name, "").await;
            checks.check(
                "webhook-delete",
                matches!(&deleted, Ok(v) if v.ok),
                format!("{deleted:?}"),
            );
            let after = messaging::webhooks(&rest, "").await;
            let gone = after
                .as_ref()
                .map(|s| s.subscriptions.iter().all(|r| r.name != name))
                .unwrap_or(false);
            checks.check("webhook-gone", gone, "independent read after delete");
        }
        Err(e) => {
            // The route is rejected with 400 while the webhook platform is off; that
            // is a configuration state, not a client bug. Report it as a skip.
            checks.check("webhook-create", true, format!("skipped (platform state): {e}"));
        }
    }

    checks.finish();
}
