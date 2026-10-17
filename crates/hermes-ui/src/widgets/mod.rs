//! Settings-page widgets: the pages whose content is a small panel with its own backend
//! data (palette grid, model picker, provider keys, archived chats, …) instead of a single
//! config key that the generic controls render.
//!
//! # Contract
//!
//! Every widget id in the settings registry that is *not* a plain control is owned by
//! exactly one module here. The mapping is the single source of truth for three places:
//!
//! * `ui/settings.slint` — which `<Area>Widget` component draws the row (`visible:` from
//!   the same id lists, see the `w-...-owned` functions there);
//! * the controller — `load_widgets()` calls [`load`] for every owned id on the open page
//!   after each publish, and `settings_action()` routes `<area>:<verb>[:arg]` ids to
//!   [`action`];
//! * this file — [`owns`] must agree with both.
//!
//! | area        | ids                                                                    |
//! |-------------|------------------------------------------------------------------------|
//! | `appearance`| `hermes-desktop-theme-v2`, `desktop.font_family`, `terminal.font_family`, `pet` |
//! | `models`    | `model.main`, `model.auxiliary`, `model.moa`, `fallback_providers`      |
//! | `providers` | `providers.accounts`, `.keys`, `.custom`, `.local`, `keys.tools`, `keys.settings`, `vault.credentials`, `vault.sources` |
//! | `system`    | `sessions.archived-list`, `gateway.mode`, `.devices`, `.updates`, `billing.overview`, `keybinds.list`, `plugins.list`, `about.version`, `about.uninstall` |
//!
//! # Rules
//!
//! * UI → back end only through `act("<area>:<verb>[:<arg>]")` (wired in `settings.slint`
//!   to `SettingsState.action`), never through a second callback.
//! * Back end → UI only by filling the module's own Slint global
//!   (`ui/w-<area>.slint`), pushed with [`Ctx::ui`] so it lands on the UI thread.
//! * A module never writes another module's global and never touches the shared
//!   `SettingsState`; anything the panel itself must do comes back as an [`Outcome`].
//! * Values written to the profile config go through [`Ctx::config_set`], which applies
//!   the same sparse-patch shape as the generic rows (`PUT /api/config?profile=`).
//! * Everything shown must come from the backend (or from `prefs.json`); no invented
//!   data, and a failure renders as an inline message, not a silent empty list.
//!
//! The contract deliberately carries one shape for four areas: while the build-out is in
//! progress some helpers are unused, so this module opts out of dead-code noise (the
//! `load`/`action` entry points and `owns()` are what the rest of the crate calls).
#![allow(dead_code)]

pub(crate) mod appearance;
pub(crate) mod models;
pub(crate) mod providers;
pub(crate) mod system;

use hermes_core::http::Rest;
use serde_json::Value;
use slint::Weak;

use crate::AppWindow;

/// Action-id prefixes routed to a module. Keep in sync with `action()`.
pub(crate) const AREAS: &[&str] = &["appearance", "models", "providers", "system"];

/// Everything a widget module needs from the controller: the REST handle to the running
/// backend, a way to reach the UI thread, and the open profile.
pub(crate) struct Ctx {
    pub rest: Rest,
    pub weak: Weak<AppWindow>,
    /// Profile whose pages are open ("" = the default profile).
    pub scope: String,
}

impl Ctx {
    /// `?profile=` for the open scope, "" for the default profile.
    pub(crate) fn profile_query(&self) -> String {
        hermes_core::http::profile_query(&self.scope)
    }

    /// Run `f` on the UI thread with the window (no-op if it is gone).
    pub(crate) fn ui(&self, f: impl FnOnce(&AppWindow) + Send + 'static) {
        crate::controller::ui(&self.weak, f);
    }

    pub(crate) async fn get_json(&self, path: &str) -> Result<Value, String> {
        self.rest.get(path).await.map_err(|e| e.to_string())
    }

    pub(crate) async fn post_json(&self, path: &str, body: &Value) -> Result<Value, String> {
        self.rest.post(path, body).await.map_err(|e| e.to_string())
    }

    pub(crate) async fn put_json(&self, path: &str, body: &Value) -> Result<Value, String> {
        self.rest.put(path, body).await.map_err(|e| e.to_string())
    }

    pub(crate) async fn delete_json(&self, path: &str) -> Result<Value, String> {
        self.rest.delete(path).await.map_err(|e| e.to_string())
    }

    pub(crate) async fn patch_json(&self, path: &str, body: &Value) -> Result<Value, String> {
        self.rest.patch(path, body).await.map_err(|e| e.to_string())
    }

    /// The scoped profile's config, as the settings pages see it.
    pub(crate) async fn config(&self) -> Result<Value, String> {
        self.get_json(&format!("/api/config{}", self.profile_query())).await
    }

    /// Write one dotted config key through the same sparse patch the generic rows use.
    pub(crate) async fn config_set(&self, dotted: &str, value: Value) -> Result<(), String> {
        let body = serde_json::json!({ "config": hermes_core::settings::sparse_patch(dotted, value) });
        self.put_json(&format!("/api/config{}", self.profile_query()), &body)
            .await
            .map(|_| ())
    }
}

/// What the settings panel should do after a widget call. Widgets keep their own state;
/// this is only for the pieces the panel owns.
pub(crate) enum Outcome {
    None,
    /// Show an inline success note.
    Notice(String),
    /// Show an inline error note.
    Error(String),
    /// Route through the panel's normal edit path (`edit_setting`): for widget controls
    /// that write a plain registry setting, e.g. the font pickers → `desktop.font_family`.
    /// Value is in the control's own text form.
    Edit(String, String),
    /// The profile config changed underneath (or this page's data feeds its rows): reload
    /// the scoped config, republish the rows and re-load this page's widgets.
    Reload,
}

/// Is `id` a widget (i.e. owned by one of the modules above)? Used by the controller to
/// pick the ids to load for the open page.
pub(crate) fn owns(id: &str) -> bool {
    appearance::owns(id) || models::owns(id) || providers::owns(id) || system::owns(id)
}

/// The area that draws `id` ("" when no module owns it). The UI switches on this string
/// to pick the widget component for the row (`SettingRowUI.area`).
pub(crate) fn area(id: &str) -> &'static str {
    if appearance::owns(id) {
        "appearance"
    } else if models::owns(id) {
        "models"
    } else if providers::owns(id) {
        "providers"
    } else if system::owns(id) {
        "system"
    } else {
        ""
    }
}

/// Apply one stored device pref that belongs to a widget panel. Called by the controller's
/// `apply_device_pref` on every edit and once per saved pref at startup, so live edits and
/// a fresh start always agree.
pub(crate) fn apply_pref(ui: &AppWindow, key: &str, value: &str) {
    appearance::apply_pref(ui, key, value);
}

/// Fill the Slint state of one widget on the open page. Called after every publish.
pub(crate) async fn load(id: &str, ctx: &Ctx) {
    if appearance::owns(id) {
        appearance::load(id, ctx).await;
    } else if models::owns(id) {
        models::load(id, ctx).await;
    } else if providers::owns(id) {
        providers::load(id, ctx).await;
    } else if system::owns(id) {
        system::load(id, ctx).await;
    }
}

/// One action from a widget, in the `"<area>:<verb>[:<arg>]"` shape. `id` may carry several
/// colon-separated arguments; modules split them themselves.
pub(crate) async fn action(id: &str, ctx: &Ctx) -> Outcome {
    let (area, rest) = id.split_once(':').unwrap_or((id, ""));
    match area {
        "appearance" => appearance::action(rest, ctx).await,
        "models" => models::action(rest, ctx).await,
        "providers" => providers::action(rest, ctx).await,
        "system" => system::action(rest, ctx).await,
        _ => Outcome::None,
    }
}

#[cfg(test)]
mod tests {
    use hermes_core::settings::{Control, SETTINGS};

    /// Every row that renders as a widget (or as the fallback list editor) must have a
    /// module behind it, or the panel would show a label with nothing under it.
    #[test]
    fn every_widget_row_has_a_module() {
        for s in SETTINGS {
            if matches!(s.control, Control::Widget(_) | Control::Fallback) {
                assert!(
                    super::owns(s.id),
                    "no widget module owns `{}` ({} / {})",
                    s.id,
                    s.group,
                    s.page
                );
                assert!(!super::area(s.id).is_empty(), "`{}` has no area", s.id);
            }
        }
    }

    /// The action-id prefixes the UI emits must map back to a module.
    #[test]
    fn area_prefixes_route() {
        for area in super::AREAS {
            assert!(
                matches!(*area, "appearance" | "models" | "providers" | "system"),
                "{area}"
            );
        }
    }
}
