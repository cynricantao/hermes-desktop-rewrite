//! Desktop pet (companion overlay): `pet.*` RPCs and the `pet.*` notifications.
//!
//! The pet is the animated mascot the desktop floats over its window (and the TUI draws in
//! half-blocks). Every method here is a JSON-RPC call on the gateway WebSocket; there is no
//! REST half. Pets are **per profile**, so the `profile`-scoped methods take `profile: &str`
//! (`""` = the launch profile, i.e. the param is omitted) — the same "pets are per-profile"
//! rule the Electron picker follows.
//!
//! Shapes come from `tui_gateway/methods_session.py` (`@_pet_method` handlers),
//! `tui_gateway/server.py::_pet_sprite_payload`, and the contracts in
//! `tui_gateway/contracts/projects_pets.py` / `billing_delegation_pets.py`. Those contracts
//! are camelCase on the wire, so the reply structs use `rename_all = "camelCase"` with
//! Rust-side snake_case names.
//!
//! # Big payloads
//! `pet.info` carries the whole spritesheet as `spritesheetBase64` (multi-MB), `pet.thumb`
//! and `pet.generate` carry PNG data URIs, and `pet.export` a base64 zip. They are modelled
//! as `String` and must never be logged — print lengths only. `pet.info` has send-once
//! semantics: pass the revision you already hold as `knownRevision` and an unchanged sheet
//! comes back as `spritesheetUnchanged: true` with the bytes elided.
//!
//! # Safety classes
//! * Read-only / local: [`pet_info`], [`pet_info_meta`], [`pet_cells`], [`pet_generate_status`].
//!   These touch only the backend's own config and installed pet files.
//! * **Network**: [`pet_gallery`] — the remote Petdex manifest (and `localOnly` only skips
//!   the *inline* fetch: the handler still warms the manifest cache on a background thread).
//! * **Spends model calls**: [`pet_generate`] (image drafts) and [`pet_hatch`] (one grounded
//!   image call per animation row, with retries).
//! * **Changes user state**: [`pet_select`] (installs + writes `display.pet.*`),
//!   [`pet_remove`] (deletes the pet directory), [`pet_rename`] (renames slug + dir),
//!   [`pet_disable`] and [`pet_scale`] (persist config). [`pet_export`] only reads, but
//!   returns the pet's files.
//!
//! # Notifications
//! [`PET_CHANGED`], [`PET_GENERATE_PROGRESS`], [`PET_HATCH_PROGRESS`] name the events; see
//! [`parse_pet_changed`], [`parse_pet_generate_progress`], [`parse_pet_hatch_progress`] (and
//! [`parse_pet_event`] to dispatch all three). They arrive as [`crate::protocol::GatewayEvent`]
//! with the payload in `payload`.

use std::collections::HashMap;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::api::{ApiError, Call, call_of, decode};
use crate::rpc::Client;

// ── Timeouts (desktop parity) ───────────────────────────────────────────────────
// `apps/desktop/src/store/pet-generate.ts` budgets far past the client's default RPC
// timeout: generation fans out several image calls, and a hatch fans out ~8 animation rows
// with up to 3 retries each (the backend's own worst case is ~30 min).

/// `pet.generate` — desktop `GENERATE_TIMEOUT_MS` (7 min).
pub const GENERATE_TIMEOUT: Duration = Duration::from_secs(420);
/// `pet.hatch` — desktop `HATCH_TIMEOUT_MS` (1 h), above the backend's ~30 min worst case.
pub const HATCH_TIMEOUT: Duration = Duration::from_secs(3600);

/// Send a call with an explicit timeout and decode the reply.
async fn call_timed_as<T: DeserializeOwned>(client: &Client, c: &Call, timeout: Duration) -> Result<T, ApiError> {
    let v = client.request_with_timeout(c.method, c.params.clone(), timeout).await?;
    decode(v)
}

/// Add `profile` to a params object (omitted for the launch profile).
fn scoped(mut params: Value, profile: &str) -> Value {
    if !profile.is_empty()
        && let Value::Object(ref mut map) = params
    {
        map.insert("profile".to_owned(), Value::String(profile.to_owned()));
    }
    params
}

// ── Shared sprite payload ───────────────────────────────────────────────────────

/// `server.py::_pet_sprite_payload` — the renderer payload, one shape for `pet.info`
/// (active pet) and `pet.hatch` (the unadopted preview). Every field is optional because
/// `pet.hatch` sends `{}` when the freshly installed pet cannot be reloaded.
///
/// **`spritesheet_base64` is the whole sheet** (multi-MB): never print it.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PetSprite {
    pub slug: Option<String>,
    pub display_name: Option<String>,
    /// `image/png` or `image/webp`.
    pub mime: Option<String>,
    /// Base64 spritesheet bytes (elided when `pet.info` answers send-once-unchanged).
    pub spritesheet_base64: Option<String>,
    /// Opaque revision for send-once (`knownRevision`).
    pub spritesheet_revision: Option<String>,
    pub frame_w: Option<i64>,
    pub frame_h: Option<i64>,
    /// The canvas' static fallback when `frames_by_state` is empty.
    pub frames_per_state: Option<i64>,
    /// Real (padding-trimmed) frame count per state.
    pub frames_by_state: HashMap<String, i64>,
    /// Frame count per spritesheet row, keyed by row name.
    pub frames_by_row: HashMap<String, i64>,
    pub loop_ms: Option<i64>,
    pub scale: Option<f64>,
    /// The state-row taxonomy: row index → state name.
    pub state_rows: Vec<String>,
}

/// `pet.info`: the sprite payload behind `enabled`, plus the send-once marker.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PetInfo {
    /// False when the display is off or the selected pet is missing: every sprite field is
    /// then absent.
    pub enabled: bool,
    /// True when the caller's `knownRevision` matched and the sheet was elided.
    pub spritesheet_unchanged: Option<bool>,
    #[serde(flatten)]
    pub sprite: PetSprite,
}

/// `pet.info.meta`: cheap active-pet metadata (no sheet bytes) — the poll/PetChanged shape.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PetInfoMeta {
    pub enabled: bool,
    pub slug: Option<String>,
    pub display_name: Option<String>,
    pub scale: Option<f64>,
    pub spritesheet_revision: Option<String>,
}

// ── pet.info / pet.info.meta ───────────────────────────────────────────────────

/// `pet.info`: the active pet for sprite renderers.
///
/// `known_revision` is the `spritesheet_revision` the caller already holds; when it still
/// matches, the reply sets `spritesheet_unchanged` and omits the multi-MB base64 sheet.
pub fn pet_info_call(profile: &str, known_revision: Option<&str>) -> Call {
    let mut params = json!({});
    if let Some(rev) = known_revision.filter(|r| !r.is_empty()) {
        params["knownRevision"] = json!(rev);
    }
    call_of("pet.info", scoped(params, profile))
}

/// Read the active pet (local config + sprite files; no network).
pub async fn pet_info(client: &Client, profile: &str, known_revision: Option<&str>) -> Result<PetInfo, ApiError> {
    crate::api::call_as(client, &pet_info_call(profile, known_revision)).await
}

/// `pet.info.meta`: cheap metadata to avoid full payload refreshes.
pub fn pet_info_meta_call(profile: &str) -> Call {
    call_of("pet.info.meta", scoped(json!({}), profile))
}

/// Read the active pet's metadata only (local; no sheet bytes).
pub async fn pet_info_meta(client: &Client, profile: &str) -> Result<PetInfoMeta, ApiError> {
    crate::api::call_as(client, &pet_info_meta_call(profile)).await
}

// ── pet.cells ──────────────────────────────────────────────────────────────────

/// `pet.cells`: half-block cell frames (or a kitty placement) for one state.
///
/// `state` empty = `idle`; `cols` 0/None = the backend's scale-derived width. `graphics`
/// opts into the kitty payload when the *backend's* terminal speaks it (irrelevant for the
/// desktop, which renders the unicode cells itself).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PetCells {
    pub enabled: bool,
    pub slug: Option<String>,
    pub display_name: Option<String>,
    pub state: Option<String>,
    pub cols: Option<i64>,
    /// Per-frame duration: `LOOP_MS / frame_count`.
    pub frame_ms: Option<f64>,
    /// Unicode: frame → row → cell `[tr,tg,tb,ta, br,bg,bb,ba]`. Kitty (`graphics` set):
    /// a list of transmit escapes. A union on the wire, hence `Value`.
    pub frames: Option<Value>,
    pub scale: Option<f64>,
    /// `"kitty"` on the graphics payload.
    pub graphics: Option<String>,
    pub image_id: Option<i64>,
    pub color: Option<String>,
    pub rows: Option<i64>,
    /// Kitty text grid that reserves the image's space.
    pub placeholder: Vec<String>,
}

/// `pet.cells`: render one state. `graphics` is the kitty opt-in (desktop leaves it off).
pub fn pet_cells_call(profile: &str, state: &str, cols: Option<u32>, graphics: bool) -> Call {
    let mut params = json!({});
    if !state.is_empty() {
        params["state"] = json!(state);
    }
    if let Some(cols) = cols.filter(|c| *c > 0) {
        params["cols"] = json!(cols);
    }
    if graphics {
        params["graphics"] = json!(true);
    }
    call_of("pet.cells", scoped(params, profile))
}

/// Render one state of the active pet to cells (local render; no network).
pub async fn pet_cells(
    client: &Client,
    profile: &str,
    state: &str,
    cols: Option<u32>,
    graphics: bool,
) -> Result<PetCells, ApiError> {
    crate::api::call_as(client, &pet_cells_call(profile, state, cols, graphics)).await
}

// ── pet.gallery ────────────────────────────────────────────────────────────────

/// One Petdex / local gallery row.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PetGalleryEntry {
    pub slug: String,
    pub display_name: String,
    pub installed: bool,
    /// Empty for entries only known locally.
    pub spritesheet_url: String,
    /// True for Petdex's hand-picked set (`/curated/` asset path); absent on local rows.
    pub curated: Option<bool>,
    /// True for pets this machine generated (`pet.hatch`).
    pub generated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PetGallery {
    pub enabled: bool,
    /// The configured active slug (`""` when none).
    pub active: String,
    pub pets: Vec<PetGalleryEntry>,
}

/// `pet.gallery`: the Petdex catalogue plus local install state.
///
/// **Network**: the handler fetches the remote Petdex manifest. `local_only` skips the
/// inline fetch (installed pets only) but *still* warms the manifest cache from a daemon
/// thread, so it is not network-free either.
pub fn pet_gallery_call(profile: &str, local_only: bool) -> Call {
    let params = if local_only {
        json!({ "localOnly": true })
    } else {
        json!({})
    };
    call_of("pet.gallery", scoped(params, profile))
}

/// List gallery pets. **Contacts the Petdex manifest** (even `local_only` prefetches).
pub async fn pet_gallery(client: &Client, profile: &str, local_only: bool) -> Result<PetGallery, ApiError> {
    crate::api::call_as(client, &pet_gallery_call(profile, local_only)).await
}

// ── pet.select / remove / rename ───────────────────────────────────────────────

/// Result of the slug-addressed mutations (`select`, `remove`, `rename`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PetSlugResult {
    pub ok: bool,
    pub slug: String,
    pub display_name: Option<String>,
}

pub fn pet_select_call(profile: &str, slug: &str) -> Call {
    call_of("pet.select", scoped(json!({ "slug": slug }), profile))
}

/// Adopt a pet: install it if needed and activate it. **Changes user state** — writes
/// `display.pet.*` to the profile's config (and can install from Petdex, i.e. network).
pub async fn pet_select(client: &Client, profile: &str, slug: &str) -> Result<PetSlugResult, ApiError> {
    crate::api::call_as(client, &pet_select_call(profile, slug)).await
}

pub fn pet_remove_call(profile: &str, slug: &str) -> Call {
    call_of("pet.remove", scoped(json!({ "slug": slug }), profile))
}

/// Uninstall a pet (deletes its directory); if it was active the display is turned off.
/// **Destructive** — there is no undo short of a `pet.export` taken beforehand.
pub async fn pet_remove(client: &Client, profile: &str, slug: &str) -> Result<PetSlugResult, ApiError> {
    crate::api::call_as(client, &pet_remove_call(profile, slug)).await
}

/// `pet.rename`: a new display name; the backend realigns the slug/directory with it.
pub fn pet_rename_call(profile: &str, slug: &str, name: &str) -> Call {
    call_of("pet.rename", scoped(json!({ "slug": slug, "name": name }), profile))
}

/// Rename a pet's display name. **Changes user state** — renames the directory and follows
/// the active slug in config; the reply's `slug` is the *new* slug.
pub async fn pet_rename(client: &Client, profile: &str, slug: &str, name: &str) -> Result<PetSlugResult, ApiError> {
    crate::api::call_as(client, &pet_rename_call(profile, slug, name)).await
}

// ── pet.export / pet.thumb ─────────────────────────────────────────────────────

/// `pet.export`: a re-importable `.zip` (pet.json + spritesheet) as base64.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PetExport {
    pub ok: bool,
    pub filename: String,
    /// **Base64 zip** — multi-MB; never log it.
    pub zip_base64: String,
}

pub fn pet_export_call(profile: &str, slug: &str) -> Call {
    call_of("pet.export", scoped(json!({ "slug": slug }), profile))
}

/// Export an installed pet as a `.zip` payload. Read-only on disk, but the reply carries the
/// pet's files as base64 (keep it in memory / stream it to a save dialog).
pub async fn pet_export(client: &Client, profile: &str, slug: &str) -> Result<PetExport, ApiError> {
    crate::api::call_as(client, &pet_export_call(profile, slug)).await
}

/// `pet.thumb`: an idle-frame PNG data URI for the picker (the desktop's CSP blocks CDN
/// `<img>`, so the gateway proxies the bytes).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PetThumb {
    pub ok: bool,
    pub slug: String,
    /// `data:image/png;base64,…` — never log it.
    pub data_uri: Option<String>,
}

pub fn pet_thumb_call(profile: &str, slug: &str, url: Option<&str>) -> Call {
    call_of(
        "pet.thumb",
        scoped(json!({ "slug": slug, "url": url.unwrap_or("") }), profile),
    )
}

/// Fetch a picker thumbnail. Local for an installed pet; for a not-yet-installed one the
/// handler fetches `url` **only when it is Petdex-hosted** (arbitrary client URLs are never
/// fetched), so a non-Petdex url is a local miss.
pub async fn pet_thumb(client: &Client, profile: &str, slug: &str, url: Option<&str>) -> Result<PetThumb, ApiError> {
    crate::api::call_as(client, &pet_thumb_call(profile, slug, url)).await
}

// ── pet.disable / pet.scale ────────────────────────────────────────────────────

/// A bare `{ok}` reply (`pet.disable`, `pet.cancel`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PetOk {
    pub ok: bool,
}

pub fn pet_disable_call(profile: &str) -> Call {
    call_of("pet.disable", scoped(json!({}), profile))
}

/// Turn the pet display off. **Changes user state** — persists `display.pet.enabled=false`.
pub async fn pet_disable(client: &Client, profile: &str) -> Result<PetOk, ApiError> {
    crate::api::call_as(client, &pet_disable_call(profile)).await
}

/// `pet.scale`: the persisted scale, clamped by the backend to the engine's bounds.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PetScale {
    pub ok: bool,
    pub scale: f64,
}

pub fn pet_scale_call(profile: &str, scale: f64) -> Call {
    call_of("pet.scale", scoped(json!({ "scale": scale }), profile))
}

/// Persist the floating pet's scale. **Changes user state** (config write); `scale` may be
/// any number — a non-number answers 4004 and the reply's `scale` is the clamped value.
pub async fn pet_scale(client: &Client, profile: &str, scale: f64) -> Result<PetScale, ApiError> {
    crate::api::call_as(client, &pet_scale_call(profile, scale)).await
}

// ── pet.cancel / pet.generate.status ───────────────────────────────────────────

pub fn pet_cancel_call(token: &str) -> Call {
    call_of("pet.cancel", json!({ "token": token }))
}

/// Stop an in-flight `pet.generate` / `pet.hatch` by token. Idempotent and off the worker
/// pool so it lands mid-generation; an unknown token is a no-op. Not profile-scoped.
pub async fn pet_cancel(client: &Client, token: &str) -> Result<PetOk, ApiError> {
    crate::api::call_as(client, &pet_cancel_call(token)).await
}

/// One image backend that can draw sprite sheets.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PetProvider {
    pub name: String,
    pub label: String,
    pub default: bool,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PetGenerateStatus {
    /// True when a reference-capable image backend is configured.
    pub available: bool,
    pub providers: Vec<PetProvider>,
}

/// `pet.generate.status`: whether generation is possible and with which providers.
/// A local config read (no network). Not profile-scoped.
pub fn pet_generate_status_call() -> Call {
    call_of("pet.generate.status", json!({}))
}

/// Check generation availability (local config read).
pub async fn pet_generate_status(client: &Client) -> Result<PetGenerateStatus, ApiError> {
    crate::api::call_as(client, &pet_generate_status_call()).await
}

// ── pet.generate ───────────────────────────────────────────────────────────────

/// Inputs for [`pet_generate_call`]: `prompt` **or** `reference_image` (a data URL) must be
/// set, otherwise the handler answers 4004. `count` is clamped by the backend to 1..=4
/// (default 4), `style` defaults to `auto`, and an empty `provider` means "backend default".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GenerateOptions {
    pub prompt: Option<String>,
    pub reference_image: Option<String>,
    pub count: Option<u32>,
    pub style: Option<String>,
    pub provider: Option<String>,
}

impl GenerateOptions {
    /// A prompt-only generation (`count`/`style`/`provider` left to the backend).
    pub fn prompt(prompt: impl Into<String>) -> Self {
        Self {
            prompt: Some(prompt.into()),
            ..Self::default()
        }
    }
}

/// One candidate base look; `data_uri` is a PNG data URL — never log it.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PetDraft {
    pub index: i64,
    pub data_uri: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PetGenerate {
    pub ok: bool,
    /// Names the staged drafts for [`pet_hatch`] (and the progress events' `token`).
    pub token: String,
    pub drafts: Vec<PetDraft>,
}

/// `pet.generate`: candidate base looks for a new pet (the draft step). Not profile-scoped.
pub fn pet_generate_call(opts: &GenerateOptions) -> Call {
    let mut params = json!({});
    if let Some(prompt) = opts.prompt.as_deref().filter(|p| !p.is_empty()) {
        params["prompt"] = json!(prompt);
    }
    if let Some(image) = opts.reference_image.as_deref().filter(|r| !r.is_empty()) {
        params["referenceImage"] = json!(image);
    }
    if let Some(count) = opts.count {
        params["count"] = json!(count);
    }
    if let Some(style) = opts.style.as_deref().filter(|s| !s.is_empty()) {
        params["style"] = json!(style);
    }
    if let Some(provider) = opts.provider.as_deref().filter(|p| !p.is_empty()) {
        params["provider"] = json!(provider);
    }
    call_of("pet.generate", params)
}

/// Generate up to four base-look drafts.
///
/// **Spends model calls** (one image generation per draft, on the configured image backend)
/// and uploads the reference image to that provider when one is given. Drafts also stream
/// as [`PET_GENERATE_PROGRESS`] events. Uses [`GENERATE_TIMEOUT`].
pub async fn pet_generate(client: &Client, opts: &GenerateOptions) -> Result<PetGenerate, ApiError> {
    call_timed_as(client, &pet_generate_call(opts), GENERATE_TIMEOUT).await
}

// ── pet.hatch ──────────────────────────────────────────────────────────────────

/// Inputs for [`pet_hatch_call`]. `token` + `name` are required; `index` picks the draft
/// (default 0), `cancel_token` is the key a concurrent [`pet_cancel`] targets (falls back
/// to `token`), and `description` / `prompt` / `style` / `provider` steer the drawing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HatchOptions {
    pub token: String,
    pub name: String,
    pub cancel_token: Option<String>,
    pub index: Option<u32>,
    pub description: Option<String>,
    pub prompt: Option<String>,
    pub style: Option<String>,
    pub provider: Option<String>,
}

impl HatchOptions {
    pub fn new(token: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            name: name.into(),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PetHatch {
    pub ok: bool,
    /// The new pet's slug — installed but **not** active.
    pub slug: String,
    pub display_name: String,
    /// Validation warnings (free-form rows) from the hatch pipeline.
    pub warnings: Vec<Value>,
    /// The renderer payload for the preview (`{}` when the pet could not be reloaded).
    pub pet: PetSprite,
}

/// `pet.hatch`: turn a base draft (`token` + `index`) into a full spritesheet pet. The pet is
/// installed but **not** active — [`pet_select`] adopts it, [`pet_remove`] discards it. Not
/// profile-scoped.
pub fn pet_hatch_call(opts: &HatchOptions) -> Call {
    let mut params = json!({ "token": opts.token, "name": opts.name });
    if let Some(cancel) = opts.cancel_token.as_deref().filter(|c| !c.is_empty()) {
        params["cancelToken"] = json!(cancel);
    }
    if let Some(index) = opts.index {
        params["index"] = json!(index);
    }
    if let Some(description) = opts.description.as_deref().filter(|d| !d.is_empty()) {
        params["description"] = json!(description);
    }
    if let Some(prompt) = opts.prompt.as_deref().filter(|p| !p.is_empty()) {
        params["prompt"] = json!(prompt);
    }
    if let Some(style) = opts.style.as_deref().filter(|s| !s.is_empty()) {
        params["style"] = json!(style);
    }
    if let Some(provider) = opts.provider.as_deref().filter(|p| !p.is_empty()) {
        params["provider"] = json!(provider);
    }
    call_of("pet.hatch", params)
}

/// Draw a full animated pet from a draft.
///
/// **Spends model calls** — one grounded image call per animation row (with retries) — and
/// **changes user state** by installing the pet on disk. Progress streams as
/// [`PET_HATCH_PROGRESS`]. Uses [`HATCH_TIMEOUT`].
pub async fn pet_hatch(client: &Client, opts: &HatchOptions) -> Result<PetHatch, ApiError> {
    call_timed_as(client, &pet_hatch_call(opts), HATCH_TIMEOUT).await
}

// ── Notifications ──────────────────────────────────────────────────────────────

/// The active pet or its spritesheet changed (the gateway's change watcher). The payload is
/// `pet.info.meta`-shaped, or `{enabled:false}` when the display is off.
pub const PET_CHANGED: &str = "pet.changed";
/// `pet.generate` progress: a token-only init frame, then one per finished draft.
pub const PET_GENERATE_PROGRESS: &str = "pet.generate.progress";
/// `pet.hatch` row-drawing progress.
pub const PET_HATCH_PROGRESS: &str = "pet.hatch.progress";

/// `pet.changed` payload.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PetChanged {
    pub enabled: bool,
    pub slug: Option<String>,
    pub display_name: Option<String>,
    pub scale: Option<f64>,
    pub spritesheet_revision: Option<String>,
}

/// `pet.generate.progress` payload. `index` / `data_uri` are absent on the init frame;
/// `count` is the requested draft count.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PetGenerateProgress {
    pub token: String,
    pub count: i64,
    pub index: Option<i64>,
    /// PNG data URL of the finished draft — never log it.
    pub data_uri: Option<String>,
}

/// `pet.hatch.progress` payload. Row events carry `state` + `done`/`total` (strings, as the
/// handler splits `"<state>:<done>:<total>"` out of `detail`); `compose` / `save` events
/// carry only `event` (+ `detail`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PetHatchProgress {
    /// `row` | `compose` | `save`.
    pub event: String,
    pub detail: Option<String>,
    pub state: Option<String>,
    pub done: Option<String>,
    pub total: Option<String>,
}

/// One decoded pet notification.
#[derive(Debug, Clone, PartialEq)]
pub enum PetEvent {
    Changed(PetChanged),
    GenerateProgress(PetGenerateProgress),
    HatchProgress(PetHatchProgress),
}

fn parse_as<T: DeserializeOwned>(payload: &Value) -> Option<T> {
    serde_json::from_value(payload.clone()).ok()
}

/// Decode a `pet.changed` payload (None for another event type or a non-object payload).
pub fn parse_pet_changed(event_type: &str, payload: &Value) -> Option<PetChanged> {
    (event_type == PET_CHANGED).then(|| parse_as(payload)).flatten()
}

/// Decode a `pet.generate.progress` payload.
pub fn parse_pet_generate_progress(event_type: &str, payload: &Value) -> Option<PetGenerateProgress> {
    (event_type == PET_GENERATE_PROGRESS)
        .then(|| parse_as(payload))
        .flatten()
}

/// Decode a `pet.hatch.progress` payload.
pub fn parse_pet_hatch_progress(event_type: &str, payload: &Value) -> Option<PetHatchProgress> {
    (event_type == PET_HATCH_PROGRESS).then(|| parse_as(payload)).flatten()
}

/// Dispatch any of the three pet notifications by `event_type`.
pub fn parse_pet_event(event_type: &str, payload: &Value) -> Option<PetEvent> {
    match event_type {
        PET_CHANGED => parse_pet_changed(event_type, payload).map(PetEvent::Changed),
        PET_GENERATE_PROGRESS => parse_pet_generate_progress(event_type, payload).map(PetEvent::GenerateProgress),
        PET_HATCH_PROGRESS => parse_pet_hatch_progress(event_type, payload).map(PetEvent::HatchProgress),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_ok<T: DeserializeOwned + Default + PartialEq + std::fmt::Debug>(v: Value) -> T {
        let out: T = decode(v.clone()).unwrap();
        let empty: T = decode(json!({})).unwrap();
        assert_eq!(empty, T::default(), "`{{}}` must decode to Default for {v}");
        out
    }

    #[test]
    fn info_builder() {
        let c = pet_info_call("", None);
        assert_eq!(c.method, "pet.info");
        assert_eq!(c.params, json!({}));

        let c = pet_info_call("swe", Some("rev-1"));
        assert_eq!(
            (c.method, c.params),
            ("pet.info", json!({"profile": "swe", "knownRevision": "rev-1"}))
        );

        // An empty revision is the same as none (never `knownRevision: ""`).
        assert_eq!(pet_info_call("swe", Some("")).params, json!({"profile": "swe"}));
    }

    #[test]
    fn info_decodes_real_payload() {
        let info: PetInfo = decode_ok(json!({
            "enabled": true, "slug": "otto", "displayName": "Otto", "mime": "image/png",
            "spritesheetBase64": "QUJD", "spritesheetRevision": "rev-1",
            "frameW": 64, "frameH": 64, "framesPerState": 4,
            "framesByState": {"idle": 4, "run": 6}, "framesByRow": {"idle": 1, "run": 1},
            "loopMs": 2400, "scale": 1.25, "stateRows": ["idle", "run"], "x-future": 1
        }));
        assert!(info.enabled);
        assert_eq!(info.sprite.slug.as_deref(), Some("otto"));
        assert_eq!(info.sprite.spritesheet_base64.as_deref(), Some("QUJD"));
        assert_eq!(info.sprite.frames_by_state.get("run"), Some(&6));
        assert_eq!(info.sprite.state_rows, ["idle", "run"]);
        assert_eq!(info.sprite.scale, Some(1.25));
        assert_eq!(info.spritesheet_unchanged, None);

        // Send-once: bytes elided, marker set.
        let once: PetInfo = decode(json!({
            "enabled": true, "slug": "otto", "spritesheetRevision": "rev-1",
            "spritesheetUnchanged": true
        }))
        .unwrap();
        assert!(once.enabled && once.spritesheet_unchanged == Some(true));
        assert!(once.sprite.spritesheet_base64.is_none());

        // Display off.
        let off: PetInfo = decode(json!({"enabled": false})).unwrap();
        assert!(!off.enabled);
        assert_eq!(off, PetInfo::default());
    }

    #[test]
    fn info_meta() {
        let c = pet_info_meta_call("swe");
        assert_eq!((c.method, c.params), ("pet.info.meta", json!({"profile": "swe"})));

        let meta: PetInfoMeta = decode_ok(json!({
            "enabled": true, "slug": "otto", "displayName": "Otto", "scale": 1.0,
            "spritesheetRevision": "rev-1"
        }));
        assert_eq!(meta.display_name.as_deref(), Some("Otto"));
        assert_eq!(meta.spritesheet_revision.as_deref(), Some("rev-1"));
    }

    #[test]
    fn cells_builder_and_decode() {
        assert_eq!(
            (
                pet_cells_call("", "", None, false).method,
                pet_cells_call("", "", None, false).params
            ),
            ("pet.cells", json!({}))
        );
        assert_eq!(
            pet_cells_call("swe", "run", Some(40), true).params,
            json!({"profile": "swe", "state": "run", "cols": 40, "graphics": true})
        );
        assert_eq!(
            pet_cells_call("", "idle", Some(0), false).params,
            json!({"state": "idle"})
        );

        let cells: PetCells = decode_ok(json!({
            "enabled": true, "slug": "otto", "displayName": "Otto", "state": "idle",
            "cols": 20, "frameMs": 600.0, "scale": 1.0,
            "frames": [[[[255, 0, 0, 255, 0, 0, 0, 0]]]]
        }));
        assert_eq!(cells.cols, Some(20));
        assert_eq!(cells.frame_ms, Some(600.0));
        assert_eq!(cells.frames.unwrap()[0][0][0][0].as_u64(), Some(255));

        // Kitty payload: `frames` are escapes, `placeholder` a text grid.
        let kitty: PetCells = decode(json!({
            "enabled": true, "slug": "otto", "state": "idle", "cols": 20, "frameMs": 600.0,
            "frames": ["\u{1b}_Gf=32;AAAA"], "graphics": "kitty", "imageId": 7,
            "color": "#336699", "rows": 10, "placeholder": ["aaaaaaaa"], "scale": 1.0
        }))
        .unwrap();
        assert_eq!(kitty.graphics.as_deref(), Some("kitty"));
        assert_eq!(kitty.image_id, Some(7));
        assert_eq!(kitty.rows, Some(10));
        assert_eq!(kitty.placeholder, ["aaaaaaaa"]);

        let off: PetCells = decode(json!({"enabled": false})).unwrap();
        assert_eq!(off, PetCells::default());
    }

    #[test]
    fn gallery() {
        assert_eq!(pet_gallery_call("", false).params, json!({}));
        assert_eq!(
            pet_gallery_call("swe", true).params,
            json!({"profile": "swe", "localOnly": true})
        );

        let g: PetGallery = decode_ok(json!({
            "enabled": true, "active": "otto",
            "pets": [
                {"slug": "otto", "displayName": "Otto", "installed": true,
                 "spritesheetUrl": "https://petdex.dev/curated/otto/sheet.png",
                 "curated": true, "generated": false},
                {"slug": "mine", "displayName": "Mine", "installed": true,
                 "spritesheetUrl": "", "generated": true}
            ]
        }));
        assert_eq!(g.active, "otto");
        assert_eq!(g.pets.len(), 2);
        assert!(g.pets[0].installed && g.pets[0].curated == Some(true));
        assert!(g.pets[1].generated && g.pets[1].spritesheet_url.is_empty());
        assert_eq!(g.pets[1].curated, None);
    }

    #[test]
    fn slug_mutations() {
        assert_eq!(
            pet_select_call("swe", "otto").params,
            json!({"profile": "swe", "slug": "otto"})
        );
        assert_eq!(pet_select_call("", "otto").method, "pet.select");
        assert_eq!(pet_remove_call("", "otto").params, json!({"slug": "otto"}));
        assert_eq!(
            pet_rename_call("swe", "otto", "Otto II").params,
            json!({"profile": "swe", "slug": "otto", "name": "Otto II"})
        );

        let r: PetSlugResult = decode_ok(json!({"ok": true, "slug": "otto", "displayName": "Otto"}));
        assert_eq!(r.slug, "otto");
        let removed: PetSlugResult = decode(json!({"ok": false, "slug": "ghost"})).unwrap();
        assert!(!removed.ok && removed.slug == "ghost");
    }

    #[test]
    fn export_and_thumb() {
        assert_eq!(pet_export_call("", "otto").params, json!({"slug": "otto"}));
        let e: PetExport = decode_ok(json!({
            "ok": true, "filename": "otto.zip", "zipBase64": "UEsDBA=="
        }));
        assert_eq!(e.filename, "otto.zip");
        assert_eq!(e.zip_base64, "UEsDBA==");

        assert_eq!(
            pet_thumb_call("", "otto", None).params,
            json!({"slug": "otto", "url": ""})
        );
        assert_eq!(
            pet_thumb_call("swe", "otto", Some("https://petdex.dev/x.png")).params,
            json!({"profile": "swe", "slug": "otto", "url": "https://petdex.dev/x.png"})
        );
        let t: PetThumb = decode_ok(json!({
            "ok": true, "slug": "otto", "dataUri": "data:image/png;base64,QUJD"
        }));
        assert_eq!(t.data_uri.as_deref(), Some("data:image/png;base64,QUJD"));
        // A miss is `{ok:false, slug}` (handler fail-open shape).
        let miss: PetThumb = decode(json!({"ok": false, "slug": "otto"})).unwrap();
        assert!(!miss.ok && miss.data_uri.is_none());
    }

    #[test]
    fn toggle_and_scale() {
        assert_eq!(pet_disable_call("").params, json!({}));
        assert_eq!(pet_disable_call("swe").params, json!({"profile": "swe"}));
        let ok: PetOk = decode_ok(json!({"ok": true}));
        assert!(ok.ok);

        assert_eq!(
            pet_scale_call("swe", 1.5).params,
            json!({"profile": "swe", "scale": 1.5})
        );
        let s: PetScale = decode_ok(json!({"ok": true, "scale": 2.0}));
        assert_eq!(s.scale, 2.0);
    }

    #[test]
    fn cancel_and_status() {
        let c = pet_cancel_call("abc123");
        assert_eq!((c.method, c.params), ("pet.cancel", json!({"token": "abc123"})));
        let ok: PetOk = decode(json!({"ok": true})).unwrap();
        assert!(ok.ok);

        let s = pet_generate_status_call();
        assert_eq!((s.method, s.params), ("pet.generate.status", json!({})));
        let st: PetGenerateStatus = decode_ok(json!({
            "available": true,
            "providers": [{"name": "openrouter", "label": "OpenRouter", "default": true}]
        }));
        assert!(st.available);
        assert_eq!(st.providers[0].name, "openrouter");
        assert!(st.providers[0].default);
    }

    #[test]
    fn generate_builder_and_decode() {
        let empty = GenerateOptions::default();
        assert_eq!(
            (pet_generate_call(&empty).method, pet_generate_call(&empty).params),
            ("pet.generate", json!({}))
        );
        assert_eq!(
            pet_generate_call(&GenerateOptions {
                prompt: Some("a red fox".into()),
                reference_image: None,
                count: Some(2),
                style: Some("auto".into()),
                provider: Some("openrouter".into()),
            })
            .params,
            json!({"prompt": "a red fox", "count": 2, "style": "auto", "provider": "openrouter"})
        );
        assert_eq!(
            pet_generate_call(&GenerateOptions {
                prompt: None,
                reference_image: Some("data:image/png;base64,QUJD".into()),
                count: None,
                style: None,
                provider: None,
            })
            .params,
            json!({"referenceImage": "data:image/png;base64,QUJD"})
        );
        assert_eq!(GenerateOptions::prompt("x").prompt.as_deref(), Some("x"));

        let g: PetGenerate = decode_ok(json!({
            "ok": true, "token": "tok1",
            "drafts": [{"index": 0, "dataUri": "data:image/png;base64,QUJD"}]
        }));
        assert_eq!(g.token, "tok1");
        assert_eq!(g.drafts[0].index, 0);
    }

    #[test]
    fn hatch_builder_and_decode() {
        let c = pet_hatch_call(&HatchOptions::new("tok1", "Otto"));
        assert_eq!(
            (c.method, c.params),
            ("pet.hatch", json!({"token": "tok1", "name": "Otto"}))
        );

        let full = pet_hatch_call(&HatchOptions {
            token: "tok1".into(),
            name: "Otto".into(),
            cancel_token: Some("cancel1".into()),
            index: Some(2),
            description: Some("a friendly otter".into()),
            prompt: Some("otter".into()),
            style: Some("pixel".into()),
            provider: Some("openrouter".into()),
        });
        assert_eq!(
            full.params,
            json!({
                "token": "tok1", "name": "Otto", "cancelToken": "cancel1", "index": 2,
                "description": "a friendly otter", "prompt": "otter", "style": "pixel",
                "provider": "openrouter"
            })
        );

        let h: PetHatch = decode_ok(json!({
            "ok": true, "slug": "otto", "displayName": "Otto",
            "warnings": [{"code": "soft_frame", "row": 3}],
            "pet": {"slug": "otto", "mime": "image/png", "frameW": 64, "frameH": 64,
                    "scale": 1.0, "stateRows": ["idle"]}
        }));
        assert_eq!(h.slug, "otto");
        assert_eq!(h.display_name, "Otto");
        assert_eq!(h.warnings.len(), 1);
        assert_eq!(h.pet.frame_w, Some(64));

        // Reload failure: `pet` is `{}` and every sprite field defaults.
        let bare: PetHatch = decode(json!({"ok": true, "slug": "otto", "displayName": "Otto"})).unwrap();
        assert_eq!(bare.pet, PetSprite::default());
        assert!(bare.warnings.is_empty());
    }

    #[test]
    fn notifications() {
        let changed = json!({"enabled": true, "slug": "otto", "displayName": "Otto",
                             "scale": 1.0, "spritesheetRevision": "rev-1"});
        let c = parse_pet_changed(PET_CHANGED, &changed).unwrap();
        assert_eq!(c.slug.as_deref(), Some("otto"));
        assert_eq!(c.spritesheet_revision.as_deref(), Some("rev-1"));
        // Display off payload.
        assert!(
            !parse_pet_changed(PET_CHANGED, &json!({"enabled": false}))
                .unwrap()
                .enabled
        );
        // Wrong event / non-object payload.
        assert!(parse_pet_changed("pet.other", &changed).is_none());
        assert!(parse_pet_changed(PET_CHANGED, &json!([1])).is_none());

        // Init frame: token + count only.
        let init = parse_pet_generate_progress(PET_GENERATE_PROGRESS, &json!({"token": "t", "count": 4})).unwrap();
        assert_eq!((init.token.as_str(), init.count, init.index), ("t", 4, None));
        assert!(init.data_uri.is_none());
        // Draft frame.
        let draft = parse_pet_generate_progress(
            PET_GENERATE_PROGRESS,
            &json!({"token": "t", "count": 4, "index": 1, "dataUri": "data:image/png;base64,QUJD"}),
        )
        .unwrap();
        assert_eq!(draft.index, Some(1));
        assert!(draft.data_uri.is_some());

        // Row progress (done/total are strings on the wire).
        let row = parse_pet_hatch_progress(
            PET_HATCH_PROGRESS,
            &json!({"event": "row", "state": "run", "done": "2", "total": "8"}),
        )
        .unwrap();
        assert_eq!(row.event, "row");
        assert_eq!((row.done.as_deref(), row.total.as_deref()), (Some("2"), Some("8")));
        // Phase progress.
        let phase = parse_pet_hatch_progress(
            PET_HATCH_PROGRESS,
            &json!({"event": "compose", "detail": "composing spritesheet"}),
        )
        .unwrap();
        assert_eq!(phase.event, "compose");
        assert!(phase.state.is_none());

        // Dispatcher.
        assert!(matches!(
            parse_pet_event(PET_CHANGED, &changed),
            Some(PetEvent::Changed(_))
        ));
        assert!(matches!(
            parse_pet_event(PET_GENERATE_PROGRESS, &json!({"token": "t", "count": 4})),
            Some(PetEvent::GenerateProgress(_))
        ));
        assert!(matches!(
            parse_pet_event(PET_HATCH_PROGRESS, &json!({"event": "save"})),
            Some(PetEvent::HatchProgress(_))
        ));
        assert!(parse_pet_event("session.info", &changed).is_none());
    }
}
