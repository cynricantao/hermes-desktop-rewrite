//! Appearance widgets → `ui/w-appearance.slint`.
//!
//! Owned ids: `hermes-desktop-theme-v2` (palette grid), `desktop.font_family` (chat font),
//! `terminal.font_family` (terminal font), `pet`.
//!
//! Backend data (verified against a live backend, payloads in
//! `~/.hermes/cache/scratch/settings-api/`):
//! * `GET /api/dashboard/themes` → `{themes: [{name, label, description}], active}`
//! * `PUT /api/dashboard/theme` → body `{theme: name}` (verify against the dashboard code)
//! * `GET /api/config` → `desktop.font_family`, `terminal.font_family`
//!
//! Action ids this module must handle (the UI side emits them via `act(...)`):
//! * `appearance:palette:<name>` — pick a palette.
//! * `appearance:font:<chat|terminal>:<family>` — set a font (empty family = theme font).
//! * `appearance:pet:<slug>` / `appearance:pet-refresh` — pet page.
//!
//! TODO(appearance): implement. Keep `owns()` authoritative — the controller and
//! `settings.slint` both rely on it.

use crate::widgets::{Ctx, Outcome};

/// Widget ids this module draws.
pub(crate) fn owns(id: &str) -> bool {
    matches!(
        id,
        "hermes-desktop-theme-v2" | "desktop.font_family" | "terminal.font_family" | "pet"
    )
}

/// Apply one stored device pref to the live window: the palette (`hermes-desktop-theme-v2`)
/// is here, so an edit in the panel and a fresh start look identical. Called from
/// `widgets::apply_pref` (controller `apply_device_pref`).
pub(crate) fn apply_pref(ui: &crate::AppWindow, key: &str, value: &str) {
    let _ = (ui, key, value);
}

/// Load (or reload) one widget's data into `AppearanceState`.
pub(crate) async fn load(id: &str, ctx: &Ctx) {
    let _ = (id, ctx);
}

/// One `appearance:<verb>[:<arg>]` action from the UI.
pub(crate) async fn action(rest: &str, ctx: &Ctx) -> Outcome {
    let _ = (rest, ctx);
    Outcome::None
}
