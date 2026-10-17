//! Model widgets → `ui/w-models.slint`.
//!
//! Owned ids: `model.main` (default model picker), `model.auxiliary` (per-task models),
//! `model.moa` (Mixture of Agents), `fallback_providers` (the Fallback Models row whose
//! `Control::Fallback` kind renders as a list editor instead of a text field).
//!
//! Backend data (payloads in `~/.hermes/cache/scratch/settings-api/`):
//! * `GET /api/model/options` → `{providers: [{slug, name, is_current, is_user_defined,
//!   models[], authenticated?, aliases[], featured_models[], …}]}`
//! * `POST /api/model/set` → set the default model (check the dashboard/desktop callers
//!   for the exact body: provider + model + profile).
//! * `GET /api/model/auxiliary` → `{tasks: [{task, provider, model, base_url,
//!   reasoning_effort, local_endpoint}]}`; the write path is the `auxiliary.*` config keys
//!   (verify in the Electron `aux-task-rows.tsx`).
//! * `GET|PUT /api/model/moa` → presets/reference_models/aggregator/fanout/… (PUT takes
//!   the same shape back).
//! * `fallback_providers` is config: a list of `{provider, model}` — read/write with
//!   `ctx.config()` / `ctx.config_set("fallback_providers", …)`.
//!
//! Action ids this module must handle:
//! * `models:pick:<provider>:<model>` — set the default model.
//! * `models:aux:<task>:<provider>:<model>` — set one auxiliary task's model.
//! * `models:aux-clear:<task>` — reset a task to `auto`.
//! * `models:moa:<verb>[:<arg>]` — reference models, aggregator, enabled, fanout.
//! * `models:fb:<add|remove|move-up|move-down>[:<index>]` — fallback list editing.
//!
//! TODO(models): implement. Keep `owns()` authoritative.

use crate::widgets::{Ctx, Outcome};

/// Widget ids this module draws.
pub(crate) fn owns(id: &str) -> bool {
    matches!(
        id,
        "model.main" | "model.auxiliary" | "model.moa" | "fallback_providers"
    )
}

/// Load (or reload) one widget's data into `ModelsState`.
pub(crate) async fn load(id: &str, ctx: &Ctx) {
    let _ = (id, ctx);
}

/// One `models:<verb>[:<arg>]` action from the UI.
pub(crate) async fn action(rest: &str, ctx: &Ctx) -> Outcome {
    let _ = (rest, ctx);
    Outcome::None
}
