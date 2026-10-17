//! Appearance widgets → `ui/w-appearance.slint`.
//!
//! Owned ids: `hermes-desktop-theme-v2` (palette grid), `desktop.font_family` (chat font),
//! `terminal.font_family` (terminal font), `pet`.
//!
//! Backend data (verified against a private live backend; payloads in
//! `~/.hermes/cache/scratch/settings-api/`):
//! * `GET /api/dashboard/themes` → `{themes: [{name, label, description}], active}`
//! * `PUT /api/dashboard/theme` → body `{name}` (the server model is `ThemeSetBody.name`;
//!   `hermes_cli/web_routers/dashboard_ui.py`, and the dashboard's own client sends
//!   `{name}` — the brief's `{theme}` shape is wrong).
//! * `GET /api/config` → `desktop.font_family`, `terminal.font_family`
//!
//! Action ids handled here (the UI emits them via `act(...)`):
//! * `appearance:palette:<name>` — pick a dashboard palette.
//! * `appearance:font:<chat|terminal>:<family>` — set a font (empty family = theme font).
//! * `appearance:font-q:<chat|terminal>:<query>` — filter the installed-font list.

use std::collections::HashSet;
use std::sync::OnceLock;

use serde_json::Value;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::widgets::{Ctx, Outcome};
use crate::{AppWindow, AppearanceState, FontFamily, PaletteCard, Theme};

/// Palette device pref; also the registry row id for that widget.
const PALETTE_KEY: &str = "hermes-desktop-theme-v2";
/// Chat / UI font — a profile config key.
const CHAT_FONT_KEY: &str = "desktop.font_family";
/// Terminal font — a profile config key.
const TERMINAL_FONT_KEY: &str = "terminal.font_family";

/// Dashboard themes this client also renders locally (see `theme.slint`). They are exactly
/// the four presets the Electron desktop shares (`apps/shared/src/theme-presets.ts`);
/// every other theme switches on the backend only and the panel says so.
const LOCAL_PALETTES: &[&str] = &["midnight", "ember", "mono", "cyberpunk"];

/// Palette grid columns. Positions are computed here so Slint needs no integer division.
const GRID_COLS: i32 = 3;
/// Most families pushed to the picker at once (the system can have 600+).
const FONT_LIMIT: usize = 240;

/// Widget ids this module draws.
pub(crate) fn owns(id: &str) -> bool {
    matches!(id, PALETTE_KEY | CHAT_FONT_KEY | TERMINAL_FONT_KEY | "pet")
}

/// Apply one stored device pref to the live window: the palette is ours, so an edit in the
/// panel and a fresh start look identical. Called from `widgets::apply_pref` (controller
/// `apply_device_pref`) on every edit and once per saved pref at startup.
pub(crate) fn apply_pref(ui: &AppWindow, key: &str, value: &str) {
    if key != PALETTE_KEY {
        return;
    }
    let name = value.trim();
    // "" / "nous" (the pref default) is today's look: `theme.slint` only special-cases the
    // four palettes above, so anything unknown keeps the default tokens.
    let palette = if name.is_empty() { "nous" } else { name };
    ui.global::<Theme>().set_palette(palette.into());
}

/// Load (or reload) one widget's data into `AppearanceState`.
pub(crate) async fn load(id: &str, ctx: &Ctx) {
    match id {
        PALETTE_KEY => load_palette(ctx).await,
        CHAT_FONT_KEY => load_font(ctx, true).await,
        TERMINAL_FONT_KEY => load_font(ctx, false).await,
        // `pet` renders a static explanation; there is no pet backend to query.
        _ => {}
    }
}

/// One `appearance:<verb>[:<arg>]` action from the UI.
pub(crate) async fn action(rest: &str, ctx: &Ctx) -> Outcome {
    let mut parts = rest.splitn(3, ':');
    match parts.next().unwrap_or("") {
        "palette" => pick_palette(parts.next().unwrap_or(""), ctx).await,
        "font" => {
            let which = parts.next().unwrap_or("chat");
            pick_font(which == "terminal", parts.next().unwrap_or(""), ctx)
        }
        "font-q" => {
            let which = parts.next().unwrap_or("chat");
            let chat = which != "terminal";
            push_font_list(ctx, chat, filter_fonts(parts.next().unwrap_or("")));
            Outcome::None
        }
        _ => Outcome::None,
    }
}

// ── Palette ─────────────────────────────────────────────────────────────────────

async fn load_palette(ctx: &Ctx) {
    let path = format!("/api/dashboard/themes{}", ctx.profile_query());
    let value = match ctx.get_json(&path).await {
        Ok(v) => v,
        Err(e) => {
            let msg = format!("Could not load palettes: {e}");
            ctx.ui(move |w| {
                w.global::<AppearanceState>().set_palette_error(msg.into());
            });
            return;
        }
    };
    let active = value.get("active").and_then(Value::as_str).unwrap_or("").to_owned();
    let mut cards: Vec<PaletteCard> = Vec::new();
    if let Some(themes) = value.get("themes").and_then(Value::as_array) {
        for (i, t) in themes.iter().enumerate() {
            let name = t.get("name").and_then(Value::as_str).unwrap_or("").to_owned();
            let label = t
                .get("label")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| name.clone());
            let description = t.get("description").and_then(Value::as_str).unwrap_or("").to_owned();
            cards.push(PaletteCard {
                name: name.clone().into(),
                label: label.into(),
                description: description.into(),
                local: LOCAL_PALETTES.contains(&name.as_str()),
                active: !active.is_empty() && name == active,
                row: i as i32 / GRID_COLS,
                col: i as i32 % GRID_COLS,
            });
        }
    }
    let rows = cards.len().div_ceil(GRID_COLS as usize).max(1) as i32;
    ctx.ui(move |w| {
        let s = w.global::<AppearanceState>();
        s.set_palettes(ModelRc::new(VecModel::from(cards)));
        s.set_palette_rows(rows);
        s.set_palette_error(SharedString::default());
    });
}

async fn pick_palette(name: &str, ctx: &Ctx) -> Outcome {
    if name.is_empty() {
        return Outcome::Error("No palette name.".into());
    }
    let body = serde_json::json!({ "name": name });
    let path = format!("/api/dashboard/theme{}", ctx.profile_query());
    if let Err(e) = ctx.put_json(&path, &body).await {
        return Outcome::Error(format!("Could not switch the dashboard theme: {e}"));
    }
    // Apply the local palette at once. The panel's edit path applies it too, but only for
    // the default profile (the widget row's store is `Store::None`, so nothing runs for it).
    let for_ui = name.to_owned();
    ctx.ui(move |w| apply_pref(w, PALETTE_KEY, &for_ui));
    // Persist the device pref so the row and the panel agree and a restart keeps the look.
    persist_pref(PALETTE_KEY, name);
    Outcome::Edit(PALETTE_KEY.to_owned(), name.to_owned())
}

/// Write one device pref the way the controller does (same file, same key, atomic write).
///
/// The palette row is a `widget()` / `Store::None` registry row, so `edit_setting` does not
/// persist an `Outcome::Edit` for it (verified in `controller.rs`: `Store::None => {}`).
/// Without this the choice would be lost on restart. If that row ever becomes a device row,
/// `edit_setting` writes the same key and this becomes a harmless duplicate write.
fn persist_pref(key: &str, value: &str) {
    let mut prefs = hermes_core::prefs::Prefs::load(hermes_core::prefs::default_path());
    let _ = prefs.set(key, Value::String(value.to_owned()));
}

// ── Fonts ───────────────────────────────────────────────────────────────────────

/// Every installed font family, de-duplicated and sorted. `fc-list : family` prints one
/// line per face ("Family,Family Light"); the first field is the family. Cached for the
/// process (the settings panel loads on every publish).
fn installed_fonts() -> &'static [String] {
    static FONTS: OnceLock<Vec<String>> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut families: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        if let Ok(out) = std::process::Command::new("fc-list").arg(":").arg("family").output() {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let first = line.split(',').next().unwrap_or("").trim();
                if first.is_empty() {
                    continue;
                }
                if seen.insert(first.to_lowercase()) {
                    families.push(first.to_owned());
                }
            }
        }
        families.sort_by_key(|f| f.to_lowercase());
        families
    })
}

/// Installed families matching `query` (case-insensitive substring), capped.
fn filter_fonts(query: &str) -> Vec<FontFamily> {
    let q = query.trim().to_lowercase();
    installed_fonts()
        .iter()
        .filter(|f| q.is_empty() || f.to_lowercase().contains(&q))
        .take(FONT_LIMIT)
        .map(|f| FontFamily {
            family: f.as_str().into(),
        })
        .collect()
}

async fn load_font(ctx: &Ctx, chat: bool) {
    let key = if chat { CHAT_FONT_KEY } else { TERMINAL_FONT_KEY };
    let mut current = String::new();
    let mut error = String::new();
    match ctx.config().await {
        Ok(cfg) => {
            current = hermes_core::settings::get(&cfg, key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
        }
        Err(e) => error = format!("Could not read {key}: {e}"),
    }
    let fonts = filter_fonts("");
    let current_s: SharedString = current.clone().into();
    let error_s: SharedString = error.into();
    ctx.ui(move |w| {
        {
            let s = w.global::<AppearanceState>();
            let model = ModelRc::new(VecModel::from(fonts));
            if chat {
                s.set_chat_fonts(model);
                s.set_chat_current(current_s);
                s.set_chat_query(SharedString::default());
            } else {
                s.set_terminal_fonts(model);
                s.set_terminal_current(current_s);
                s.set_terminal_query(SharedString::default());
            }
            s.set_fonts_error(error_s);
        }
        // Mirror the controller's startup mapping (empty = the bundled font) so the picker
        // and the live app never disagree.
        let family: SharedString = if current.is_empty() {
            if chat { "Inter" } else { "JetBrains Mono" }.into()
        } else {
            current.as_str().into()
        };
        let t = w.global::<Theme>();
        if chat {
            t.set_ui_font(family);
        } else {
            t.set_mono_font(family);
        }
    });
}

fn pick_font(terminal: bool, family: &str, ctx: &Ctx) -> Outcome {
    // Live preview; the panel's edit path saves and applies the same mapping.
    let for_ui = family.to_owned();
    ctx.ui(move |w| {
        let value: SharedString = if for_ui.is_empty() {
            if terminal { "JetBrains Mono" } else { "Inter" }.into()
        } else {
            for_ui.as_str().into()
        };
        let t = w.global::<Theme>();
        if terminal {
            t.set_mono_font(value);
        } else {
            t.set_ui_font(value);
        }
    });
    let key = if terminal { TERMINAL_FONT_KEY } else { CHAT_FONT_KEY };
    Outcome::Edit(key.to_owned(), family.to_owned())
}

fn push_font_list(ctx: &Ctx, chat: bool, list: Vec<FontFamily>) {
    let fonts = list;
    ctx.ui(move |w| {
        let s = w.global::<AppearanceState>();
        let model = ModelRc::new(VecModel::from(fonts));
        if chat {
            s.set_chat_fonts(model);
        } else {
            s.set_terminal_fonts(model);
        }
    });
}
