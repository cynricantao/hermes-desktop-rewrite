//! Live checks for the models/providers area (`crates/hermes-core/src/api/models.rs`).
//!
//! ```sh
//! scripts/live-test-backend.sh            # prints HERMES_DESKTOP_PORT=<port>
//! HERMES_DESKTOP_PORT=<port> cargo test -p hermes-core --test live_api_models -- --ignored --nocapture
//! ```
//!
//! Only local reads are exercised here. **Builder-only, never run live** (each one writes
//! config, spends or reveals a credential, downloads, spawns a process or deletes a file):
//!
//! * `POST /api/model/set`, `PUT /api/model/moa` — write the profile's `config.yaml`.
//! * `POST /api/providers/validate`, `/custom-endpoints/validate` — contact the provider /
//!   the user's endpoint and send the key to it.
//! * `POST /api/providers/oauth/{id}/start`, `/submit`, `DELETE .../sessions/{id}`,
//!   `DELETE /api/providers/oauth/{id}` — network to the provider, save or delete credentials.
//! * `POST|DELETE /api/providers/custom-endpoints` (+`/activate`) — write config and `.env`.
//! * `POST /api/local-models/{download,download-browsed,quickstart,runtime/install,sideload}`
//!   and `/models/{id}` DELETE — Hugging Face/mainline downloads, process spawn, file deletes.
//! * `POST /api/local-models/{activate,eject,server,download/pause,download/resume}` — config
//!   write / GPU memory / process control.
//! * `GET /api/local-models/{catalog,search,search/files}` — `catalog` kicks a background
//!   catalog refresh over the network; the other two query huggingface.co directly.
//! * RPCs `model.disconnect` (destroys credentials) and `model.save_key` (writes a secret).

mod common;

use common::{Checks, backend};
use hermes_core::api::models;

#[tokio::test]
#[ignore = "needs a private test backend: HERMES_DESKTOP_PORT=<port>"]
async fn live_models_reads() {
    let (rest, client) = backend().await;
    let mut c = Checks::default();

    // GET /api/model/info — resolved model metadata. `capabilities` may be empty.
    match models::model_info(&rest, "").await {
        Ok(info) => {
            c.check(
                "model/info",
                info.effective_context_length >= 0 && (info.model.is_empty() || !info.provider.is_empty()),
                format!(
                    "model={:?} provider={:?} auto_ctx={} cfg_ctx={} eff_ctx={} caps_family={:?}",
                    info.model,
                    info.provider,
                    info.auto_context_length,
                    info.config_context_length,
                    info.effective_context_length,
                    info.capabilities.model_family
                ),
            );
        }
        Err(e) => c.check("model/info", false, e.to_string()),
    }

    // GET /api/model/options — the picker inventory (cached read, no refresh).
    match models::model_options(&rest, "", false, false, false).await {
        Ok(o) => {
            let with_models = o.providers.iter().filter(|p| !p.model_ids().is_empty()).count();
            c.check(
                "model/options",
                true,
                format!(
                    "current={:?}/{:?} providers={} with_models={}",
                    o.provider,
                    o.model,
                    o.providers.len(),
                    with_models
                ),
            );
            // The shared protocol type is what the picker/rows code consumes.
            let first = o.providers.first();
            c.check(
                "model/options rows",
                first.is_none_or(|p| !p.slug.is_empty()),
                format!("first_slug={:?}", first.map(|p| p.slug.as_str())),
            );
        }
        Err(e) => c.check("model/options", false, e.to_string()),
    }

    // Same payload over the gateway RPC.
    match models::model_options_rpc(&client, "", false, false, false, "").await {
        Ok(o) => c.check(
            "model.options rpc",
            !o.providers.is_empty() || o.model.is_empty(),
            format!("providers={} model={:?}", o.providers.len(), o.model),
        ),
        Err(e) => c.check("model.options rpc", false, e.to_string()),
    }

    // GET /api/model/auxiliary — built-in slots always come first.
    match models::auxiliary_models(&rest, "").await {
        Ok(a) => c.check(
            "model/auxiliary",
            !a.tasks.is_empty() && a.tasks.iter().all(|t| !t.task.is_empty()),
            format!("tasks={} main={:?}/{:?}", a.tasks.len(), a.main.provider, a.main.model),
        ),
        Err(e) => c.check("model/auxiliary", false, e.to_string()),
    }

    // GET /api/model/moa — normalized config; `default_preset` is never empty.
    match models::moa(&rest, "").await {
        Ok(m) => c.check(
            "model/moa",
            !m.default_preset.is_empty() && m.presets.contains_key(&m.default_preset),
            format!(
                "default={:?} active={:?} presets={:?}",
                m.default_preset,
                m.active_preset,
                m.presets.keys().collect::<Vec<_>>()
            ),
        ),
        Err(e) => c.check("model/moa", false, e.to_string()),
    }

    // GET /api/model/recommended-default — `model` may legitimately be empty.
    match models::recommended_default(&rest, "", "").await {
        Ok(d) => c.check(
            "model/recommended-default",
            d.provider.is_empty() && d.model.is_empty() && d.free_tier.is_none(),
            format!(
                "provider={:?} model={:?} free_tier={:?}",
                d.provider, d.model, d.free_tier
            ),
        ),
        Err(e) => c.check("model/recommended-default", false, e.to_string()),
    }

    // GET /api/providers/custom-endpoints — config read; previews are masked.
    match models::custom_endpoints(&rest, "").await {
        Ok(list) => c.check(
            "providers/custom-endpoints",
            list.endpoints
                .iter()
                .all(|e| !e.id.is_empty() && !e.base_url.is_empty()),
            format!("endpoints={}", list.endpoints.len()),
        ),
        Err(e) => c.check("providers/custom-endpoints", false, e.to_string()),
    }

    // GET /api/providers/oauth — credential-store status reads (no OAuth refresh).
    match models::oauth_providers(&rest, "").await {
        Ok(list) => {
            let logged_in = list.providers.iter().filter(|p| p.status.logged_in).count();
            c.check(
                "providers/oauth",
                !list.providers.is_empty() && list.providers.iter().all(|p| !p.id.is_empty()),
                format!("providers={} logged_in={}", list.providers.len(), logged_in),
            );
        }
        Err(e) => c.check("providers/oauth", false, e.to_string()),
    }

    // GET /api/local-models/jobs — in-memory job table, no network.
    match models::local_jobs(&rest).await {
        Ok(j) => c.check(
            "local-models/jobs",
            j.jobs.iter().all(|job| !job.job_id.is_empty()),
            format!("jobs={}", j.jobs.len()),
        ),
        Err(e) => c.check("local-models/jobs", false, e.to_string()),
    }

    // A forgotten job id is a 404, not a decode failure.
    match models::local_job(&rest, "live-api-models-missing").await {
        Ok(j) => c.check("local-models/jobs/{id} 404", false, format!("unexpected job {j:?}")),
        Err(e) => c.check(
            "local-models/jobs/{id} 404",
            e.to_string().contains("404"),
            e.to_string(),
        ),
    }

    c.finish();
}
