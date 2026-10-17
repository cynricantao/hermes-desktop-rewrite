//! Session / gateway / about / plugin / billing widgets → `ui/w-system.slint`.
//!
//! Owned ids: `sessions.archived-list`, `gateway.mode`, `gateway.devices`,
//! `gateway.updates`, `billing.overview`, `keybinds.list`, `plugins.list`,
//! `about.version`, `about.uninstall`.
//!
//! Backend data (payloads in `~/.hermes/cache/scratch/settings-api/`):
//! * `GET /api/sessions?...` → session list rows; `GET /api/sessions/stats` → `{total,
//!   active_store, archived, messages}`; `DELETE /api/sessions/empty` and
//!   `POST /api/sessions/prune` clean up; archive/restore per session with
//!   `PATCH|DELETE /api/sessions/{id}` (check the params the dashboard sends).
//! * `GET /api/status` → `{version, release_date, gateway_running, gateway_state,
//!   can_update_hermes}`; `GET /api/host/identity` → `{ok, protocolVersion, pid, role}`;
//!   `GET /api/hermes/update/check` → `{install_method, current_version, behind,
//!   update_available, can_apply, update_command, message}`; `POST /api/hermes/update`
//!   applies it; `POST /api/gateway/{restart|start|stop|drain}` drives the gateway.
//! * `GET /api/portal` → `{logged_in, portal_url, account_tier, free_tier, features[]}` and
//!   `GET /api/analytics/usage` → `{totals, by_model[], daily[], period_days}` for the
//!   billing/usage page.
//! * `GET /api/dashboard/plugins` → `[{name, label, description, version, enabled?…}]`;
//!   enable/disable/visibility under the same prefix.
//! * Keybindings have no backend: read the real bindings out of the app source
//!   (`crates/hermes-ui/src/controller.rs`, `crates/hermes-app/src/main.rs`,
//!   `crates/hermes-ui/ui/*.slint`) and render exactly what the app supports — no
//!   invented rows, and say so where the Electron app has a feature this client does not.
//!
//! Action ids this module must handle:
//! * `sessions-refresh`, `sessions-restore:<id>`, `sessions-delete:<id>`, `sessions-prune`,
//!   `sessions-empty-clean`
//! * `gateway-restart`, `gateway-refresh`, `conn-save`, `conn-forget:<id>`, `conn-use:<id>`
//! * `about-refresh`, `update-apply`, `uninstall-reveal`
//! * `plugins-refresh`, `plugin-toggle:<name>[:enable|disable]`, `plugins-rescan`
//! * `billing-refresh`
//!
//! TODO(system): implement. Keep `owns()` authoritative.

use crate::widgets::{Ctx, Outcome};

/// Widget ids this module draws.
pub(crate) fn owns(id: &str) -> bool {
    matches!(
        id,
        "sessions.archived-list"
            | "gateway.mode"
            | "gateway.devices"
            | "gateway.updates"
            | "billing.overview"
            | "keybinds.list"
            | "plugins.list"
            | "about.version"
            | "about.uninstall"
    )
}

/// Load (or reload) one widget's data into `SystemState`.
pub(crate) async fn load(id: &str, ctx: &Ctx) {
    let _ = (id, ctx);
}

/// One `system:<verb>[:<arg>]` action from the UI.
pub(crate) async fn action(rest: &str, ctx: &Ctx) -> Outcome {
    let _ = (rest, ctx);
    Outcome::None
}
