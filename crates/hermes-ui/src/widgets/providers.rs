//! Provider / keys / vault widgets → `ui/w-providers.slint`.
//!
//! Owned ids: `providers.accounts`, `providers.keys`, `providers.custom`,
//! `providers.local`, `keys.tools`, `keys.settings`, `vault.credentials`,
//! `vault.sources`.
//!
//! Backend data (payloads in `~/.hermes/cache/scratch/settings-api/`):
//! * `GET /api/providers/oauth` → `{providers: [{id, name, flow: device_code|external,
//!   cli_command, docs_url, disconnectable, status: {logged_in, source_label,
//!   token_preview, expires_at…}}]}`; sign-in: `POST /api/providers/oauth/{id}/start` →
//!   device code, then `GET /api/providers/oauth/{id}/poll/{session_id}` (see the
//!   dashboard's own flow for the exact bodies); disconnect: `DELETE
//!   /api/providers/oauth/{id}`.
//! * `GET /api/env` → every credential env var: `{KEY: {is_set, redacted_value,
//!   description, url, category, is_password, provider, provider_label,
//!   provider_profiles[], advanced, custom, …}}`. Write: `PUT /api/env`, delete:
//!   `DELETE /api/env`, reveal: `POST /api/env/reveal` (check the dashboard callers for
//!   the body shape). The providers page filters to provider rows; the keys page shows
//!   the rest.
//! * `GET /api/providers/custom-endpoints` → `{endpoints: [...], current: {...}}`;
//!   add/validate/activate/delete under the same prefix.
//! * `GET /api/local-models/status` + `/api/local-models/hardware` → runtime, models dir,
//!   active model, GPU/RAM facts; `POST /api/local-models/server` starts/stops the server,
//!   `/activate`, `/eject` pick a model.
//! * `GET /api/tools/toolsets` → `[{name, label, description, platform, enabled,
//!   available, configured, tools[]}]`; toggle with `PUT /api/tools/toolsets/{name}`.
//! * `GET /api/credentials/pool` → `{providers: [{provider, entries: [{index, id, label,
//!   auth_type, source, priority, last_status, request_count, token_preview,
//!   has_refresh}]}]}` for the vault page; `DELETE /api/credentials/pool/{provider}/{index}`.
//!
//! Action ids this module must handle (UI emits `<area>:<verb>[:<arg>]`, this module gets
//! the part after the first colon):
//! * `oauth-start:<provider>`, `oauth-poll:<provider>:<session-id>`, `oauth-submit:<provider>:<code>`,
//!   `oauth-disconnect:<provider>`, `oauth-refresh`
//! * `env-set:<KEY>`, `env-del:<KEY>`, `env-reveal:<KEY>`, `env-refresh`
//! * `ce-add`, `ce-delete:<id>`, `ce-validate:<id>`, `ce-activate:<id>`, `ce-refresh`
//! * `lm-server:<start|stop>`, `lm-activate:<model-id>`, `lm-eject`, `lm-refresh`
//! * `tools-toggle:<name>`, `tools-refresh`
//! * `vault-refresh`, `vault-del:<provider>:<index>`
//!
//! TODO(providers): implement. Keep `owns()` authoritative.

use crate::widgets::{Ctx, Outcome};

/// Widget ids this module draws.
pub(crate) fn owns(id: &str) -> bool {
    matches!(
        id,
        "providers.accounts"
            | "providers.keys"
            | "providers.custom"
            | "providers.local"
            | "keys.tools"
            | "keys.settings"
            | "vault.credentials"
            | "vault.sources"
    )
}

/// Load (or reload) one widget's data into `ProvidersState`.
pub(crate) async fn load(id: &str, ctx: &Ctx) {
    let _ = (id, ctx);
}

/// One `providers:<verb>[:<arg>]` action from the UI.
pub(crate) async fn action(rest: &str, ctx: &Ctx) -> Outcome {
    let _ = (rest, ctx);
    Outcome::None
}
